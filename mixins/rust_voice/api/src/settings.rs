//! Настройки и «живые» показания голоса. Меню пишет, голосовой поток и оверлей читают: всё на атомиках,
//! поэтому ни мьютексов между потоками, ни зависимости от API игры. Файл настроек: ~/.config/horizon-voice-mod.conf.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering::Relaxed};
use std::sync::Mutex;
use std::time::{Duration, Instant};

// ---------- то, что настраивает игрок ----------
static MIC_MUTED: AtomicBool = AtomicBool::new(false);
static VOICE_OFF: AtomicBool = AtomicBool::new(false);
static THRESHOLD: AtomicU32 = AtomicU32::new(0x3CA3_D70A); // 0.02f32
static MIC_GAIN: AtomicU32 = AtomicU32::new(100); // проценты, 0..=200
static VOICE_VOLUME: AtomicU32 = AtomicU32::new(100); // проценты, 0..=200
static OVERLAY_ON: AtomicBool = AtomicBool::new(true);
/// Громкость источников-групп (строка «Группы» во вкладке «Громкость»), проценты 0..=200.
static GROUP_VOLUME: AtomicU32 = AtomicU32::new(100);
/// Шумоподавление микрофона, проценты 0..=100 (0 — выключено).
static NOISE_PCT: AtomicU32 = AtomicU32::new(0);
/// Значок статуса перед ником игрока над головой (говорит, замьючен, без мода).
static NAMETAGS_ON: AtomicBool = AtomicBool::new(true);
/// Выбранный микрофон по имени; пусто — системный по умолчанию.
static MIC_DEVICE: Mutex<String> = Mutex::new(String::new());
/// Микрофоны, которые нашла система: (значение для конфига, название для меню). Заполняет голосовой поток.
static MIC_LIST: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
/// Растёт при каждой смене микрофона: голосовой поток по нему понимает, что поток записи надо открыть заново.
static MIC_GEN: AtomicU32 = AtomicU32::new(0);
/// Меню просит заново перечислить микрофоны.
static MIC_REFRESH: AtomicBool = AtomicBool::new(false);
/// Какой выбор устройства открыт в меню: 0 — никакой, 1 — микрофон, 2 — вывод. И с какой строки показан список.
static PICKER: AtomicU8 = AtomicU8::new(0);
static MIC_SCROLL: AtomicU32 = AtomicU32::new(0);
static OVERLAY_LABELS: AtomicBool = AtomicBool::new(true);
/// 0 левый верхний, 1 правый верхний, 2 левый нижний, 3 правый нижний.
static OVERLAY_CORNER: AtomicU8 = AtomicU8::new(1);
/// «Отключить устройство ввода»: микрофон закрыт совсем (система не видит, что он занят).
static MIC_OFF: AtomicBool = AtomicBool::new(false);
/// Фильтр гула (срез ниже 80 Гц) и автогромкость микрофона.
static HUM_FILTER: AtomicBool = AtomicBool::new(true);
static AGC_ON: AtomicBool = AtomicBool::new(false);
/// Пространственный звук: глухость за блоками, стерео по направлению и «лёгкий» HRTF поверх него.
static OCCLUSION: AtomicBool = AtomicBool::new(true);
static DIRECTIONAL: AtomicBool = AtomicBool::new(true);
static HRTF: AtomicBool = AtomicBool::new(true);
/// Звук огибает углы: голос из-за стены слышно со стороны двери или проёма.
static ROUTES: AtomicBool = AtomicBool::new(true);
/// Устройство вывода по имени (пусто — системное), список, который нашла система, и счётчик смены.
static OUT_DEVICE: Mutex<String> = Mutex::new(String::new());
static OUT_LIST: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
static OUT_GEN: AtomicU32 = AtomicU32::new(0);
/// Вывод открылся в стерео (иначе направленный звук сводится в моно).
static OUT_STEREO: AtomicBool = AtomicBool::new(false);
static OUT_OK: AtomicBool = AtomicBool::new(false);
/// Через какой мод голоса сервера подключились: «Plasmo Voice», «Simple Voice Chat» или пусто.
static PROTOCOL: Mutex<String> = Mutex::new(String::new());
/// «Запомнить шум»: профиль шума микрофона (мощность по частотам), счётчик его смены, запрос записи из
/// меню и прогресс записи в процентах (u32::MAX — не пишем).
static NOISE_PROFILE: Mutex<Option<Vec<f32>>> = Mutex::new(None);
static NOISE_PROFILE_GEN: AtomicU32 = AtomicU32::new(0);
static LEARN_REQ: AtomicBool = AtomicBool::new(false);
static LEARN_PCT: AtomicU32 = AtomicU32::new(u32::MAX);

/// Громкость отдельных игроков: ник в нижнем регистре -> проценты 0..=200. Игрока со 100% здесь нет.
static PLAYER_VOL: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);
/// ПКМ + колесо мыши при наведении на игрока меняет его громкость (можно выключить в меню).
static WHEEL_VOLUME: AtomicBool = AtomicBool::new(true);
/// Громкость игроков менялась, а на диск ещё не сохранена: (когда менялась).
static PLAYER_VOL_DIRTY: Mutex<Option<Instant>> = Mutex::new(None);
/// Последняя подсказка «ник: громкость» над прицелом: (ник, проценты, когда).
static VOL_TOAST: Mutex<Option<(String, u32, Instant)>> = Mutex::new(None);
/// В меню на вкладке «Игроки»: с какой строки показан список.
static PLAYERS_SCROLL: AtomicU32 = AtomicU32::new(0);

