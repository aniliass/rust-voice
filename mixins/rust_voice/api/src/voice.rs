//! Голос: микрофон -> Opus -> AES -> UDP на сервер, и обратно: UDP -> AES -> Opus -> динамики.
//! Весь звук живёт в одном рабочем потоке; игровой поток общается с ним через `Outbox` и канал.

use crate::{crypto, denoise, dsp, plasmo, roster, settings, spatial, svc, LockExt};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, SampleRate, StreamConfig};
use rusty_opus::{Application, OpusDecoder, OpusEncoder, SignalType};
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const RATE: u32 = 48_000;
/// 20 мс при 48 кГц, моно.
pub const FRAME: usize = 960;
/// Активация нейросетью: с какой уверенности в речи включаемся и до какой держимся; тише `VAD_FLOOR`
/// (около −56 дБ) — никогда (нейросеть иногда видит «речь» в почти тишине).
const VAD_START: f32 = 0.6;
const VAD_KEEP: f32 = 0.3;
const VAD_FLOOR: f32 = 0.0016;
const MAX_QUEUE: usize = RATE as usize; // не больше секунды в очереди
const PREFILL_FRAMES: usize = 3; // сколько кадров копим перед воспроизведением источника
/// Сколько UDP может молчать, прежде чем сессия считается мёртвой (сервер шлёт keep-alive раз в 2-4 с).
/// Сколько кадров голоса SVC можно придержать (~1,5 с), пока выход из группы или возвращение в пути.
const SVC_HOLD_MAX: usize = 75;
const STALE_AFTER: Duration = Duration::from_secs(15);
/// Файл-выключатель микрофона: пока `~/.config/horizon-voice-mod.mute` существует, микрофон молчит.
fn mute_file() -> std::path::PathBuf {
    settings::config_dir().join("horizon-voice-mod.mute")
}
/// Источник «проверки звука»: ваш же микрофон в ваших динамиках (в очередь воспроизведения, на сервер не уходит).
const TEST_SOURCE: [u8; 16] = [0xFF; 16];

/// TCP-пакеты Plasmo, которые игровой поток отправит от нашего имени.
pub type Outbox = Arc<Mutex<Vec<Vec<u8>>>>;
pub type Logger = Arc<dyn Fn(String) + Send + Sync>;

pub struct SessionConfig {
    pub aes: [u8; 16],
    pub activation_id: [u8; 16],
    pub distance: i16,
    pub bitrate: i32,
    /// Без сервера: только проверка звука (`Link::Offline`), «подключено» не показываем.
    pub offline: bool,
}

pub struct VoiceCtl {
    stop: Arc<AtomicBool>,
    stale: Arc<AtomicBool>,
    tx: mpsc::Sender<SessionConfig>,
}

impl VoiceCtl {
    pub fn configure(&self, cfg: SessionConfig) {
        let _ = self.tx.send(cfg);
    }

    /// От сервера давно ничего не приходило по UDP: сессия, скорее всего, уже не его.
    pub fn is_stale(&self) -> bool {
        self.stale.load(Ordering::Relaxed)
    }
}

impl Drop for VoiceCtl {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        settings::set_connected(false);
        settings::set_speaking(false);
        settings::set_speaking_to(false, false);
    }
}

/// С каким сервером голоса говорим: Plasmo Voice или Simple Voice Chat.
pub enum Link {
    Plasmo(plasmo::ConnectionInfo),
    Svc {
        host: String,
        port: u16,
        session: svc::Session,
        /// Как часто сервер шлёт KeepAlive (мс): если молчит в 10 раз дольше, сессия мертва.
        keep_alive_ms: i32,
    },
    /// Без сервера: микрофон и динамики работают только для «Проверки звука» в меню, на сеть ничего не уходит.
    Offline,
}

impl Link {
    fn addr(&self) -> String {
        match self {
            Link::Plasmo(c) => host_port(&c.ip, c.port),
            // сокет нужен коду ниже, но в него ничего не пишем
            Link::Offline => "127.0.0.1:9".to_string(),
            Link::Svc { host, port, .. } => host_port(host, port),
        }
    }
}

/// «адрес:порт» для UDP. IPv6 — в квадратных скобках, иначе двоеточия адреса спутаются с портом.
fn host_port(host: &str, port: impl std::fmt::Display) -> String {
    let h = host.trim().trim_start_matches('[').trim_end_matches(']');
    if h.contains(':') { format!("[{h}]:{port}") } else { format!("{h}:{port}") }
}

/// Локальный адрес того же семейства, что и сервер: IPv4-сокет (`0.0.0.0`) до IPv6-сервера не достучится.
fn local_bind_addr(remote: &SocketAddr) -> SocketAddr {
    match remote {
        SocketAddr::V4(_) => SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
        SocketAddr::V6(_) => SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)),
    }
}

/// UDP-сокет к серверу голоса: сначала узнаём адрес (имя может дать и IPv4, и IPv6), потом открываем
/// сокет под его семейство. Пробуем адреса по очереди, пока какой-нибудь не подойдёт.
fn connect_udp(addr: &str) -> Result<UdpSocket, String> {
    let targets = addr.to_socket_addrs().map_err(|e| format!("адрес {addr}: {e}"))?;
    let mut last = format!("адрес {addr}: не нашёлся");
    for t in targets {
        match UdpSocket::bind(local_bind_addr(&t)).and_then(|s| s.connect(t).map(|_| s)) {
            Ok(s) => return Ok(s),
            Err(e) => last = format!("connect {t}: {e}"),
        }
    }
    Err(last)
}

/// Запускает рабочий поток для новой UDP-сессии. Старый поток остановится, когда его `VoiceCtl` уничтожат.
pub fn start(conn: Link, outbox: Outbox, log: Logger) -> VoiceCtl {
    let stop = Arc::new(AtomicBool::new(false));
    let stale = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let stop2 = Arc::clone(&stop);
    let stale2 = Arc::clone(&stale);
    std::thread::spawn(move || {
        if let Err(e) = run(conn, rx, stop2, stale2, outbox, &log) {
            log(format!("голос остановлен: {e}"));
        }
    });
    VoiceCtl { stop, stale, tx }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Передача в одну активацию: идёт ли сейчас и номер последнего пакета (у каждой активации свой счёт).
#[derive(Default)]
struct ActStream {
    active: bool,
    seq: i64,
}

/// Что активация делает с этим кадром (`ClientActivation.Result`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Res {
    Not,
    Activated,
    End,
}

/// Что делать с кадром на сервере SVC. Сервер отдаёт голос того, кто в группе, группе (в «открытой» —
/// ещё и тем, кто рядом), поэтому шлём одним потоком, а «только рядом» в обычной группе делается так: пока
/// говорит «Локальный», мод выходит из группы и возвращается после (groups.rs). `route` — `groups::ROUTE_*`,
/// `can_detach` — из группы можно так выйти. Возвращает (в группе говорит «Локальный» — для groups.rs;
/// голос придержан до выхода или возвращения — его надо накопить и отправить следом).
fn svc_route(
    decisions: &mut [(usize, Res, settings::ActKind)],
    acts: &[plasmo::Activation],
    in_group: bool,
    route: u8,
    can_detach: bool,
    active: &dyn Fn(&[u8; 16]) -> bool,
) -> (bool, bool) {
    let on = |d: &(usize, Res, settings::ActKind), near: bool| acts[d.0].proximity == near && d.1 == Res::Activated;
    let group_on = decisions.iter().any(|d| on(d, false));
    let local_on = decisions.iter().any(|d| on(d, true));
    let any = group_on || local_on;
    let wanted = local_on && !group_on;
    let stop = |d: &mut (usize, Res, settings::ActKind)| {
        d.1 = if active(&acts[d.0].id) { Res::End } else { Res::Not };
    };
    let has_group_act = decisions.iter().any(|d| !acts[d.0].proximity);
    let mut held = false;
    match route {
        // выходим из группы или возвращаемся: голос ждёт
        crate::groups::ROUTE_TRANSIT => {
            held = any;
            decisions.iter_mut().for_each(stop);
        }
        // временно вне группы: только «Локальный»; «Группы» ждёт возвращения
        crate::groups::ROUTE_LOCAL => {
            held = group_on;
            decisions.iter_mut().filter(|d| !acts[d.0].proximity).for_each(stop);
        }
        // в группе заговорил «Локальный»: ждём, пока сервер выпустит из группы
        _ if in_group && has_group_act && wanted && can_detach => {
            held = true;
            decisions.iter_mut().for_each(stop);
        }
        _ if in_group && has_group_act => {
            // в группе: всё, что сработало, — одним потоком по линии «Группы» (голос с предзаписью важнее клавиши)
            let kinds: Vec<settings::ActKind> = decisions.iter().filter(|d| d.1 == Res::Activated).map(|d| d.2).collect();
            let kind = kinds.iter().copied().find(|k| *k == settings::ActKind::Voice).or(kinds.first().copied());
            for d in decisions.iter_mut() {
                if acts[d.0].proximity {
                    stop(d);
                } else if let Some(k) = kind {
                    d.1 = Res::Activated;
                    d.2 = k;
                }
            }
        }
        _ => decisions.iter_mut().filter(|d| !acts[d.0].proximity && d.1 == Res::Activated).for_each(stop),
    }
    (wanted && in_group, held)
}

