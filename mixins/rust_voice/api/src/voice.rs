//! Голос: микрофон -> Opus -> AES -> UDP на сервер, и обратно: UDP -> AES -> Opus -> динамики.
//! Весь звук живёт в одном рабочем потоке; игровой поток общается с ним через `Outbox` и канал.

use crate::{crypto, denoise, dsp, plasmo, roster, settings, spatial, svc, LockExt};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, SampleRate, StreamConfig};
use rusty_opus::{Application, OpusDecoder, OpusEncoder, SignalType};
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const RATE: u32 = 48_000;
/// 20 мс при 48 кГц, моно.
pub const FRAME: usize = 960;
const MAX_QUEUE: usize = RATE as usize; // не больше секунды в очереди
const PREFILL_FRAMES: usize = 3; // сколько кадров копим перед воспроизведением источника
/// Сколько UDP может молчать, прежде чем сессия считается мёртвой (сервер шлёт keep-alive раз в 2-4 с).
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
        settings::set_hearing(0);
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
}

impl Link {
    fn addr(&self) -> String {
        match self {
            Link::Plasmo(c) => format!("{}:{}", c.ip, c.port),
            Link::Svc { host, port, .. } => {
                if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") }
            }
        }
    }
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
}

impl SrcQueue {
    fn new(gain: f32) -> SrcQueue {
        SrcQueue { q: VecDeque::new(), started: false, gain, nick: None, max_dist: 0.0, sp: dsp::Spatializer::new(), positional: true }
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
    speaking: bool,
    /// Когда передавать: предзапись, гистерезис, мягкие края (`dsp::Gate`).
    gate: dsp::Gate,
    /// Какой по счёту профиль шума уже применён (settings::noise_profile_gen).
    profile_gen: u32,
    packet: [u8; 1500],
    /// Шумоподавление микрофона (сила в настройках, 0 — выключено).
    ns: denoise::Denoiser,
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
fn input_names() -> Vec<(String, String)> {
    device_names(true)
}

/// Устройства вывода для меню, так же, как микрофоны (синки PipeWire/Pulse идут как `pulse:<имя>`).
fn output_names() -> Vec<(String, String)> {
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
        let mic = Arc::clone(mic);
        let s = d
            .build_input_stream(
                &config,
                move |data: &[f32], _| {
                    let mut q = mic.lock_ok();
                    q.extend(data.iter().copied());
                    if q.len() > MAX_QUEUE {
                        let n = q.len() - MAX_QUEUE;
                        q.drain(..n);
                    }
                },
                |e| eprintln!("ошибка микрофона: {e}"),
                None,
            )
            .map_err(|e| log(format!("микрофон «{name}» не открылся: {e}")))
            .ok()?;
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
        None
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
        if src.sp.mix(&mut src.q, data, channels, src.gain, &target, mode) {
            src.started = false;
        }
    }
    // игрока, которого вы заглушили до 0%, «слышно» не считаем
    settings::set_hearing(qs.by_source.values().filter(|e| e.started && e.gain > 0.0).count() as u32);
    drop(qs);
    let (vol, off) = (settings::voice_volume(), settings::voice_off());
    for v in data.iter_mut() {
        *v = if off { 0.0 } else { dsp::soft_limit(*v * vol).clamp(-1.0, 1.0) };
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
        settings::set_connected(true);
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
            speaking: false,
            gate: dsp::Gate::new(),
            profile_gen: settings::noise_profile_gen(),
            packet: [0u8; 1500],
            ns: {
                let mut d = denoise::Denoiser::new();
                d.set_profile(settings::noise_profile());
                d
            },
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
            self.ns.process(&mut frame, noise);
            if (gain - 1.0).abs() > f32::EPSILON {
                for v in frame.iter_mut() {
                    *v = (*v * gain).clamp(-1.0, 1.0);
                }
            }
            let rms = (frame.iter().map(|s| s * s).sum::<f32>() / FRAME as f32).sqrt();
            settings::set_level(rms); // меню рисует по нему полосу уровня
            let loud = rms >= threshold;
            let current = frame.clone();
            // Голосовой порог с гистерезисом и предзаписью — общий для всех активаций «по голосу».
            let voice_frames = match self.gate.process(frame, rms, threshold, None, silenced) {
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
        let decisions = decide(
            &acts,
            !voice_frames.is_empty(),
            &|slot| settings::slot_held(slot),
            &|id| streams.get(id).is_some_and(|s| s.active),
            silenced,
        );
        let any = decisions.iter().any(|d| d.1 == Res::Activated);
        // куда уходит голос: рядом или в группу (у Plasmo это разные активации, у SVC решает сервер по группе)
        let (near, group) = match link {
            Link::Svc { .. } if roster::self_group().is_some() => (false, any),
            Link::Svc { .. } => (any, false),
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
        let mut timeline = if voice_used { voice_frames } else if any { vec![current] } else { Vec::new() };
        if settings::agc_on() {
            let last = timeline.len().saturating_sub(1);
            for (i, f) in timeline.iter_mut().enumerate() {
                self.agc.process(f, i == last && loud);
            }
        }

        if settings::mic_test() {
            // Проверка звука: слышите то же, что услышали бы другие, но на сервер ничего не уходит.
            self.end_all(&acts, outbox);
            if any {
                let mut qs = self.play.lock_ok();
                let src = qs.by_source.entry(TEST_SOURCE).or_insert_with(|| SrcQueue::new(1.0));
                for f in &timeline {
                    src.q.extend(f.iter().copied());
                }
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
                            Link::Svc { .. } => self.packet[..len].to_vec(),
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
                            Link::Svc { session, .. } => session.wrap(svc::T_MIC, &svc::mic(data, st.seq, false)),
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
        // Чей это голос: известно из SourceInfo; пока не знаем, спрашиваем у сервера (SourceInfoRequest, тип 15).
        let nick = roster::source_nick(a.source_id);
        if nick.is_none() && roster::should_ask(a.source_id) {
            let mut ask = vec![15u8];
            ask.extend_from_slice(&a.source_id);
            outbox.lock_ok().push(ask);
        }
        let group = roster::source_is_group(a.source_id);
        self.play_voice(a.source_id, a.seq, &opus, a.distance.max(0) as f32, nick, !group, group);
    }

    /// Чужой голос SVC (Opus уже открыт: шифровался весь пакет). Имя — из состояний игроков SVC.
    fn on_svc_sound(&mut self, s: svc::Sound) {
        let nick = roster::nick_by_uuid(u128::from_be_bytes(s.sender));
        let positional = matches!(s.kind, svc::SoundKind::Player { .. });
        let group = matches!(s.kind, svc::SoundKind::Group);
        self.play_voice(s.channel, s.seq, &s.opus, s.distance, nick, positional, group);
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
        let src = match self.sources.get_mut(&source_id) {
            Some(s) => s,
            None => {
                let Ok(dec) = OpusDecoder::new(RATE as i32, 1) else { return };
                self.sources.entry(source_id).or_insert(Source { dec, last_seq: seq.saturating_sub(1), last_seen: Instant::now() })
            }
        };
        if seq <= src.last_seq {
            return; // старый или повторный пакет
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
        }
        let mut qs = self.play.lock_ok();
        let src = qs.by_source.entry(source_id).or_insert_with(|| SrcQueue::new(gain));
        src.gain = gain;
        src.max_dist = distance.max(0.0);
        src.positional = positional;
        if src.nick != nick {
            src.nick = nick;
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
    Check([u8; 16]),
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
    let sock = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("bind: {e}"))?;
    sock.connect(&addr).map_err(|e| format!("connect {addr}: {e}"))?;
    sock.set_read_timeout(Some(Duration::from_millis(10))).map_err(|e| e.to_string())?;
    let proto = if matches!(conn, Link::Svc { .. }) { "Simple Voice Chat" } else { "Plasmo Voice" };
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
                            let _ = sock.send(&session.wrap(svc::T_AUTHENTICATE, &[]));
                        }
                        SvcStage::Check(nonce) => {
                            let _ = sock.send(&session.wrap(svc::T_CONNECTION_CHECK, &svc::connection_check(nonce)));
                        }
                        SvcStage::Ready => {}
                    }
                    sent_pings += 1;
                    last_ping = Instant::now();
                }
            }
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
                                if let (SvcStage::Auth, Some(nonce)) = (&svc_stage, svc::parse_auth_ack(&body)) {
                                    log("SVC: сервер принял ключ, проверяю связь".into());
                                    svc_stage = SvcStage::Check(nonce);
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