// ---------- что происходит сейчас (пишет голосовой поток) ----------
static LEVEL: AtomicU32 = AtomicU32::new(0); // f32: громкость микрофона (RMS) после усиления
static SPEAKING: AtomicBool = AtomicBool::new(false);
static HEARING: AtomicU32 = AtomicU32::new(0);
static CONNECTED: AtomicBool = AtomicBool::new(false);
static MIC_OK: AtomicBool = AtomicBool::new(false);
static DISTANCE: AtomicU32 = AtomicU32::new(0);
/// Активации сервера (по весу: важнейшая первая) и ваши настройки каждой по её имени.
static ACTS: Mutex<Vec<crate::plasmo::Activation>> = Mutex::new(Vec::new());
static ACT_PREFS: Mutex<Option<HashMap<String, ActPref>>> = Mutex::new(None);
/// Выбранные дистанции надо сообщить серверу (PlayerActivationDistances).
static ACT_DIST_DIRTY: AtomicBool = AtomicBool::new(false);
/// Клавиша рации сейчас зажата (пишет хук key в lib.rs, читает голосовой поток).
/// Клавиши рации зажаты: слот 0 — «Локальный», 1 и 2 — следующие активации по весу.
static PTT_HELD: [AtomicBool; PTT_SLOTS] = [const { AtomicBool::new(false) }; PTT_SLOTS];
static PTT_LABELS: Mutex<[String; PTT_SLOTS]> = Mutex::new([const { String::new() }; PTT_SLOTS]);
/// Клавиши действий, назначенные в меню (вкладка «Клавиши»): действие -> имя клавиши (`UNBOUND_KEY` — убрана).
static ACTION_BINDS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
/// Затухание голоса с расстоянием (вкладка «Продвинутые»).
static DIST_FADE: AtomicBool = AtomicBool::new(true);
/// Короткая подсказка над хотбаром («Микрофон выключен», «Дистанция: 32»): текст и когда показали.
static NOTICE: Mutex<Option<(String, Instant)>> = Mutex::new(None);
/// Клавиши рации, назначенные прямо в меню мода (пусто — берём из Options > Mods). `UNBOUND_KEY` — убрана.
static PTT_BINDS: Mutex<[String; PTT_SLOTS]> = Mutex::new([const { String::new() }; PTT_SLOTS]);
/// Проверка звука: микрофон играет в ваши динамики и не уходит на сервер. Не сохраняется, выключается с закрытием меню.
static MIC_TEST: AtomicBool = AtomicBool::new(false);
static PTT_KEY_LABEL: Mutex<String> = Mutex::new(String::new());
/// Клавиша открытия меню в виде, который игрок видит (`M`): выбирается в Options > Mods, мод лишь читает её у игры.
static MENU_KEY_LABEL: Mutex<String> = Mutex::new(String::new());

pub const DEFAULT_THRESHOLD: f32 = 0.02;
/// Имя и клавиша по умолчанию для Options > Mods (имя клавиши в vanilla: `key.keyboard.m`).
pub const MENU_KEY_NAME: &str = "open_menu";
pub const MENU_KEY_DEFAULT: &str = "key.keyboard.m";
/// Рация: у vanilla клавиши названы по положению на клавиатуре, «Я» на русской раскладке — это `key.keyboard.z`.
pub const PTT_KEY_NAME: &str = "push_to_talk";
pub const PTT_KEY_DEFAULT: &str = "key.keyboard.z";

/// Подпись клавиши: у букв добавляем русскую букву той же клавиши (`key.keyboard.z` -> «Я (Z)»), потому что
/// у vanilla клавиши названы по месту на клавиатуре, а раскладка у игрока русская.
pub fn ptt_label_for(name: &str) -> String {
    const EN: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,.";
    const RU: &str = "йцукенгшщзхъфывапролджэячсмитьбю";
    const UK: &str = "йцукенгшщзхїфівапролджєячсмитьбю";
    if name.is_empty() || name == UNBOUND_KEY {
        return String::new();
    }
    let short = name.strip_prefix("key.keyboard.").unwrap_or("");
    let uk = match crate::i18n::lang() {
        crate::i18n::Lang::En => return key_label(name), // буква своей раскладки нужна только русскому и украинскому
        crate::i18n::Lang::Uk => true,
        crate::i18n::Lang::Ru => false,
    };
    let ru = match short {
        "left.bracket" => Some('х'),
        "right.bracket" => Some(if uk { 'ї' } else { 'ъ' }),
        "semicolon" => Some('ж'),
        "apostrophe" => Some(if uk { 'є' } else { 'э' }),
        "comma" => Some('б'),
        "period" => Some('ю'),
        s if s.len() == 1 => EN.find(s).and_then(|i| (if uk { UK } else { RU }).chars().nth(EN[..i].chars().count())),
        _ => None,
    };
    match ru {
        Some(r) => format!("{} ({})", r.to_uppercase(), key_label(name)),
        None => key_label(name),
    }
}

// ---------- клавиши действий ----------
/// Действия, которым можно назначить клавишу во вкладке «Клавиши»: (имя в конфиге, подпись).
/// «menu» — открыть меню (без назначения берётся из Options > Mods, по умолчанию M).
pub const ACTIONS: [(&str, &str); 6] = [
    ("menu", "Открыть меню"),
    ("mute", "Выключить микрофон"),
    ("deafen", "Выключить голосовой чат"),
    ("occlusion", "Вкл/выкл заглушение"),
    ("dist_up", "Увеличить дистанцию"),
    ("dist_down", "Уменьшить дистанцию"),
];

pub fn action_binding(action: &str) -> Option<String> {
    ACTION_BINDS.lock().ok().and_then(|g| g.as_ref().and_then(|m| m.get(action).cloned())).filter(|k| !k.is_empty())
}
pub fn set_action_binding(action: &str, key: Option<&str>) {
    if let Ok(mut g) = ACTION_BINDS.lock() {
        let m = g.get_or_insert_with(HashMap::new);
        match key {
            Some(k) => {
                m.insert(action.to_string(), k.to_string());
            }
            None => {
                m.remove(action);
            }
        }
    }
}
/// Какое действие (кроме меню) назначено на эту клавишу.
pub fn action_for_key(key: &str) -> Option<&'static str> {
    if key == UNBOUND_KEY {
        return None;
    }
    ACTIONS.iter().map(|a| a.0).filter(|a| *a != "menu").find(|a| action_binding(a).as_deref() == Some(key))
}

/// Сдвинуть дистанцию «Локального» на соседнюю разрешённую сервером. Возвращает новую.
pub fn step_proximity_distance(up: bool) -> Option<i32> {
    let a = activation(PROXIMITY)?;
    let choices = a.distance_choices();
    if choices.is_empty() {
        return None;
    }
    let cur = act_distance(&a);
    let i = choices.iter().position(|d| *d == cur).unwrap_or(0);
    let j = if up { (i + 1) % choices.len() } else { (i + choices.len() - 1) % choices.len() };
    let mut p = act_pref(PROXIMITY);
    p.distance = Some(choices[j]);
    set_act_pref(PROXIMITY, p);
    Some(choices[j])
}

