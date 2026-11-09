mod acoustics;
mod aim;
mod crypto;
mod denoise;
mod dsp;
mod faces;
mod i18n;
mod icons;
mod menu;
mod nametag;
mod plasmo;
mod roster;
mod settings;
mod spatial;
mod svc;
mod voice;

use horizon_client::extras::Corner;
use horizon_client::mod_api::{
    ClientPacketEvent, ClientboundPlayPacket, CommandEvent, Flow, Hooks, Host, HudEvent, KeyEvent, ModsEvent,
    MouseScrollingEvent, NativeMod, RenderNameTagEvent, ScreenMouseButtonPressedPreEvent, ScreenMouseButtonReleasedPreEvent, ScreenMouseScrolledPreEvent, ScreenRenderPostEvent,
    ServerboundPlayPacket, TickEvent,
};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Лог лежит в папке мода рядом с настройками: ~/.config/horizon-voice-mod.log.
fn log_path() -> &'static std::path::Path {
    static P: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    P.get_or_init(|| settings::config_dir().join("horizon-voice-mod.log"))
}

/// Замок, который не паникует, если другой поток упал, держа его: данные тут простые (очереди, флаги),
/// а паника в звуковом потоке или хуке потянула бы за собой игру.
pub(crate) trait LockExt<T> {
    fn lock_ok(&self) -> std::sync::MutexGuard<'_, T>;
}
impl<T> LockExt<T> for Mutex<T> {
    fn lock_ok(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|e| e.into_inner())
    }
}
/// Id мода из horizon-mixin.toml: по нему игра открывает наш экран.
const MOD_ID: &str = "rust_voice";
/// Сколько тиков пытаться открыть меню после команды (чат ещё может быть открыт и мешать).
const OPEN_RETRY_TICKS: u32 = 40;
/// Id serverbound-пакета CustomPayload в play-состоянии (ids.rs: play::serverbound).
const CUSTOM_PAYLOAD_ID: i32 = 22;
/// Игровой тик = 50 мс. PlayerInfo повторяем раз в 3 с, пока сервер не пришлёт Connection.
const INFO_PERIOD_TICKS: u32 = 60;
/// После (пере)входа на сервер первый PlayerInfo уходит через секунду: сначала должен дойти register.
const INFO_FIRST_TICKS: u32 = 20;
/// Сколько раз подряд сессию можно пересоздать из-за молчащего UDP (дальше проблема не в нас).
const MAX_STALE_RESETS: u32 = 5;

pub fn hooks() -> Box<dyn Hooks> {
    Box::new(Mod { state: Mutex::new(State::new()) })
}

struct State {
    start: Instant,
    sent: bool,
    attempts: u32,
    seen_channels: HashSet<String>,
    /// Пара ключей на эту сессию игры: закрытая часть нигде не сохраняется.
    key: crypto::RsaKey,
    aes_key: Option<[u8; 16]>,
    voice: Option<voice::VoiceCtl>,
    outbox: voice::Outbox,
    /// Пришёл ли Connection (сервер принял наш PlayerInfo).
    connected: bool,
    info_sent: u32,
    ticks_since_info: u32,
    stale_resets: u32,
    /// Меню просили открыть: пробуем на каждом тике, пока оно не нарисуется.
    open_ticks: u32,
    /// Что UDP молчит и сессию пересоздать нельзя, уже записано в лог (чтобы не писать каждый тик).
    stale_logged: bool,
    /// Голос идёт через Simple Voice Chat (а не Plasmo) и какое состояние (микрофон выкл, голос выкл) мы ему
    /// последним сообщили.
    svc: bool,
    svc_state_sent: Option<(bool, bool)>,
}

impl State {
    fn new() -> Self {
        let start = Instant::now();
        let key = crypto::RsaKey::generate(2048);
        let s = State {
            start,
            sent: false,
            attempts: 0,
            seen_channels: HashSet::new(),
            key,
            aes_key: None,
            voice: None,
            outbox: Arc::new(Mutex::new(Vec::new())),
            connected: false,
            info_sent: 0,
            ticks_since_info: 0,
            stale_resets: 0,
            open_ticks: 0,
            stale_logged: false,
            svc: false,
            svc_state_sent: None,
        };
        settings::load();
        voice::reset_log(log_path());
        s.log("voice mod loaded");
        s.log(&format!("RSA-2048 ключ создан за {} мс", start.elapsed().as_millis()));
        s
    }