/// Решение для одного кадра: какие активации передают и какие замолчали.
/// `voice` — сработал ли голосовой порог (с предзаписью), `held(slot)` — зажата ли клавиша рации,
/// `active(id)` — шла ли передача, `silenced` — микрофон выключен целиком.
/// Логика как у Plasmo (`VoiceAudioCapture`): активации по весу; сработавшая непрозрачная (`transitive` =
/// false) глушит остальные своего рода (рядом / не рядом); «Локальный» решается последним.
fn decide(
    acts: &[plasmo::Activation],
    voice: bool,
    held: &dyn Fn(usize) -> bool,
    active: &dyn Fn(&[u8; 16]) -> bool,
    silenced: bool,
) -> Vec<(usize, Res, settings::ActKind)> {
    let stop = |id: &[u8; 16]| if active(id) { Res::End } else { Res::Not };
    let raw = |a: &plasmo::Activation, kind: settings::ActKind, parent: Res| match kind {
        settings::ActKind::Voice if voice => Res::Activated,
        settings::ActKind::Ptt if settings::ptt_slot_in(acts, &a.name).is_some_and(held) => Res::Activated,
        settings::ActKind::Inherit if parent == Res::Activated => Res::Activated,
        _ => stop(&a.id),
    };
    let parent_kind = settings::act_pref(settings::PROXIMITY).kind;
    let parent = acts.iter().find(|a| a.name == settings::PROXIMITY).map_or(Res::Not, |p| raw(p, parent_kind, Res::Not));
    let mut blocked = [false, false]; // [не рядом, рядом]
    let mut out = Vec::new();
    for (i, a) in acts.iter().enumerate().filter(|(_, a)| a.name != settings::PROXIMITY) {
        let pref = settings::act_pref(&a.name);
        let res = if silenced || pref.off || blocked[a.proximity as usize] { stop(&a.id) } else { raw(a, pref.kind, parent) };
        if res == Res::Activated && !a.transitive {
            blocked[a.proximity as usize] = true;
        }
        out.push((i, res, pref.kind));
    }
    if let Some(i) = acts.iter().position(|a| a.name == settings::PROXIMITY) {
        let pref = settings::act_pref(settings::PROXIMITY);
        let a = &acts[i];
        let res = if silenced || pref.off || blocked[a.proximity as usize] { stop(&a.id) } else { parent };
        out.push((i, res, pref.kind));
    }
    out
}

/// Источник чужого голоса: декодер живёт в рабочем потоке, а в динамики уходит только очередь сэмплов.
struct Source {
    dec: OpusDecoder,
    last_seq: i64,
    last_seen: Instant,
}

/// Очередь воспроизведения одного источника.
struct SrcQueue {
    q: VecDeque<f32>,
    /// Уже начали играть (накопили `PREFILL_FRAMES`).
    started: bool,
    /// Громкость этого игрока (1.0 — как есть). Обновляется с каждым пакетом его голоса.
    gain: f32,
    /// Чей голос (если сервер уже сказал) и до скольких блоков он слышен: по ним звук ставится в мир.
    nick: Option<String>,
    max_dist: f32,
    /// Пространственный звук этого источника (сглаженное направление, фильтры, задержка между ушами).
    sp: dsp::Spatializer,
    /// Голос привязан к месту в мире (игрок рядом). Группа SVC — нет: звучит ровно, по центру.
    positional: bool,
    /// Действует ползунок «Локальный»: голоса рядом и проверка звука. Не группы и не звуки мира —
    /// у них свои ползунки.
    local: bool,
}

impl SrcQueue {
    fn new(gain: f32) -> SrcQueue {
        SrcQueue {
            q: VecDeque::new(),
            started: false,
            gain,
            nick: None,
            max_dist: 0.0,
            sp: dsp::Spatializer::new(),
            positional: true,
            local: false,
        }
    }

    /// Громкость с учётом ползунка «Локальный» (`local_vol`), если он к этому источнику относится.
    fn effective_gain(&self, local_vol: f32) -> f32 {
        if self.local { self.gain * local_vol } else { self.gain }
    }
}

#[derive(Default)]
struct Queues {
    by_source: HashMap<[u8; 16], SrcQueue>,
}

struct Audio {
    cfg: SessionConfig,
    _in_stream: Option<cpal::Stream>,
    /// Какой по счёту выбор микрофона уже применён (settings::mic_gen).
    mic_gen: u32,
    _out_stream: Option<cpal::Stream>,
    /// Какой по счёту выбор устройства вывода уже применён (settings::out_gen).
    out_gen: u32,
    mic: Arc<Mutex<VecDeque<f32>>>,
    play: Arc<Mutex<Queues>>,
    enc: Option<OpusEncoder>,
    sources: HashMap<[u8; 16], Source>,
    /// Потоки голоса по активациям: идёт ли сейчас передача и номер последнего пакета.
    streams: HashMap<[u8; 16], ActStream>,
    /// SVC: один счётчик пакетов на весь микрофон (сервер не знает наших «активаций»).
    svc_seq: i64,
    /// SVC: голос, придержанный, пока мод выходит из группы или возвращается в неё.
    svc_hold: Vec<Vec<f32>>,
    speaking: bool,
    /// Активация нейросетью: сейчас «говорим» (гистерезис: включаемся увереннее, чем выключаемся).
    vad_open: bool,
    /// Уверенность нейросети в прошлом кадре: включаемся, только если речь два кадра подряд (щелчок — один).
    vad_prev: f32,
    /// Когда передавать: предзапись, гистерезис, мягкие края (`dsp::Gate`).
    gate: dsp::Gate,
    /// Какой по счёту профиль шума уже применён (settings::noise_profile_gen).
    profile_gen: u32,
    packet: [u8; 1500],
    /// Шумоподавление микрофона (сила в настройках, 0 — выключено).
    ns: denoise::Denoiser,
    /// Шумодав нейросетью (RNNoise), если он выбран в меню.
    rnn: Option<crate::rnnoise::Rnnoise>,
    /// Фильтр гула и автогромкость (переключатели во вкладке «Устройства»).
    hum: dsp::HighPass,
    agc: dsp::Agc,
}

/// Настоящие микрофоны PipeWire/PulseAudio: (имя источника, описание). Их ALSA не показывает: там только
/// общие `pipewire` и `pulse`, за которыми прячется системный выбор. Нет `pactl` — список пустой.
fn pulse_list(what: &str) -> Vec<(String, String)> {
    let Ok(out) = std::process::Command::new("pactl").args(["list", what]).env("LC_ALL", "C").output() else {
        return Vec::new();
    };
    parse_pactl_sources(&String::from_utf8_lossy(&out.stdout))
}

fn parse_pactl_sources(text: &str) -> Vec<(String, String)> {
    let mut list = Vec::new();
    let mut name: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("Source #") || line.starts_with("Sink #") {
            name = None;
        } else if let Some(n) = line.strip_prefix("Name: ") {
            name = Some(n.trim().to_string());
        } else if let Some(d) = line.strip_prefix("Description: ") {
            if let Some(n) = name.take() {
                if !n.ends_with(".monitor") {
                    list.push((n, d.trim().to_string()));
                }
            }
        }
    }
    list
}

/// Микрофоны для меню: (значение в конфиге, название). Источники PipeWire/Pulse идут как `pulse:<имя>`.
pub fn input_names() -> Vec<(String, String)> {
    device_names(true)
}

/// Устройства вывода для меню, так же, как микрофоны (синки PipeWire/Pulse идут как `pulse:<имя>`).
pub fn output_names() -> Vec<(String, String)> {
    device_names(false)
}

fn device_names(input: bool) -> Vec<(String, String)> {
    let mut list: Vec<(String, String)> = pulse_list(if input { "sources" } else { "sinks" })
        .into_iter()
        .map(|(n, d)| (format!("{PULSE_PREFIX}{n}"), d))
        .collect();
    let has_pulse = !list.is_empty();
    let host = cpal::default_host();
    let alsa: Vec<String> = if input {
        host.input_devices().map(|it| it.filter_map(|d| d.name().ok()).collect()).unwrap_or_default()
    } else {
        host.output_devices().map(|it| it.filter_map(|d| d.name().ok()).collect()).unwrap_or_default()
    };
    for n in alsa {
        // Общие виртуальные устройства при наличии настоящих списка только запутывают.
        if has_pulse && matches!(n.as_str(), "default" | "pipewire" | "pulse" | "null") {
            continue;
        }
        list.push((n.clone(), n));
    }
    list
}