/// Говорящие в группе — отдельным списком слева, остальные справа (иначе один общий список).
static GROUP_SPLIT: AtomicBool = AtomicBool::new(true);
pub fn group_split_on() -> bool { GROUP_SPLIT.load(Relaxed) }
pub fn set_group_split_on(v: bool) { GROUP_SPLIT.store(v, Relaxed) }

/// Куда сейчас уходит наш голос: рядом (proximity) и/или в группу.
static SPEAKING_NEAR: AtomicBool = AtomicBool::new(false);
static SPEAKING_GROUP: AtomicBool = AtomicBool::new(false);
pub fn speaking_near() -> bool { SPEAKING_NEAR.load(Relaxed) }
pub fn speaking_group() -> bool { SPEAKING_GROUP.load(Relaxed) }
pub fn set_speaking_to(near: bool, group: bool) {
    SPEAKING_NEAR.store(near, Relaxed);
    SPEAKING_GROUP.store(group, Relaxed);
}

pub fn dist_fade_on() -> bool { DIST_FADE.load(Relaxed) }
pub fn set_dist_fade_on(v: bool) { DIST_FADE.store(v, Relaxed) }

/// Показать подсказку над хотбаром на пару секунд.
pub fn notice(text: &str) {
    if let Ok(mut n) = NOTICE.lock() {
        *n = Some((text.to_string(), Instant::now()));
    }
}
/// Подсказка, если ещё не погасла: (текст, видимость 0..=1).
pub fn current_notice() -> Option<(String, f32)> {
    const FOR: Duration = Duration::from_millis(1800);
    let n = NOTICE.lock().ok()?;
    let (text, at) = n.as_ref()?;
    let age = at.elapsed();
    if age >= FOR {
        return None;
    }
    Some((text.clone(), ((FOR - age).as_secs_f32() / FOR.as_secs_f32() * 3.0).min(1.0)))
}

/// Клавиша, назначенная в меню мода для слота рации (None — не назначали, берём из Options > Mods).
pub fn slot_binding(slot: usize) -> Option<String> {
    PTT_BINDS.lock().ok().and_then(|b| b.get(slot).cloned()).filter(|k| !k.is_empty())
}
pub fn set_slot_binding(slot: usize, key: Option<&str>) {
    if let Ok(mut b) = PTT_BINDS.lock() {
        if let Some(s) = b.get_mut(slot) {
            *s = key.unwrap_or_default().to_string();
        }
    }
    set_slot_held(slot, false);
}
pub fn ptt_key_label() -> String {
    match PTT_KEY_LABEL.lock() {
        Ok(l) if !l.is_empty() => l.clone(),
        _ => ptt_label_for(PTT_KEY_DEFAULT),
    }
}
pub fn set_ptt_key_label(v: &str) {
    if let Ok(mut l) = PTT_KEY_LABEL.lock() {
        if *l != v {
            *l = v.to_string();
        }
    }
}

/// «Локальный» по клавише (рация), а не по голосу.
pub fn push_to_talk() -> bool { act_pref(PROXIMITY).kind == ActKind::Ptt }
pub fn set_push_to_talk(v: bool) {
    let mut p = act_pref(PROXIMITY);
    p.kind = if v { ActKind::Ptt } else { ActKind::Voice };
    set_act_pref(PROXIMITY, p);
}
pub fn slot_held(slot: usize) -> bool { PTT_HELD.get(slot).is_some_and(|h| h.load(Relaxed)) }
pub fn set_slot_held(slot: usize, v: bool) {
    if let Some(h) = PTT_HELD.get(slot) {
        h.store(v, Relaxed);
    }
}
/// Подпись клавиши слота (пусто — не назначена).
pub fn slot_key_label(slot: usize) -> String {
    PTT_LABELS.lock().ok().and_then(|l| l.get(slot).cloned()).unwrap_or_default()
}
pub fn set_slot_key_label(slot: usize, v: &str) {
    if let Ok(mut l) = PTT_LABELS.lock() {
        if let Some(s) = l.get_mut(slot) {
            if *s != v {
                *s = v.to_string();
            }
        }
    }
}
pub fn mic_test() -> bool { MIC_TEST.load(Relaxed) }
pub fn set_mic_test(v: bool) { MIC_TEST.store(v, Relaxed) }

/// `key.keyboard.m` -> `M`, `key.keyboard.left.shift` -> `LEFT SHIFT`, `key.mouse.4` -> `MOUSE 4`.
pub fn key_label(name: &str) -> String {
    let short = name
        .strip_prefix("key.keyboard.")
        .or_else(|| name.strip_prefix("key."))
        .unwrap_or(name);
    short.replace(['.', '_'], " ").to_uppercase()
}

pub fn menu_key_label() -> String {
    match MENU_KEY_LABEL.lock() {
        Ok(l) if !l.is_empty() => l.clone(),
        _ => key_label(MENU_KEY_DEFAULT),
    }
}
pub fn set_menu_key_label(v: &str) {
    if let Ok(mut l) = MENU_KEY_LABEL.lock() {
        if *l != v {
            *l = v.to_string();
        }
    }
}

pub fn mic_muted() -> bool { MIC_MUTED.load(Relaxed) }
pub fn set_mic_muted(v: bool) { MIC_MUTED.store(v, Relaxed) }
pub fn voice_off() -> bool { VOICE_OFF.load(Relaxed) }
pub fn set_voice_off(v: bool) { VOICE_OFF.store(v, Relaxed) }
/// Микрофон не передаёт: выключен сам или выключен весь голосовой чат.
pub fn mic_silenced() -> bool { mic_muted() || voice_off() }

pub fn threshold() -> f32 { f32::from_bits(THRESHOLD.load(Relaxed)) }
pub fn set_threshold(v: f32) { THRESHOLD.store(v.clamp(0.0005, 0.5).to_bits(), Relaxed) }