    /// Начать хендшейк заново: PlayerInfo и register уйдут сами, как только сработает таймер.
    fn restart_handshake(&mut self) {
        self.svc = false;
        self.svc_state_sent = None;
        settings::set_protocol("");
        self.voice = None; // Drop останавливает рабочий поток и закрывает аудио
        self.stale_logged = false;
        self.aes_key = None;
        self.connected = false;
        self.info_sent = 0;
        self.ticks_since_info = INFO_PERIOD_TICKS - INFO_FIRST_TICKS;
    }

    /// Вход на сервер или смена бэкенда за прокси (Login / StartConfiguration): всё, что мы знали о
    /// прошлом сервере, больше не действует. Новый сервер не знает ни наш `minecraft:register`,
    /// ни наш PlayerInfo, а старая UDP-сессия на нём мертва, поэтому начинаем с нуля.
    fn reset_session(&mut self, why: &str) {
        self.log(&format!("новая сессия на сервере ({why}): сбрасываю голос и начинаю хендшейк заново"));
        self.restart_handshake();
        self.sent = false; // tick заново отправит minecraft:register, как только появится play-состояние
        self.attempts = 0;
        self.stale_resets = 0;
        self.seen_channels.clear(); // чтобы в логе снова было видно, какие каналы объявил новый сервер
        self.outbox.lock_ok().clear();
        roster::reset(); // игроки и источники голоса прошлого сервера
        aim::reset();
        faces::reset();
        spatial::reset();
        settings::reset_activations(); // у нового сервера свои активации: придут в Config
    }

    fn log(&self, line: &str) {
        voice::append_log(log_path(), &format!("[{:>7} ms] {line}", self.start.elapsed().as_millis()));
    }
}

struct Mod {
    state: Mutex<State>,
}

/// ARGB значка -> ближайший легаси-код цвета (`§a`, `§c`...), потому что `event.text` — строка.
fn legacy_color(argb: u32) -> &'static str {
    match argb & 0x00FF_FFFF {
        0x55FF55 => "§a", // говорит
        0xFF5555 => "§c", // замьючен сервером или микрофоном
        0xFFAA00 => "§6", // голос выключен у игрока
        0xAAAAAA => "§7", // без мода
        _ => "§f",
    }
}

/// Строка тега: значок цветом статуса, сброс цвета, дальше текст тега. Без `\n`: игра рисует его как «LF».
fn badge_line(glyph: &str, color: u32, rest: &str) -> String {
    format!("{}{glyph}§r {rest}", legacy_color(color))
}

/// Собирает serverbound CustomPayload, не называя его тип: декодируем готовое тело.
fn custom_payload(channel: &str, data: &[u8]) -> Option<ServerboundPlayPacket> {
    let body = plasmo::payload_body(channel, data);
    match ServerboundPlayPacket::decode(CUSTOM_PAYLOAD_ID, &body) {
        Ok(p @ ServerboundPlayPacket::CustomPayload(_)) => Some(p),
        _ => None,
    }
}

/// Пакеты Simple Voice Chat (`voicechat:*`): ключ сессии и состояния игроков.
fn on_svc_packet(st: &mut State, host: &dyn Host, channel: &str, data: &[u8]) {
    match channel {
        svc::CH_SECRET => {
            let Some(secret) = svc::parse_secret(data) else {
                st.log("SVC secret: не разобрался");
                return;
            };
            if st.connected && !st.svc {
                st.log("SVC secret пропускаю: голос уже идёт через Plasmo Voice");
                return;
            }
            roster::set_self_uuid(u128::from_be_bytes(secret.player));
            // адрес голоса: свой хост из конфига сервера SVC или адрес игрового сервера
            let server = host.client_state().and_then(|s| s.server_address).unwrap_or_else(|| "127.0.0.1".to_string());
            let (vhost, port) = svc::voice_address(&secret, &server);
            st.log(&format!(
                "SVC secret: голос {vhost}:{port}, дистанция {}, кодек {}, mtu {}, keep-alive {} мс, группы {}",
                secret.distance, secret.codec, secret.mtu, secret.keep_alive, secret.groups
            ));
            st.voice = None; // старая сессия (если была) закрывается
            st.svc = true;
            st.svc_state_sent = None;
            st.connected = true;
            settings::set_protocol("Simple Voice Chat");
            settings::reset_activations(); // у SVC нет активаций Plasmo: голос рядом, как «Локальный»
            settings::set_distance(secret.distance.max(0.0) as u32);
            let logger = voice::file_logger(log_path(), st.start);
            let link = voice::Link::Svc {
                host: vhost,
                port,
                session: svc::Session::new(&secret.key, secret.connection_id),
                keep_alive_ms: secret.keep_alive,
            };
            let v = voice::start(link, Arc::clone(&st.outbox), logger);
            v.configure(voice::SessionConfig {
                aes: [0; 16],
                activation_id: plasmo::activation_id("proximity"),
                distance: secret.distance.clamp(0.0, i16::MAX as f64) as i16,
                bitrate: 0,
            });
            st.voice = Some(v);
        }
        svc::CH_STATES => match svc::parse_states(data) {
            Some(list) => roster::on_svc_states(&list),
            None => st.log("SVC states: не разобрался"),
        },
        svc::CH_STATE => {
            if let Some(s) = svc::parse_state(data) {
                roster::on_svc_state(&s);
            }
        }
        svc::CH_REMOVE_STATE => {
            if let Some(id) = svc::parse_remove_state(data) {
                roster::on_svc_remove(id);
            }
        }
        _ => {}
    }
}