const PULSE_PREFIX: &str = "pulse:";

/// Направляет запись в источник PipeWire/Pulse через окружение. `set_var` небезопасен, пока другие потоки
/// читают окружение, поэтому окно сделано как можно короче, а вне Linux (где pactl-источников нет) ничего не трогаем.
#[cfg(target_os = "linux")]
fn route_input(src: &str) {
    #[allow(unused_unsafe)] // в редакции 2024 set_var объявлен unsafe
    unsafe {
        std::env::set_var("PULSE_SOURCE", src);
        std::env::set_var("PIPEWIRE_NODE", src);
    }
}
#[cfg(not(target_os = "linux"))]
fn route_input(_src: &str) {}

#[cfg(target_os = "linux")]
fn unroute_input() {
    #[allow(unused_unsafe)]
    unsafe {
        std::env::remove_var("PULSE_SOURCE");
        std::env::remove_var("PULSE_SINK");
        std::env::remove_var("PIPEWIRE_NODE");
    }
}

#[cfg(target_os = "linux")]
fn route_output(sink: &str) {
    #[allow(unused_unsafe)]
    unsafe {
        std::env::set_var("PULSE_SINK", sink);
        std::env::set_var("PIPEWIRE_NODE", sink);
    }
}
#[cfg(not(target_os = "linux"))]
fn route_output(_sink: &str) {}
#[cfg(not(target_os = "linux"))]
fn unroute_input() {}

/// Открывает запись с микрофона, выбранного в меню (пусто — системный по умолчанию). Нет такого — берём по умолчанию.
fn open_input(mic: &Arc<Mutex<VecDeque<f32>>>, log: &Logger) -> Option<cpal::Stream> {
    if settings::mic_off() {
        log("микрофон отключён в меню: запись закрыта".into());
        return None;
    }
    let host = cpal::default_host();
    let config = StreamConfig { channels: 1, sample_rate: SampleRate(RATE), buffer_size: BufferSize::Default };
    let wanted = settings::mic_device();
    let by_name = |name: &str| {
        host.input_devices().ok().and_then(|mut it| it.find(|d| d.name().ok().as_deref() == Some(name)))
    };
    // Источник PipeWire/Pulse: направляем запись в него через переменные окружения, пока открываем поток
    // (системные «pulse»/«pipewire» читают их при подключении), и сразу убираем, чтобы не задеть вывод звука.
    let mut routed = false;
    let device = if let Some(src) = wanted.strip_prefix(PULSE_PREFIX) {
        route_input(src);
        routed = true;
        by_name("pulse").or_else(|| by_name("pipewire")).or_else(|| host.default_input_device())
    } else if wanted.is_empty() {
        host.default_input_device()
    } else {
        by_name(&wanted).or_else(|| {
            log(format!("микрофон «{wanted}» не найден, беру системный"));
            host.default_input_device()
        })
    };
    let input = device.and_then(|d| {
        let name = d.name().unwrap_or_default();
        let push = {
            let mic = Arc::clone(mic);
            move |samples: &mut dyn Iterator<Item = f32>| {
                let mut q = mic.lock_ok();
                q.extend(samples);
                if q.len() > MAX_QUEUE {
                    let n = q.len() - MAX_QUEUE;
                    q.drain(..n);
                }
            }
        };
        // сначала как нам удобно: моно, 48 кГц
        let direct = push.clone();
        let built = d.build_input_stream(
            &config,
            move |data: &[f32], _| direct(&mut data.iter().copied()),
            |e| eprintln!("ошибка микрофона: {e}"),
            None,
        );
        let s = match built {
            Ok(s) => s,
            Err(e) => {
                // Windows (WASAPI) и некоторые устройства открываются только в родном формате: берём его,
                // сводим каналы в моно и пересчитываем частоту в 48 кГц сами
                log(format!("микрофон «{name}» не открылся как моно 48 кГц ({e}), пробую родной формат"));
                open_input_native(&d, push, log).map_err(|e| log(format!("микрофон «{name}» не открылся: {e}"))).ok()?
            }
        };
        s.play().ok()?;
        log(format!("микрофон: {name} ({})", if wanted.is_empty() { "по умолчанию" } else { wanted.as_str() }));
        Some(s)
    });
    if routed {
        unroute_input();
    }
    if input.is_none() {
        log("микрофона нет: говорить не сможешь, слушать сможешь".into());
    }
    input
}

/// Микрофон в его родном формате (каналы, частота, тип сэмплов): сводим в моно и пересчитываем в 48 кГц.
fn open_input_native(
    d: &cpal::Device,
    push: impl FnMut(&mut dyn Iterator<Item = f32>) + Send + 'static,
    log: &Logger,
) -> Result<cpal::Stream, String> {
    let def = d.default_input_config().map_err(|e| e.to_string())?;
    let (channels, rate) = (def.channels().max(1) as usize, def.sample_rate().0);
    log(format!("микрофон: родной формат {channels} кан., {rate} Гц, {:?}", def.sample_format()));
    let config: StreamConfig = def.config();
    let state = Arc::new(Mutex::new((dsp::Resampler::<1>::new(rate, RATE), push, Vec::<[f32; 1]>::new(), Vec::<f32>::new())));
    let feed = move |data: &mut dyn Iterator<Item = f32>| {
        let mut g = state.lock_ok();
        let (rs, push, frames, out) = &mut *g;
        frames.clear();
        let mut acc = 0.0;
        for (i, v) in data.enumerate() {
            acc += v;
            if (i + 1) % channels == 0 {
                frames.push([acc / channels as f32]);
                acc = 0.0;
            }
        }
        out.clear();
        rs.push(frames, &mut |f| out.push(f[0]));
        push(&mut out.iter().copied());
    };
    let err = |e| eprintln!("ошибка микрофона: {e}");
    let s = match def.sample_format() {
        cpal::SampleFormat::F32 => {
            let feed = feed;
            d.build_input_stream(&config, move |data: &[f32], _| feed(&mut data.iter().copied()), err, None)
        }
        cpal::SampleFormat::I16 => {
            let feed = feed;
            d.build_input_stream(&config, move |data: &[i16], _| feed(&mut data.iter().map(|&v| v as f32 / 32_768.0)), err, None)
        }
        cpal::SampleFormat::U16 => {
            let feed = feed;
            d.build_input_stream(
                &config,
                move |data: &[u16], _| feed(&mut data.iter().map(|&v| (v as f32 - 32_768.0) / 32_768.0)),
                err,
                None,
            )
        }
        other => return Err(format!("формат {other:?} не поддерживается")),
    };
    s.map_err(|e| e.to_string())
}

/// Вывод в родном формате устройства: сводим голоса в 48 кГц стерео, пересчитываем в его частоту
/// и раскладываем по его каналам (левый, правый, остальные — тишина).
fn open_output_native(d: &cpal::Device, play: &Arc<Mutex<Queues>>, log: &Logger) -> Result<(cpal::Stream, bool), String> {
    let def = d.default_output_config().map_err(|e| e.to_string())?;
    let (channels, rate) = (def.channels().max(1) as usize, def.sample_rate().0);
    log(format!("вывод: родной формат {channels} кан., {rate} Гц, {:?}", def.sample_format()));
    let config: StreamConfig = def.config();
    let play = Arc::clone(play);
    let stereo = channels >= 2;
    // готовые кадры в частоте устройства, ещё не отданные
    let mut ready: VecDeque<[f32; 2]> = VecDeque::new();
    let mut rs = dsp::Resampler::<2>::new(RATE, rate);
    let mut chunk = vec![0f32; 256 * 2];
    let mut frames: Vec<[f32; 2]> = Vec::with_capacity(256);
    let mut fill = move |n_frames: usize, out: &mut dyn FnMut(usize, [f32; 2])| {
        while ready.len() < n_frames {
            mix_output(&play, &mut chunk, 2);
            frames.clear();
            frames.extend(chunk.chunks_exact(2).map(|c| [c[0], c[1]]));
            rs.push(&frames, &mut |f| ready.push_back(f));
        }
        for i in 0..n_frames {
            let f = ready.pop_front().unwrap_or([0.0, 0.0]);
            let f = if stereo { f } else { [(f[0] + f[1]) * 0.5, 0.0] };
            out(i, f);
        }
    };
    let err = |e| eprintln!("ошибка вывода: {e}");
    let s = match def.sample_format() {
        cpal::SampleFormat::F32 => d.build_output_stream(
            &config,
            move |data: &mut [f32], _| {
                data.fill(0.0);
                fill(data.len() / channels, &mut |i, f| {
                    data[i * channels] = f[0];
                    if channels >= 2 {
                        data[i * channels + 1] = f[1];
                    }
                });
            },
            err,
            None,
        ),
        cpal::SampleFormat::I16 => d.build_output_stream(
            &config,
            move |data: &mut [i16], _| {
                data.fill(0);
                let to = |v: f32| (v.clamp(-1.0, 1.0) * 32_767.0) as i16;
                fill(data.len() / channels, &mut |i, f| {
                    data[i * channels] = to(f[0]);
                    if channels >= 2 {
                        data[i * channels + 1] = to(f[1]);
                    }
                });
            },
            err,
            None,
        ),
        other => return Err(format!("формат {other:?} не поддерживается")),
    };
    s.map(|s| (s, stereo)).map_err(|e| e.to_string())
}