pub fn mic_gain_pct() -> u32 { MIC_GAIN.load(Relaxed) }
pub fn set_mic_gain_pct(v: u32) { MIC_GAIN.store(v.min(200), Relaxed) }
pub fn mic_gain() -> f32 { mic_gain_pct() as f32 / 100.0 }
pub fn voice_volume_pct() -> u32 { VOICE_VOLUME.load(Relaxed) }
pub fn set_voice_volume_pct(v: u32) { VOICE_VOLUME.store(v.min(200), Relaxed) }
pub fn voice_volume() -> f32 { voice_volume_pct() as f32 / 100.0 }
pub fn group_volume_pct() -> u32 { GROUP_VOLUME.load(Relaxed) }
pub fn set_group_volume_pct(v: u32) { GROUP_VOLUME.store(v.min(200), Relaxed) }
pub fn noise_pct() -> u32 { NOISE_PCT.load(Relaxed) }
pub fn set_noise_pct(v: u32) { NOISE_PCT.store(v.min(100), Relaxed) }
/// Сила шумоподавления 0.0..=1.0.
pub fn noise_strength() -> f32 { noise_pct() as f32 / 100.0 }

pub fn mic_device() -> String { MIC_DEVICE.lock().map(|d| d.clone()).unwrap_or_default() }
pub fn set_mic_device(name: &str) {
    if let Ok(mut d) = MIC_DEVICE.lock() {
        if *d != name {
            *d = name.to_string();
            MIC_GEN.fetch_add(1, Relaxed);
        }
    }
}
pub fn mic_gen() -> u32 { MIC_GEN.load(Relaxed) }
pub fn mic_list() -> Vec<(String, String)> { MIC_LIST.lock().map(|l| l.clone()).unwrap_or_default() }
pub fn set_mic_list(v: Vec<(String, String)>) {
    if let Ok(mut l) = MIC_LIST.lock() {
        *l = v;
    }
}
pub fn request_mic_refresh() { MIC_REFRESH.store(true, Relaxed) }
pub fn take_mic_refresh() -> bool { MIC_REFRESH.swap(false, Relaxed) }
pub fn mic_picker() -> bool { PICKER.load(Relaxed) == 1 }
pub fn set_mic_picker(v: bool) { PICKER.store(if v { 1 } else { 0 }, Relaxed) }
pub fn out_picker() -> bool { PICKER.load(Relaxed) == 2 }
pub fn set_out_picker(v: bool) { PICKER.store(if v { 2 } else { 0 }, Relaxed) }

pub fn mic_off() -> bool { MIC_OFF.load(Relaxed) }
pub fn set_mic_off(v: bool) {
    if MIC_OFF.swap(v, Relaxed) != v {
        MIC_GEN.fetch_add(1, Relaxed); // голосовой поток закроет или откроет запись
    }
}
pub fn hum_filter() -> bool { HUM_FILTER.load(Relaxed) }
pub fn set_hum_filter(v: bool) { HUM_FILTER.store(v, Relaxed) }
pub fn agc_on() -> bool { AGC_ON.load(Relaxed) }
pub fn set_agc_on(v: bool) { AGC_ON.store(v, Relaxed) }
pub fn occlusion_on() -> bool { OCCLUSION.load(Relaxed) }
pub fn set_occlusion_on(v: bool) { OCCLUSION.store(v, Relaxed) }
pub fn directional_on() -> bool { DIRECTIONAL.load(Relaxed) }
pub fn set_directional_on(v: bool) { DIRECTIONAL.store(v, Relaxed) }
pub fn hrtf_on() -> bool { HRTF.load(Relaxed) }
pub fn routes_on() -> bool { ROUTES.load(Relaxed) }
pub fn set_routes_on(v: bool) { ROUTES.store(v, Relaxed) }
pub fn set_hrtf_on(v: bool) { HRTF.store(v, Relaxed) }

pub fn out_device() -> String { OUT_DEVICE.lock().map(|d| d.clone()).unwrap_or_default() }
pub fn set_out_device(name: &str) {
    if let Ok(mut d) = OUT_DEVICE.lock() {
        if *d != name {
            *d = name.to_string();
            OUT_GEN.fetch_add(1, Relaxed);
        }
    }
}
pub fn out_gen() -> u32 { OUT_GEN.load(Relaxed) }
pub fn out_list() -> Vec<(String, String)> { OUT_LIST.lock().map(|l| l.clone()).unwrap_or_default() }
pub fn set_out_list(v: Vec<(String, String)>) {
    if let Ok(mut l) = OUT_LIST.lock() {
        *l = v;
    }
}
pub fn out_stereo() -> bool { OUT_STEREO.load(Relaxed) }
pub fn set_out_stereo(v: bool) { OUT_STEREO.store(v, Relaxed) }
pub fn out_ok() -> bool { OUT_OK.load(Relaxed) }
pub fn protocol() -> String { PROTOCOL.lock().map(|p| p.clone()).unwrap_or_default() }
pub fn set_protocol(v: &str) {
    if let Ok(mut p) = PROTOCOL.lock() {
        *p = v.to_string();
    }
}

pub fn noise_profile() -> Option<Vec<f32>> { NOISE_PROFILE.lock().ok().and_then(|p| p.clone()) }
pub fn set_noise_profile(v: Option<Vec<f32>>) {
    if let Ok(mut p) = NOISE_PROFILE.lock() {
        *p = v;
    }
    NOISE_PROFILE_GEN.fetch_add(1, Relaxed);
}
pub fn noise_profile_gen() -> u32 { NOISE_PROFILE_GEN.load(Relaxed) }
pub fn request_noise_learn() {
    LEARN_REQ.store(true, Relaxed);
    LEARN_PCT.store(0, Relaxed);
}
pub fn take_learn_request() -> bool { LEARN_REQ.swap(false, Relaxed) }
/// Прогресс записи шума 0..1, None — не пишем.
pub fn learn_progress() -> Option<f32> {
    let v = LEARN_PCT.load(Relaxed);
    (v != u32::MAX).then(|| v as f32 / 100.0)
}
pub fn set_learn_progress(v: Option<f32>) {
    LEARN_PCT.store(v.map_or(u32::MAX, |f| (f.clamp(0.0, 1.0) * 100.0) as u32), Relaxed);
}