/// Клавиша меню: назначенная во вкладке «Клавиши», иначе из Options > Mods (по умолчанию M).
fn menu_key(host: &dyn Host) -> String {
    settings::action_binding("menu").unwrap_or_else(|| host.mod_key(MOD_ID, settings::MENU_KEY_NAME, settings::MENU_KEY_DEFAULT))
}

/// Действие по клавише: переключить и показать подсказку над хотбаром. Микрофон и голосовой чат подсказкой
/// не пишутся: их видно по значку у хотбара (перечёркнутый микрофон или наушники).
fn run_action(action: &str) {
    match action {
        "mute" => settings::set_mic_muted(!settings::mic_muted()),
        "deafen" => settings::set_voice_off(!settings::voice_off()),
        "occlusion" => {
            settings::set_occlusion_on(!settings::occlusion_on());
            let s = if settings::occlusion_on() { "включено" } else { "выключено" };
            settings::notice(&format!("Заглушение через блоки {s}"));
        }
        "dist_up" | "dist_down" => match settings::step_proximity_distance(action == "dist_up") {
            Some(d) => settings::notice(&format!("Дистанция «Локальный»: {d}")),
            None => settings::notice("Сервер не даёт менять дистанцию"),
        },
        _ => return,
    }
    settings::save();
}

/// Клавиша рации слота: назначенная в меню мода, иначе выбранная в Options > Mods, иначе по умолчанию.
fn ptt_key(host: &dyn Host, slot: usize) -> String {
    if let Some(k) = settings::slot_binding(slot) {
        return k;
    }
    let default = if slot == 0 { settings::PTT_KEY_DEFAULT } else { settings::UNBOUND_KEY };
    host.mod_key(MOD_ID, settings::PTT_SLOT_KEYS[slot], default)
}

/// Поля события колеса: тут ЕДИНСТВЕННОЕ место, зависящее от устройства `MouseScrollingEvent` (хук `mouse_scrolling`, InputEvent.MouseScrollingEvent). Если после обновления
/// игры поля называются иначе, править нужно только эти две функции.
fn wheel_right_button(e: &MouseScrollingEvent) -> bool {
    e.buttons[2] // физические кнопки: левая, средняя, правая
}
fn wheel_steps(e: &MouseScrollingEvent) -> i32 {
    e.accumulated[1] as i32 // целые шаги по вертикали: вверх +, вниз -
}

impl Hooks for Mod {
    /// Options > Mods: строка мода с выбором клавиши меню (по умолчанию M). Выбор хранит игра, мы читаем его в `key`.
    fn mods(&self, _host: &mut dyn Host, event: &mut ModsEvent<'_>) {
        event.mods.push(NativeMod {
            id: MOD_ID.to_string(),
            title: "Rust Voice".to_string(),
            description: i18n::tr("Голосовой чат, совместимый с Plasmo Voice и Simple Voice Chat. Меню: клавиша ниже, /voice или /vm.").into_owned(),
            keys: vec![
                [
                    settings::MENU_KEY_NAME.to_string(),
                    i18n::tr("Открыть меню голоса").into_owned(),
                    settings::MENU_KEY_DEFAULT.to_string(),
                ],
                [
                    settings::PTT_KEY_NAME.to_string(),
                    i18n::tr("Рация: говорить по клавише (Я)").into_owned(),
                    settings::PTT_KEY_DEFAULT.to_string(),
                ],
                [
                    settings::PTT_SLOT_KEYS[1].to_string(),
                    i18n::tr("Рация 2: вторая активация сервера (например, группы)").into_owned(),
                    settings::UNBOUND_KEY.to_string(),
                ],
                [
                    settings::PTT_SLOT_KEYS[2].to_string(),
                    i18n::tr("Рация 3: третья активация сервера").into_owned(),
                    settings::UNBOUND_KEY.to_string(),
                ],
            ],
        });
    }