/// Открывает вывод на устройство из меню (пусто — системное). Сначала стерео (для направленного звука),
/// не вышло — моно: тогда левое и правое ухо сводятся вместе.
fn open_output(play: &Arc<Mutex<Queues>>, log: &Logger) -> Option<cpal::Stream> {
    let host = cpal::default_host();
    let wanted = settings::out_device();
    let by_name = |name: &str| {
        host.output_devices().ok().and_then(|mut it| it.find(|d| d.name().ok().as_deref() == Some(name)))
    };
    let mut routed = false;
    let device = if let Some(sink) = wanted.strip_prefix(PULSE_PREFIX) {
        route_output(sink);
        routed = true;
        by_name("pulse").or_else(|| by_name("pipewire")).or_else(|| host.default_output_device())
    } else if wanted.is_empty() {
        host.default_output_device()
    } else {
        by_name(&wanted).or_else(|| {
            log(format!("вывод «{wanted}» не найден, беру системный"));
            host.default_output_device()
        })
    };
    let out = device.and_then(|d| {
        let name = d.name().unwrap_or_default();
        for channels in [2u16, 1] {
            let config = StreamConfig { channels, sample_rate: SampleRate(RATE), buffer_size: BufferSize::Default };
            let play = Arc::clone(play);
            let built = d.build_output_stream(
                &config,
                move |data: &mut [f32], _| mix_output(&play, data, channels as usize),
                |e| eprintln!("ошибка вывода: {e}"),
                None,
            );
            match built {
                Ok(s) if s.play().is_ok() => {
                    settings::set_out_stereo(channels == 2);
                    log(format!("вывод: {name} ({})", if channels == 2 { "стерео" } else { "моно" }));
                    return Some(s);
                }
                Ok(_) => log(format!("вывод «{name}» ({channels} кан.) не запустился")),
                Err(e) => log(format!("вывод «{name}» ({channels} кан.) не открылся: {e}")),
            }
        }
        // Windows (WASAPI) и некоторые устройства открываются только в родном формате
        match open_output_native(&d, play, log) {
            Ok((s, stereo)) if s.play().is_ok() => {
                settings::set_out_stereo(stereo);
                log(format!("вывод: {name} (родной формат, {})", if stereo { "стерео" } else { "моно" }));
                Some(s)
            }
            Ok(_) => {
                log(format!("вывод «{name}» в родном формате не запустился"));
                None
            }
            Err(e) => {
                log(format!("вывод «{name}» в родном формате не открылся: {e}"));
                None
            }
        }
    });
    if routed {
        unroute_input();
    }
    if out.is_none() {
        log("устройства вывода нет: ничего не услышишь".into());
    }
    settings::set_out_ok(out.is_some());
    out
}

/// Сводит все источники в буфер вывода: у каждого своя громкость, место в мире и глухость за стеной.
fn mix_output(play: &Arc<Mutex<Queues>>, data: &mut [f32], channels: usize) {
    data.fill(0.0);
    let mode = dsp::Mode {
        directional: settings::directional_on() && channels >= 2,
        hrtf: settings::hrtf_on(),
        occlusion: settings::occlusion_on(),
    };
    let (vol, off) = (settings::voice_volume(), settings::voice_off());
    let mut qs = play.lock_ok();
    for src in qs.by_source.values_mut() {
        if !src.started {
            if src.q.len() >= PREFILL_FRAMES * FRAME {
                src.started = true;
            } else {
                continue;
            }
        }
        let target = if src.positional {
            src.nick.as_deref().and_then(|n| spatial::params(n, src.max_dist)).unwrap_or(dsp::Params::CENTER)
        } else {
            dsp::Params::CENTER
        };
        // голос без места (группа, проверка звука) — по центру и без HRTF: окраска ушей ему ни к чему
        let m = if src.positional { mode } else { dsp::Mode { directional: false, ..mode } };
        // «Локальный» — только голосам рядом: группы и музыку он не трогает (у них свои ползунки)
        let gain = src.effective_gain(vol);
        if src.sp.mix(&mut src.q, data, channels, gain, &target, m) {
            src.started = false;
        }
    }
    drop(qs);
    for v in data.iter_mut() {
        *v = if off { 0.0 } else { dsp::soft_limit(*v).clamp(-1.0, 1.0) };
    }
}

fn open_streams(
    mic: &Arc<Mutex<VecDeque<f32>>>,
    play: &Arc<Mutex<Queues>>,
    log: &Logger,
) -> (Option<cpal::Stream>, Option<cpal::Stream>) {
    let input = open_input(mic, log);
    let output = open_output(play, log);
    (input, output)
}

impl Audio {
    fn new(cfg: SessionConfig, log: &Logger) -> Audio {
        let mic = Arc::new(Mutex::new(VecDeque::with_capacity(MAX_QUEUE)));
        let play = Arc::new(Mutex::new(Queues::default()));
        let mic_gen = settings::mic_gen();
        let out_gen = settings::out_gen();
        let (i, o) = open_streams(&mic, &play, log);
        settings::set_mic_ok(i.is_some());
        settings::set_mic_list(input_names());
        settings::set_out_list(output_names());
        settings::set_distance(cfg.distance.max(0) as u32);
        settings::set_connected(!cfg.offline);
        let enc = OpusEncoder::new(RATE as i32, 1, Application::Voip).ok().map(|mut e| {
            e.bitrate_bps = if cfg.bitrate > 0 { cfg.bitrate } else { 24_000 };
            e.signal_type = Some(SignalType::Voice);
            e
        });
        if enc.is_none() {
            log("Opus-кодировщик не создался".into());
        }
        Audio {
            cfg,
            _in_stream: i,
            mic_gen,
            _out_stream: o,
            out_gen,
            mic,
            play,
            enc,
            sources: HashMap::new(),
            streams: HashMap::new(),
            svc_seq: 0,
            svc_hold: Vec::new(),
            speaking: false,
            vad_open: false,
            vad_prev: 0.0,
            gate: dsp::Gate::new(),
            profile_gen: settings::noise_profile_gen(),
            packet: [0u8; 1500],
            ns: {
                let mut d = denoise::Denoiser::new();
                d.set_profile(settings::noise_profile());
                d
            },
            rnn: None,
            hum: dsp::HighPass::new(80.0),
            agc: dsp::Agc::new(),
        }
    }

    /// Применяет выбор микрофона из меню: открывает запись заново и обновляет список устройств.
    fn sync_mic(&mut self, log: &Logger) {
        if settings::take_learn_request() {
            self.ns.start_learning();
            settings::set_learn_progress(Some(0.0));
        }
        let pg = settings::noise_profile_gen();
        if pg != self.profile_gen {
            self.profile_gen = pg;
            self.ns.set_profile(settings::noise_profile());
        }
        if settings::take_mic_refresh() {
            settings::set_mic_list(input_names());
            settings::set_out_list(output_names());
        }
        let out = settings::out_gen();
        if out != self.out_gen {
            self.out_gen = out;
            self._out_stream = None; // очереди источников остаются: голос продолжится на новом устройстве
            self._out_stream = open_output(&self.play, log);
        }
        let wanted = settings::mic_gen();
        if wanted != self.mic_gen {
            self.mic_gen = wanted;
            self._in_stream = None; // старый поток закрываем до открытия нового
            self.mic.lock_ok().clear();
            self.ns.reset();
            self.rnn = None;
            self.hum.reset();
            self.agc.reset();
            self.gate.reset();
            self._in_stream = open_input(&self.mic, log);
            settings::set_mic_ok(self._in_stream.is_some());
        }
    }