/// Профиль шума лежит отдельным файлом: числа через пробел (он привязан к микрофону и комнате).
fn noise_path() -> std::path::PathBuf {
    config_dir().join("horizon-voice-mod.noise")
}
pub fn save_noise_profile() {
    let p = noise_path();
    match noise_profile() {
        Some(v) => {
            let text: Vec<String> = v.iter().map(|x| format!("{x:e}")).collect();
            let _ = std::fs::write(p, text.join(" "));
        }
        None => {
            let _ = std::fs::remove_file(p);
        }
    }
}
fn load_noise_profile() {
    let Ok(text) = std::fs::read_to_string(noise_path()) else { return };
    let v: Vec<f32> = text.split_whitespace().filter_map(|t| t.parse().ok()).collect();
    if v.len() == crate::denoise::BINS {
        set_noise_profile(Some(v));
    }
}
pub fn set_out_ok(v: bool) { OUT_OK.store(v, Relaxed) }
pub fn mic_scroll() -> usize { MIC_SCROLL.load(Relaxed) as usize }
pub fn set_mic_scroll(v: usize) { MIC_SCROLL.store(v as u32, Relaxed) }
pub fn nametags_on() -> bool { NAMETAGS_ON.load(Relaxed) }
pub fn set_nametags_on(v: bool) { NAMETAGS_ON.store(v, Relaxed) }
pub fn overlay_on() -> bool { OVERLAY_ON.load(Relaxed) }
pub fn set_overlay_on(v: bool) { OVERLAY_ON.store(v, Relaxed) }
pub fn overlay_labels() -> bool { OVERLAY_LABELS.load(Relaxed) }
pub fn set_overlay_labels(v: bool) { OVERLAY_LABELS.store(v, Relaxed) }
pub fn overlay_corner() -> u8 { OVERLAY_CORNER.load(Relaxed).min(3) }
pub fn set_overlay_corner(v: u8) { OVERLAY_CORNER.store(v.min(3), Relaxed) }
/// Угол списка говорящих в группе (когда группа показывается отдельно). По умолчанию слева сверху.
static GROUP_CORNER: AtomicU8 = AtomicU8::new(0);
pub fn group_corner() -> u8 { GROUP_CORNER.load(Relaxed).min(3) }
pub fn set_group_corner(v: u8) { GROUP_CORNER.store(v.min(3), Relaxed) }
/// Во вкладке «Оверлей» сейчас переставляется список группы (а не обычный). Не сохраняется.
static PLACING_GROUP: AtomicBool = AtomicBool::new(false);
pub fn placing_group() -> bool { PLACING_GROUP.load(Relaxed) }
pub fn set_placing_group(v: bool) { PLACING_GROUP.store(v, Relaxed) }

pub fn level() -> f32 { f32::from_bits(LEVEL.load(Relaxed)) }
pub fn set_level(v: f32) { LEVEL.store(v.to_bits(), Relaxed) }
pub fn speaking() -> bool { SPEAKING.load(Relaxed) }
pub fn set_speaking(v: bool) { SPEAKING.store(v, Relaxed) }
pub fn hearing() -> u32 { HEARING.load(Relaxed) }
pub fn set_hearing(v: u32) { HEARING.store(v, Relaxed) }
pub fn connected() -> bool { CONNECTED.load(Relaxed) }
pub fn set_connected(v: bool) { CONNECTED.store(v, Relaxed) }
pub fn mic_ok() -> bool { MIC_OK.load(Relaxed) }
pub fn set_mic_ok(v: bool) { MIC_OK.store(v, Relaxed) }
/// Дистанция «Локального», которую вы сейчас используете (для громкости и обхода стен).
pub fn distance() -> u32 {
    match activation(PROXIMITY) {
        Some(a) => act_distance(&a).max(0) as u32,
        None => DISTANCE.load(Relaxed),
    }
}
pub fn set_distance(v: u32) { DISTANCE.store(v, Relaxed) }

// ---------- активации ----------
pub const PROXIMITY: &str = "proximity";
/// Сколько клавиш рации: «Локальный» и ещё две активации.
pub const PTT_SLOTS: usize = 3;
/// Имена клавиш рации в Options > Mods для слотов 1 и 2 (слот 0 — `PTT_KEY_NAME`). Не назначены.
pub const PTT_SLOT_KEYS: [&str; PTT_SLOTS] = [PTT_KEY_NAME, "push_to_talk_2", "push_to_talk_3"];
pub const UNBOUND_KEY: &str = "key.keyboard.unknown";

/// Когда активация передаёт голос.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActKind {
    /// Громче порога (как «Локальный» по умолчанию).
    Voice,
    /// Пока зажата её клавиша.
    Ptt,
    /// Вместе с «Локальным»: когда передаёт он, передаёт и она.
    Inherit,
}

impl ActKind {
    pub fn key(self) -> &'static str {
        match self {
            ActKind::Voice => "voice",
            ActKind::Ptt => "ptt",
            ActKind::Inherit => "inherit",
        }
    }
    fn parse(s: &str) -> Option<ActKind> {
        match s {
            "voice" => Some(ActKind::Voice),
            "ptt" => Some(ActKind::Ptt),
            "inherit" => Some(ActKind::Inherit),
            _ => None,
        }
    }
}

/// Ваши настройки одной активации.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActPref {
    pub kind: ActKind,
    /// Выключена кнопкой с микрофоном.
    pub off: bool,
    /// Выбранная дистанция (None — по умолчанию сервера).
    pub distance: Option<i32>,
}

/// По умолчанию, как в Plasmo: «Локальный» по голосу, остальные по клавише (пока не назначите — молчат).
pub fn default_pref(name: &str) -> ActPref {
    ActPref { kind: if name == PROXIMITY { ActKind::Voice } else { ActKind::Ptt }, off: false, distance: None }
}

pub fn act_pref(name: &str) -> ActPref {
    ACT_PREFS.lock().ok().and_then(|g| g.as_ref().and_then(|m| m.get(name).copied())).unwrap_or_else(|| default_pref(name))
}