    /// Клавиша меню: открывает меню, а повторное нажатие закрывает. Над чатом, инвентарём и другими экранами
    /// `mod_screen_open` ничего не делает (он открывает экран только поверх мира), поэтому «m» в чате не мешает.
    fn key(&self, host: &mut dyn Host, event: &mut KeyEvent<'_>) {
        // Назначение клавиши рации в меню: следующая нажатая клавиша уходит туда, игре и рации — нет.
        if menu::capture_key(&*event.key, event.pressed) {
            return;
        }
        // Рация: держим флаг, пока клавиша зажата (используется, только если в меню выбран режим «По клавише»).
        for slot in 0..settings::PTT_SLOTS {
            let key = ptt_key(&*host, slot);
            if key != settings::UNBOUND_KEY && event.key == key.as_str() {
                settings::set_slot_held(slot, event.pressed);
            }
        }
        // Поле поиска во вкладке «Громкость» забирает буквы, цифры, «_» и Backspace (в том числе клавишу меню).
        if menu::on_key(&*event.key, event.pressed) {
            return;
        }
        if !event.pressed {
            return;
        }
        // Клавиши действий (вкладка «Клавиши»): только в игре, чтобы буква в чате не выключала микрофон.
        if host.playing() {
            if let Some(action) = settings::action_for_key(&*event.key) {
                run_action(action);
                return;
            }
        }
        let chosen = menu_key(&*host);
        if event.key != chosen.as_str() {
            return;
        }
        if menu::is_live() {
            host.mod_screen_close(MOD_ID);
        } else {
            menu::on_open();
            host.mod_screen_open(MOD_ID);
        }
    }

    /// Над головой игрока: два пиксельных значка (микрофон и динамик) ВЫШЕ ника, их рисует `nametag.rs` в `hud`.
    /// Здесь только запоминаем позицию игрока с видимым тегом. Если камера игры недоступна и оверлей не работает,
    /// остаётся запасной вариант: значок-символ перед ником в той же строке (`\n` в теге игра рисует как «LF»).
    /// Ник без значков не трогаем, чтобы у остальных остались цвет команды и всё, что дорисовала игра.
    fn render_name_tag(&self, host: &dyn Host, event: &mut RenderNameTagEvent<'_>) {
        // Где стоят игроки, нужно и без значков: по этому выбираем, на кого вы смотрите (ПКМ + колесо = громкость).
        if event.kind == "minecraft:player" && event.vanilla_visible {
            if let (Some(name), Some(pos)) = (
                host.entity(event.entity_id).and_then(|e| e.name(host)),
                host.entity(event.entity_id).and_then(|e| e.position(host)),
            ) {
                aim::note(&name, pos);
            }
        }
        if !settings::nametags_on() || event.kind != "minecraft:player" || !event.vanilla_visible {
            return;
        }
        let Some(name) = host.entity(event.entity_id).and_then(|e| e.name(host)) else { return };
        // Позицию запоминаем у всех игроков (а не только у тех, у кого сейчас есть значок): так значок
        // успевает плавно погаснуть, когда игрок замолчал.
        if nametag::overlay_ok() {
            if let Some(pos) = host.entity(event.entity_id).and_then(|e| e.position(host)) {
                nametag::note(&name, pos, nametag::is_sneaking(host, event.entity_id));
                return;
            }
        }
        if roster::indicator(&name).is_none() {
            return;
        }
        if let Some((glyph, color)) = roster::badge(roster::status(&name)) {
            // текст, который игра уже собрала (с префиксами сервера), оставляем; если его нет, берём ник
            let rest = event.text.take().filter(|t| !t.is_empty()).unwrap_or_else(|| name.to_string());
            *event.text = Some(badge_line(glyph, color, &rest));
        }
    }