    /// Забирает накопленные кадры микрофона, кодирует и шлёт, пока ты говоришь.
    fn pump_mic(&mut self, sock: &UdpSocket, link: &Link, outbox: &Outbox) {
        let silenced = mute_file().exists() || settings::mic_silenced();
        let gain = settings::mic_gain();
        let threshold = settings::threshold();
        let noise = settings::noise_strength();
        loop {
            let mut frame: Vec<f32> = {
                let mut q = self.mic.lock_ok();
                if q.len() < FRAME {
                    return;
                }
                q.drain(..FRAME).collect()
            };
            if settings::hum_filter() {
                self.hum.process(&mut frame);
            }
            // «Запомнить шум»: пишем тишину до шумоподавления (профиль — это шум, который оно уберёт)
            if self.ns.learning().is_some() {
                if let Some(profile) = self.ns.learn(&frame) {
                    settings::set_noise_profile(Some(profile));
                    settings::set_learn_progress(None);
                    settings::save_noise_profile();
                } else {
                    settings::set_learn_progress(self.ns.learning());
                }
            }
            // шумодав: нейросеть RNNoise (как в Plasmo) или своё спектральное; переключение — с чистого листа.
            // Нейросеть нужна и для активации по голосу: тогда она идёт всегда (без шумодава — только слушает).
            let ai_clean = settings::noise_ai() && noise > 0.0;
            if ai_clean || settings::vad_ai() {
                let r = self.rnn.get_or_insert_with(crate::rnnoise::Rnnoise::new);
                r.process(&mut frame, if ai_clean { noise } else { 0.0 });
                settings::set_speech(r.vad());
                self.ns.process(&mut frame, if ai_clean { 0.0 } else { noise });
            } else {
                self.rnn = None;
                settings::set_speech(0.0);
                self.ns.process(&mut frame, noise);
            }
            if (gain - 1.0).abs() > f32::EPSILON {
                for v in frame.iter_mut() {
                    *v = (*v * gain).clamp(-1.0, 1.0);
                }
            }
            let rms = (frame.iter().map(|s| s * s).sum::<f32>() / FRAME as f32).sqrt();
            settings::set_level(rms); // меню рисует по нему полосу уровня
            // Активация: по нейросети (речь ли это, а не громко ли) или по порогу в дБ.
            let (loud, by_ai) = if settings::vad_ai() {
                let p = settings::speech();
                let on = rms >= VAD_FLOOR && if self.vad_open { p >= VAD_KEEP } else { p >= VAD_START && self.vad_prev >= VAD_KEEP };
                self.vad_prev = p;
                self.vad_open = on;
                (on, Some(on))
            } else {
                self.vad_open = false;
                (rms >= threshold, None)
            };
            let current = frame.clone();
            // Голосовой порог с гистерезисом и предзаписью — общий для всех активаций «по голосу».
            let voice_frames = match self.gate.process(frame, rms, threshold, by_ai, silenced) {
                dsp::GateOut::Send(frames) => frames,
                _ => Vec::new(),
            };
            self.frame_activations(voice_frames, current, loud, silenced, sock, link, outbox);
        }
    }

    /// Разослать кадр по активациям сервера (или закончить реплику в тех, что замолчали).
    #[allow(clippy::too_many_arguments)]
    fn frame_activations(
        &mut self,
        voice_frames: Vec<Vec<f32>>,
        current: Vec<f32>,
        loud: bool,
        silenced: bool,
        sock: &UdpSocket,
        link: &Link,
        outbox: &Outbox,
    ) {
        let mut acts = settings::server_activations();
        if acts.is_empty() {
            // сервер не прислал список (или не разобрали): одна «Локальная» активация, как раньше
            acts.push(plasmo::Activation {
                name: settings::PROXIMITY.to_string(),
                translation: String::new(),
                id: self.cfg.activation_id,
                distances: Vec::new(),
                default_distance: self.cfg.distance as i32,
                proximity: true,
                transitive: true,
                stereo: false,
                weight: 0,
            });
        }
        let streams = &self.streams;
        let mut decisions = decide(
            &acts,
            !voice_frames.is_empty(),
            &|slot| settings::slot_held(slot),
            &|id| streams.get(id).is_some_and(|s| s.active),
            silenced,
        );
        // SVC: куда идёт голос, решает сервер — по группе (svc_route). Голос, придержанный на время выхода из
        // группы или возвращения, копится и уходит следом, как только линия готова: начало фразы не теряется.
        let mut flush: Vec<Vec<f32>> = Vec::new();
        if matches!(link, Link::Svc { .. }) {
            use std::sync::atomic::Ordering::Relaxed;
            let would_voice = decisions.iter().any(|&(_, r, k)| r == Res::Activated && k == settings::ActKind::Voice);
            let route = crate::groups::SVC_ROUTE.load(Relaxed);
            let in_group = roster::self_group().is_some() || route != crate::groups::ROUTE_NORMAL;
            let (wanted, held) = svc_route(
                &mut decisions,
                &acts,
                in_group,
                route,
                crate::groups::SVC_CAN_DETACH.load(Relaxed),
                &|id| streams.get(id).is_some_and(|s| s.active),
            );
            crate::groups::SVC_LOCAL_WANTED.store(wanted, Relaxed);
            if held {
                let frames = if would_voice { voice_frames.clone() } else { vec![current.clone()] };
                self.svc_hold.extend(frames);
                let over = self.svc_hold.len().saturating_sub(SVC_HOLD_MAX);
                self.svc_hold.drain(..over);
            } else if !self.svc_hold.is_empty() {
                flush = std::mem::take(&mut self.svc_hold);
                // уже замолчали — придержанное всё равно уходит, по той линии, куда идёт голос сейчас
                if !decisions.iter().any(|d| d.1 == Res::Activated) {
                    let to_group = route == crate::groups::ROUTE_NORMAL && roster::self_group().is_some();
                    if let Some(d) = decisions.iter_mut().find(|d| acts[d.0].proximity != to_group) {
                        d.1 = Res::Activated;
                        d.2 = settings::ActKind::Voice;
                    }
                }
            }
        }
        let any = decisions.iter().any(|d| d.1 == Res::Activated);
        // куда уходит голос: рядом или в группу (у Plasmo это разные активации, у SVC решает сервер по группе)
        let (near, group) = match link {
            Link::Svc { .. } if crate::groups::SVC_ROUTE.load(std::sync::atomic::Ordering::Relaxed) == crate::groups::ROUTE_LOCAL => (any, false),
            Link::Svc { .. } if roster::self_group().is_some() => (false, any),
            Link::Svc { .. } | Link::Offline => (any, false),
            Link::Plasmo(_) => (
                decisions.iter().any(|&(i, r, _)| r == Res::Activated && acts[i].proximity),
                decisions.iter().any(|&(i, r, _)| r == Res::Activated && !acts[i].proximity),
            ),
        };
        settings::set_speaking_to(near && !silenced, group && !silenced);
        let voice_used = decisions.iter().any(|&(_, r, k)| {
            r == Res::Activated
                && (k == settings::ActKind::Voice || (k == settings::ActKind::Inherit && !settings::push_to_talk()))
        });
        // что кодируем: при голосе — кадры с предзаписью, иначе (только рация) — текущий кадр
        // проверка звука: слышно всегда, без рации и порога (кадр уже прошёл шумодав и усиление)
        let test_frame = settings::mic_test().then(|| current.clone());
        let mut timeline = if voice_used { voice_frames } else if any { vec![current] } else { Vec::new() };
        if !flush.is_empty() {
            flush.append(&mut timeline);
            timeline = flush;
        }
        if settings::agc_on() {
            let last = timeline.len().saturating_sub(1);
            for (i, f) in timeline.iter_mut().enumerate() {
                self.agc.process(f, i == last && loud);
            }
        }

        let offline = matches!(link, Link::Offline);
        if settings::mic_test() || offline {
            // Проверка звука: слышите то же, что услышали бы другие, но на сервер ничего не уходит.
            // Без сервера (`Link::Offline`) голос только для проверки — и тоже никуда не уходит.
            self.end_all(&acts, outbox);
            if let Some(cur) = test_frame {
                let mut qs = self.play.lock_ok();
                let src = qs.by_source.entry(TEST_SOURCE).or_insert_with(|| SrcQueue::new(1.0));
                src.local = true; // себя слышно с той же громкостью, что игроков рядом
                // всегда текущий кадр: кадры активации с предзаписью повторили бы уже сыгранное
                src.q.extend(cur.iter().copied());
                if src.q.len() > MAX_QUEUE {
                    let n = src.q.len() - MAX_QUEUE;
                    src.q.drain(..n);
                }
            }
            return;
        }

        // каждый кадр кодируем один раз и шлём во все сработавшие активации
        let mut packets: Vec<Vec<u8>> = Vec::with_capacity(timeline.len());
        if any {
            if let Some(enc) = self.enc.as_mut() {
                for f in &timeline {
                    if let Ok(len) = enc.encode(f, FRAME, &mut self.packet) {
                        // Plasmo шифрует сам Opus, SVC — весь UDP-пакет целиком (ниже, при отправке)
                        packets.push(match link {
                            Link::Plasmo(_) => crypto::aes_encrypt(&self.cfg.aes, &self.packet[..len]),
                            Link::Svc { .. } | Link::Offline => self.packet[..len].to_vec(),
                        });
                    }
                }
            }
        }
        for (i, res, kind) in decisions {
            let a = &acts[i];
            let distance = settings::act_distance(a).clamp(0, i16::MAX as i32) as i16;
            let st = self.streams.entry(a.id).or_default();
            match res {
                Res::Activated => {
                    let uses_voice = kind == settings::ActKind::Voice
                        || (kind == settings::ActKind::Inherit && !settings::push_to_talk());
                    // рация берёт только последний кадр: предзапись — для голосового порога
                    let send: &[Vec<u8>] =
                        if uses_voice || packets.is_empty() { &packets } else { &packets[packets.len() - 1..] };
                    for data in send {
                        st.seq += 1;
                        let pkt = match link {
                            Link::Plasmo(c) => plasmo::udp_player_audio(&c.secret, now_ms(), st.seq, data, &a.id, distance),
                            Link::Svc { session, .. } => {
                                self.svc_seq += 1;
                                session.wrap(svc::T_MIC, &svc::mic(data, self.svc_seq, false))
                            }
                            Link::Offline => continue,
                        };
                        let _ = sock.send(&pkt);
                    }
                    st.active = true;
                }
                Res::End => {
                    // у SVC конца реплики нет: сервер сам понимает по тишине
                    if matches!(link, Link::Plasmo(_)) {
                        st.seq += 1;
                        outbox.lock_ok().push(plasmo::player_audio_end(st.seq, &a.id, distance));
                    }
                    st.active = false;
                }
                Res::Not => {}
            }
        }
        if any != self.speaking {
            self.speaking = any;
            settings::set_speaking(any);
        }
    }