pub fn set_act_pref(name: &str, mut p: ActPref) {
    if name == PROXIMITY && p.kind == ActKind::Inherit {
        p.kind = ActKind::Ptt; // «Локальный» не может наследовать сам себя
    }
    let old = act_pref(name);
    if let Ok(mut g) = ACT_PREFS.lock() {
        let m = g.get_or_insert_with(HashMap::new);
        if p == default_pref(name) {
            m.remove(name);
        } else {
            m.insert(name.to_string(), p);
        }
    }
    if old.distance != p.distance {
        ACT_DIST_DIRTY.store(true, Relaxed);
    }
}

/// Активации сервера по порядку: по весу, при равном — по имени.
pub fn server_activations() -> Vec<crate::plasmo::Activation> {
    ACTS.lock().map(|a| a.clone()).unwrap_or_default()
}
pub fn activation(name: &str) -> Option<crate::plasmo::Activation> {
    ACTS.lock().ok()?.iter().find(|a| a.name == name).cloned()
}

fn sort_acts(v: &mut [crate::plasmo::Activation]) {
    v.sort_by(|a, b| b.weight.cmp(&a.weight).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
}

/// Config пришёл: весь список активаций сервера.
pub fn set_server_activations(mut v: Vec<crate::plasmo::Activation>) {
    sort_acts(&mut v);
    if let Ok(mut a) = ACTS.lock() {
        *a = v;
    }
    ACT_DIST_DIRTY.store(true, Relaxed);
}
/// ActivationRegister: сервер добавил (или обновил) активацию.
pub fn register_activation(act: crate::plasmo::Activation) {
    if let Ok(mut a) = ACTS.lock() {
        a.retain(|x| x.id != act.id);
        a.push(act);
        sort_acts(&mut a);
    }
    ACT_DIST_DIRTY.store(true, Relaxed);
}
/// ActivationUnregister: активации больше нет.
pub fn unregister_activation(id: [u8; 16]) {
    if let Ok(mut a) = ACTS.lock() {
        a.retain(|x| x.id != id);
    }
}

/// Дистанция, с которой активация передаёт голос сейчас (выбранная, если сервер её разрешает).
pub fn act_distance(a: &crate::plasmo::Activation) -> i32 {
    a.allowed_distance(act_pref(&a.name).distance.unwrap_or(a.default_distance))
}

/// Пора ли сообщить серверу выбранные дистанции.
pub fn take_distances_dirty() -> bool { ACT_DIST_DIRTY.swap(false, Relaxed) }

/// Какой слот клавиши рации у активации: «Локальный» — 0, остальные по порядку — 1, 2; None — клавиш не хватило.
pub fn ptt_slot(name: &str) -> Option<usize> {
    ptt_slot_in(&server_activations(), name)
}

/// То же по заданному списку активаций (уже отсортированному по весу).
pub fn ptt_slot_in(acts: &[crate::plasmo::Activation], name: &str) -> Option<usize> {
    if name == PROXIMITY {
        return Some(0);
    }
    let i = acts.iter().filter(|a| a.name != PROXIMITY).position(|a| a.name == name)?;
    (i + 1 < PTT_SLOTS).then_some(i + 1)
}

pub fn reset_activations() {
    if let Ok(mut a) = ACTS.lock() {
        a.clear();
    }
}

// ---------- громкость отдельных игроков ----------
/// На сколько процентов меняет громкость один щелчок колеса.
pub const WHEEL_VOLUME_STEP: u32 = 10;
/// Сколько показывается подсказка над прицелом после последнего щелчка.
const TOAST_FOR: Duration = Duration::from_millis(1600);
/// Через сколько после последнего изменения громкости игроков пишем файл настроек.
const SAVE_AFTER: Duration = Duration::from_millis(1000);

fn vol_key(nick: &str) -> String {
    nick.trim().to_lowercase()
}

/// Громкость игрока в процентах (100, если её не меняли).
pub fn player_volume_pct(nick: &str) -> u32 {
    let k = vol_key(nick);
    PLAYER_VOL.lock().ok().and_then(|g| g.as_ref().and_then(|m| m.get(&k).copied())).unwrap_or(100)
}
/// Множитель громкости игрока (1.0 — без изменений).
pub fn player_volume(nick: &str) -> f32 {
    player_volume_pct(nick) as f32 / 100.0
}
pub fn set_player_volume_pct(nick: &str, pct: u32) {
    let (k, pct) = (vol_key(nick), pct.min(200));
    if k.is_empty() {
        return;
    }
    if let Ok(mut g) = PLAYER_VOL.lock() {
        let m = g.get_or_insert_with(HashMap::new);
        if pct == 100 {
            m.remove(&k);
        } else {
            m.insert(k, pct);
        }
    }
    if let Ok(mut d) = PLAYER_VOL_DIRTY.lock() {
        *d = Some(Instant::now());
    }
}
/// Все игроки, у которых громкость не 100%: (ник в нижнем регистре, проценты).
pub fn player_volumes() -> Vec<(String, u32)> {
    let mut v: Vec<(String, u32)> =
        PLAYER_VOL.lock().ok().and_then(|g| g.as_ref().map(|m| m.iter().map(|(k, p)| (k.clone(), *p)).collect())).unwrap_or_default();
    v.sort();
    v
}
pub fn clear_player_volumes() {
    if let Ok(mut g) = PLAYER_VOL.lock() {
        *g = None;
    }
    if let Ok(mut d) = PLAYER_VOL_DIRTY.lock() {
        *d = Some(Instant::now());
    }
}
pub fn wheel_volume_on() -> bool { WHEEL_VOLUME.load(Relaxed) }
pub fn set_wheel_volume_on(v: bool) { WHEEL_VOLUME.store(v, Relaxed) }
pub fn players_scroll() -> usize { PLAYERS_SCROLL.load(Relaxed) as usize }
pub fn set_players_scroll(v: usize) { PLAYERS_SCROLL.store(v as u32, Relaxed) }

/// Показать над прицелом «ник: громкость».
pub fn show_volume_toast(nick: &str, pct: u32) {
    if let Ok(mut t) = VOL_TOAST.lock() {
        *t = Some((nick.to_string(), pct, Instant::now()));
    }
}
/// Подсказка, если она ещё не погасла: (ник, проценты, насколько видна 0..=1).
pub fn volume_toast() -> Option<(String, u32, f32)> {
    let t = VOL_TOAST.lock().ok()?;
    let (nick, pct, at) = t.as_ref()?;
    let age = at.elapsed();
    if age >= TOAST_FOR {
        return None;
    }
    // последняя треть времени плавно гаснет
    let left = (TOAST_FOR - age).as_secs_f32() / TOAST_FOR.as_secs_f32();
    Some((nick.clone(), *pct, (left * 3.0).min(1.0)))
}

/// Колесо крутят много раз подряд: файл пишем один раз, когда игрок перестал (зовёт tick).
pub fn save_if_volumes_due() {
    let due = PLAYER_VOL_DIRTY.lock().ok().is_some_and(|d| d.is_some_and(|t| t.elapsed() >= SAVE_AFTER));
    if due {
        if let Ok(mut d) = PLAYER_VOL_DIRTY.lock() {
            *d = None;
        }
        save();
    }
}

/// Вернуть ползунки к значениям по умолчанию (переключатели не трогаем).
pub fn reset_sliders() {
    set_threshold(DEFAULT_THRESHOLD);
    set_mic_gain_pct(100);
    set_voice_volume_pct(100);
    set_group_volume_pct(100);
    set_noise_pct(0);
}

// ---------- файл ----------
/// Папка мода: `~/.config` (на Windows `%USERPROFILE%\.config`). Здесь лежат настройки, лог и файл-выключатель
/// микрофона: это папка самого игрока, не общая `/tmp`, где их мог бы подменить другой пользователь системы.
pub fn config_dir() -> std::path::PathBuf {
    let base = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join(".config")
}

fn path() -> std::path::PathBuf {
    config_dir().join("horizon-voice-mod.conf")
}

pub fn save() {
    let p = path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text = format!(
        "mic_muted={}\nvoice_off={}\nthreshold={}\nmic_gain={}\nvoice_volume={}\ngroup_volume={}\noverlay_on={}\noverlay_labels={}\noverlay_corner={}\nactivation={}\nnametags={}\nnoise={}\nwheel_volume={}\nmic_device={}\nmic_off={}\nhum_filter={}\nagc={}\nocclusion={}\ndirectional={}\nhrtf={}\nroutes={}\nout_device={}\n",
        mic_muted(),
        voice_off(),
        threshold(),
        mic_gain_pct(),
        voice_volume_pct(),
        group_volume_pct(),
        overlay_on(),
        overlay_labels(),
        overlay_corner(),
        if push_to_talk() { "ptt" } else { "voice" }, // для старых версий мода
        nametags_on(),
        noise_pct(),
        wheel_volume_on(),
        mic_device().replace(['\n', '\r'], " "),
        mic_off(),
        hum_filter(),
        agc_on(),
        occlusion_on(),
        directional_on(),
        hrtf_on(),
        routes_on(),
        out_device().replace(['\n', '\r'], " "),
    );
    let mut text = text;
    for (action, _) in ACTIONS {
        if let Some(k) = action_binding(action) {
            text.push_str(&format!("key.{action}={}\n", k.replace(['\n', '\r', '='], "")));
        }
    }
    text.push_str(&format!("dist_fade={}\n", dist_fade_on()));
    text.push_str(&format!("group_split={}\n", group_split_on()));
    text.push_str(&format!("group_corner={}\n", group_corner()));
    text.push_str(&format!("lang={}\n", crate::i18n::pref_key(crate::i18n::pref())));
    for slot in 0..PTT_SLOTS {
        if let Some(k) = slot_binding(slot) {
            text.push_str(&format!("ptt_key.{slot}={}\n", k.replace(['\n', '\r', '='], "")));
        }
    }
    let prefs: Vec<(String, ActPref)> =
        ACT_PREFS.lock().ok().and_then(|g| g.as_ref().map(|m| m.iter().map(|(k, v)| (k.clone(), *v)).collect())).unwrap_or_default();
    for (name, p) in prefs {
        let name: String = name.chars().filter(|c| !matches!(c, '=' | '\n' | '\r')).collect();
        let dist = p.distance.map(|d| d.to_string()).unwrap_or_default();
        text.push_str(&format!("act.{name}={},{},{dist}\n", p.kind.key(), if p.off { "off" } else { "on" }));
    }
    for (nick, pct) in player_volumes() {
        // ник бывает только из букв, цифр и «_», но на всякий случай выкидываем всё, что ломает строку
        let nick: String = nick.chars().filter(|c| !matches!(c, '=' | '\n' | '\r')).collect();
        text.push_str(&format!("pv.{nick}={pct}\n"));
    }
    let _ = std::fs::write(p, text);
}

pub fn load() {
    load_noise_profile();
    if let Ok(text) = std::fs::read_to_string(path()) {
        if let Ok(mut g) = PLAYER_VOL.lock() {
            *g = None;
        }
        // строки act.* идут после activation=, поэтому новые настройки «Локального» важнее старой
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "mic_muted" => set_mic_muted(v == "true"),
                "voice_off" => set_voice_off(v == "true"),
                "threshold" => {
                    if let Ok(t) = v.parse::<f32>() {
                        set_threshold(t)
                    }
                }
                "mic_gain" => {
                    if let Ok(t) = v.parse::<u32>() {
                        set_mic_gain_pct(t)
                    }
                }
                "voice_volume" => {
                    if let Ok(t) = v.parse::<u32>() {
                        set_voice_volume_pct(t)
                    }
                }
                "group_volume" => {
                    if let Ok(t) = v.parse::<u32>() {
                        set_group_volume_pct(t)
                    }
                }
                "overlay_on" => set_overlay_on(v == "true"),
                "activation" => set_push_to_talk(v == "ptt"),
                "nametags" => set_nametags_on(v == "true"),
                "noise" => {
                    if let Ok(t) = v.parse::<u32>() {
                        set_noise_pct(t)
                    }
                }
                "mic_device" => set_mic_device(v),
                "mic_off" => set_mic_off(v == "true"),
                "hum_filter" => set_hum_filter(v != "false"),
                "agc" => set_agc_on(v == "true"),
                "occlusion" => set_occlusion_on(v != "false"),
                "directional" => set_directional_on(v != "false"),
                "hrtf" => set_hrtf_on(v != "false"),
                "routes" => set_routes_on(v != "false"),
                "out_device" => set_out_device(v),
                "wheel_volume" => set_wheel_volume_on(v != "false"),
                "overlay_labels" => set_overlay_labels(v == "true"),
                "overlay_corner" => {
                    if let Ok(t) = v.parse::<u8>() {
                        set_overlay_corner(t)
                    }
                }
                other => {
                    if let (Some(nick), Ok(pct)) = (other.strip_prefix("pv."), v.parse::<u32>()) {
                        set_player_volume_pct(nick, pct);
                    } else if let Some(action) = other.strip_prefix("key.").filter(|a| ACTIONS.iter().any(|x| x.0 == *a)) {
                        set_action_binding(action, Some(v));
                    } else if other == "dist_fade" {
                        set_dist_fade_on(v != "false");
                    } else if other == "group_split" {
                        set_group_split_on(v != "false");
                    } else if other == "group_corner" {
                        if let Ok(t) = v.parse::<u8>() {
                            set_group_corner(t);
                        }
                    } else if other == "lang" {
                        crate::i18n::set_pref(crate::i18n::pref_from_key(v));
                    } else if let Some(slot) = other.strip_prefix("ptt_key.").and_then(|n| n.parse::<usize>().ok()) {
                        set_slot_binding(slot, Some(v));
                    } else if let Some(name) = other.strip_prefix("act.") {
                        let mut parts = v.split(',');
                        let mut p = default_pref(name);
                        if let Some(k) = parts.next().and_then(ActKind::parse) {
                            p.kind = k;
                        }
                        p.off = parts.next() == Some("off");
                        p.distance = parts.next().and_then(|d| d.parse().ok());
                        set_act_pref(name, p);
                    }
                }
            }
        }
        // загрузка не считается правкой: сохранять заново нечего
        if let Ok(mut d) = PLAYER_VOL_DIRTY.lock() {
            *d = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_threshold_bits_match() {
        assert_eq!(threshold(), DEFAULT_THRESHOLD);
    }

    #[test]
    fn key_labels_are_readable() {
        assert_eq!(key_label("key.keyboard.m"), "M");
        assert_eq!(key_label("key.keyboard.left.shift"), "LEFT SHIFT");
        assert_eq!(key_label("key.mouse.4"), "MOUSE 4");
        assert_eq!(menu_key_label(), "M");
    }

    #[test]
    fn ptt_label_mentions_ya_for_z() {
        assert_eq!(ptt_label_for("key.keyboard.z"), "Я (Z)");
        assert_eq!(ptt_label_for("key.keyboard.v"), "М (V)");
        assert_eq!(ptt_label_for("key.keyboard.g"), "П (G)");
        assert_eq!(ptt_label_for("key.keyboard.semicolon"), "Ж (SEMICOLON)");
        assert_eq!(ptt_label_for("key.keyboard.left.alt"), "LEFT ALT");
        assert_eq!(ptt_label_for(UNBOUND_KEY), "");
        assert_eq!(ptt_key_label(), "Я (Z)");
    }

    #[test]
    fn threshold_is_clamped() {
        set_threshold(10.0);
        assert_eq!(threshold(), 0.5);
        set_threshold(0.0);
        assert_eq!(threshold(), 0.0005);
        set_threshold(DEFAULT_THRESHOLD);
    }

    #[test]
    fn activation_prefs_default_and_slots() {
        use crate::plasmo::Activation;
        let act = |name: &str, weight: i32| Activation {
            name: name.into(),
            translation: String::new(),
            id: crate::plasmo::activation_id(name),
            distances: vec![8, 16, 32],
            default_distance: 16,
            proximity: name == PROXIMITY,
            transitive: false,
            stereo: false,
            weight,
        };
        set_server_activations(vec![act(PROXIMITY, 1), act("groups", 5), act("party", 3), act("extra", 2)]);
        let names: Vec<String> = server_activations().into_iter().map(|a| a.name).collect();
        assert_eq!(names, ["groups", "party", "extra", PROXIMITY], "по весу");
        assert_eq!(act_pref(PROXIMITY).kind, ActKind::Voice);
        assert_eq!(act_pref("groups").kind, ActKind::Ptt, "остальные по клавише, как в Plasmo");
        assert_eq!((ptt_slot(PROXIMITY), ptt_slot("groups"), ptt_slot("party"), ptt_slot("extra")), (Some(0), Some(1), Some(2), None));
        assert!(take_distances_dirty());
        set_act_pref(PROXIMITY, ActPref { distance: Some(32), ..act_pref(PROXIMITY) });
        assert!(take_distances_dirty(), "дистанцию надо сообщить серверу");
        assert_eq!(distance(), 32);
        set_act_pref(PROXIMITY, ActPref { distance: Some(20), ..act_pref(PROXIMITY) });
        assert_eq!(distance(), 16, "20 нет в списке сервера — по умолчанию");
        set_act_pref(PROXIMITY, ActPref { kind: ActKind::Inherit, ..act_pref(PROXIMITY) });
        assert_eq!(act_pref(PROXIMITY).kind, ActKind::Ptt, "«Локальный» не наследует сам себя");
        set_act_pref(PROXIMITY, default_pref(PROXIMITY));
        reset_activations();
    }

    #[test]
    fn player_volume_is_per_nick_and_case_insensitive() {
        clear_player_volumes();
        assert_eq!(player_volume_pct("Steve"), 100);
        set_player_volume_pct("Steve", 150);
        assert_eq!(player_volume_pct("steve"), 150);
        assert_eq!(player_volume("STEVE"), 1.5);
        assert_eq!(player_volume_pct("Alex"), 100);
        set_player_volume_pct("Steve", 999);
        assert_eq!(player_volume_pct("Steve"), 200, "больше 200% нельзя");
        set_player_volume_pct("Steve", 100);
        assert!(player_volumes().is_empty(), "100% не храним");
        set_player_volume_pct("Bob", 0);
        assert_eq!(player_volumes(), vec![("bob".to_string(), 0)]);
        clear_player_volumes();
    }
}