    fn tick(&self, host: &mut dyn Host, _event: &mut TickEvent) {
        settings::set_menu_key_label(&settings::ptt_label_for(&menu_key(&*host)));
        for slot in 0..settings::PTT_SLOTS {
            let label = settings::ptt_label_for(&ptt_key(&*host, slot));
            if slot == 0 {
                settings::set_ptt_key_label(&label);
            }
            settings::set_slot_key_label(slot, &label);
        }
        settings::save_if_volumes_due();
        refresh_faces(host);
        // где игроки относительно вас (для громкости по расстоянию, стерео и стен); hud обновляет чаще
        if settings::connected() {
            spatial::update(&*host);
        }
        menu::ROUTED_COUNT.store(spatial::routed_count() as u32, std::sync::atomic::Ordering::Relaxed);
        // Проверка звука работает, только пока открыто меню: закрыли его (Esc) — выключаем, иначе эхо.
        if settings::mic_test() && !menu::is_live() {
            settings::set_mic_test(false);
        }
        let mut st = self.state.lock_ok();
        if st.open_ticks > 0 {
            st.open_ticks -= 1;
            if menu::is_live() {
                st.open_ticks = 0;
            } else {
                host.mod_screen_open(MOD_ID);
            }
        }
        // SVC: сообщаем серверу, выключены ли у нас микрофон и голос (он показывает это другим игрокам)
        if st.svc && st.connected {
            let now = (settings::mic_muted() || settings::mic_off(), settings::voice_off());
            if st.svc_state_sent != Some(now) {
                if let Some(pk) = custom_payload(svc::CH_CLIENT_STATE, &svc::client_state(now.0, now.1)) {
                    if host.send_packet(pk).is_ok() {
                        st.svc_state_sent = Some(now);
                    }
                }
            }
        }
        if st.sent && st.connected && !st.svc && settings::take_distances_dirty() {
            // выбранные в меню дистанции активаций: сервер должен их знать (PlayerActivationDistances)
            let list: Vec<([u8; 16], i32)> =
                settings::server_activations().iter().map(|a| (a.id, settings::act_distance(a))).collect();
            if !list.is_empty() {
                if let Some(pk) = custom_payload(plasmo::CHANNEL, &plasmo::activation_distances_packet(&list)) {
                    let _ = host.send_packet(pk);
                }
            }
        }
        if st.sent {
            // Пакеты, которые просит отправить голосовой поток (конец реплики и т.п.).
            let items: Vec<Vec<u8>> = st.outbox.lock_ok().drain(..).collect();
            for payload in items {
                if let Some(pk) = custom_payload(plasmo::CHANNEL, &payload) {
                    let _ = host.send_packet(pk);
                }
            }
            // UDP молчит дольше STALE_AFTER: сервер забыл нашу сессию (например, после смены бэкенда,
            // которую мы не заметили). Просим новую: register + PlayerInfo, и ждём свежий Connection.
            if st.connected && st.stale_resets < MAX_STALE_RESETS && st.voice.as_ref().is_some_and(|v| v.is_stale()) {
                st.stale_resets += 1;
                let n = st.stale_resets;
                st.log(&format!("UDP молчит: запрашиваю сессию заново (#{n} из {MAX_STALE_RESETS})"));
                st.restart_handshake();
            } else if st.voice.as_ref().is_some_and(|v| v.is_stale()) && !st.stale_logged {
                // UDP молчит, а пересоздать сессию нельзя: в логе будет видно почему.
                st.stale_logged = true;
                let (connected, resets) = (st.connected, st.stale_resets);
                st.log(&format!("UDP молчит, сессию не пересоздаю: connected={connected}, перезапусков {resets} из {MAX_STALE_RESETS}"));
            }
            // Сервер мог не прислать PlayerInfoRequest (или не получить наш register, а тогда он не шлёт
            // нам ничего по каналу): сами повторяем register и PlayerInfo раз в 3 секунды, пока не
            // придёт Connection.
            if !st.connected && st.info_sent < 15 {
                st.ticks_since_info += 1;
                if st.ticks_since_info >= INFO_PERIOD_TICKS {
                    st.ticks_since_info = 0;
                    st.info_sent += 1;
                    let n = st.info_sent;
                    // Сначала register: Velocity/Paper шлют клиенту по каналу, только если он его объявил.
                    let reg_ok = custom_payload(plasmo::REGISTER_CHANNEL, &plasmo::register_data())
                        .map(|pk| host.send_packet(pk).is_ok())
                        .unwrap_or(false);
                    let info = plasmo::player_info_packet(&st.key.public_der(), false, false);
                    let ok = custom_payload(plasmo::CHANNEL, &info)
                        .map(|pk| host.send_packet(pk).is_ok())
                        .unwrap_or(false);
                    // и Simple Voice Chat: просим ключ (сервер без SVC просто промолчит)
                    let svc_ok = custom_payload(svc::CH_REQUEST_SECRET, &svc::request_secret(svc::COMPATIBILITY_VERSION))
                        .map(|pk| host.send_packet(pk).is_ok())
                        .unwrap_or(false);
                    st.log(&format!(
                        "хендшейк #{n}: minecraft:register {}, PlayerInfo {}, SVC request_secret {}",
                        if reg_ok { "ушёл" } else { "не ушёл" },
                        if ok { "ушёл" } else { "не ушёл" },
                        if svc_ok { "ушёл" } else { "не ушёл" }
                    ));
                }
            }
            return;
        }
        st.attempts += 1;

        let Some(register) = custom_payload(plasmo::REGISTER_CHANNEL, &plasmo::register_data()) else {
            st.log("ERROR: не удалось собрать CustomPayload (id 22 не подошёл)");
            st.sent = true;
            return;
        };
        // Пока игрок не в play-состоянии, send_packet возвращает ошибку: пробуем на каждом тике.
        if host.send_packet(register).is_err() {
            if st.attempts % 200 == 1 {
                st.log("ждём play-состояние (send_packet вернул ошибку)");
            }
            return;
        }
        st.log("отправлен minecraft:register (plasmo:voice/v2, /installed)");
        st.sent = true;
        st.info_sent = 0;
        st.ticks_since_info = INFO_PERIOD_TICKS - INFO_FIRST_TICKS;
    }