    /// Закончить все идущие передачи (проверка звука, смена микрофона).
    fn end_all(&mut self, acts: &[plasmo::Activation], outbox: &Outbox) {
        for a in acts {
            let distance = settings::act_distance(a).clamp(0, i16::MAX as i32) as i16;
            if let Some(st) = self.streams.get_mut(&a.id).filter(|s| s.active) {
                st.seq += 1;
                st.active = false;
                outbox.lock_ok().push(plasmo::player_audio_end(st.seq, &a.id, distance));
            }
        }
        if self.speaking {
            self.speaking = false;
            settings::set_speaking(false);
            settings::set_speaking_to(false, false);
        }
    }

    /// Чужой голос Plasmo: расшифровать, узнать, чей он, и сыграть.
    fn on_source_audio(&mut self, a: plasmo::SourceAudio, outbox: &Outbox) {
        let Some(opus) = crypto::aes_decrypt(&self.cfg.aes, &a.data) else { return };
        // Чей это звук и где он: известно из SourceInfo; пока не знаем, спрашиваем у сервера (SourceInfoRequest, тип 15).
        let src = roster::source(a.source_id);
        if src.is_none() && roster::should_ask(a.source_id) {
            let mut ask = vec![15u8];
            ask.extend_from_slice(&a.source_id);
            outbox.lock_ok().push(ask);
        }
        let dist = a.distance.max(0) as f32;
        match src {
            Some(roster::Src::Player { nick, group }) => self.play_voice(a.source_id, a.seq, &opus, dist, Some(nick), !group, group),
            // звук мира (пластинка pv-addon-discs, моб): ставим в его точку, со всеми фишками — стены, обход углов
            Some(roster::Src::Static { pos, name }) => {
                crate::notes::set_title(&spatial::emitter_key(&a.source_id), name.as_deref());
                self.play_world(a.source_id, a.seq, &opus, dist, spatial::At::Pos(pos))
            }
            Some(roster::Src::Entity { id, name }) => {
                crate::notes::set_title(&spatial::emitter_key(&a.source_id), name.as_deref());
                self.play_world(a.source_id, a.seq, &opus, dist, spatial::At::Entity(id))
            }
            Some(roster::Src::Direct { sender }) => self.play_voice(a.source_id, a.seq, &opus, dist, sender, false, false),
            None => self.play_voice(a.source_id, a.seq, &opus, dist, None, false, false),
        }
    }

    /// Чужой голос SVC (Opus уже открыт: шифровался весь пакет). Имя — из состояний игроков SVC.
    fn on_svc_sound(&mut self, s: svc::Sound) {
        let nick = roster::nick_by_uuid(u128::from_be_bytes(s.sender));
        let positional = matches!(s.kind, svc::SoundKind::Player { .. });
        let group = matches!(s.kind, svc::SoundKind::Group);
        self.play_voice(s.channel, s.seq, &s.opus, s.distance, nick, positional, group);
    }

    /// Звук мира (не голос): своя громкость «Музыка и звуки», в списке говорящих и над никами не отмечается.
    fn play_world(&mut self, source_id: [u8; 16], seq: i64, opus: &[u8], distance: f32, at: spatial::At) {
        let key = spatial::emitter_key(&source_id);
        spatial::emit(&key, at);
        settings::set_server_has_world(true); // звук мира пришёл: значит, его ползунок нужен
        let Some(decoded) = self.decode(source_id, seq, opus) else { return };
        // ноты над пластинкой вылетают в такт: громкость кадра
        let rms = (decoded.iter().map(|s| s * s).sum::<f32>() / decoded.len().max(1) as f32).sqrt();
        crate::notes::on_audio(&key, rms);
        // выбор взглядом: тот, на который смотрим, громко, остальные рядом — тихо фоном
        let gain = settings::music_volume_pct() as f32 / 100.0 * crate::notes::focus_gain(&key);
        self.enqueue(source_id, decoded, gain, distance, Some(key), true, false);
    }

    /// Декодировать Opus источника (с заполнением потерь). `None` — старый или повторный пакет.
    fn decode(&mut self, source_id: [u8; 16], seq: i64, opus: &[u8]) -> Option<Vec<f32>> {
        let src = match self.sources.get_mut(&source_id) {
            Some(s) => s,
            None => {
                let dec = OpusDecoder::new(RATE as i32, 1).ok()?;
                self.sources.entry(source_id).or_insert(Source { dec, last_seq: seq.saturating_sub(1), last_seen: Instant::now() })
            }
        };
        if seq <= src.last_seq {
            return None;
        }
        let gap = seq.saturating_sub(src.last_seq).saturating_sub(1).clamp(0, 3);
        src.last_seq = seq;
        src.last_seen = Instant::now();
        let mut pcm = vec![0f32; FRAME];
        let mut decoded: Vec<f32> = Vec::with_capacity(FRAME * (gap as usize + 1));
        for _ in 0..gap {
            if src.dec.decode(&[], FRAME, &mut pcm).is_ok() {
                decoded.extend_from_slice(&pcm);
            }
        }
        if src.dec.decode(opus, FRAME, &mut pcm).is_ok() {
            decoded.extend_from_slice(&pcm);
        }
        Some(decoded)
    }

    /// Общая часть: декодировать Opus (с заполнением потерь) и положить в очередь воспроизведения.
    #[allow(clippy::too_many_arguments)]
    fn play_voice(
        &mut self,
        source_id: [u8; 16],
        seq: i64,
        opus: &[u8],
        distance: f32,
        nick: Option<String>,
        positional: bool,
        group: bool,
    ) {
        let Some(decoded) = self.decode(source_id, seq, opus) else { return };
        if let Some(n) = &nick {
            roster::mark_talking(n);
            if group {
                roster::mark_talking_group(n);
            } else {
                roster::mark_talking_near(n);
            }
            // громкость его голоса: капсула значка над ником заполняется по ней
            let rms = (decoded.iter().map(|s| s * s).sum::<f32>() / decoded.len().max(1) as f32).sqrt();
            roster::mark_level(n, rms);
        }
        // Громкость именно этого игрока; пока не знаем, чей это голос, играем как есть.
        let mut gain = nick.as_deref().map(settings::player_volume).unwrap_or(1.0);
        if group {
            gain *= settings::group_volume_pct() as f32 / 100.0; // ползунок «Группы» во вкладке «Громкость»
            settings::set_server_has_groups(true);
        }
        self.enqueue(source_id, decoded, gain, distance, nick, positional, !group);
    }

    /// Положить звук источника в очередь воспроизведения. `key` — ник игрока или ключ звука мира (`@…`):
    /// по нему звук ставится в мир.
    #[allow(clippy::too_many_arguments)]
    fn enqueue(
        &mut self,
        source_id: [u8; 16],
        decoded: Vec<f32>,
        gain: f32,
        distance: f32,
        key: Option<String>,
        positional: bool,
        local: bool,
    ) {
        let mut qs = self.play.lock_ok();
        let src = qs.by_source.entry(source_id).or_insert_with(|| SrcQueue::new(gain));
        src.gain = gain;
        src.max_dist = distance.max(0.0);
        src.positional = positional;
        src.local = local;
        if src.nick != key {
            src.nick = key;
        }
        src.q.extend(decoded);
        if src.q.len() > MAX_QUEUE {
            let n = src.q.len() - MAX_QUEUE;
            src.q.drain(..n);
        }
    }