    fn client_packet(&self, host: &mut dyn Host, event: &mut ClientPacketEvent<'_>) {
        match &event.packet {
            ClientboundPlayPacket::Login(_) => {
                self.state.lock_ok().reset_session("Login");
                return;
            }
            ClientboundPlayPacket::StartConfiguration(_) => {
                self.state.lock_ok().reset_session("StartConfiguration");
                return;
            }
            ClientboundPlayPacket::CustomPayload(_) => {}
            _ => return,
        }
        let ClientboundPlayPacket::CustomPayload(p) = &event.packet else { return };
        let mut st = self.state.lock_ok();
        if st.seen_channels.insert(p.channel.clone()) {
            st.log(&format!("канал от сервера: {}", p.channel));
        }
        if p.channel == plasmo::REGISTER_CHANNEL {
            let list: Vec<String> = p.data.split(|b| *b == 0).map(|c| String::from_utf8_lossy(c).into_owned()).collect();
            st.log(&format!("сервер регистрирует каналы: {:?}", list));
        }
        if p.channel.starts_with("voicechat:") {
            on_svc_packet(&mut st, &*host, &p.channel, &p.data);
            return;
        }
        if p.channel != plasmo::CHANNEL {
            return;
        }
        let data = &p.data;
        let Some(&ty) = data.first() else { return };
        // Аудио-события не логируем подробно: только хендшейк и редкие пакеты.
        if !matches!(ty, 7 | 8 | 9 | 16 | 18) {
            st.log(&format!("plasmo пакет {} ({}), {} байт", ty, plasmo::tcp_name(ty), data.len()));
        }
        match ty {
            // PlayerInfoRequest: отвечаем так же, как настоящий клиент.
            2 => {
                let info = plasmo::player_info_packet(&st.key.public_der(), false, false);
                match custom_payload(plasmo::CHANNEL, &info) {
                    Some(pk) => {
                        if host.send_packet(pk).is_err() {
                            st.log("ERROR: PlayerInfo не ушёл");
                        }
                    }
                    None => st.log("ERROR: не удалось собрать PlayerInfo"),
                }
            }
            // Кто на сервере с голосом, у кого что выключено, и чей это источник голоса: для значков над никами.
            7 => {
                if !roster::on_player_list(&data[1..]) {
                    st.log("PlayerList: не разобрался");
                }
            }
            // Сервер добавил или убрал активацию (например, аддон групп включили на ходу)
            19 => match plasmo::parse_activation_register(&data[1..]) {
                Some(a) => {
                    st.log(&format!("активация добавлена: {} (вес {}, дистанции {:?})", a.name, a.weight, a.distances));
                    settings::register_activation(a);
                }
                None => st.log("ActivationRegister: не разобрался"),
            },
            20 => {
                if let Some(id) = plasmo::parse_uuid(&data[1..]) {
                    settings::unregister_activation(id);
                }
            }
            // PlayerDisconnect: игрок вышел — убираем из списка «Громкость» и значков
            9 => {
                roster::on_player_disconnect(&data[1..]);
            }
            8 => {
                roster::on_player_update(&data[1..]);
            }
            16 => {
                roster::on_source_info(&data[1..]);
            }
            // Connection: адрес и секрет UDP. Запускаем (заново) голосовой поток.
            1 => match plasmo::parse_connection(&data[1..]) {
                Some(_) if st.svc => st.log("Connection Plasmo пропускаю: голос уже идёт через Simple Voice Chat"),
                Some(c) => {
                    settings::set_protocol("Plasmo Voice");
                    st.connected = true;
                    st.log(&format!("Connection: udp {}:{}", c.ip, c.port));
                    let logger = voice::file_logger(log_path(), st.start);
                    st.voice = Some(voice::start(voice::Link::Plasmo(c), Arc::clone(&st.outbox), logger));
                }
                None => st.log("Connection: не разобрался"),
            },
            // Config: частота, Opus, зашифрованный AES-ключ.
            3 => match plasmo::parse_config(&data[1..]) {
                Some(cfg) => {
                    st.log(&format!(
                        "Config: {} Гц, mtu {}, кодек {} {:?}, шифрование {:?}, proximity {:?}",
                        cfg.sample_rate, cfg.mtu, cfg.codec, cfg.codec_params, cfg.aes_algorithm, cfg.proximity_distance
                    ));
                    st.log(&format!("Config: активации {:?}, разбор: {}", cfg.activation_names, cfg.activations_note));
                    settings::set_server_activations(cfg.activations.clone());
                    match st.key.decrypt_pkcs1(&cfg.aes_key_encrypted) {
                        Some(k) if k.len() == 16 => {
                            let mut a = [0u8; 16];
                            a.copy_from_slice(&k);
                            st.aes_key = Some(a);
                            let bitrate = cfg
                                .codec_params
                                .iter()
                                .find(|(k, _)| k == "bitrate")
                                .and_then(|(_, v)| v.parse::<i32>().ok())
                                .unwrap_or(0);
                            let sc = voice::SessionConfig {
                                aes: a,
                                activation_id: plasmo::activation_id("proximity"),
                                distance: cfg.proximity_distance.unwrap_or(32) as i16,
                                bitrate,
                            };
                            match &st.voice {
                                Some(v) => v.configure(sc),
                                None => st.log("Config пришёл без UDP-сессии"),
                            }
                        }
                        Some(k) => st.log(&format!("AES-ключ неожиданной длины: {} байт", k.len())),
                        None => st.log("ERROR: RSA-расшифровка AES-ключа не удалась"),
                    }
                }
                None => st.log("Config: не разобрался"),
            },
            _ => {}
        }
    }