    fn prune(&mut self) {
        let now = Instant::now();
        self.sources.retain(|_, s| now.duration_since(s.last_seen) < Duration::from_secs(30));
        let alive: Vec<[u8; 16]> = self.sources.keys().copied().collect();
        self.play.lock_ok().by_source.retain(|k, _| *k == TEST_SOURCE || alive.contains(k));
    }
}

/// Где UDP-сессия SVC: шлём Authenticate, потом ConnectionCheck, потом просто держим связь.
enum SvcStage {
    Auth,
    /// Ждём ConnectionCheckAck; внутри — тело ConnectionCheck (nonce в 2.7, пусто в 2.6).
    Check(Vec<u8>),
    Ready,
}

fn run(
    conn: Link,
    rx: mpsc::Receiver<SessionConfig>,
    stop: Arc<AtomicBool>,
    stale: Arc<AtomicBool>,
    outbox: Outbox,
    log: &Logger,
) -> Result<(), String> {
    let addr = conn.addr();
    let sock = connect_udp(&addr)?;
    sock.set_read_timeout(Some(Duration::from_millis(10))).map_err(|e| e.to_string())?;
    let proto = match conn {
        Link::Svc { .. } => "Simple Voice Chat",
        Link::Plasmo(_) => "Plasmo Voice",
        Link::Offline => "без сервера, проверка звука",
    };
    log(format!("UDP-сессия к {addr} ({proto})"));

    let mut audio: Option<Audio> = None;
    let mut buf = [0u8; 4096];
    let mut last_ping = Instant::now() - Duration::from_secs(10);
    let mut last_prune = Instant::now();
    let mut last_stats = Instant::now();
    let (mut got, mut got_audio, mut sent_pings) = (0u64, 0u64, 0u64);
    // Сколько пакетов каждого UDP-типа пришло (индекс = тип, всё чужое в последней ячейке).
    let mut by_type = [0u64; 12];
    let mut last_rx = Instant::now();
    let mut svc_stage = SvcStage::Auth;
    // SVC: сколько можно молчать, прежде чем сессия считается мёртвой
    let stale_after = match &conn {
        Link::Svc { keep_alive_ms, .. } => Duration::from_millis((*keep_alive_ms).clamp(500, 30_000) as u64 * 10),
        Link::Plasmo(_) => STALE_AFTER,
        Link::Offline => Duration::from_secs(24 * 3600),
    };

    while !stop.load(Ordering::Relaxed) {
        match &conn {
            Link::Plasmo(c) => {
                if last_ping.elapsed() >= Duration::from_secs(3) {
                    let _ = sock.send(&plasmo::udp_ping(&c.secret, now_ms()));
                    sent_pings += 1;
                    last_ping = Instant::now();
                }
            }
            Link::Svc { session, .. } => {
                // рукопожатие раз в секунду, пока сервер не подтвердит
                if last_ping.elapsed() >= Duration::from_secs(1) {
                    match &svc_stage {
                        SvcStage::Auth => {
                            let _ = sock.send(&session.wrap(svc::T_AUTHENTICATE, &session.authenticate_body()));
                        }
                        SvcStage::Check(body) => {
                            let _ = sock.send(&session.wrap(svc::T_CONNECTION_CHECK, body));
                        }
                        SvcStage::Ready => {}
                    }
                    sent_pings += 1;
                    last_ping = Instant::now();
                }
            }
            Link::Offline => {}
        }
        while let Ok(cfg) = rx.try_recv() {
            log(format!(
                "голос включён: дистанция {}, битрейт {}",
                cfg.distance,
                if cfg.bitrate > 0 { cfg.bitrate } else { 24_000 }
            ));
            audio = Some(Audio::new(cfg, log));
        }
        if let Ok(n) = sock.recv(&mut buf) {
            got += 1;
            match &conn {
                Link::Offline => {}
                Link::Plasmo(_) => {
                    last_rx = Instant::now();
                    match plasmo::udp_split(&buf[..n]) {
                        Some((3, body)) => {
                            by_type[3] += 1;
                            if let (Some(a), Some(au)) = (plasmo::parse_source_audio(body), audio.as_mut()) {
                                got_audio += 1;
                                au.on_source_audio(a, &outbox);
                            }
                        }
                        Some((ty, _)) => by_type[(ty as usize).min(11)] += 1,
                        None => by_type[0] += 1, // не наш формат (нет магии)
                    }
                }
                Link::Svc { session, .. } => match session.unwrap(&buf[..n]) {
                    Some((ty, body)) => {
                        last_rx = Instant::now();
                        by_type[(ty as usize).min(11)] += 1;
                        match ty {
                            svc::T_AUTHENTICATE_ACK => {
                                if let (SvcStage::Auth, Some(check)) = (&svc_stage, session.auth_ack(&body)) {
                                    log("SVC: сервер принял ключ, проверяю связь".into());
                                    svc_stage = SvcStage::Check(check);
                                    last_ping = Instant::now() - Duration::from_secs(2);
                                }
                            }
                            svc::T_CONNECTION_CHECK_ACK => {
                                if matches!(svc_stage, SvcStage::Check(_)) {
                                    log("SVC: связь есть, голос подключён".into());
                                    svc_stage = SvcStage::Ready;
                                    settings::set_connected(true);
                                }
                            }
                            svc::T_KEEP_ALIVE => {
                                let _ = sock.send(&session.wrap(svc::T_KEEP_ALIVE, &[]));
                            }
                            t if svc::is_ping(t) => {
                                let _ = sock.send(&session.wrap(t, &body));
                            }
                            svc::T_PLAYER_SOUND | svc::T_GROUP_SOUND | svc::T_LOCATION_SOUND => {
                                if let (Some(sound), Some(au)) = (svc::parse_sound(ty, &body), audio.as_mut()) {
                                    got_audio += 1;
                                    au.on_svc_sound(sound);
                                }
                            }
                            _ => {}
                        }
                    }
                    None => by_type[0] += 1, // не расшифровался: чужой или битый
                },
            }
        }
        let is_stale = last_rx.elapsed() >= stale_after;
        if is_stale != stale.load(Ordering::Relaxed) {
            stale.store(is_stale, Ordering::Relaxed);
            if is_stale {
                log(format!("UDP молчит {} с", stale_after.as_secs()));
            }
        }
        if let Some(au) = audio.as_mut() {
            au.sync_mic(log);
            au.pump_mic(&sock, &conn, &outbox);
            if last_prune.elapsed() >= Duration::from_secs(1) {
                au.prune();
                last_prune = Instant::now();
            }
        }
        if last_stats.elapsed() >= Duration::from_secs(10) {
            log(format!(
                "статистика ({proto}): отправлено служебных {sent_pings}, пакетов {got}, аудио {got_audio}, по типам {:?}, последний пакет {} с назад",
                by_type,
                last_rx.elapsed().as_secs()
            ));
            last_stats = Instant::now();
        }
    }
    Ok(())
}

/// Для логов потока: пишет строку в тот же файл, что и основной мод.
pub fn file_logger(path: &'static std::path::Path, start: Instant) -> Logger {
    Arc::new(move |line: String| {
        append_log(path, &format!("[{:>7} ms] voice: {line}", start.elapsed().as_millis()));
    })
}

/// Больше этого лог не растёт: дойдя до предела, он начинается с чистого файла.
const MAX_LOG_BYTES: u64 = 1 << 20;

/// Очищает лог при старте мода (и создаёт папку, если её ещё нет).
pub fn reset_log(path: &std::path::Path) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, "");
}

/// Дописывает строку в лог. Общий для основного мода и рабочего потока голоса.
pub fn append_log(path: &std::path::Path, line: &str) {
    if std::fs::metadata(path).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false) {
        let _ = std::fs::write(path, "");
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(name: &str, weight: i32, proximity: bool, transitive: bool) -> plasmo::Activation {
        plasmo::Activation {
            name: name.into(),
            translation: String::new(),
            id: plasmo::activation_id(name),
            distances: vec![],
            default_distance: 16,
            proximity,
            transitive,
            stereo: false,
            weight,
        }
    }

    fn res_of(d: &[(usize, Res, settings::ActKind)], acts: &[plasmo::Activation], name: &str) -> Res {
        d.iter().find(|x| acts[x.0].name == name).map(|x| x.1).unwrap()
    }

    #[test]
    fn local_slider_does_not_touch_groups_or_music() {
        let mut near = SrcQueue::new(0.8);
        near.local = true;
        let group = SrcQueue::new(0.5); // «Группы» уже в gain, «Локальный» не действует
        let music = SrcQueue::new(0.7);
        assert_eq!(near.effective_gain(0.0), 0.0, "«Локальный» 0% глушит голоса рядом");
        assert_eq!(group.effective_gain(0.0), 0.5, "…но не группы");
        assert_eq!(music.effective_gain(0.0), 0.7, "…и не музыку");
        assert_eq!(near.effective_gain(1.5), 0.8 * 1.5);
    }

    #[test]
    fn udp_address_keeps_ipv6_apart_from_port() {
        assert_eq!(host_port("5.83.140.202", 24454), "5.83.140.202:24454");
        assert_eq!(host_port("2001:db8::1", 24454), "[2001:db8::1]:24454");
        assert_eq!(host_port("[2001:db8::1]", 24454), "[2001:db8::1]:24454", "скобки не удваиваются");
        assert_eq!(host_port("voice.example.org", 24454), "voice.example.org:24454");
        let v4: SocketAddr = "5.83.140.202:24454".parse().unwrap();
        let v6: SocketAddr = "[2001:db8::1]:24454".parse().unwrap();
        assert!(local_bind_addr(&v4).is_ipv4());
        assert!(local_bind_addr(&v6).is_ipv6());
        assert_eq!(local_bind_addr(&v6).port(), 0);
    }

    #[test]
    fn udp_socket_matches_server_address_family() {
        let s = connect_udp("127.0.0.1:9").unwrap();
        assert!(s.local_addr().unwrap().is_ipv4());
        // IPv6 может быть выключен на машине с тестами: тогда проверять нечего
        if UdpSocket::bind("[::1]:0").is_ok() {
            let s = connect_udp(&host_port("::1", 9)).unwrap();
            assert!(s.local_addr().unwrap().is_ipv6(), "к IPv6-серверу — IPv6-сокет");
        }
        assert!(connect_udp("not a host").is_err());
    }

    /// Поддельный сервер SVC на 127.0.0.1: проверяем, что клиент проходит рукопожатие (Authenticate ->
    /// ConnectionCheck с тем же nonce) и отвечает на KeepAlive, всё в формате сервера (без id соединения).
    #[test]
    fn svc_handshake_against_fake_server() {
        let key = [3u8; 16];
        let conn_id = [4u8; 16];
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        server.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let port = server.local_addr().unwrap().port();
        let srv = svc::Session::new(&key, conn_id);
        let log: Logger = Arc::new(|_l: String| {});
        settings::set_connected(false);
        let ctl = start(
            Link::Svc { host: "127.0.0.1".into(), port, session: svc::Session::new(&key, conn_id), keep_alive_ms: 1000 },
            Arc::new(Mutex::new(Vec::new())),
            log,
        );
        let mut buf = [0u8; 2048];
        // клиент шлёт: 0xFF, id соединения, зашифрованное. Сервер снимает id и расшифровывает.
        let mut recv = |want: u8| -> Vec<u8> {
            loop {
                let (n, from) = server.recv_from(&mut buf).expect("клиент молчит");
                assert_eq!(buf[0], 0xFF);
                assert_eq!(&buf[1..17], &conn_id, "id соединения в каждом пакете");
                let (ty, body) = srv.unwrap(&[&buf[..1], &buf[17..n]].concat()).expect("расшифровали");
                if ty == want {
                    server.connect(from).unwrap();
                    return body;
                }
            }
        };
        let to_client = |ty: u8, body: &[u8]| {
            let dg = srv.wrap(ty, body);
            [&dg[..1], &dg[17..]].concat() // у сервера в пакете нет id соединения
        };
        recv(svc::T_AUTHENTICATE);
        let nonce = [9u8; 16];
        server.send(&to_client(svc::T_AUTHENTICATE_ACK, &nonce)).unwrap();
        let check = recv(svc::T_CONNECTION_CHECK);
        assert_eq!(check, nonce, "ConnectionCheck несёт nonce из AuthenticateAck");
        server.send(&to_client(svc::T_CONNECTION_CHECK_ACK, &[])).unwrap();
        server.send(&to_client(svc::T_KEEP_ALIVE, &[])).unwrap();
        recv(svc::T_KEEP_ALIVE);
        assert!(settings::connected(), "голос подключён");
        drop(ctl);
    }

    #[test]
    fn svc_local_in_a_normal_group_waits_for_detach() {
        use crate::groups::{ROUTE_LOCAL, ROUTE_NORMAL, ROUTE_TRANSIT};
        use settings::ActKind::{Ptt, Voice};
        let a = |name: &str, proximity: bool| plasmo::Activation {
            name: name.into(),
            translation: String::new(),
            id: plasmo::activation_id(name),
            distances: vec![],
            default_distance: 48,
            proximity,
            transitive: true,
            stereo: false,
            weight: if proximity { 1 } else { 10 },
        };
        let acts = vec![a("groups", false), a(settings::PROXIMITY, true)];
        let run = |g: Res, l: Res, lk, in_group, route, can| {
            let mut d = vec![(0, g, Ptt), (1, l, lk)];
            let (w, held) = svc_route(&mut d, &acts, in_group, route, can, &|_| false);
            (d[0].1, d[1].1, w, held)
        };
        use Res::{Activated as A, Not as N};
        // в группе: «Группы» — группе; «Локальный» (клавиша или голос) — ждём выхода, голос копится
        assert_eq!(run(A, N, Ptt, true, ROUTE_NORMAL, true), (A, N, false, false));
        assert_eq!(run(N, A, Ptt, true, ROUTE_NORMAL, true), (N, N, true, true));
        assert_eq!(run(N, A, Voice, true, ROUTE_NORMAL, true), (N, N, true, true));
        // выйти нельзя (не обычная группа, пароль неизвестен, в группе никого) — в группу
        assert_eq!(run(N, A, Ptt, true, ROUTE_NORMAL, false), (A, N, true, false));
        // в пути — копим; вне группы на время — только рядом, «Группы» ждёт
        assert_eq!(run(N, A, Ptt, true, ROUTE_TRANSIT, true), (N, N, true, true));
        assert_eq!(run(N, A, Ptt, true, ROUTE_LOCAL, true), (N, A, true, false));
        assert_eq!(run(A, N, Ptt, true, ROUTE_LOCAL, true), (N, N, false, true));
        // не в группе — как обычно
        assert_eq!(run(A, A, Ptt, false, ROUTE_NORMAL, true), (N, A, false, false));
    }

    #[test]
    fn activations_follow_plasmo_rules() {
        let acts = vec![act("groups", 5, false, false), act(settings::PROXIMITY, 1, true, true)];
        let none = |_: usize| false;
        let idle = |_: &[u8; 16]| false;
        // по умолчанию: «Локальный» по голосу, «Группы» по клавише (не зажата)
        let d = decide(&acts, true, &none, &idle, false);
        assert_eq!(res_of(&d, &acts, settings::PROXIMITY), Res::Activated);
        assert_eq!(res_of(&d, &acts, "groups"), Res::Not);
        // зажали клавишу групп (слот 1): говорим и в группу; группы «не рядом», «Локальный» не глушат
        let g = |s: usize| s == 1;
        let d = decide(&acts, true, &g, &idle, false);
        assert_eq!(res_of(&d, &acts, "groups"), Res::Activated);
        assert_eq!(res_of(&d, &acts, settings::PROXIMITY), Res::Activated);
        // микрофон выключен: всё, что передавало, заканчивается
        let busy = |_: &[u8; 16]| true;
        let d = decide(&acts, true, &g, &busy, true);
        assert!(d.iter().all(|x| x.1 == Res::End));
        // активация «рядом» и непрозрачная (например, шёпот) глушит «Локальный»
        let acts = vec![act("whisper", 9, true, false), act(settings::PROXIMITY, 1, true, true)];
        let d = decide(&acts, true, &g, &busy, false);
        assert_eq!(res_of(&d, &acts, "whisper"), Res::Activated);
        assert_eq!(res_of(&d, &acts, settings::PROXIMITY), Res::End, "заглушён шёпотом");
    }

    #[test]
    fn pactl_output_is_parsed_without_monitors() {
        let text = "Source #1\n\tState: RUNNING\n\tName: alsa_output.pci.analog-stereo.monitor\n\tDescription: Monitor of Built-in Audio\n\nSource #2\n\tName: alsa_input.usb-Blue_Yeti-00.analog-stereo\n\tDescription: Blue Yeti Analog Stereo\n";
        assert_eq!(
            parse_pactl_sources(text),
            vec![("alsa_input.usb-Blue_Yeti-00.analog-stereo".to_string(), "Blue Yeti Analog Stereo".to_string())]
        );
    }
}
#[cfg(test)]
mod offline_tests {
    #[test]
    fn offline_session_runs_without_network() {
        let out = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log: super::Logger = std::sync::Arc::new(|_s: String| {});
        let v = super::start(super::Link::Offline, out.clone(), log);
        v.configure(super::SessionConfig { aes: [0; 16], activation_id: [0; 16], distance: 32, bitrate: 0, offline: true });
        crate::settings::set_mic_test(true);
        std::thread::sleep(std::time::Duration::from_millis(400));
        assert!(!crate::settings::connected(), "без сервера «подключено» не показываем");
        assert!(out.lock().unwrap().is_empty(), "на сервер ничего не ушло");
        crate::settings::set_mic_test(false);
        drop(v);
    }
}