    /// Оверлей-иконки на HUD. Заодно узнаём размер GUI: по углам, которые отдаёт `corners`.
    fn hud(&self, host: &mut dyn Host, event: &mut HudEvent<'_>) {
        let (right, _) = event.corners.place(Corner::TopRight, 0.0, 0.0);
        let (_, bottom) = event.corners.place(Corner::BottomLeft, 0.0, 0.0);
        menu::set_gui_size(right as i32 + 2, bottom as i32 + 2);
        if settings::connected() {
            spatial::update(&*host);
        }
        if settings::nametags_on() {
            nametag::draw(&*host, &mut *event.batch, right + 2.0, bottom + 2.0);
        }
        let gui_w = right as i32 + 2;
        let gui_h = bottom as i32 + 2;
        // «Ник: 120%» под прицелом после ПКМ + колесо.
        if settings::volume_toast().is_some() {
            let mut gui = Gui { host: &*host, batch: &mut *event.batch };
            menu::draw_volume_toast(&mut gui, gui_w, gui_h);
        }
        // «Микрофон выключен», «Дистанция: 32» после клавиш действий
        {
            let mut gui = Gui { host: &*host, batch: &mut *event.batch };
            menu::draw_notice(&mut gui, gui_w, gui_h);
        }
        // Кто сейчас говорит: голова, ник и динамик в углу экрана.
        {
            let mut gui = Gui { host: &*host, batch: &mut *event.batch };
            menu::draw_speakers(&mut gui, gui_w, gui_h);
        }
        let Some((w, h)) = menu::overlay_size() else { return };
        // Иконка по центру хотбара: между сердечками и едой, над рядом сердечек (ряд сердечек: 39 пикселей от низа).
        let x = gui_w / 2 - w / 2;
        let y = gui_h - 39 - h + OVERLAY_LIFT;
        let mut gui = Gui { host: &*host, batch: &mut *event.batch };
        menu::draw_overlay(&mut gui, x, y);
    }

    /// ПКМ + колесо мыши, пока смотрите на игрока: меняет громкость именно его голоса (на 10% за щелчок, от 0 до 200%).
    /// Пока на кого-то наведено и ПКМ зажата, колесо не переключает слот хотбара. Выключается в меню, вкладка «Игроки».
    fn mouse_scrolling(&self, host: &mut dyn Host, event: &mut MouseScrollingEvent) -> Flow {
        if !settings::wheel_volume_on() || !wheel_right_button(event) {
            return Flow::Continue;
        }
        let Some(nick) = aim::looked_at(&*host) else { return Flow::Continue };
        let steps = wheel_steps(event);
        if steps != 0 {
            let now = settings::player_volume_pct(&nick) as i32;
            let next = (now + steps * settings::WHEEL_VOLUME_STEP as i32).clamp(0, 200) as u32;
            settings::set_player_volume_pct(&nick, next);
            settings::show_volume_toast(&nick, next);
        }
        Flow::Cancel
    }

    /// `/voice` (или `/vm`) открывает меню; `/voice mute` и `/voice deaf` переключают микрофон и голосовой чат.
    fn command(&self, _host: &mut dyn Host, event: &mut CommandEvent<'_>) -> Flow {
        let cmd = event.command.trim().trim_start_matches('/');
        let mut parts = cmd.split_whitespace();
        if !matches!(parts.next(), Some("voice") | Some("vm")) {
            return Flow::Continue;
        }
        match parts.next() {
            Some("mute") => {
                settings::set_mic_muted(!settings::mic_muted());
                settings::save();
            }
            Some("deaf") => {
                settings::set_voice_off(!settings::voice_off());
                settings::save();
            }
            _ => {
                menu::on_open();
                self.state.lock_ok().open_ticks = OPEN_RETRY_TICKS;
            }
        }
        Flow::Cancel
    }

    /// Каждый кадр, пока открыт экран мода: рисуем меню (экраны других мест игры пропускаем).
    fn screen_render_post(&self, host: &dyn Host, event: &mut ScreenRenderPostEvent<'_>) {
        if event.screen.id != "mod" {
            return;
        }
        let mouse = (event.screen.mouse[0] as i32, event.screen.mouse[1] as i32);
        let mut gui = Gui { host, batch: &mut *event.batch };
        menu::draw(&mut gui, mouse);
    }

    fn screen_mouse_button_pressed_pre(&self, _host: &dyn Host, event: &mut ScreenMouseButtonPressedPreEvent<'_>) -> Flow {
        if event.screen.id == "mod" && menu::press() {
            Flow::Cancel
        } else {
            Flow::Continue
        }
    }

    /// Колесо над меню: прокрутка вкладки «Устройства» (на ней строк больше, чем помещается).
    fn screen_mouse_scrolled_pre(&self, _host: &dyn Host, event: &mut ScreenMouseScrolledPreEvent<'_>) -> Flow {
        if event.screen.id == "mod" && menu::on_scroll(event.scroll[1] as f32) {
            Flow::Cancel
        } else {
            Flow::Continue
        }
    }

    fn screen_mouse_button_released_pre(&self, _host: &dyn Host, event: &mut ScreenMouseButtonReleasedPreEvent<'_>) -> Flow {
        if event.screen.id == "mod" {
            menu::release();
        }
        Flow::Continue
    }
}

/// Скины для голов в меню и в списке говорящих: пока меню открыто или голос подключён, раз в секунду просим у игры список игроков со скинами
/// (скин по умолчанию заменится настоящим, когда он скачается). Закрыли меню: следующее открытие обновит сразу.
fn refresh_faces(host: &mut dyn Host) {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let mut last = LAST.lock_ok();
    // Меню открыто или идёт голосовой чат (список говорящих рисует головы): иначе скины не нужны.
    if !menu::is_live() && !settings::connected() {
        *last = None;
        return;
    }
    if last.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(1)) {
        return;
    }
    *last = Some(Instant::now());
    drop(last);
    faces::set(host.client_profiles().into_iter().map(|p| (p.name, p.skin)));
}

/// На сколько пикселей GUI нижний край иконки заходит на ряд сердечек (больше — ниже, меньше — выше).
const OVERLAY_LIFT: i32 = 3;

/// Холст меню: прямоугольники и текст через помощники игры (`host.fill`, `host.text`).
struct Gui<'a> {
    host: &'a dyn Host,
    batch: &'a mut horizon_client::hud::HudBatch,
}

impl menu::Canvas for Gui<'_> {
    fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, argb: u32) {
        self.host.fill(&mut *self.batch, [x as f32, y as f32, w as f32, h as f32], argb);
    }
    fn text(&mut self, s: &str, x: i32, y: i32, rgb: u32, shadow: bool) {
        self.host.text(&mut *self.batch, &i18n::tr(s), [x as f32, y as f32], rgb, shadow);
    }
    /// Голова: лицо 8×8 со смещением (8, 8) в скине 64×64 и поверх него второй слой (40, 8), как рисует игра.
    fn face(&mut self, skin: &str, x: i32, y: i32, size: i32) {
        use horizon_client::hud_layout::{Blend, GuiDraw};
        let layer = |u: f32| GuiDraw::Blit {
            texture: std::borrow::Cow::Owned(skin.to_string()),
            blend: Blend::Textured,
            x,
            y,
            w: size,
            h: size,
            uv: [u, 8.0],
            src: [8, 8],
            tex: [64, 64],
            color: 0xFFFF_FFFF,
        };
        self.host.gui_paint(&mut *self.batch, &[layer(8.0), layer(40.0)]);
    }
}

#[cfg(test)]
mod name_tag_tests {
    use super::*;

    #[test]
    fn badge_stays_on_one_line() {
        let s = badge_line("✖", 0xFFFF_5555, "Rowan_s123");
        assert_eq!(s, "§c✖§r Rowan_s123");
        assert!(!s.contains('\n'));
    }
}