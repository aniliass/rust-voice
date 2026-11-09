//! Меню «Rust Voice» (совместим с Plasmo Voice) в ванильном стиле Minecraft/Plasmo: тёмное окно, серые кнопки со скосом и чёрной обводкой, плоские
//! строки, свои пиксельные иконки (`crate::icons`) с нашими анимациями (блик, волны звука, вращение, подпрыгивание).
//! строки, ползунок с процентом внутри. Поверх этого анимации: плавное наведение, скользящие ручки и переключатели,
//! «ползущее» подчёркивание вкладки, всплытие окна и проявление страниц.
//!
//! Тут нет ни одного вызова API игры: всё рисуется через [`Canvas`] (прямоугольники и текст), а ввод приходит
//! через [`press`], [`release`] и позицию мыши в [`draw`]. Привязка к Horizon живёт в lib.rs.
//! Меню «непосредственного режима»: каждый кадр виджеты сразу рисуются и сразу проверяют клик.

use crate::faces;
use crate::icons::{self, Icon};
use crate::roster;
use crate::settings as S;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Всё, что нужно меню от игры: залить прямоугольник и написать строку (координаты в GUI-пикселях).
pub trait Canvas {
    fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, argb: u32);
    fn text(&mut self, s: &str, x: i32, y: i32, rgb: u32, shadow: bool);
    /// Голова игрока (лицо 8×8 и второй слой из скина) в квадрате `size`×`size`. `skin` — имя текстуры скина.
    /// Холст без картинок ничего не рисует, и меню тогда ставит заглушку.
    fn face(&mut self, _skin: &str, _x: i32, _y: i32, _size: i32) {}
}

// ---------- цвета (ванильная палитра: серые кнопки, чёрные рамки, белый текст; один фирменный акцент) ----------
const WHITE: u32 = 0xFFFF_FFFF;
const BLACK: u32 = 0xFF00_0000;
/// Рамка полей и карточек.
const OUTLINE: u32 = 0xFF55_5555;
const BORDER: u32 = OUTLINE;
const FIELD: u32 = 0xFF14_1414;
/// Тёмная дорожка ползунка.
const P_DEEP: u32 = 0xFF0A_0A0A;
/// Заливка дорожки до 100%: нейтральная, как в игре.
const P_FILL: u32 = 0xFF3A_3A3A;
/// Фирменный цвет: яркий фиолетовый. Подчёркивание вкладки, фокус поиска, слово «Rust» в названии,
/// заливка ползунка выше 100%. Хотите другой цвет: меняйте только эти четыре строки.
const RUST: u32 = 0xFFA8_55F7;
const RUST_TEXT: u32 = 0xA855F7;
const RUST_DK: u32 = 0xFF6D_32B8;
/// Включённый переключатель.
const P_ACC: u32 = 0xFF8E_42E0;
/// Зелёный «говорит»: подсветка головы игрока.
const LED: u32 = 0xFF55_FF55;
const BTN: u32 = 0xFF45_4545;
const BTN_HOVER: u32 = 0xFF62_6262;
const BTN_OFF: u32 = 0xFF6A_2A2A;
const BTN_SEL: u32 = 0xFF58_5858;
/// Рамка выбранной карточки.
const ACCENT: u32 = RUST;
/// Жёлтый для названий клавиш (rgb без альфы).
const ACCENT_TEXT: u32 = 0xFFFF55;
/// Громкость выше 100%: золотой, как код §6 в игре.
const OVER: u32 = 0xFFAA00;
const GREEN: u32 = 0xFF3F_C95A;
const TEXT: u32 = 0xFFFF_FF;
const DIM: u32 = 0xA0A0A0;
const ERR: u32 = 0xFF55_55;
const OK: u32 = 0x55FF_55;

/// Лупа 8×8 для поля поиска.
static LENS: Icon = Icon {
    w: 8,
    h: 8,
    runs: &[
        (2, 1, 3, 0xFFA0_A0A0),
        (1, 2, 1, 0xFFA0_A0A0),
        (5, 2, 1, 0xFFA0_A0A0),
        (1, 3, 1, 0xFFA0_A0A0),
        (5, 3, 1, 0xFFA0_A0A0),
        (1, 4, 1, 0xFFA0_A0A0),
        (5, 4, 1, 0xFFA0_A0A0),
        (2, 5, 3, 0xFFA0_A0A0),
        (5, 5, 1, 0xFFA0_A0A0),
        (6, 6, 1, 0xFFA0_A0A0),
        (7, 7, 1, 0xFFA0_A0A0),
    ],
};

// ---------- ширина текста ----------
/// Приблизительная ширина строки шрифтом игры, в GUI-пикселях (с точностью до пикселя-двух).
/// Ширина строки в пикселях GUI — той, что будет на экране (с переводом, см. `i18n`).
pub fn text_width(s: &str) -> i32 {
    crate::i18n::tr(s)
        .chars()
        .map(|ch| match ch {
            'i' | '.' | ',' | ':' | ';' | '!' | '|' => 2,
            'l' | '\'' | '`' => 3,
            ' ' | 'I' | 't' | '[' | ']' | '*' => 4,
            'f' | 'k' | '<' | '>' | '(' | ')' => 5,
            '@' | '~' => 7,
            _ => 6,
        })
        .sum()
}

// ---------- иконки ----------
/// Нарисовать иконку отрезками. `scale` — во сколько раз увеличить (иконки 16×16 и 8×8).
pub fn draw_icon(c: &mut dyn Canvas, icon: &Icon, x: i32, y: i32, scale: i32) {
    for &(rx, ry, n, argb) in icon.runs {
        c.fill(x + rx as i32 * scale, y + ry as i32 * scale, n as i32 * scale, scale, argb);
    }
}

// ---------- состояние меню ----------
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Devices,
    Players,
    Activation,
    Overlay,
    Status,
    Controls,
}

/// Порядок вкладок как в Plasmo Voice: Устройства, Громкость, Активации, Оверлей, Продвинутые, Клавиши.
const TABS: [Tab; 6] = [Tab::Devices, Tab::Players, Tab::Activation, Tab::Overlay, Tab::Status, Tab::Controls];
const TITLE: &str = "Rust Voice";
/// Версия мода из Cargo.toml.
const VERSION: &str = env!("CARGO_PKG_VERSION");

impl Tab {
    fn title(self) -> &'static str {
        match self {
            Tab::Devices => "Устройства",
            Tab::Players => "Громкость",
            Tab::Activation => "Активации",
            Tab::Overlay => "Оверлей",
            Tab::Controls => "Клавиши",
            Tab::Status => "Продвинутые",
        }
    }
    fn icon(self, selected: bool) -> &'static Icon {
        match (self, selected) {
            (Tab::Devices, true) => &icons::TAB_DEVICES,
            (Tab::Devices, false) => &icons::TAB_DEVICES_OFF,
            (Tab::Players, true) => &icons::TAB_VOLUME,
            (Tab::Players, false) => &icons::TAB_VOLUME_OFF,
            (Tab::Activation, true) => &icons::TAB_ACTIVATION,
            (Tab::Activation, false) => &icons::TAB_ACTIVATION_OFF,
            (Tab::Overlay, true) => &icons::TAB_OVERLAY,
            (Tab::Overlay, false) => &icons::TAB_OVERLAY_OFF,
            (Tab::Controls, true) => &icons::TAB_HOTKEYS,
            (Tab::Controls, false) => &icons::TAB_HOTKEYS_OFF,
            (Tab::Status, true) => &icons::TAB_ADVANCED,
            (Tab::Status, false) => &icons::TAB_ADVANCED_OFF,
        }
    }
}

struct State {
    tab: Tab,
    mx: i32,
    my: i32,
    /// Левая кнопка сейчас зажата.
    down: bool,
    /// Был клик, который кадр ещё не разобрал.
    pressed: bool,
    /// Номер ползунка, который тянут (0 — никакой).
    active: u32,
    gui_w: i32,
    gui_h: i32,
    /// Настройки менялись: сохраним, когда отпустят кнопку.
    dirty: bool,
    last_draw: Option<Instant>,
    /// Строка поиска во вкладке «Громкость» и в фокусе ли она (по умолчанию да, как в Plasmo).
    search: String,
    search_focus: bool,
    /// Громкость до «выключить» у кнопки с динамиком: (ключ, проценты). Ключ — ник или `\0local`/`\0group`.
    muted_prev: Vec<(String, u32)>,
    /// Анимации. Появление окна и проявление страницы после смены вкладки (оба 0..1, 1 — готово),
    /// x подчёркивания вкладки (меньше нуля — ещё не задан) и плавающие значения виджетов: (ключ, значение).
    open_t: f32,
    tab_t: f32,
    ind_x: f32,
    ind_w: f32,
    /// В какую сторону переключили вкладку: +1 вправо, -1 влево (страница наезжает с этой стороны).
    tab_dir: f32,
    anim: Vec<(u64, f32)>,
    /// Прокрутка вкладки «Устройства»: куда крутят колесом и что показано сейчас (плавно догоняет), в пикселях.
    dev_scroll: f32,
    dev_shown: f32,
    /// Сколько пикселей можно прокрутить (страница считает каждый кадр).
    dev_max: f32,
    /// Ждём нажатия клавиши: для слота рации или для действия (назначение клавиши в меню).
    binding: Option<Bind>,
}

static ST: Mutex<State> = Mutex::new(State {
    tab: Tab::Devices,
    mx: 0,
    my: 0,
    down: false,
    pressed: false,
    active: 0,
    gui_w: 427,
    gui_h: 240,
    dirty: false,
    last_draw: None,
    search: String::new(),
    search_focus: true,
    muted_prev: Vec::new(),
    open_t: 0.0,
    tab_t: 1.0,
    ind_x: -1.0,
    ind_w: 0.0,
    tab_dir: 1.0,
    anim: Vec::new(),
    dev_scroll: 0.0,
    dev_shown: 0.0,
    dev_max: 0.0,
    binding: None,
});

fn lock() -> std::sync::MutexGuard<'static, State> {
    match ST.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// Размер GUI в пикселях (для центрирования окна). Берётся из HUD, пока игрок ещё в мире.
pub fn set_gui_size(w: i32, h: i32) {
    if (200..4000).contains(&w) && (150..3000).contains(&h) {
        let mut st = lock();
        st.gui_w = w;
        st.gui_h = h;
    }
}

/// Меню открывают заново: забыть зажатые кнопки.
pub fn on_open() {
    let mut st = lock();
    st.down = false;
    st.pressed = false;
    st.active = 0;
    st.search.clear();
    st.search_focus = true;
    st.open_t = 0.0;
    st.tab_t = 1.0;
    st.ind_x = -1.0;
    st.anim.clear();
    st.dev_scroll = 0.0;
    st.dev_shown = 0.0;
    st.binding = None;
    S::set_mic_test(false);
    S::set_mic_picker(false);
}

/// Колесо над меню (`dy` > 0 — вверх). Прокручивает вкладку «Устройства»; true — колесо наше.
pub fn on_scroll(dy: f32) -> bool {
    let mut st = lock();
    if !st.last_draw.is_some_and(|t| t.elapsed() < Duration::from_millis(300)) {
        return false;
    }
    // щелчок колеса — строка; у тачпада бывают доли щелчка, их округляем, но не до нуля
    let rows = if dy.abs() < 0.5 { dy.signum() as i32 } else { dy.round() as i32 };
    match st.tab {
        Tab::Devices if S::mic_picker() || S::out_picker() => {
            S::set_mic_scroll((S::mic_scroll() as i32 - rows).max(0) as usize);
            true
        }
        Tab::Devices | Tab::Activation | Tab::Status | Tab::Controls | Tab::Overlay => {
            st.dev_scroll = (st.dev_scroll - dy * DEV_ROW as f32).clamp(0.0, st.dev_max);
            true
        }
        Tab::Players => {
            S::set_players_scroll((S::players_scroll() as i32 - rows).max(0) as usize);
            true
        }
        _ => false,
    }
}

/// Тонкая полоса прокрутки списка: `first` — первая видимая строка из `total`, видно `shown`.
fn list_scrollbar(ui: &mut Ui, x: i32, y: i32, h: i32, first: usize, shown: usize, total: usize) {
    if total <= shown || h <= 0 {
        return;
    }
    let bar = (h * shown as i32 / total as i32).max(10);
    let max_first = (total - shown) as f32;
    let by = y + ((h - bar) as f32 * (first as f32 / max_first).clamp(0.0, 1.0)).round() as i32;
    ui.fill(x, y, 2, h, 0x30FF_FFFF);
    ui.fill(x, by, 2, bar, mul_alpha(RUST, 0.8));
}

/// Рисуется ли меню прямо сейчас (последний кадр был не позже 300 мс назад).
pub fn is_live() -> bool {
    lock().last_draw.is_some_and(|t| t.elapsed() < Duration::from_millis(300))
}

/// Нажали левую кнопку мыши. Возвращает true, если нажатие наше (меню открыто) и его не надо отдавать игре.
pub fn press() -> bool {
    let mut st = lock();
    if !st.last_draw.is_some_and(|t| t.elapsed() < Duration::from_millis(300)) {
        return false;
    }
    st.down = true;
    st.pressed = true;
    true
}

/// Нажата или отпущена клавиша, пока открыта вкладка «Громкость» с полем поиска в фокусе: буквы, цифры, «_»
/// (клавиша минус) и Backspace идут в поиск. Возвращает true, если клавишу забрало поле (меню по ней не закрывать).
pub fn on_key(key: &str, pressed: bool) -> bool {
    let mut st = lock();
    if !st.last_draw.is_some_and(|t| t.elapsed() < Duration::from_millis(300)) || st.tab != Tab::Players || !st.search_focus {
        return false;
    }
    let Some(k) = key.strip_prefix("key.keyboard.") else { return false };
    let ch = match k {
        "backspace" => None,
        "minus" => Some('_'),
        _ => {
            let mut it = k.chars();
            match (it.next(), it.next()) {
                (Some(c), None) if c.is_ascii_lowercase() || c.is_ascii_digit() => Some(c),
                _ => return false,
            }
        }
    };
    if pressed {
        match ch {
            Some(c) if st.search.chars().count() < 16 => st.search.push(c),
            Some(_) => {}
            None => {
                st.search.pop();
            }
        }
    }
    true
}

/// Чему назначаем клавишу в меню.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bind {
    /// Слот рации (0 — «Локальный»).
    Slot(usize),
    /// Действие из `settings::ACTIONS`.
    Action(&'static str),
}

/// Пока в меню ждём клавишу для рации: нажатая клавиша становится клавишей рации (Esc — отмена,
/// Backspace/Delete — убрать клавишу). Возвращает true, если клавишу забрали (игре и рации она не идёт).
pub fn capture_key(key: &str, pressed: bool) -> bool {
    let mut st = lock();
    let Some(bind) = st.binding else { return false };
    if !st.last_draw.is_some_and(|t| t.elapsed() < Duration::from_millis(300)) {
        st.binding = None;
        return false;
    }
    if !pressed {
        return true;
    }
    st.binding = None;
    drop(st);
    let chosen = match key {
        "key.keyboard.escape" => return true,
        "key.keyboard.backspace" | "key.keyboard.delete" => S::UNBOUND_KEY,
        k => k,
    };
    match bind {
        Bind::Slot(slot) => {
            S::set_slot_binding(slot, Some(chosen));
            S::set_slot_key_label(slot, &S::ptt_label_for(chosen));
        }
        Bind::Action(a) => {
            // меню без клавиши не открыть: Backspace у него возвращает клавишу из Options > Mods
            if a == "menu" && chosen == S::UNBOUND_KEY {
                S::set_action_binding(a, None);
            } else {
                S::set_action_binding(a, Some(chosen));
                if a == "menu" {
                    S::set_menu_key_label(&S::ptt_label_for(chosen));
                }
            }
        }
    }
    S::save();
    true
}

pub fn release() {
    let mut st = lock();
    st.down = false;
}

// ---------- анимации ----------
/// Ключ анимации виджета: вид + позиция (маска в 20 бит, чтобы отрицательные координаты не портили ключ).
fn akey(kind: u64, x: i32, y: i32) -> u64 {
    (kind << 44) ^ (((x as i64 & 0xF_FFFF) as u64) << 22) ^ ((y as i64 & 0xF_FFFF) as u64)
}

/// Умножить прозрачность цвета ARGB на `a` (0..1).
fn mul_alpha(argb: u32, a: f32) -> u32 {
    let al = ((argb >> 24) as f32 * a.clamp(0.0, 1.0)).round() as u32;
    (al << 24) | (argb & 0x00FF_FFFF)
}

/// Замедление к концу: быстрый старт, мягкая остановка.
fn ease_out(t: f32) -> f32 {
    let u = 1.0 - t.clamp(0.0, 1.0);
    1.0 - u * u * u
}

/// Время в миллисекундах по кругу в час: для анимаций, которым не нужно состояние.
fn now_ms() -> f32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_millis() % 3_600_000) as f32)
        .unwrap_or(0.0)
}

/// Эффекты, которые можно навесить на иконку при рисовании. Всё считается по пикселям иконки, поэтому
/// выглядит как пиксель-арт, а не как размытая картинка.
#[derive(Clone, Copy)]
pub struct Fx {
    /// Прозрачность 0..1.
    pub a: f32,
    /// Сдвиг в пикселях иконки.
    pub dx: f32,
    pub dy: f32,
    /// Блик: 0..1 — где полоса света на диагонали иконки, меньше нуля — блика нет.
    pub glint: f32,
    /// Столбцы начиная с `ripple_from` мерцают бегущей волной (звук); `ripple` — сила 0..1, `phase` — фаза.
    pub ripple_from: u8,
    pub ripple: f32,
    pub phase: f32,
    /// Поворот по часовой стрелке, радианы.
    pub spin: f32,
    /// Подмешать цвет `tint` (RGB) на долю `tint_t` (0..1).
    pub tint: u32,
    pub tint_t: f32,
}

impl Fx {
    pub const NONE: Fx =
        Fx { a: 1.0, dx: 0.0, dy: 0.0, glint: -1.0, ripple_from: 0, ripple: 0.0, phase: 0.0, spin: 0.0, tint: 0, tint_t: 0.0 };
}

/// Иконка с эффектами `fx`. Без эффектов рисуется быстро, готовыми отрезками.
pub fn draw_icon_fx(c: &mut dyn Canvas, icon: &Icon, x: i32, y: i32, scale: i32, fx: Fx) {
    let ox = x + (fx.dx * scale as f32).round() as i32;
    let oy = y + (fx.dy * scale as f32).round() as i32;
    if fx.glint < 0.0 && fx.ripple <= 0.0 && fx.spin == 0.0 && fx.tint_t <= 0.0 {
        for &(rx, ry, n, argb) in icon.runs {
            c.fill(ox + rx as i32 * scale, oy + ry as i32 * scale, n as i32 * scale, scale, mul_alpha(argb, fx.a));
        }
        return;
    }
    // раскладываем иконку на пиксели и рисуем каждый отдельно
    let (w, h) = ((icon.w as usize).min(16), (icon.h as usize).min(16));
    let mut grid = [[0u32; 16]; 16];
    for &(rx, ry, n, argb) in icon.runs {
        for i in 0..n as usize {
            let (gx, gy) = (rx as usize + i, ry as usize);
            if gx < 16 && gy < 16 {
                grid[gy][gx] = argb;
            }
        }
    }
    let (sin, cos) = (-fx.spin).sin_cos();
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    for py in 0..h {
        for px in 0..w {
            // поворот: для каждого пикселя результата берём пиксель исходной иконки, который в него попал
            let (sx, sy) = if fx.spin != 0.0 {
                let (vx, vy) = (px as f32 + 0.5 - cx, py as f32 + 0.5 - cy);
                let (qx, qy) = (vx * cos - vy * sin + cx, vx * sin + vy * cos + cy);
                if qx < 0.0 || qy < 0.0 {
                    continue;
                }
                (qx as usize, qy as usize)
            } else {
                (px, py)
            };
            if sx >= w || sy >= h {
                continue;
            }
            let mut col = grid[sy][sx];
            if col >> 24 == 0 {
                continue;
            }
            let mut al = fx.a;
            if fx.ripple > 0.0 && px >= fx.ripple_from as usize {
                let k = (px - fx.ripple_from as usize) as f32;
                let pulse = 0.5 + 0.5 * (fx.phase - k * 0.8).sin();
                al *= 1.0 - fx.ripple * 0.7 * (1.0 - pulse);
            }
            if fx.glint >= 0.0 {
                let diag = (px + py) as f32 / (w + h) as f32;
                let d = (diag - (fx.glint * 1.6 - 0.3)).abs();
                if d < 0.12 {
                    col = lerp_argb(col, (col & 0xFF00_0000) | 0x00FF_FFFF, (1.0 - d / 0.12) * 0.85);
                }
            }
            if fx.tint_t > 0.0 {
                col = lerp_argb(col, (col & 0xFF00_0000) | (fx.tint & 0x00FF_FFFF), fx.tint_t.min(1.0));
            }
            c.fill(ox + px as i32 * scale, oy + py as i32 * scale, scale, scale, mul_alpha(col, al));
        }
    }
}

/// Что иконка на кнопке делает при наведении и переключении.
#[derive(Clone, Copy)]
enum Anim {
    /// Микрофон: при включении/выключении трясётся; пока вы говорите, «дышит» зелёным.
    Mic { live: bool },
    /// Динамик: при наведении по нему бегут звуковые волны; при переключении подпрыгивает.
    Spk,
    /// Сброс: при наведении крутится по часовой стрелке.
    Reset,
}

// ---------- виджеты ----------
struct Ui<'a> {
    c: &'a mut dyn Canvas,
    mx: i32,
    my: i32,
    pressed: bool,
    down: bool,
    active: u32,
    changed: bool,
    tip: Option<String>,
    /// Размер GUI, чтобы страница знала, сколько строк игроков помещается.
    gw: i32,
    gh: i32,
    /// Мигающая «_» в поле поиска: видна первую половину каждой секунды.
    blink: bool,
    /// Секунд с прошлого кадра (не больше 0.05).
    dt: f32,
    /// Плавающие значения анимаций: (ключ, значение). Живут между кадрами в `State`.
    anim: Vec<(u64, f32)>,
    /// Всплытие окна и проявление страницы после смены вкладки (0..1, уже со сглаживанием).
    open: f32,
    tab_t: f32,
    /// x и ширина скользящего подчёркивания вкладки.
    ind_x: f32,
    ind_w: f32,
    tab_dir: f32,
    /// Видимая полоса страницы по Y: строки, которые в неё не помещаются целиком, не рисуются (прокрутка).
    clip: (i32, i32),
}

impl Ui<'_> {
    /// Помещается ли строка высотой `h` с верхом `y` в видимую полосу.
    fn vis(&self, y: i32, h: i32) -> bool {
        y >= self.clip.0 && y + h <= self.clip.1
    }

    /// Переключатель в духе Plasmo: поле как у ползунка, ручка слева («Выкл») или справа («Вкл»), едет плавно.
    /// `enabled` = false — бледный и не нажимается. Возвращает новое значение.
    fn onoff(&mut self, x: i32, y: i32, w: i32, on: bool, enabled: bool) -> bool {
        const KNOB: i32 = 8;
        let a = self.ease(akey(15, x, y), 1.0, if enabled { 1.0 } else { 0.4 }, 14.0);
        let hot = if enabled { self.hot(16, x, y, w, 20) } else { 0.0 };
        let to = if on { 1.0 } else { 0.0 };
        let t = self.ease(akey(17, x, y), to, to, 18.0);
        self.fill(x, y, w, 20, mul_alpha(lerp_argb(BLACK, 0xFFC8_C8C8, hot * 0.8), a));
        self.fill(x + 1, y + 1, w - 2, 18, mul_alpha(lerp_argb(0xFF26_2626, 0xFF34_3434, hot), a));
        let kx = x + (t * (w - KNOB) as f32).round() as i32;
        if kx > x + 1 {
            self.fill(x + 1, y + 1, kx - x - 1, 18, mul_alpha(lerp_argb(0xFF26_2626, RUST_DK, t), a));
        }
        self.vbtn(kx, y, KNOB, 20, 0xFF6C_6C6C, 0xFF8C_8C8C, hot, a);
        let label = if on { "Вкл" } else { "Выкл" };
        let col = if enabled { TEXT } else { DIM };
        self.text(label, x + (w - text_width(label)) / 2, y + 6, col);
        if enabled && self.clicked(x, y, w, 20) {
            self.changed = true;
            return !on;
        }
        on
    }

    fn face(&mut self, skin: &str, x: i32, y: i32, size: i32) {
        self.c.face(skin, x, y, size);
    }

    /// Плавно приблизить значение под `key` к `target` (`speed` — чем больше, тем быстрее). Впервые видим
    /// значение — стартуем с `init`.
    fn ease(&mut self, key: u64, init: f32, target: f32, speed: f32) -> f32 {
        let k = 1.0 - (-speed * self.dt).exp();
        if let Some(pos) = self.anim.iter().position(|e| e.0 == key) {
            let v = &mut self.anim[pos].1;
            *v += (target - *v) * k;
            if (target - *v).abs() < 0.003 {
                *v = target;
            }
            return *v;
        }
        if self.anim.len() > 400 {
            self.anim.clear();
        }
        let v = init + (target - init) * k;
        self.anim.push((key, v));
        v
    }

    /// Плавное наведение на прямоугольник: 0 — мышь не над ним, 1 — над ним.
    fn hot(&mut self, kind: u64, x: i32, y: i32, w: i32, h: i32) -> f32 {
        let t = if self.hover(x, y, w, h) { 1.0 } else { 0.0 };
        self.ease(akey(kind, x, y), 0.0, t, 16.0)
    }

    /// Цвет, плавно переходящий от `base` к `hov`, пока мышь над прямоугольником.
    fn shade(&mut self, x: i32, y: i32, w: i32, h: i32, base: u32, hov: u32) -> u32 {
        let t = self.hot(1, x, y, w, h);
        lerp_argb(base, hov, t)
    }

    /// Положение ручки: пока тянут, она точно под мышью, иначе плавно едет к значению (сброс, «липкие» 100%).
    fn glide(&mut self, key: u64, target: f32, dragging: bool) -> f32 {
        if dragging {
            match self.anim.iter().position(|e| e.0 == key) {
                Some(pos) => self.anim[pos].1 = target,
                None => self.anim.push((key, target)),
            }
            target
        } else {
            self.ease(key, target, target, 22.0)
        }
    }

    /// Строка списка: лёгкая светлая подложка под курсором.
    fn row_hover(&mut self, x: i32, y: i32, w: i32, h: i32) {
        let t = self.hot(10, x, y, w, h);
        if t > 0.0 {
            self.fill(x, y, w, h, mul_alpha(0x1CFF_FFFF, t));
        }
    }

    /// Ванильная кнопка: чёрная обводка (при наведении светлеет), серая грань со скосом. `hot` — наведение 0..1,
    /// `a` — прозрачность 0..1.
    fn vbtn(&mut self, x: i32, y: i32, w: i32, h: i32, base: u32, hov: u32, hot: f32, a: f32) {
        let outline = mul_alpha(lerp_argb(BLACK, 0xFFE0_E0E0, hot * 0.9), a);
        let face = mul_alpha(lerp_argb(base, hov, hot), a);
        self.fill(x, y, w, h, outline);
        self.fill(x + 1, y + 1, w - 2, h - 2, face);
        if w > 4 && h > 4 {
            self.fill(x + 1, y + 1, w - 2, 1, mul_alpha(0x40FF_FFFF, a));
            self.fill(x + 1, y + h - 2, w - 2, 1, mul_alpha(0x5500_0000, a));
        }
    }

    /// Ползунок громкости как у Plasmo: тёмная дорожка, процент по центру, ручка — серая кнопка.
    /// 0..1 -> `Some(новое)`, пока тянут.
    fn mc_slider(&mut self, id: u32, x: i32, y: i32, w: i32, h: i32, v: f32, label: &str, color: u32) -> Option<f32> {
        const KNOB: i32 = 8;
        let over = self.hover(x, y, w, h);
        let mut grab = false;
        if self.pressed && over {
            self.pressed = false;
            self.active = id;
            grab = true;
        }
        let mut out = None;
        if self.active == id {
            if self.down || grab {
                out = Some(((self.mx - x - KNOB / 2) as f32 / (w - KNOB) as f32).clamp(0.0, 1.0));
                self.changed = true;
            } else {
                self.active = 0;
            }
        }
        let shown = out.unwrap_or(v).clamp(0.0, 1.0);
        let shown = self.glide(akey(8, id as i32, y), shown, out.is_some());
        let hot_t = if over || self.active == id { 1.0 } else { 0.0 };
        let hot = self.ease(akey(9, id as i32, y), 0.0, hot_t, 16.0);
        self.fill(x, y, w, h, lerp_argb(BLACK, 0xFFC8_C8C8, hot * 0.8));
        self.fill(x + 1, y + 1, w - 2, h - 2, lerp_argb(0xFF26_2626, 0xFF34_3434, hot));
        let kx = x + (shown * (w - KNOB) as f32).round() as i32;
        // заливка до ручки: серая до 100%, «ржавая» дальше
        let end = kx + KNOB / 2;
        let mid = x + w / 2;
        let lo = end.min(mid) - (x + 1);
        if lo > 0 {
            self.fill(x + 1, y + 1, lo, h - 2, P_FILL);
        }
        if end > mid {
            self.fill(mid, y + 1, end - mid, h - 2, RUST_DK);
        }
        // тонкая отметка 100% посередине
        self.fill(mid, y + 2, 1, h - 4, 0x33FF_FFFF);
        self.vbtn(kx, y, KNOB, h, 0xFF6C_6C6C, 0xFF8C_8C8C, hot, 1.0);
        let tw = text_width(label);
        self.text(label, x + (w - tw) / 2, y + (h - 8) / 2, color);
        out
    }

    /// Кнопка с иконкой «Сбросить»: когда сбрасывать нечего (`enabled` = false), она плавно гаснет и не нажимается.
    fn reset_button(&mut self, x: i32, y: i32, enabled: bool, tip: &str) -> bool {
        let a = self.ease(akey(4, x, y), 0.0, if enabled { 1.0 } else { 0.0 }, 14.0);
        if a < 0.02 {
            return false;
        }
        let hit = self.icon_button_a(x, y, 20, &icons::RESET, Anim::Reset, false, tip, a);
        hit && enabled
    }

    fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, argb: u32) {
        self.c.fill(x, y, w, h, argb);
    }
    fn text(&mut self, s: &str, x: i32, y: i32, rgb: u32) {
        self.c.text(s, x, y, rgb, true);
    }
    /// Плоское поле: 1 px рамки и заливка (углы прямые, как в игре).
    fn frame(&mut self, x: i32, y: i32, w: i32, h: i32, border: u32, bg: u32) {
        self.fill(x, y, w, h, border);
        self.fill(x + 1, y + 1, w - 2, h - 2, bg);
    }
    /// Поле со скосом, как у кнопки (для плиток-переключателей и строк выбора).
    fn vbox(&mut self, x: i32, y: i32, w: i32, h: i32, border: u32, face: u32) {
        self.frame(x, y, w, h, border, face);
        if w > 4 && h > 4 {
            self.fill(x + 1, y + 1, w - 2, 1, 0x40FF_FFFF);
            self.fill(x + 1, y + h - 2, w - 2, 1, 0x5500_0000);
        }
    }
    fn icon(&mut self, icon: &Icon, x: i32, y: i32, scale: i32) {
        draw_icon(&mut *self.c, icon, x, y, scale);
    }
    fn hover(&self, x: i32, y: i32, w: i32, h: i32) -> bool {
        self.mx >= x && self.mx < x + w && self.my >= y && self.my < y + h
    }
    /// Клик по прямоугольнику в этом кадре. Клик «съедается»: второй виджет под ним его уже не увидит.
    fn clicked(&mut self, x: i32, y: i32, w: i32, h: i32) -> bool {
        let hit = self.pressed && self.hover(x, y, w, h);
        if hit {
            self.pressed = false;
        }
        hit
    }
    fn tooltip(&mut self, x: i32, y: i32, w: i32, h: i32, s: &str) {
        if self.tip.is_none() && self.hover(x, y, w, h) {
            self.tip = Some(s.to_string());
        }
    }

    fn icon_fx(&mut self, icon: &Icon, x: i32, y: i32, scale: i32, fx: Fx) {
        draw_icon_fx(&mut *self.c, icon, x, y, scale, fx);
    }

    /// Квадратная кнопка с иконкой. `bad` — красноватая (выключено), цвет меняется плавно.
    fn icon_button(&mut self, x: i32, y: i32, size: i32, icon: &Icon, anim: Anim, bad: bool, tip: &str) -> bool {
        self.icon_button_a(x, y, size, icon, anim, bad, tip, 1.0)
    }

    /// То же с прозрачностью `a` (для появления и исчезновения). `anim` — анимация иконки.
    fn icon_button_a(&mut self, x: i32, y: i32, size: i32, icon: &Icon, anim: Anim, bad: bool, tip: &str, a: f32) -> bool {
        let hot = self.hot(2, x, y, size, size);
        let bad_to = if bad { 1.0 } else { 0.0 };
        let bad_t = self.ease(akey(3, x, y), bad_to, bad_to, 14.0);
        let base = lerp_argb(BTN, BTN_OFF, bad_t);
        let hov = lerp_argb(BTN_HOVER, 0xFF8A_3A3A, bad_t);
        self.vbtn(x, y, size, size, base, hov, hot, a);

        let secs = now_ms() / 1000.0;
        // 0 в покое, 1 посередине перехода «включено ↔ выключено»
        let flip = (std::f32::consts::PI * bad_t).sin();
        let mut fx = Fx { a, ..Fx::NONE };
        // когда на кнопку только навели, по иконке пробегает блик
        if hot > 0.02 && hot < 0.9 {
            fx.glint = hot;
        }
        match anim {
            Anim::Mic { live } => {
                fx.dx = (secs * 70.0).sin() * flip * 1.5;
                if live && !bad {
                    let p = 0.5 + 0.5 * (secs * 9.0).sin();
                    fx.dy = -p;
                    fx.tint = LED;
                    fx.tint_t = 0.12 + 0.3 * p;
                }
            }
            Anim::Spk => {
                fx.dy = -flip * 1.5;
                if !bad {
                    fx.ripple = hot;
                    fx.ripple_from = 10;
                    fx.phase = secs * 9.0;
                }
            }
            Anim::Reset => {
                fx.spin = hot * std::f32::consts::TAU;
            }
        }
        let side = icon.w as i32;
        draw_icon_fx(&mut *self.c, icon, x + (size - side) / 2, y + (size - side) / 2, 1, fx);
        if a < 0.5 {
            return false;
        }
        self.tooltip(x, y, size, size, tip);
        self.clicked(x, y, size, size)
    }

    fn button(&mut self, x: i32, y: i32, w: i32, h: i32, label: &str) -> bool {
        let hot = self.hot(2, x, y, w, h);
        self.vbtn(x, y, w, h, BTN, BTN_HOVER, hot, 1.0);
        let tw = text_width(label);
        self.text(label, x + (w - tw) / 2, y + (h - 8) / 2, TEXT);
        self.clicked(x, y, w, h)
    }

    /// Строка «подпись — переключатель»: ручка плавно скользит, дорожка меняет цвет. Возвращает новое значение.
    fn toggle(&mut self, x: i32, y: i32, w: i32, label: &str, on: bool) -> bool {
        let h = 16;
        let hot = self.hot(5, x, y, w, h);
        if hot > 0.0 {
            self.fill(x, y, w, h, mul_alpha(0x2AFF_FFFF, hot));
        }
        self.text(label, x + 2, y + 4, TEXT);
        let to = if on { 1.0 } else { 0.0 };
        let t = self.ease(akey(6, x, y), to, to, 16.0);
        let sx = x + w - 30;
        self.fill(sx, y + 1, 28, 14, BLACK);
        self.fill(sx + 1, y + 2, 26, 12, lerp_argb(0xFF3A_3A3A, P_ACC, t));
        let kx = sx + 1 + (t * 14.0).round() as i32;
        self.vbtn(kx, y + 2, 12, 12, 0xFFB0_B0B0, 0xFFD0_D0D0, hot, 1.0);
        if self.clicked(x, y, w, h) {
            self.changed = true;
            return !on;
        }
        on
    }

    /// Ползунок. `v` — положение ручки 0..1, `fill` — до какой доли и каким цветом залить дорожку
    /// (для порога это живой уровень микрофона). Возвращает новое положение, если его двигают.
    fn slider(&mut self, id: u32, x: i32, y: i32, w: i32, h: i32, v: f32, fill: (f32, u32)) -> Option<f32> {
        const KNOB: i32 = 6;
        let over = self.hover(x - 2, y - 3, w + 4, h + 6);
        let mut grab = false;
        if self.pressed && over {
            self.pressed = false;
            self.active = id;
            grab = true;
        }
        let mut out = None;
        if self.active == id {
            if self.down || grab {
                let nv = ((self.mx - x - KNOB / 2) as f32 / (w - KNOB) as f32).clamp(0.0, 1.0);
                out = Some(nv);
                self.changed = true;
            } else {
                self.active = 0;
            }
        }
        let shown = out.unwrap_or(v).clamp(0.0, 1.0);
        let shown = self.glide(akey(8, id as i32, y), shown, out.is_some());
        let hot_t = if over || self.active == id { 1.0 } else { 0.0 };
        let hot = self.ease(akey(9, id as i32, y), 0.0, hot_t, 16.0);
        self.frame(x, y, w, h, lerp_argb(OUTLINE, 0xFFD0_D0D0, hot), P_DEEP);
        let fw = ((fill.0.clamp(0.0, 1.0)) * (w - 2) as f32) as i32;
        if fw > 0 {
            self.fill(x + 1, y + 1, fw, h - 2, fill.1);
        }
        let kx = x + (shown * (w - KNOB) as f32).round() as i32;
        self.vbtn(kx, y - 2, KNOB, h + 4, 0xFF6C_6C6C, 0xFF8C_8C8C, hot, 1.0);
        out
    }
}

// ---------- dB ----------
fn to_db(x: f32) -> f32 {
    20.0 * x.max(1e-5).log10()
}
fn db_frac(db: f32) -> f32 {
    ((db + 60.0) / 60.0).clamp(0.0, 1.0)
}
fn frac_to_threshold(f: f32) -> f32 {
    10f32.powf((f.clamp(0.0, 1.0) * 60.0 - 60.0) / 20.0)
}

// ---------- страницы ----------
/// Обрезает строку по ширине, добавляя «…».
fn fit(s: &str, max_w: i32) -> String {
    // режем уже переведённую строку: иначе обрезок не нашёлся бы в словаре
    let s = &*crate::i18n::tr(s);
    if text_width(s) <= max_w {
        return s.to_string();
    }
    let mut out = String::new();
    for c in s.chars() {
        let next = format!("{out}{c}…");
        if text_width(&next) > max_w {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

const PICK_ROWS: usize = 6;
const DEFAULT_MIC: &str = "Системный по умолчанию";
const DEFAULT_OUT: &str = "Системный по умолчанию";
/// Шаг строк вкладки «Устройства» (как у Plasmo) и на сколько прокручивает один щелчок колеса.
const DEV_ROW: i32 = 24;

/// Выбор устройства (микрофона или вывода): список вместо обычной страницы.
fn page_device_picker(ui: &mut Ui, x: i32, y: i32, w: i32, output: bool) {
    let close = |_: ()| {
        S::set_mic_picker(false);
    };
    ui.text(if output { "Выберите устройство вывода" } else { "Выберите микрофон" }, x, y, TEXT);
    let back = "< Назад";
    let bw = text_width(back) + 12;
    let face = ui.shade(x + w - bw, y - 3, bw, 16, BTN, BTN_HOVER);
    ui.vbox(x + w - bw, y - 3, bw, 16, BLACK, face);
    ui.text(back, x + w - bw + 6, y + 1, TEXT);
    if ui.clicked(x + w - bw, y - 3, bw, 16) {
        close(());
        return;
    }

    let current = if output { S::out_device() } else { S::mic_device() };
    let mut names = vec![(String::new(), (if output { DEFAULT_OUT } else { DEFAULT_MIC }).to_string())];
    names.extend(if output { S::out_list() } else { S::mic_list() });
    let scroll = S::mic_scroll().min(names.len().saturating_sub(PICK_ROWS));
    S::set_mic_scroll(scroll); // чтобы колесо вверх сразу работало, даже если прокрутили «за край»
    for (i, (value, label)) in names.iter().enumerate().skip(scroll).take(PICK_ROWS) {
        let ry = y + 16 + (i - scroll) as i32 * 22;
        let sel = *value == current;
        let bg = if sel { BTN_SEL } else { ui.shade(x, ry, w, 20, FIELD, BTN_HOVER) };
        ui.vbox(x, ry, w, 20, if sel { ACCENT } else { BORDER }, bg);
        ui.icon(if output { &icons::SPK } else { &icons::MIC }, x + 2, ry + 2, 1);
        ui.text(&fit(label, w - 30), x + 22, ry + 6, TEXT);
        if ui.clicked(x, ry, w, 20) {
            if output {
                S::set_out_device(value);
            } else {
                S::set_mic_device(value);
            }
            ui.changed = true;
            close(());
            return;
        }
    }
    if names.len() > PICK_ROWS {
        list_scrollbar(ui, x + w + 4, y + 16, PICK_ROWS as i32 * 22 - 2, scroll, PICK_ROWS, names.len());
    } else {
        ui.text("Нужного нет? Подключите его и откройте список заново.", x, y + 16 + names.len() as i32 * 22 + 4, DIM);
    }
}

/// Строка вкладки «Устройства» в духе Plasmo: подпись слева, элемент справа, «сбросить» с краю.
/// Возвращает (x элемента, ширина элемента).
fn dev_row(ui: &mut Ui, x: i32, y: i32, w: i32, label: &str) -> (i32, i32) {
    let lw = w * 45 / 100;
    ui.text(&fit(label, lw - 4), x, y + 6, TEXT);
    (x + lw, w - lw - 24)
}

/// Выпадающий список устройства: название выбранного, «▼», клик открывает выбор.
fn device_dropdown(ui: &mut Ui, cx: i32, ry: i32, cw: i32, label: &str, color: u32, tip: &str) -> bool {
    let face = ui.shade(cx, ry, cw, 20, FIELD, 0xFF26_2626);
    ui.vbox(cx, ry, cw, 20, BORDER, face);
    ui.text(&fit(label, cw - 20), cx + 5, ry + 6, color);
    ui.text("▼", cx + cw - 12, ry + 6, DIM);
    ui.tooltip(cx, ry, cw, 20, tip);
    ui.clicked(cx, ry, cw, 20)
}

/// Подпись выбранного устройства по его значению в конфиге.
fn device_label(value: &str, list: Vec<(String, String)>, default: &str) -> String {
    if value.is_empty() {
        return default.to_string();
    }
    list.into_iter().find(|(v, _)| v == value).map(|(_, l)| l).unwrap_or_else(|| value.to_string())
}

/// Строка «подпись — переключатель — сброс к значению по умолчанию».
fn onoff_row(ui: &mut Ui, x: i32, ry: i32, w: i32, label: &str, tip: &str, on: bool, default: bool, enabled: bool) -> bool {
    let rx = x + w - 20;
    let (cx, cw) = dev_row(ui, x, ry, w, label);
    ui.tooltip(x, ry, cx - x, 20, tip);
    let mut v = ui.onoff(cx, ry, cw, on, enabled);
    if ui.reset_button(rx, ry, enabled && on != default, if default { "Сбросить (вкл)" } else { "Сбросить (выкл)" }) {
        v = default;
        ui.changed = true;
    }
    v
}

fn page_devices(ui: &mut Ui, x: i32, y: i32, w: i32) {
    if S::mic_picker() || S::out_picker() {
        // список устройств не прокручивается вместе со страницей: рисуем от верха колонки
        let top = ui.clip.0 + 4;
        page_device_picker(ui, x, top, w, S::out_picker());
        return;
    }
    let rx = x + w - 20;
    // Раскладка: y каждой строки считается по порядку, рисуется только то, что видно (прокрутка колесом).
    let mut ry = y;
    let heading_at = |ui: &mut Ui, ry: &mut i32, s: &str| {
        if ui.vis(*ry, 10) {
            heading(ui, x, w, s, *ry);
        }
        *ry += 14;
    };

    // ---------- Микрофон ----------
    heading_at(ui, &mut ry, "Микрофон");

    // устройство ввода (выпадающий список)
    if ui.vis(ry, 20) {
        let (cx, cw) = dev_row(ui, x, ry, w, "Устройство ввода");
        let dev = S::mic_device();
        let (label, color) = if S::mic_off() {
            ("Отключено".to_string(), DIM)
        } else if S::mic_ok() || !S::connected() {
            (device_label(&dev, S::mic_list(), DEFAULT_MIC), TEXT)
        } else {
            ("Не доступен".to_string(), ERR)
        };
        if device_dropdown(ui, cx, ry, cw, &label, color, "Выбрать микрофон") {
            S::request_mic_refresh();
            S::set_mic_scroll(0);
            S::set_mic_picker(true);
        }
        if ui.reset_button(rx, ry, !dev.is_empty(), "Системный микрофон") {
            S::set_mic_device("");
            ui.changed = true;
        }
    }
    ry += DEV_ROW;

    if ui.vis(ry, 20) {
        let v = onoff_row(
            ui,
            x,
            ry,
            w,
            "Отключить устройство",
            "Микрофон закрывается совсем: система не видит, что он занят. Слушать других можно",
            S::mic_off(),
            false,
            true,
        );
        S::set_mic_off(v);
    }
    ry += DEV_ROW;

    // громкость микрофона
    if ui.vis(ry, 20) {
        let (cx, cw) = dev_row(ui, x, ry, w, "Громкость микрофона");
        let pct = S::mic_gain_pct();
        if let Some(v) = ui.mc_slider(1, cx, ry, cw, 20, pct as f32 / 200.0, &format!("{pct}%"), slider_color(pct)) {
            S::set_mic_gain_pct((v * 200.0).round() as u32);
        }
        if ui.reset_button(rx, ry, pct != 100, "Сбросить (100%)") {
            S::set_mic_gain_pct(100);
            ui.changed = true;
        }
    }
    ry += DEV_ROW;

    // порог активации: полоса показывает живой уровень микрофона, ручка — порог
    if ui.vis(ry, 20) {
        let (cx, cw) = dev_row(ui, x, ry, w, "Порог активации");
        let thr = S::threshold();
        let thr_db = to_db(thr);
        let level = S::level();
        let lvl_color = if S::mic_silenced() || S::mic_off() {
            0xFF70_7070
        } else if level >= thr {
            GREEN
        } else {
            0xFF2F_7A40
        };
        if let Some(v) = ui.slider(2, cx, ry, cw, 20, db_frac(thr_db), (db_frac(to_db(level)), lvl_color)) {
            S::set_threshold(frac_to_threshold(v));
        }
        let t = format!("{} дБ", thr_db.round() as i32);
        ui.text(&t, cx + (cw - text_width(&t)) / 2, ry + 6, TEXT);
        ui.tooltip(cx, ry, cw, 20, "Ручка — порог: полоса должна выходить за неё, когда вы говорите");
        if ui.reset_button(rx, ry, (thr - S::DEFAULT_THRESHOLD).abs() > 1e-6, "Сбросить порог") {
            S::set_threshold(S::DEFAULT_THRESHOLD);
            ui.changed = true;
        }
    }
    ry += DEV_ROW;

    // шумоподавление
    if ui.vis(ry, 20) {
        let (cx, cw) = dev_row(ui, x, ry, w, "Шумоподавление");
        let np = S::noise_pct();
        let t = if np == 0 { "Выкл".to_string() } else { format!("{np}%") };
        if let Some(v) = ui.mc_slider(4, cx, ry, cw, 20, np as f32 / 100.0, &t, TEXT) {
            S::set_noise_pct((v * 100.0).round() as u32);
        }
        ui.tooltip(x, ry, cx - x, 20, "Своё спектральное: убирает ровный шум (кулер, улица). Сильнее — чище, но голос «плоский»");
        if ui.reset_button(rx, ry, np != 0, "Выключить") {
            S::set_noise_pct(0);
            ui.changed = true;
        }
    }
    ry += DEV_ROW;

    // «Запомнить шум»: 3 секунды тишины, и шумодав точно знает ваш кулер
    if ui.vis(ry, 20) {
        let (cx, cw) = dev_row(ui, x, ry, w, "Запомнить шум");
        ui.tooltip(x, ry, cx - x, 20, "Помолчите 3 секунды: шумодав запомнит ваш фон и не будет глушить долгую речь");
        let learned = S::noise_profile().is_some();
        match S::learn_progress() {
            Some(p) => {
                // идёт запись: полоса прогресса, просим молчать
                ui.frame(cx, ry, cw, 20, RUST, P_DEEP);
                let fw = ((cw - 2) as f32 * p) as i32;
                if fw > 0 {
                    ui.fill(cx + 1, ry + 1, fw, 18, RUST_DK);
                }
                let left = ((1.0 - p) * crate::denoise::LEARN_SECS).max(0.0);
                let t = format!("Молчите… {left:.1} с");
                ui.text(&t, cx + (cw - text_width(&t)) / 2, ry + 6, TEXT);
            }
            None => {
                let label = if learned { "Запомнен ✓ (ещё раз)" } else { "Записать 3 с тишины" };
                let can = S::connected() && S::mic_ok();
                let hot = if can { ui.hot(18, cx, ry, cw, 20) } else { 0.0 };
                ui.vbtn(cx, ry, cw, 20, BTN, BTN_HOVER, hot, if can { 1.0 } else { 0.5 });
                ui.text(&fit(label, cw - 8), cx + (cw - text_width(&fit(label, cw - 8))) / 2, ry + 6, if learned { OK } else { TEXT });
                if !can {
                    ui.tooltip(cx, ry, cw, 20, "Нужно подключение к голосу и микрофон");
                } else if ui.clicked(cx, ry, cw, 20) {
                    S::request_noise_learn();
                }
            }
        }
        if ui.reset_button(rx, ry, learned && S::learn_progress().is_none(), "Забыть шум") {
            S::set_noise_profile(None);
            S::save_noise_profile();
        }
    }
    ry += DEV_ROW;

    if ui.vis(ry, 20) {
        let v = onoff_row(ui, x, ry, w, "Фильтр гула", "Срезает всё ниже 80 Гц: гул, вентилятор, стук по столу. Голос не трогает", S::hum_filter(), true, true);
        S::set_hum_filter(v);
    }
    ry += DEV_ROW;

    if ui.vis(ry, 20) {
        let v = onoff_row(
            ui,
            x,
            ry,
            w,
            "Автогромкость",
            "Тихий голос подтягивается сам, громкий смех не хрипит. Подстраивается только пока вы говорите",
            S::agc_on(),
            false,
            true,
        );
        S::set_agc_on(v);
    }
    ry += DEV_ROW + 6;

    // ---------- Устройство вывода ----------
    heading_at(ui, &mut ry, "Устройство вывода");

    if ui.vis(ry, 20) {
        let (cx, cw) = dev_row(ui, x, ry, w, "Устройство вывода");
        let dev = S::out_device();
        let (label, color) = if S::out_ok() || !S::connected() {
            (device_label(&dev, S::out_list(), DEFAULT_OUT), TEXT)
        } else {
            ("Не доступно".to_string(), ERR)
        };
        if device_dropdown(ui, cx, ry, cw, &label, color, "Выбрать наушники или колонки") {
            S::request_mic_refresh();
            S::set_mic_scroll(0);
            S::set_out_picker(true);
        }
        if ui.reset_button(rx, ry, !dev.is_empty(), "Системный вывод") {
            S::set_out_device("");
            ui.changed = true;
        }
    }
    ry += DEV_ROW;

    if ui.vis(ry, 20) {
        let (cx, cw) = dev_row(ui, x, ry, w, "Громкость голосового чата");
        let vp = S::voice_volume_pct();
        if let Some(v) = ui.mc_slider(3, cx, ry, cw, 20, vp as f32 / 200.0, &format!("{vp}%"), slider_color(vp)) {
            S::set_voice_volume_pct((v * 200.0).round() as u32);
        }
        if ui.reset_button(rx, ry, vp != 100, "Сбросить (100%)") {
            S::set_voice_volume_pct(100);
            ui.changed = true;
        }
    }
    ry += DEV_ROW;

    if ui.vis(ry, 20) {
        let v = onoff_row(
            ui,
            x,
            ry,
            w,
            "Заглушение через блоки",
            "За стеной голос не только тише, но и глуше, как в жизни. Стекло звук пропускает",
            S::occlusion_on(),
            true,
            true,
        );
        S::set_occlusion_on(v);
    }
    ry += DEV_ROW;

    if ui.vis(ry, 20) {
        let v = onoff_row(
            ui,
            x,
            ry,
            w,
            "Звук огибает углы",
            "Голос из-за стены идёт в обход: через двери, окна и проёмы, и слышно его со стороны проёма",
            S::routes_on(),
            true,
            S::occlusion_on(),
        );
        S::set_routes_on(v);
    }
    ry += DEV_ROW;

    let stereo = S::out_stereo() || !S::connected();
    if ui.vis(ry, 20) {
        let tip = if stereo { "Голос слышно с той стороны, где стоит игрок" } else { "Устройство вывода моно: направление не слышно" };
        let v = onoff_row(ui, x, ry, w, "Направленные источники", tip, S::directional_on(), true, stereo);
        S::set_directional_on(v);
    }
    ry += DEV_ROW;

    if ui.vis(ry, 20) {
        let ok = stereo && S::directional_on();
        let v = onoff_row(
            ui,
            x,
            ry,
            w,
            "HRTF",
            "Объём в наушниках: задержка между ушами и «тень головы», сзади голос глуше, чем спереди",
            S::hrtf_on(),
            true,
            ok,
        );
        S::set_hrtf_on(v);
    }
    ry += DEV_ROW + 4;

    // состояние и проверка звука
    if ui.vis(ry, 10) {
        let state = if S::voice_off() {
            ("Голосовой чат выключен", ERR)
        } else if S::mic_off() {
            ("Микрофон отключён", ERR)
        } else if S::mic_muted() {
            ("Микрофон выключен", ERR)
        } else if S::speaking() {
            ("Вас слышно", OK)
        } else {
            ("Тишина", DIM)
        };
        ui.text(state.0, x, ry, state.1);
    }
    ry += 12;
    if ui.vis(ry, 18) {
        let testing = S::mic_test();
        let label = if testing { "Проверка звука: ВКЛ" } else { "Проверка звука" };
        let bg = if testing { 0xFF2F_7A40 } else { ui.shade(x, ry, 150, 18, BTN, BTN_HOVER) };
        ui.vbox(x, ry, 150, 18, if testing { GREEN } else { BLACK }, bg);
        ui.text(label, x + (150 - text_width(label)) / 2, ry + 5, TEXT);
        if ui.clicked(x, ry, 150, 18) {
            S::set_mic_test(!testing);
        }
        let hint = if !S::connected() {
            ("Нужно подключение к голосу", ERR)
        } else if !S::mic_ok() {
            ("Микрофон не доступен", ERR)
        } else if testing && S::push_to_talk() {
            ("Держите клавишу рации", DIM)
        } else if testing {
            ("Говорите: вы слышите себя", OK)
        } else {
            ("Лучше в наушниках", DIM)
        };
        ui.text(&fit(hint.0, w - 156), x + 156, ry + 5, hint.1);
    }
    ry += 22;

    // сколько можно прокрутить: низ содержимого минус низ видимой полосы (y уже сдвинут на прокрутку)
    let content = ry - y;
    let view = ui.clip.1 - ui.clip.0;
    let max = (content - view + 4).max(0);
    lock().dev_max = max as f32;
    if max > 0 {
        // тонкая полоса прокрутки справа от колонки
        let track_x = x + w + 4;
        let (t0, t1) = (ui.clip.0, ui.clip.1);
        let th = t1 - t0;
        let bar = (th * view / content.max(1)).max(12);
        let shown = lock().dev_shown;
        let by = t0 + ((th - bar) as f32 * (shown / max as f32).clamp(0.0, 1.0)).round() as i32;
        ui.fill(track_x, t0, 2, th, 0x30FF_FFFF);
        ui.fill(track_x, by, 2, bar, mul_alpha(RUST, 0.8));
    }
}

/// Сколько игроков сейчас слышно в обход стен: пишет lib.rs (меню не зависит от API игры), читает «Продвинутые».
pub static ROUTED_COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Номера ползунков: 1..=4 заняты страницами «Устройства» и «Продвинутые», 10 и 11 — источники,
/// игроки идут с этого номера плюс номер строки.
const PLAYER_SLIDER_BASE: u32 = 100;
const SRC_LOCAL: u32 = 10;
const SRC_GROUP: u32 = 11;
/// Размеры строки игрока, как в Plasmo: голова 24×24, ползунок 100×20 и две кнопки 20×20.
const FACE: i32 = 24;
const ROW_H: i32 = 30;
const CTL_W: i32 = 100 + 4 + 20 + 4 + 20;
const KEY_LOCAL: &str = "\0local";
const KEY_GROUP: &str = "\0group";

fn pct_label(pct: u32) -> String {
    format!("{pct}%")
}

fn slider_color(pct: u32) -> u32 {
    if pct > 100 { OVER } else { TEXT }
}

/// Кнопка с динамиком: «выключить» запоминает громкость, повторное нажатие её возвращает.
/// Возвращает новую громкость, если на кнопку нажали.
fn toggle_mute(key: &str, pct: u32) -> Option<u32> {
    let mut st = lock();
    if pct > 0 {
        st.muted_prev.retain(|(k, _)| k != key);
        st.muted_prev.push((key.to_string(), pct));
        Some(0)
    } else {
        let prev = st.muted_prev.iter().position(|(k, _)| k == key).map(|i| st.muted_prev.remove(i).1);
        Some(prev.filter(|&p| p > 0).unwrap_or(100))
    }
}

/// Строка «Локальный» / «Группы»: значок, название, ползунок, сброс (только если громкость не 100%) и динамик у правого края.
fn source_row(ui: &mut Ui, id: u32, x: i32, y: i32, w: i32, icon: &Icon, label: &str, key: &str, pct: u32) -> Option<u32> {
    let mut out = None;
    ui.row_hover(x - 3, y - 1, w + 6, 22);
    ui.icon(icon, x + 2, y + 2, 1);
    ui.text(label, x + 22, y + 6, TEXT);
    let sx = x + w - CTL_W;
    if let Some(v) = ui.mc_slider(id, sx, y, 100, 20, pct as f32 / 200.0, &pct_label(pct), slider_color(pct)) {
        let mut np = (v * 200.0).round() as u32;
        if (95..=105).contains(&np) {
            np = 100;
        }
        out = Some(np);
    }
    let (mi, tip) = if pct == 0 { (&icons::SPK_MENU_OFF, "Включить звук") } else { (&icons::SPK_MENU, "Выключить звук") };
    if ui.icon_button(sx + 128, y, 20, mi, Anim::Spk, pct == 0, tip) {
        out = toggle_mute(key, pct);
    }
    if ui.reset_button(sx + 104, y, pct != 100, "Сбросить громкость") {
        out = Some(100);
    }
    out
}

/// Заголовок раздела: просто текст по центру, без линий и украшений (как в Plasmo).
fn heading(ui: &mut Ui, x: i32, w: i32, s: &str, ty: i32) {
    centered(ui, x, w, s, ty, 0xE0E0E0);
}

/// Строка по центру столбца.
fn centered(ui: &mut Ui, x: i32, w: i32, s: &str, ty: i32, color: u32) {
    ui.text(s, x + (w - text_width(s)) / 2, ty, color);
}

fn page_players(ui: &mut Ui, x: i32, y: i32, w: i32) {
    heading(ui, x, w, "Громкость источников", y - 1);
    let local = S::voice_volume_pct();
    if let Some(np) = source_row(ui, SRC_LOCAL, x, y + 18, w, &icons::SPK, "Локальный", KEY_LOCAL, local) {
        S::set_voice_volume_pct(np);
        ui.changed = true;
    }
    let group = S::group_volume_pct();
    if let Some(np) = source_row(ui, SRC_GROUP, x, y + 42, w, &icons::SPK_GROUP, "Группы", KEY_GROUP, group) {
        S::set_group_volume_pct(np);
        ui.changed = true;
    }

    heading(ui, x, w, "Громкость игроков", y + 71);

    // Поле поиска: тёмное, с лупой, подсказкой и мигающим золотым курсором, пока в фокусе.
    let (fy, fh) = (y + 88, 20);
    let (search, focus) = {
        let st = lock();
        (st.search.clone(), st.search_focus)
    };
    let focus_t = ui.ease(akey(11, x, fy), 0.0, if focus { 1.0 } else { 0.0 }, 14.0);
    ui.frame(x, fy, w, fh, lerp_argb(OUTLINE, RUST, focus_t), P_DEEP);
    ui.icon(&LENS, x + 5, fy + 6, 1);
    let sx = x + 17;
    if search.is_empty() {
        ui.text("Поиск игрока…", sx + 3, fy + 6, 0x6E6E6E);
    } else {
        ui.text(&search, sx, fy + 6, TEXT);
    }
    if focus && ui.blink {
        let cx = sx + if search.is_empty() { 0 } else { text_width(&search) + 1 };
        ui.fill(cx, fy + 14, 5, 1, WHITE);
    }
    if ui.clicked(x, fy, w, fh) {
        lock().search_focus = true;
    } else if ui.pressed && !ui.hover(x, fy, w, fh) {
        // клик мимо поля: фокус снимаем (сам клик достанется тому, что под курсором)
        lock().search_focus = false;
    }

    let needle = search.to_lowercase();
    let list: Vec<roster::MenuPlayer> =
        roster::menu_players().into_iter().filter(|p| needle.is_empty() || p.nick.to_lowercase().contains(&needle)).collect();
    let top = y + 116;
    if list.is_empty() {
        let msg = if !S::connected() {
            "Нет подключения к голосу."
        } else if needle.is_empty() {
            "Пока никого: игроки с голосом появятся здесь."
        } else {
            "Игроков с таким ником нет."
        };
        centered(ui, x, w, msg, top + 8, DIM);
        return;
    }

    // сколько строк помещается; остальное — колесом мыши
    let rows = (((ui.gh - top) / ROW_H).max(1) as usize).min(8);
    let scroll = S::players_scroll().min(list.len().saturating_sub(rows));
    S::set_players_scroll(scroll); // игроки вышли — список стал короче, прокрутку подтягиваем
    for (i, p) in list.iter().enumerate().skip(scroll).take(rows) {
        let row = i - scroll;
        let ry = top + row as i32 * ROW_H;
        let pct = S::player_volume_pct(&p.nick);

        // карточка строки (у говорящего зелёная кромка) и рамка головы, как слот предмета
        ui.row_hover(x - 3, ry - 2, w + 6, 28);
        let talk_t = ui.ease(akey(12, x, ry), 0.0, if p.talking { 1.0 } else { 0.0 }, 12.0);
        if talk_t > 0.0 {
            ui.fill(x - 1, ry - 1, FACE + 2, FACE + 2, mul_alpha(LED, talk_t));
        }
        match faces::skin(&p.nick) {
            Some(skin) => ui.face(&skin, x, ry, FACE),
            None => {
                ui.fill(x, ry, FACE, FACE, 0xFF3A_3A3A);
                let initial: String = p.nick.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
                ui.text(&initial, x + (FACE - text_width(&initial)) / 2, ry + 8, TEXT);
            }
        }
        let mic_tip = if p.mic_off { "Микрофон выключен" } else if p.talking { "Говорит" } else { "Микрофон включён" };
        ui.tooltip(x, ry, FACE, FACE, &format!("{}: {mic_tip}", p.nick));

        let name_color = lerp_argb(if p.mic_off { DIM } else { TEXT }, OK, talk_t);
        // выключенный микрофон: иконка «микрофон с красной чертой» справа от ника
        let name_w = w - CTL_W - FACE - 10 - 38 - if p.mic_off { 18 } else { 0 };
        let name = fit(&p.nick, name_w);
        ui.text(&name, x + FACE + 4, ry + 8, name_color);
        if p.mic_off {
            ui.icon(&icons::MIC_MENU_OFF, x + FACE + 4 + text_width(&name) + 2, ry + 4, 1);
        }
        if p.talking {
            // три прыгающие полоски рядом с ником, как в списке говорящих
            let bx = x + FACE + 4 + text_width(&name) + 6;
            let phase = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| (d.as_millis() / 110 % 1000) as i32)
                .unwrap_or(0);
            for k in 0..3 {
                let hgt = 3 + (phase * (k + 3) + k * 5 + row as i32 * 2) % 7;
                ui.fill(bx + k * 3, ry + 16 - hgt, 2, hgt, LED);
            }
        }

        // ползунок 0..200%, у середины «липкость»: 100% легко поймать мышью
        let sx = x + w - CTL_W;
        let sy = ry + 2;
        let id = PLAYER_SLIDER_BASE + row as u32;
        if let Some(v) = ui.mc_slider(id, sx, sy, 100, 20, pct as f32 / 200.0, &pct_label(pct), slider_color(pct)) {
            let mut np = (v * 200.0).round() as u32;
            if (95..=105).contains(&np) {
                np = 100;
            }
            S::set_player_volume_pct(&p.nick, np);
        }
        let (mi, tip) = if pct == 0 { (&icons::SPK_MENU_OFF, "Включить звук") } else { (&icons::SPK_MENU, "Выключить звук") };
        if ui.icon_button(sx + 128, sy, 20, mi, Anim::Spk, pct == 0, tip) {
            if let Some(np) = toggle_mute(&p.nick.to_lowercase(), pct) {
                S::set_player_volume_pct(&p.nick, np);
                ui.changed = true;
            }
        }
        if ui.reset_button(sx + 104, sy, pct != 100, "Сбросить громкость") {
            S::set_player_volume_pct(&p.nick, 100);
            ui.changed = true;
        }
    }

    list_scrollbar(ui, x + w + 4, top - 2, rows.min(list.len()) as i32 * ROW_H - 4, scroll, rows, list.len());
}

/// Подпись активации: известные по-русски, остальные — имя с большой буквы.
fn activation_title(a: &crate::plasmo::Activation) -> String {
    match a.name.as_str() {
        "proximity" => "Локальный".to_string(),
        "groups" => "Группы".to_string(),
        "whisper" => "Шёпот".to_string(),
        "priority" => "Приоритет".to_string(),
        other => {
            let mut c = other.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
    }
}

fn kind_label(k: S::ActKind) -> &'static str {
    match k {
        S::ActKind::Voice => "Голос",
        S::ActKind::Ptt => "По клавише",
        S::ActKind::Inherit => "Как «Локальный»",
    }
}

/// Следующий тип по щелчку: «Голос» → «По клавише» → «Как «Локальный»» (только не у самого «Локального»).
fn next_kind(k: S::ActKind, proximity: bool) -> S::ActKind {
    match k {
        S::ActKind::Voice => S::ActKind::Ptt,
        S::ActKind::Ptt if !proximity => S::ActKind::Inherit,
        _ => S::ActKind::Voice,
    }
}

/// Вкладка «Активации», как у Plasmo: для каждой активации сервера тип, выключатель, клавиша рации
/// и дистанция. Прокручивается колесом, если активаций много.
fn page_activation(ui: &mut Ui, x: i32, y: i32, w: i32) {
    let rx = x + w - 20;
    let mut acts = S::server_activations();
    let connected = !acts.is_empty();
    if !connected {
        // ещё нет списка с сервера: показываем «Локальный», чтобы можно было выбрать голос или рацию заранее
        acts.push(crate::plasmo::Activation {
            name: S::PROXIMITY.to_string(),
            translation: String::new(),
            id: crate::plasmo::activation_id(S::PROXIMITY),
            distances: Vec::new(),
            default_distance: 0,
            proximity: true,
            transitive: true,
            stereo: false,
            weight: 0,
        });
    }
    // «Локальный» первым (как у Plasmo), дальше по весу
    acts.sort_by_key(|a| a.name != S::PROXIMITY);
    let mut ry = y;
    for (n, a) in acts.iter().enumerate() {
        let title = activation_title(a);
        let mut pref = S::act_pref(&a.name);
        let before = pref;
        if ui.vis(ry, 10) {
            heading(ui, x, w, &title, ry);
        }
        ry += 14;

        // тип активации + выключатель (микрофон) + сброс
        if ui.vis(ry, 20) {
            let (cx, cw) = dev_row(ui, x, ry, w, "Тип активации");
            let dw = cw - 24;
            if device_dropdown(ui, cx, ry, dw, kind_label(pref.kind), if pref.off { DIM } else { TEXT }, "Щёлкните, чтобы сменить") {
                pref.kind = next_kind(pref.kind, a.name == S::PROXIMITY);
                ui.changed = true;
            }
            let (icon, tip) =
                if pref.off { (&icons::MIC_MENU_OFF, "Активация выключена: включить") } else { (&icons::MIC_MENU, "Выключить эту активацию") };
            let live = S::speaking() && !pref.off;
            if ui.icon_button(cx + cw - 20, ry, 20, icon, Anim::Mic { live }, pref.off, tip) {
                pref.off = !pref.off;
                ui.changed = true;
            }
            let def = S::default_pref(&a.name);
            if ui.reset_button(rx, ry, pref.kind != def.kind || pref.off, "Сбросить тип") {
                pref.kind = def.kind;
                pref.off = false;
                ui.changed = true;
            }
        }
        ry += DEV_ROW;

        // клавиша рации: щёлкните по полю и нажмите клавишу (или выберите в Options > Mods)
        if ui.vis(ry, 20) {
            let slot = S::ptt_slot(&a.name).or((!connected).then_some(0));
            let tip = if pref.kind == S::ActKind::Ptt {
                "Щёлкните и нажмите клавишу: говорите, пока она зажата"
            } else {
                "Щёлкните и нажмите клавишу (работает в режиме «По клавише»)"
            };
            match slot {
                Some(k) => slot_key_row(ui, x, ry, w, "Клавиша рации", k, tip),
                None => {
                    let (cx, cw) = dev_row(ui, x, ry, w, "Клавиша рации");
                    ui.frame(cx, ry, cw, 20, BORDER, FIELD);
                    ui.text("свободных клавиш нет", cx + 6, ry + 6, DIM);
                }
            }
        }
        ry += DEV_ROW;

        // дистанция: только из того, что разрешает сервер
        let choices = a.distance_choices();
        if !choices.is_empty() {
            if ui.vis(ry, 20) {
                let (cx, cw) = dev_row(ui, x, ry, w, &format!("Дистанция «{title}»"));
                let cur = S::act_distance(a);
                let idx = choices.iter().position(|d| *d == cur).unwrap_or(0);
                let frac = if choices.len() > 1 { idx as f32 / (choices.len() - 1) as f32 } else { 0.0 };
                let id = 200 + n as u32;
                if let Some(v) = ui.mc_slider(id, cx, ry, cw, 20, frac, &format!("{cur}"), TEXT) {
                    let i = (v * (choices.len() - 1) as f32).round() as usize;
                    pref.distance = Some(choices[i.min(choices.len() - 1)]);
                }
                ui.tooltip(x, ry, cx - x, 20, "Дальше — вас слышно дальше. Варианты задаёт сервер");
                if ui.reset_button(rx, ry, cur != a.default_distance, "Сбросить дистанцию") {
                    pref.distance = None;
                    ui.changed = true;
                }
            }
            ry += DEV_ROW;
        }
        if pref != before {
            S::set_act_pref(&a.name, pref);
        }
        ry += 6;
    }

    if ui.vis(ry, 10) {
        let note = if connected { "Активации и дистанции задаёт сервер." } else { "Остальные активации появятся после подключения к голосу." };
        ui.text(note, x, ry, DIM);
    }
    ry += 14;

    page_scroll(ui, x, y, w, ry);
}

const CORNER_NAMES: [&str; 4] = ["Слева сверху", "Справа сверху", "Слева снизу", "Справа снизу"];

fn page_overlay(ui: &mut Ui, x: i32, y: i32, w: i32) {
    let on = ui.toggle(x, y, w, "Включить оверлей", S::overlay_on());
    S::set_overlay_on(on);
    let split = ui.toggle(x, y + 24, w, "Группа отдельным списком", S::group_split_on());
    S::set_group_split_on(split);
    if !split {
        S::set_placing_group(false);
    }

    ui.text("Где список говорящих", x, y + 54, TEXT);
    let mut my0 = y + 66;
    if split {
        // какой список сейчас переставляем: обычный или группы
        for (i, (grp, label)) in [(false, "Обычные"), (true, "Группа")].into_iter().enumerate() {
            let (bx, bw) = (x + i as i32 * 62, 60);
            let sel = S::placing_group() == grp;
            let bg = if sel { BTN_SEL } else { ui.shade(bx, my0, bw, 16, 0xFF24_2424, BTN_HOVER) };
            ui.vbox(bx, my0, bw, 16, if sel { ACCENT } else { BLACK }, bg);
            let color = if grp { TALK_GROUP & 0xFF_FFFF } else { TEXT };
            ui.text(label, bx + (bw - text_width(label)) / 2, my0 + 4, color);
            if ui.clicked(bx, my0, bw, 16) {
                S::set_placing_group(grp);
                ui.changed = true;
            }
        }
        my0 += 20;
    }
    let (mx0, mw, mh) = (x, 124, 70);
    ui.frame(mx0, my0, mw, mh, BORDER, P_DEEP);
    let zones = [
        (mx0 + 3, my0 + 3),
        (mx0 + mw - 3 - 36, my0 + 3),
        (mx0 + 3, my0 + mh - 3 - 24),
        (mx0 + mw - 3 - 36, my0 + mh - 3 - 24),
    ];
    let near = S::overlay_corner() as usize;
    let group = S::group_corner() as usize;
    let placing = if S::placing_group() { group } else { near };
    for (i, &(zx, zy)) in zones.iter().enumerate() {
        let has_near = i == near;
        let has_group = split && i == group;
        let bg = if has_near || has_group { BTN_SEL } else { ui.shade(zx, zy, 36, 24, 0xFF24_2424, BTN_HOVER) };
        ui.vbox(zx, zy, 36, 24, if i == placing { ACCENT } else { BLACK }, bg);
        match (has_near, has_group) {
            (true, true) => {
                ui.icon(&icons::MIC, zx + 2, zy + 4, 1);
                ui.icon(&icons::MIC_GROUP, zx + 18, zy + 4, 1);
            }
            (true, false) => ui.icon(&icons::MIC, zx + 10, zy + 4, 1),
            (false, true) => ui.icon(&icons::MIC_GROUP, zx + 10, zy + 4, 1),
            _ => {}
        }
        ui.tooltip(zx, zy, 36, 24, CORNER_NAMES[i]);
        if ui.clicked(zx, zy, 36, 24) {
            if S::placing_group() {
                S::set_group_corner(i as u8);
            } else {
                S::set_overlay_corner(i as u8);
            }
            ui.changed = true;
        }
    }
    let rx = x + mw + 12;
    if split {
        ui.text("Обычные:", rx, my0 + 2, DIM);
        ui.text(CORNER_NAMES[near], rx, my0 + 13, TEXT);
        ui.text("Группа:", rx, my0 + 28, DIM);
        ui.text(CORNER_NAMES[group], rx, my0 + 39, TALK_GROUP & 0xFF_FFFF);
        ui.text("Выберите список и щёлкните угол.", rx, my0 + 56, DIM);
    } else {
        ui.text("Сейчас:", rx, my0 + 8, DIM);
        ui.text(CORNER_NAMES[near], rx, my0 + 20, TEXT);
    }

    let mut ry = my0 + mh + 10;
    ui.text("Значок у хотбара, пока вы говорите:", x, ry, DIM);
    ry += 12;
    ui.frame(x, ry, 22, 22, BORDER, 0x5000_0000);
    ui.icon(&icons::MIC, x + 3, ry + 3, 1);
    ui.tooltip(x, ry, 22, 22, "Говорите рядом (локальный голос)");
    ui.text("рядом", x + 26, ry + 7, DIM);
    let gx = x + 26 + text_width("рядом") + 12;
    ui.frame(gx, ry, 22, 22, BORDER, 0x5000_0000);
    ui.icon(&icons::MIC_GROUP, gx + 3, ry + 3, 1);
    ui.tooltip(gx, ry, 22, 22, "Говорите в группу");
    ui.text("в группу", gx + 26, ry + 7, DIM);
    ry += 26;
    ui.frame(x, ry, 22, 22, BORDER, 0x5000_0000);
    ui.icon(&icons::MIC_MUTED, x + 3, ry + 3, 1);
    ui.tooltip(x, ry, 22, 22, "Микрофон выключен");
    ui.text("микрофон выкл", x + 26, ry + 7, DIM);
    let hx = x + 26 + text_width("микрофон выкл") + 12;
    ui.frame(hx, ry, 22, 22, BORDER, 0x5000_0000);
    ui.icon(&icons::HEADPHONES_OFF, hx + 3, ry + 3, 1);
    ui.tooltip(hx, ry, 22, 22, "Голосовой чат выключен: вы никого не слышите, и микрофон тоже молчит");
    ui.text("звук выкл", hx + 26, ry + 7, DIM);
    ry += 30;

    let tags = ui.toggle(x, ry, w, "Значок статуса над ником игрока", S::nametags_on());
    S::set_nametags_on(tags);
    ry += 26;
    ui.text("Кто говорит в вашей группе, отмечен фиолетовой рамкой.", x, ry, DIM);
    ry += 14;
    page_scroll(ui, x, y, w, ry);
}

/// Поле клавиши, как у Plasmo: щёлкнули — «Нажмите клавишу…», следующая клавиша назначается. `label` — что
/// назначено сейчас (пусто — ничего), `held` — показать точку «зажата». Возвращает true, если назначение сброшено.
#[allow(clippy::too_many_arguments)]
fn key_row(ui: &mut Ui, x: i32, ry: i32, w: i32, title: &str, bind: Bind, label: &str, held: Option<bool>, tip: &str, can_reset: bool) -> bool {
    let rx = x + w - 20;
    let (cx, cw) = dev_row(ui, x, ry, w, title);
    let waiting = lock().binding == Some(bind);
    let hot = ui.hot(19, cx, ry, cw, 20);
    let border = if waiting { RUST } else { lerp_argb(BORDER, 0xFFD0_D0D0, hot) };
    ui.frame(cx, ry, cw, 20, border, if waiting { 0xFF20_1830 } else { FIELD });
    let (text, color) = if waiting {
        ((if ui.blink { "Нажмите клавишу…" } else { "Нажмите клавишу" }).to_string(), TEXT)
    } else if label.is_empty() {
        ("Нет — щёлкните".to_string(), DIM)
    } else {
        (label.to_string(), ACCENT_TEXT)
    };
    let room = if held.is_some() { cw - 24 } else { cw - 12 };
    let t = fit(&text, room);
    ui.text(&t, cx + (cw - text_width(&t)) / 2, ry + 6, color);
    if let Some(h) = held {
        ui.text(if h { "●" } else { "○" }, cx + cw - 12, ry + 6, if h { GREEN } else { DIM });
    }
    ui.tooltip(cx, ry, cw, 20, if waiting { "Esc — отмена, Backspace — убрать клавишу" } else { tip });
    if ui.clicked(cx, ry, cw, 20) {
        lock().binding = if waiting { None } else { Some(bind) };
    }
    let reset = ui.reset_button(rx, ry, can_reset && !waiting, "Сбросить клавишу");
    if reset {
        ui.changed = true;
    }
    reset
}

/// Клавиша рации слота (с точкой «зажата» и сбросом к Options > Mods).
fn slot_key_row(ui: &mut Ui, x: i32, ry: i32, w: i32, title: &str, slot: usize, tip: &str) {
    let label = S::slot_key_label(slot);
    if key_row(ui, x, ry, w, title, Bind::Slot(slot), &label, Some(S::slot_held(slot)), tip, S::slot_binding(slot).is_some()) {
        S::set_slot_binding(slot, None);
    }
}

/// Строка «подпись — значение» в рамке поля (только показать).
fn info_row(ui: &mut Ui, x: i32, ry: i32, w: i32, title: &str, value: &str, color: u32, tip: &str) {
    let (cx, cw) = dev_row(ui, x, ry, w, title);
    ui.frame(cx, ry, cw, 20, BORDER, 0xFF1A_1A1A);
    let t = fit(value, cw - 12);
    ui.text(&t, cx + (cw - text_width(&t)) / 2, ry + 6, color);
    ui.tooltip(x, ry, w - 24, 20, tip);
}

/// Прокрутка длинной страницы: сколько можно прокрутить и полоса справа (как у «Устройств»).
fn page_scroll(ui: &mut Ui, x: i32, y: i32, w: i32, bottom: i32) {
    let content = bottom - y;
    let view = ui.clip.1 - ui.clip.0;
    let max = (content - view + 4).max(0);
    lock().dev_max = max as f32;
    if max > 0 {
        let shown = lock().dev_shown;
        let (t0, th) = (ui.clip.0, ui.clip.1 - ui.clip.0);
        let bar = (th * view / content.max(1)).max(12);
        let by = t0 + ((th - bar) as f32 * (shown / max as f32).clamp(0.0, 1.0)).round() as i32;
        ui.fill(x + w + 4, t0, 2, th, 0x30FF_FFFF);
        ui.fill(x + w + 4, by, 2, bar, mul_alpha(RUST, 0.8));
    }
}

/// Вкладка «Клавиши», как у Plasmo: у каждого действия поле клавиши; щёлкните и нажмите нужную.
fn page_controls(ui: &mut Ui, x: i32, y: i32, w: i32) {
    let mut ry = y;
    let section = |ui: &mut Ui, ry: &mut i32, s: &str| {
        if ui.vis(*ry, 10) {
            heading(ui, x, w, s, *ry);
        }
        *ry += 14;
    };
    let action = |ui: &mut Ui, ry: &mut i32, name: &'static str, tip: &str| {
        if ui.vis(*ry, 20) {
            let title = S::ACTIONS.iter().find(|a| a.0 == name).map_or(name, |a| a.1);
            let label = if name == "menu" {
                S::menu_key_label()
            } else {
                S::action_binding(name).map(|k| S::ptt_label_for(&k)).unwrap_or_default()
            };
            if key_row(ui, x, *ry, w, title, Bind::Action(name), &label, None, tip, S::action_binding(name).is_some()) {
                S::set_action_binding(name, None);
            }
        }
        *ry += DEV_ROW;
    };

    section(ui, &mut ry, "Основные");
    action(ui, &mut ry, "menu", "Открыть и закрыть это меню. Без своей клавиши — из Options > Mods");
    action(ui, &mut ry, "mute", "Включить или выключить микрофон (в игре, не в чате)");
    action(ui, &mut ry, "deafen", "Включить или выключить весь голосовой чат");
    if ui.vis(ry, 20) {
        info_row(ui, x, ry, w, "Кнопка действия", "ПКМ", ACCENT_TEXT, "Наведитесь на игрока, зажмите ПКМ и крутите колесо — его громкость");
    }
    ry += DEV_ROW + 6;

    section(ui, &mut ry, "Рация");
    let acts = S::server_activations();
    for slot in 0..S::PTT_SLOTS {
        // подпись: какая активация сейчас на этом слоте
        let name = if slot == 0 {
            Some("Локальный".to_string())
        } else {
            acts.iter().filter(|a| a.name != S::PROXIMITY).nth(slot - 1).map(activation_title)
        };
        if ui.vis(ry, 20) {
            let title = match &name {
                Some(n) => format!("Рация «{n}»"),
                None => format!("Рация {}", slot + 1),
            };
            slot_key_row(ui, x, ry, w, &title, slot, "Говорите, пока зажата (если у активации тип «По клавише»)");
        }
        ry += DEV_ROW;
    }
    ry += 6;

    section(ui, &mut ry, "Затухание звука через блоки");
    action(ui, &mut ry, "occlusion", "Включить или выключить заглушение через блоки");
    ry += 6;

    section(ui, &mut ry, "Дистанция голоса «Локальный»");
    action(ui, &mut ry, "dist_up", "Следующая дистанция из разрешённых сервером");
    action(ui, &mut ry, "dist_down", "Предыдущая дистанция из разрешённых сервером");
    ry += 6;

    section(ui, &mut ry, "Команды в чате");
    for (cmd, what) in [("/voice", "открыть меню (или /vm)"), ("/voice mute", "микрофон вкл/выкл"), ("/voice deaf", "голосовой чат вкл/выкл")] {
        if ui.vis(ry, 10) {
            ui.text(cmd, x, ry, ACCENT_TEXT);
            ui.text(what, x + 84, ry, DIM);
        }
        ry += 12;
    }
    ry += 4;
    page_scroll(ui, x, y, w, ry);
}

fn yes_no(v: bool) -> (&'static str, u32) {
    if v {
        ("Да", OK)
    } else {
        ("Нет", ERR)
    }
}

/// Вкладка «Продвинутые», как у Plasmo: разделы с переключателями, состояние голоса и сброс.
fn page_status(ui: &mut Ui, x: i32, y: i32, w: i32) {
    let mut ry = y;
    let section = |ui: &mut Ui, ry: &mut i32, s: &str| {
        if ui.vis(*ry, 10) {
            heading(ui, x, w, s, *ry);
        }
        *ry += 14;
    };

    section(ui, &mut ry, "Визуальные и интерфейс");
    if ui.vis(ry, 20) {
        let v = onoff_row(ui, x, ry, w, "Значки над никами", "Микрофон или динамик над головой: говорит, выключен, без мода", S::nametags_on(), true, true);
        S::set_nametags_on(v);
    }
    ry += DEV_ROW;
    if ui.vis(ry, 20) {
        let v = onoff_row(
            ui,
            x,
            ry,
            w,
            "Громкость колесом",
            "Наведитесь на игрока, зажмите ПКМ и крутите колесо — меняется его громкость",
            S::wheel_volume_on(),
            true,
            true,
        );
        S::set_wheel_volume_on(v);
    }
    ry += DEV_ROW;
    if ui.vis(ry, 20) {
        use crate::i18n::Pref;
        let (cx, cw) = dev_row(ui, x, ry, w, "Язык");
        let cur = crate::i18n::pref();
        let label = match cur {
            Pref::Auto => format!("{} ({})", crate::i18n::tr("Авто"), crate::i18n::lang().native()),
            Pref::Fixed(l) => l.native().to_string(),
        };
        if device_dropdown(ui, cx, ry, cw, &label, TEXT, "Язык меню: авто — как в игре") {
            crate::i18n::set_pref(crate::i18n::next_pref(cur));
            ui.changed = true;
        }
    }
    ry += DEV_ROW + 6;

    section(ui, &mut ry, "Звуковой движок");
    if ui.vis(ry, 20) {
        let v = onoff_row(ui, x, ry, w, "Громкость расстояния", "Дальше игрок — тише его голос. Выключите — всех слышно одинаково", S::dist_fade_on(), true, true);
        S::set_dist_fade_on(v);
    }
    ry += DEV_ROW;
    if ui.vis(ry, 20) {
        let (out, color) = if !S::connected() {
            ("—", DIM)
        } else if S::out_stereo() {
            ("Стерео", TEXT)
        } else {
            ("Моно", OVER)
        };
        info_row(ui, x, ry, w, "Вывод звука", out, color, "Направленный звук и HRTF работают только в стерео");
    }
    ry += DEV_ROW;
    if ui.vis(ry, 20) {
        let n = ROUTED_COUNT.load(std::sync::atomic::Ordering::Relaxed);
        info_row(ui, x, ry, w, "Слышно в обход стен", &n.to_string(), TEXT, "Сколько игроков сейчас слышно через двери и проёмы");
    }
    ry += DEV_ROW + 6;

    section(ui, &mut ry, "Состояние");
    let proto = S::protocol();
    let rows: [(&str, String, u32, &str); 5] = [
        ("Подключение к голосу", yes_no(S::connected()).0.to_string(), yes_no(S::connected()).1, "Сервер принял ваш голосовой клиент"),
        (
            "Протокол",
            if proto.is_empty() { "—".to_string() } else { proto },
            TEXT,
            "Мод сам понимает, что стоит на сервере: Plasmo Voice или Simple Voice Chat",
        ),
        ("Микрофон", yes_no(S::mic_ok()).0.to_string(), yes_no(S::mic_ok()).1, "Микрофон открылся"),
        ("Дистанция «Локальный»", if S::distance() > 0 { S::distance().to_string() } else { "—".to_string() }, TEXT, "Сменить: вкладка «Активации»"),
        ("Слышно источников", S::hearing().to_string(), TEXT, "Сколько голосов играет прямо сейчас"),
    ];
    for (title, value, color, tip) in rows.iter() {
        if ui.vis(ry, 20) {
            info_row(ui, x, ry, w, title, value, *color, tip);
        }
        ry += DEV_ROW;
    }
    ry += 6;

    if ui.vis(ry, 18) {
        if ui.button(x, ry, 150, 18, "Сбросить настройки") {
            S::reset_sliders();
            S::set_push_to_talk(false);
            S::set_overlay_on(true);
            S::set_overlay_labels(true);
            S::set_overlay_corner(1);
            S::set_mic_muted(false);
            S::set_voice_off(false);
            S::set_dist_fade_on(true);
            ui.changed = true;
        }
        ui.text("Rust Voice · MIT", x + 160, ry + 5, DIM);
    }
    ry += 22;
    page_scroll(ui, x, y, w, ry);
}

// ---------- окно ----------
/// Экран как у Plasmo Voice, но со своими анимациями: шапка (значок и название, кнопки микрофона и динамика,
/// вкладки с подписями), под ней колонка настроек по центру на тёмной подложке. Если вкладки с подписями не
/// помещаются в одну строку с названием, они переезжают во вторую строку шапки; если и там тесно, подпись остаётся
/// только у выбранной вкладки. При открытии шапка опускается сверху, колонка поднимается; при смене вкладки
/// страница наезжает сбоку.
fn window(ui: &mut Ui, gw: i32, gh: i32, tab: Tab) -> Tab {
    let open = ui.open;
    const BANDS: i32 = 24;
    for i in 0..BANDS {
        let y0 = gh * i / BANDS;
        let y1 = if i == BANDS - 1 { gh + 64 } else { gh * (i + 1) / BANDS };
        let dist = ((i as f32 + 0.5) / BANDS as f32 - 0.5).abs() * 2.0;
        let a = (84.0 + dist * 48.0) / 255.0;
        ui.fill(0, y0, gw + 64, y1 - y0, mul_alpha(0xFF08_0808, a * open));
    }

    let ms = now_ms();
    let secs = ms / 1000.0;

    // ---------- раскладка шапки ----------
    const GAP: i32 = 3;
    let n = TABS.len() as i32;
    // ширина вкладки с подписью: отступ 5, иконка 8, зазор 4, подпись, отступ 5
    let full: Vec<i32> = TABS.iter().map(|t| 22 + text_width(t.title())).collect();
    let full_total = full.iter().sum::<i32>() + GAP * (n - 1);
    let title_w = text_width(TITLE);
    let ver_w = text_width(VERSION);
    // слева: значок 16, название, версия, 5 полосок звука
    let left_end = 8 + 18 + title_w + 5 + ver_w + 8 + 15 + 12;
    let spk_x = gw - 8 - 20;
    let mic_x = spk_x - 24;
    let right_start = mic_x - 12;
    let (labelled, two_rows) = if full_total <= right_start - left_end {
        (true, false)
    } else if full_total <= gw - 16 {
        (true, true)
    } else {
        (false, false)
    };
    let hh: i32 = if two_rows { 50 } else { 26 };

    // ---------- шапка ----------
    let hy = -(((1.0 - open) * hh as f32).round() as i32);
    ui.fill(0, hy, gw, hh - 1, 0xDC10_1010);
    ui.fill(0, hy + hh - 1, gw, 1, 0xFF3A_3A3A);
    ui.fill(0, hy + hh, gw, 1, 0x6000_0000);
    if two_rows {
        ui.fill(0, hy + 25, gw, 1, 0x28FF_FFFF);
    }
    // «ржавый» блик ползёт по нижней линии шапки
    let sweep = (ms % 3600.0) / 3600.0;
    let sx0 = -60 + (sweep * (gw + 120) as f32) as i32;
    let (sa, sb) = (sx0.max(0), (sx0 + 60).min(gw));
    if sb > sa {
        ui.fill(sa, hy + hh - 1, sb - sa, 1, mul_alpha(RUST, 0.75));
    }

    // значок Plasmo: чуть покачивается, раз в несколько секунд по нему пробегает блик
    let mut logo = Fx::NONE;
    logo.dy = 0.5 * (secs * 2.2).sin();
    let g = (ms % 4200.0) / 1000.0;
    if g < 1.0 {
        logo.glint = g;
    }
    ui.icon_fx(&icons::LOGO, 8, hy + 5, 2, logo);
    let tx0 = 8 + 18;
    ui.text("Rust", tx0, hy + 9, RUST_TEXT);
    ui.text("Voice", tx0 + text_width("Rust "), hy + 9, TEXT);
    ui.text(VERSION, tx0 + title_w + 5, hy + 9, 0x6E6E6E);

    // живая «волна» после названия: качается сама и бьётся в такт вашему голосу
    let lvl = if S::mic_silenced() { 0.0 } else { db_frac(to_db(S::level())) };
    let wx = tx0 + title_w + 5 + ver_w + 8;
    for i in 0..5 {
        let idle = 0.5 + 0.5 * (ms / 420.0 + i as f32 * 0.9).sin();
        let target = 2.0 + idle * 2.0 + lvl * (6.0 - (i as f32 - 2.0).abs() * 1.2);
        let hgt = ui.ease(akey(20, i, 0), 2.0, target, 24.0).round() as i32;
        let col = lerp_argb(0xFF6A_6A6A, RUST, (lvl * 2.0).min(1.0));
        ui.fill(wx + i * 3, hy + 18 - hgt, 2, hgt, col);
    }

    // микрофон и динамик справа: кнопки 20×20
    let by = hy + 3;
    let mic_off = S::mic_muted() || S::voice_off();
    let mic_icon = if mic_off { &icons::MIC_MENU_OFF } else { &icons::MIC_MENU };
    let mic_tip = if S::mic_muted() { "Микрофон: ВЫКЛ" } else { "Микрофон: ВКЛ" };
    let live = overlay_talking();
    if ui.icon_button(mic_x, by, 20, mic_icon, Anim::Mic { live }, mic_off, mic_tip) {
        S::set_mic_muted(!S::mic_muted());
        ui.changed = true;
    }
    let spk_icon = if S::voice_off() { &icons::SPK_MENU_OFF } else { &icons::SPK_MENU };
    let spk_tip = if S::voice_off() { "Голосовой чат: ВЫКЛ" } else { "Голосовой чат: ВКЛ" };
    if ui.icon_button(spk_x, by, 20, spk_icon, Anim::Spk, S::voice_off(), spk_tip) {
        S::set_voice_off(!S::voice_off());
        ui.changed = true;
    }

    // ---------- вкладки ----------
    let cur = tab;
    let mut tab = tab;
    // насколько каждая вкладка выбрана (плавно); считаем один раз за кадр
    let mut sel_ts = Vec::with_capacity(TABS.len());
    for (i, &t) in TABS.iter().enumerate() {
        let to = if t == cur { 1.0 } else { 0.0 };
        sel_ts.push(ui.ease(akey(14, i as i32, 0), to, to, 16.0));
    }
    let ws: Vec<i32> = if labelled {
        full
    } else {
        // совсем тесно: у выбранной вкладки подпись плавно «выезжает», у остальных одни иконки (название в подсказке)
        TABS.iter().enumerate().map(|(i, t)| 22 + (sel_ts[i] * (text_width(t.title()) + 2) as f32).round() as i32).collect()
    };
    let total = ws.iter().sum::<i32>() + GAP * (n - 1);
    let start = if two_rows {
        (gw / 2 - total / 2).max(8)
    } else {
        (gw / 2 - total / 2).max(left_end).min((right_start - total).max(8))
    };
    let mut pos = Vec::with_capacity(ws.len());
    let mut at = start;
    for &w in &ws {
        pos.push(at);
        at += w + GAP;
    }
    let sel_i = TABS.iter().position(|&t| t == cur).unwrap_or(0);
    let (want_x, want_w) = (pos[sel_i] as f32, ws[sel_i] as f32);
    if ui.ind_x < 0.0 {
        ui.ind_x = want_x;
        ui.ind_w = want_w;
    } else {
        let k = 1.0 - (-20.0 * ui.dt).exp();
        ui.ind_x += (want_x - ui.ind_x) * k;
        ui.ind_w += (want_w - ui.ind_w) * k;
    }
    let ty = if two_rows { hy + 28 } else { hy + 4 };
    let th = 18;
    for (i, &t) in TABS.iter().enumerate() {
        let (tx, w) = (pos[i], ws[i]);
        let sel = t == cur;
        let sel_t = sel_ts[i];
        let hot = ui.hot(13, tx, ty, w, th);
        ui.vbtn(tx, ty, w, th, lerp_argb(BTN, 0xFF2B_2B2B, sel_t), BTN_HOVER, hot, 1.0);
        // иконка: при наведении подпрыгивает и по ней бежит блик; выбранная «подскакивает» при переходе;
        // у «Громкости» по полоскам бежит волна
        let mut fx = Fx::NONE;
        fx.dy = -hot;
        if sel {
            fx.dy -= (std::f32::consts::PI * ui.tab_t).sin() * 2.0;
        }
        if hot > 0.02 && hot < 0.9 {
            fx.glint = hot;
        }
        if t == Tab::Players && (sel || hot > 0.05) {
            fx.ripple = hot.max(sel_t * 0.8);
            fx.ripple_from = 1;
            fx.phase = secs * 8.0;
        }
        let ix = if labelled { tx + 5 } else { tx + 7 - (sel_t * 2.0).round() as i32 };
        ui.icon_fx(t.icon(sel || hot > 0.5), ix, ty + 5, 1, fx);
        if labelled || sel_t > 0.5 {
            let col = lerp_argb(lerp_argb(DIM, TEXT, hot), TEXT, sel_t);
            ui.text(t.title(), tx + 17, ty + 5, col);
        } else {
            ui.tooltip(tx, ty, w, th, t.title());
        }
        if ui.clicked(tx, ty, w, th) {
            tab = t;
        }
    }
    // подчёркивание плавно переезжает на выбранную вкладку (и меняет ширину под неё)
    let (ix, iw) = (ui.ind_x.round() as i32, ui.ind_w.round() as i32);
    ui.fill(ix, hy + hh - 3, iw, 1, mul_alpha(RUST, 0.25));
    ui.fill(ix, hy + hh - 2, iw, 2, RUST);

    // ---------- колонка настроек ----------
    // по центру, на тёмной подложке; при смене вкладки наезжает со стороны, при открытии поднимается
    let cw = (gw - 40).clamp(224, 348);
    let col_x = (gw - cw) / 2;
    ui.fill(col_x - 10, hy + hh + 1, cw + 20, gh, mul_alpha(0x5800_0000, open));
    ui.fill(col_x - 10, hy + hh + 1, 1, gh, mul_alpha(0x20FF_FFFF, open));
    ui.fill(col_x + cw + 9, hy + hh + 1, 1, gh, mul_alpha(0x20FF_FFFF, open));
    let dx = (ui.tab_dir * (1.0 - ui.tab_t) * 24.0).round() as i32;
    let rise = ((1.0 - open) * 12.0).round() as i32;
    ui.gh = gh - 8;
    let (cx, cy) = (col_x + dx, hh + 12 + rise);
    ui.clip = (hh + 6, gh - 4);
    match tab {
        Tab::Devices | Tab::Activation | Tab::Status | Tab::Controls | Tab::Overlay => {
            let scroll = {
                let mut st = lock();
                st.dev_scroll = st.dev_scroll.clamp(0.0, st.dev_max);
                let k = 1.0 - (-18.0 * ui.dt).exp();
                st.dev_shown += (st.dev_scroll - st.dev_shown) * if ui.dt > 0.0 { k } else { 1.0 };
                if (st.dev_scroll - st.dev_shown).abs() < 0.5 {
                    st.dev_shown = st.dev_scroll;
                }
                st.dev_shown.round() as i32
            };
            match tab {
                Tab::Devices => page_devices(ui, cx, cy - scroll, cw),
                Tab::Activation => page_activation(ui, cx, cy - scroll, cw),
                Tab::Status => page_status(ui, cx, cy - scroll, cw),
                Tab::Overlay => page_overlay(ui, cx, cy - scroll, cw),
                _ => page_controls(ui, cx, cy - scroll, cw),
            }
        }
        Tab::Players => page_players(ui, cx, cy, cw),
    }
    tab
}

/// Нарисовать меню (вызывается каждый кадр, пока открыт экран мода). `mouse` — позиция мыши в GUI-пикселях.
pub fn draw(c: &mut dyn Canvas, mouse: (i32, i32)) {
    let now = Instant::now();
    let (tab, pressed, down, active, gw, gh, dt, anim, open_raw, tab_raw, ind_x, ind_w, tab_dir) = {
        let mut st = lock();
        st.mx = mouse.0;
        st.my = mouse.1;
        // между кадрами больше четверти секунды — меню открыли заново, анимации с нуля
        let dt = st.last_draw.map(|t| now.duration_since(t).as_secs_f32()).filter(|d| *d < 0.25).unwrap_or(0.0).min(0.05);
        st.last_draw = Some(now);
        let anim = std::mem::take(&mut st.anim);
        (st.tab, st.pressed, st.down, st.active, st.gui_w, st.gui_h, dt, anim, st.open_t, st.tab_t, st.ind_x, st.ind_w, st.tab_dir)
    };
    let open_raw = (open_raw + dt / 0.22).min(1.0);
    let tab_raw = (tab_raw + dt / 0.16).min(1.0);
    let blink = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_millis() < 500)
        .unwrap_or(true);
    let mut ui = Ui {
        c,
        mx: mouse.0,
        my: mouse.1,
        pressed,
        down,
        active,
        changed: false,
        tip: None,
        gw,
        gh,
        blink,
        dt,
        anim,
        open: ease_out(open_raw),
        tab_t: ease_out(tab_raw),
        ind_x,
        ind_w,
        tab_dir,
        clip: (0, gh),
    };
    let new_tab = window(&mut ui, gw, gh, tab);

    // Подсказка у курсора.
    if let Some(tip) = ui.tip.take() {
        let tw = text_width(&tip) + 8;
        let tx = (mouse.0 + 10).min(gw - tw - 2).max(2);
        let ty = (mouse.1 + 12).min(gh - 16).max(2);
        ui.frame(tx, ty, tw, 14, 0xFF3C_1E78, 0xF010_0010);
        ui.text(&tip, tx + 4, ty + 3, TEXT);
    }
    let (changed, active_now, anim, ind_x, ind_w) = (ui.changed, ui.active, std::mem::take(&mut ui.anim), ui.ind_x, ui.ind_w);

    let mut st = lock();
    if new_tab != tab {
        // у «Устройств» и «Активаций» общая прокрутка: на новой вкладке начинаем сверху
        st.dev_scroll = 0.0;
        st.dev_shown = 0.0;
        let idx = |x: Tab| TABS.iter().position(|&t| t == x).unwrap_or(0);
        st.tab_t = 0.0;
        st.tab_dir = if idx(new_tab) > idx(tab) { 1.0 } else { -1.0 };
    } else {
        st.tab_t = tab_raw;
        st.tab_dir = tab_dir;
    }
    st.tab = new_tab;
    st.open_t = open_raw;
    st.ind_x = ind_x;
    st.ind_w = ind_w;
    st.anim = anim;
    st.pressed = false;
    st.active = active_now;
    if changed {
        st.dirty = true;
    }
    if st.dirty && !st.down && active_now == 0 {
        st.dirty = false;
        drop(st);
        S::save();
    }
}

// ---------- оверлей на HUD ----------
/// Переливание 0..=1..=0 с периодом 1.6 с: значок и карточка того, кто говорит и рядом, и в группу.
fn shimmer() -> f32 {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| (d.as_millis() % 1_600) as f32).unwrap_or(0.0);
    let t = 0.5 - 0.5 * (ms / 1_600.0 * std::f32::consts::TAU).cos();
    // подольше держимся у краёв (зелёный и фиолетовый), переход — быстрее
    t * t * (3.0 - 2.0 * t)
}

/// Иконка посередине между двумя (`t` 0 — первая, 1 — вторая). Иконки одной формы: отрезки совпадают,
/// меняются только цвета (так нарисованы MIC и MIC_GROUP).
fn draw_icon_blend(c: &mut dyn Canvas, a: &Icon, b: &Icon, t: f32, x: i32, y: i32) {
    if t <= 0.0 || a.runs.len() != b.runs.len() {
        draw_icon(c, if t >= 0.5 { b } else { a }, x, y, 1);
        return;
    }
    for (&(rx, ry, n, ca), &(_, _, _, cb)) in a.runs.iter().zip(b.runs) {
        c.fill(x + rx as i32, y + ry as i32, n as i32, 1, lerp_argb(ca, cb, t.min(1.0)));
    }
}

/// Значок микрофона виден, только пока вы говорите: микрофон работает и голос реально уходит на сервер.
fn overlay_talking() -> bool {
    S::speaking() && !S::mic_silenced() && S::mic_ok()
}

/// Что показать: (говорю рядом, говорю в группу). Если голос ушёл, а куда — неизвестно, считаем «рядом».
fn overlay_targets() -> (bool, bool) {
    let (near, group) = (S::speaking_near(), S::speaking_group());
    if near || group { (near, group) } else { (true, false) }
}

/// Что сейчас показывает значок у хотбара.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Badge {
    /// Голосовой чат выключен: перечёркнутые наушники (микрофон при этом тоже не работает).
    Deaf,
    /// Микрофон выключен: перечёркнутый микрофон.
    Muted,
    /// Говорите: рядом и/или в группу.
    Talking { near: bool, group: bool },
}

fn overlay_badge() -> Option<Badge> {
    if !S::overlay_on() || !S::connected() {
        return None;
    }
    if S::voice_off() {
        return Some(Badge::Deaf); // наушники главнее: без них и микрофон молчит
    }
    if S::mic_muted() || S::mic_off() {
        return Some(Badge::Muted);
    }
    if overlay_talking() {
        let (near, group) = overlay_targets();
        return Some(Badge::Talking { near, group });
    }
    None
}

/// Размер оверлея, если его надо показывать сейчас: пока вы говорите, или пока микрофон / голосовой чат
/// выключены (тогда значок висит постоянно, чтобы не забыть).
pub fn overlay_size() -> Option<(i32, i32)> {
    overlay_badge().map(|_| (2 + 16 + 3, 22))
}

/// С какого момента показывается перечёркнутый значок: черта дорисовывается сверху вниз за 0.25 с.
static SLASH_SINCE: Mutex<Option<(bool, Instant)>> = Mutex::new(None);

/// Нарисовать оверлей в точке (x, y), размер взят из [`overlay_size`]. Фона нет.
pub fn draw_overlay(c: &mut dyn Canvas, x: i32, y: i32) {
    let Some(badge) = overlay_badge() else {
        *SLASH_SINCE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        return;
    };
    match badge {
        Badge::Deaf | Badge::Muted => {
            let deaf = badge == Badge::Deaf;
            let since = {
                let mut g = SLASH_SINCE.lock().unwrap_or_else(|e| e.into_inner());
                match *g {
                    Some((d, t)) if d == deaf => t,
                    _ => {
                        let now = Instant::now();
                        *g = Some((deaf, now));
                        now
                    }
                }
            };
            let reveal = (since.elapsed().as_secs_f32() / 0.25).min(1.0);
            let icon = if deaf { &icons::HEADPHONES_OFF } else { &icons::MIC_MUTED };
            draw_icon_slash(c, icon, x + 2, y + 2, reveal);
        }
        Badge::Talking { near, group } => {
            *SLASH_SINCE.lock().unwrap_or_else(|e| e.into_inner()) = None;
            let t = match (near, group) {
                (true, true) => shimmer(),
                (false, true) => 1.0,
                _ => 0.0,
            };
            draw_icon_blend(&mut *c, &icons::MIC, &icons::MIC_GROUP, t, x + 2, y + 2);
        }
    }
}

/// Иконка с красной чертой, которая проявляется сверху вниз (`reveal` 0..=1).
fn draw_icon_slash(c: &mut dyn Canvas, icon: &Icon, x: i32, y: i32, reveal: f32) {
    let limit = 1.0 + reveal.clamp(0.0, 1.0) * 14.0;
    for &(rx, ry, n, argb) in icon.runs {
        let (r, g, b) = ((argb >> 16) & 0xFF, (argb >> 8) & 0xFF, argb & 0xFF);
        let red = r > g + 50 && r > b + 50;
        if red && ry as f32 >= limit {
            continue;
        }
        c.fill(x + rx as i32, y + ry as i32, n as i32, 1, argb);
    }
}

/// Прямоугольник со скруглёнными краями: `insets` — на сколько пикселей срезан каждый из верхних (и нижних) рядов.
/// Полосы не накладываются друг на друга, поэтому годится и для полупрозрачных цветов.
fn rounded_with(c: &mut dyn Canvas, x: i32, y: i32, w: i32, h: i32, argb: u32, insets: &[i32]) {
    let n = insets.len() as i32;
    let widest = insets.iter().copied().max().unwrap_or(0);
    if h < 2 * n + 1 || w <= 2 * widest {
        c.fill(x, y, w, h, argb);
        return;
    }
    for (i, &d) in insets.iter().enumerate() {
        c.fill(x + d, y + i as i32, w - 2 * d, 1, argb);
        c.fill(x + d, y + h - 1 - i as i32, w - 2 * d, 1, argb);
    }
    c.fill(x, y + n, w, h - 2 * n, argb);
}

/// Цвет между двумя ARGB-цветами (t от 0 до 1).
fn lerp_argb(a: u32, b: u32, t: f32) -> u32 {
    let mix = |sh: u32| {
        let (x, y) = (((a >> sh) & 0xFF) as f32, ((b >> sh) & 0xFF) as f32);
        ((x + (y - x) * t).round() as u32) & 0xFF
    };
    (mix(24) << 24) | (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

/// Скругление радиусом 3 пикселя (карточки и плашки).
fn rounded(c: &mut dyn Canvas, x: i32, y: i32, w: i32, h: i32, argb: u32) {
    rounded_with(c, x, y, w, h, argb, &[3, 1, 1]);
}

/// Список говорящих: тёмная скруглённая плашка, голова с зелёной рамкой, ник и три прыгающие полоски звука.
/// Показывается, пока кто-то говорит (оверлей включён и голос подключён). Угол берётся из настройки
/// «Расположение оверлея».
pub fn draw_speakers(c: &mut dyn Canvas, gw: i32, gh: i32) {
    if !S::overlay_on() || !S::connected() {
        return;
    }
    let talking: Vec<roster::MenuPlayer> = roster::menu_players().into_iter().filter(|p| p.talking).collect();
    if talking.is_empty() {
        return;
    }
    let near_corner = S::overlay_corner();
    if S::group_split_on() {
        // у каждого списка свой угол; в одном углу — группа идёт следом за обычными
        let (group, near): (Vec<_>, Vec<_>) = talking.into_iter().partition(|p| p.group);
        let used = talk_list(c, &near, near_corner, 0, gw, gh, None);
        let gc = S::group_corner();
        let skip = if gc == near_corner && used > 0 { used + 6 } else { 0 };
        talk_list(c, &group, gc, skip, gw, gh, Some("Группа"));
    } else {
        talk_list(c, &talking, near_corner, 0, gw, gh, None);
    }
}

/// Цвет рамки и полосок: 0 — зелёный (рядом), 1 — фиолетовый (группа). Между ними идём через голубой, а не
/// через серый: переливание остаётся ярким.
fn talk_tint(g: f32) -> u32 {
    const MID: u32 = 0xFF4F_A8FF;
    if g <= 0.5 { lerp_argb(TALK_NEAR, MID, g * 2.0) } else { lerp_argb(MID, TALK_GROUP, g * 2.0 - 1.0) }
}

/// Цвет говорящего: зелёный — рядом, фиолетовый — в вашей группе.
const TALK_NEAR: u32 = 0xFF55_FF55;
const TALK_GROUP: u32 = 0xFFA8_55F7;

/// Список говорящих в углу `corner` (0 слева сверху, 1 справа сверху, 2 слева снизу, 3 справа снизу): голова
/// в рамке, ник и пляшущие полоски. `skip` — сколько пикселей от края уже занято другим списком в том же углу.
/// `header` — подпись над списком (с иконкой группы). Возвращает высоту нарисованного (0 — ничего).
fn talk_list(c: &mut dyn Canvas, list: &[roster::MenuPlayer], corner: u8, skip: i32, gw: i32, gh: i32, header: Option<&str>) -> i32 {
    let (left, top) = (corner % 2 == 0, corner < 2);
    let list: Vec<&roster::MenuPlayer> = list.iter().take(6).collect();
    if list.is_empty() {
        return 0;
    }
    const ROW: i32 = 20;
    const GAP: i32 = 3;
    const HEAD: i32 = 18;
    let names: Vec<String> = list.iter().map(|p| fit(&p.nick, 100)).collect();
    let nick_w = names.iter().map(|n| text_width(n)).max().unwrap_or(0);
    // отступ 3, голова с рамкой 18, зазор 4, ник, зазор 7, полоски 8, отступ 6
    let w = 3 + 18 + 4 + nick_w + 7 + 8 + 6;
    let head_h = if header.is_some() { HEAD } else { 0 };
    let h = head_h + list.len() as i32 * (ROW + GAP) - GAP;
    let x = if left { 4 } else { gw - w - 4 };
    let y_top = if top { 4 + skip } else { gh - h - 4 - skip };
    if let Some(title) = header {
        let tw = 16 + 3 + text_width(title) + 6;
        let hx = if left { x } else { x + w - tw };
        rounded(c, hx, y_top, tw, 16, 0x9010_1010);
        draw_icon(&mut *c, &icons::SPK_GROUP, hx + 1, y_top, 1);
        c.text(title, hx + 1 + 16 + 3, y_top + 4, TALK_GROUP & 0xFF_FFFF, true);
    }
    let y0 = y_top + head_h;
    // миллисекунды по модулю суток: иначе i32 переполняется и полоски получают отрицательную высоту
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| (d.as_millis() % 86_400_000) as i32).unwrap_or(0);
    let phase = ms / 110;
    let glow = shimmer();
    for (i, (p, name)) in list.iter().zip(&names).enumerate() {
        let y = y0 + i as i32 * (ROW + GAP);
        // 0 — рядом (зелёная карточка), 1 — группа (фиолетовая), между — говорит и туда и туда: переливается
        let g = if p.talk_near && p.talk_group { glow } else if p.group || p.talk_group { 1.0 } else { 0.0 };
        let tint = talk_tint(g);
        rounded(c, x, y, w, ROW, lerp_argb(0x9010_1010, 0xA028_1048, g));
        // голова в цветной рамке: зелёная — рядом, фиолетовая — в группе
        c.fill(x + 3, y + 1, 18, 18, tint);
        match faces::skin(&p.nick) {
            Some(skin) => c.face(&skin, x + 4, y + 2, 16),
            None => {
                c.fill(x + 4, y + 2, 16, 16, 0xFF3A_3A3A);
                let initial: String = p.nick.chars().next().map(|ch| ch.to_uppercase().collect()).unwrap_or_default();
                c.text(&initial, x + 4 + (16 - text_width(&initial)) / 2, y + 6, TEXT, true);
            }
        }
        c.text(name, x + 3 + 18 + 4, y + 6, lerp_argb(0xFF00_0000 | TEXT, 0xFFE6_D2FF, g) & 0xFF_FFFF, true);
        // три полоски звука разной высоты, пляшут по времени (у каждого игрока со своим сдвигом)
        let bx = x + w - 6 - 8;
        for k in 0..3 {
            let hgt = 4 + (phase * (k + 3) + k * 5 + i as i32 * 2).rem_euclid(7);
            c.fill(bx + k * 3, y + 15 - hgt, 2, hgt, tint);
        }
    }
    h
}

/// Подсказка над хотбаром после клавиш действий («Микрофон выключен», «Дистанция: 32»).
pub fn draw_notice(c: &mut dyn Canvas, gw: i32, gh: i32) {
    let Some((text, alpha)) = S::current_notice() else { return };
    let a = ((alpha.clamp(0.0, 1.0) * 255.0) as u32).max(4);
    let w = text_width(&text) + 12;
    let (x, y) = (gw / 2 - w / 2, gh - 72);
    c.fill(x, y, w, 14, (a * 0x60 / 255) << 24);
    c.text(&text, gw / 2 - text_width(&text) / 2, y + 3, TEXT, true);
}

/// Подсказка над прицелом после ПКМ + колесо: «ник  120%» и полоса громкости. `gw`, `gh` — размер GUI.
pub fn draw_volume_toast(c: &mut dyn Canvas, gw: i32, gh: i32) {
    let Some((nick, pct, alpha)) = S::volume_toast() else { return };
    let a = ((alpha.clamp(0.0, 1.0) * 255.0) as u32).max(4);
    let label = if pct == 0 { format!("{nick}: выкл") } else { format!("{nick}: {pct}%") };
    let bar_w = 80;
    let w = (text_width(&label) + 12).max(bar_w + 12);
    let (x, y) = (gw / 2 - w / 2, gh / 2 + 18);
    c.fill(x, y, w, 24, (a * 0x50 / 255) << 24);
    let text_rgb = if pct > 100 { OVER } else if pct == 0 { ERR } else { TEXT };
    c.text(&label, gw / 2 - text_width(&label) / 2, y + 3, text_rgb, true);
    let bx = gw / 2 - bar_w / 2;
    c.fill(bx, y + 15, bar_w, 4, (a << 24) | 0x24_2424);
    let fw = (pct.min(200) as f32 / 200.0 * bar_w as f32) as i32;
    c.fill(bx, y + 15, fw, 4, (a << 24) | if pct > 100 { 0xFF_AA00 } else { 0x8B_8B8B });
    c.fill(bx + bar_w / 2, y + 14, 1, 6, (a << 24) | 0xFF_FFFF);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Rec {
        fills: usize,
        texts: Vec<String>,
    }
    impl Canvas for Rec {
        fn fill(&mut self, _x: i32, _y: i32, w: i32, h: i32, _argb: u32) {
            assert!(w >= 0 && h >= 0, "отрицательный размер {w}x{h}");
            self.fills += 1;
        }
        fn text(&mut self, s: &str, _x: i32, _y: i32, _rgb: u32, _shadow: bool) {
            self.texts.push(s.to_string());
        }
    }

    #[test]
    fn db_mapping_round_trips() {
        for f in [0.0f32, 0.25, 0.5, 0.9] {
            let t = frac_to_threshold(f);
            assert!((db_frac(to_db(t)) - f).abs() < 0.01, "f={f} t={t}");
        }
    }

    /// Тесты делят глобальное состояние меню и настроек, поэтому идут по очереди.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn mic_picker_lists_devices() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        S::set_mic_list(vec![
            ("pulse:alsa_input.usb".to_string(), "USB Microphone".to_string()),
            ("HDA".to_string(), "Built-in Audio".to_string()),
        ]);
        S::set_mic_picker(true);
        lock().tab = *TABS.iter().find(|t| t.title() == "Устройства").expect("вкладка устройств");
        let mut r = Rec::default();
        draw(&mut r, (100, 100));
        S::set_mic_picker(false);
        assert!(r.texts.iter().any(|s| s == "Выберите микрофон"), "{:?}", r.texts);
        assert!(r.texts.iter().any(|s| s == "USB Microphone"));
        assert!(r.texts.iter().any(|s| s == DEFAULT_MIC));
    }

    #[test]
    fn long_device_names_are_cut() {
        let long = "Очень длинное название микрофона с кучей подробностей внутри скобок (hw:2,0)";
        let cut = fit(long, 120);
        assert!(text_width(&cut) <= 120 && cut.ends_with('…'), "{cut}");
        assert_eq!(fit("USB", 120), "USB");
    }

    #[test]
    fn text_width_is_sane() {
        assert_eq!(text_width(""), 0);
        assert!(text_width("Микрофон") > text_width("Мик"));
    }

    #[test]
    fn icons_are_drawn() {
        let mut r = Rec::default();
        draw_icon(&mut r, &icons::MIC, 0, 0, 1);
        assert!(r.fills > 10);
    }

    #[test]
    fn every_tab_draws() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        S::set_mic_picker(false);
        for t in TABS {
            lock().tab = t;
            let mut r = Rec::default();
            draw(&mut r, (100, 100));
            assert!(r.fills > 50, "{}", t.title());
            assert!(r.texts.iter().any(|s| s == "Voice"), "название «Rust Voice» в шапке");
        }
    }

    #[test]
    fn devices_tab_scrolls_to_reveal_spatial_rows() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        S::set_mic_picker(false);
        on_open();
        lock().tab = Tab::Devices;
        lock().gui_w = 427;
        lock().gui_h = 240;
        let mut r = Rec::default();
        draw(&mut r, (0, 0));
        assert!(r.texts.iter().any(|s| s == "Отключить устройство"), "{:?}", r.texts);
        assert!(!r.texts.iter().any(|s| s == "HRTF"), "на маленьком экране низ страницы не виден");
        assert!(on_scroll(-50.0), "колесо на вкладке «Устройства» наше");
        for _ in 0..3 {
            let mut r = Rec::default();
            draw(&mut r, (0, 0));
        }
        // прокрутка доезжает плавно: дадим ей кадры со временем
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(5));
            let mut r = Rec::default();
            draw(&mut r, (0, 0));
        }
        let mut r = Rec::default();
        draw(&mut r, (0, 0));
        assert!(r.texts.iter().any(|s| s == "HRTF"), "{:?}", r.texts);
        assert!(r.texts.iter().any(|s| s == "Заглушение через блоки"));
        on_open();
        lock().tab = Tab::Devices;
    }

    #[test]
    fn slash_is_drawn_top_down() {
        let reds = |reveal: f32| {
            let mut r = Rec::default();
            draw_icon_slash(&mut r, &icons::HEADPHONES_OFF, 0, 0, reveal);
            r.fills
        };
        assert!(reds(0.0) < reds(0.5) && reds(0.5) < reds(1.0), "черта дорисовывается");
    }

    #[test]
    fn group_mic_blends_with_the_plain_one() {
        let pos = |i: &Icon| i.runs.iter().map(|r| (r.0, r.1, r.2)).collect::<Vec<_>>();
        assert_eq!(pos(&icons::MIC), pos(&icons::MIC_GROUP), "одна форма: переливание по отрезкам");
        let mut r = Rec::default();
        draw_icon_blend(&mut r, &icons::MIC, &icons::MIC_GROUP, 0.5, 0, 0);
        assert_eq!(r.fills, icons::MIC.runs.len());
        let t = shimmer();
        assert!((0.0..=1.0).contains(&t));
    }

    #[test]
    fn group_talkers_get_their_own_list_with_header() {
        let mp = |nick: &str, group: bool| roster::MenuPlayer {
            nick: nick.to_string(),
            talking: true,
            group,
            talk_near: !group,
            talk_group: group,
            mic_off: false,
        };
        let mut r = Rec::default();
        talk_list(&mut r, &[mp("Alex", true)], 0, 0, 400, 300, Some("Группа"));
        assert_eq!(r.texts.first().map(String::as_str), Some("Группа"));
        assert!(r.texts.iter().any(|t| t == "Alex"));
        let mut r = Rec::default();
        assert!(talk_list(&mut r, &[mp("Steve", false)], 1, 0, 400, 300, None) > 0);
        assert!(r.texts.iter().any(|t| t == "Steve") && !r.texts.iter().any(|t| t == "Группа"));
        let mut r = Rec::default();
        assert_eq!(talk_list(&mut r, &[], 0, 0, 400, 300, Some("Группа")), 0);
        assert!(r.texts.is_empty() && r.fills == 0, "пустая группа — ни заголовка, ни рамки");
    }

    #[test]
    fn players_list_scrolls_by_wheel_without_buttons() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        S::set_mic_picker(false);
        on_open();
        lock().tab = Tab::Players;
        let mut r = Rec::default();
        draw(&mut r, (0, 0));
        S::set_players_scroll(0);
        assert!(on_scroll(-2.0), "колесо вниз на вкладке «Громкость» наше");
        assert_eq!(S::players_scroll(), 2);
        assert!(on_scroll(1.0));
        assert_eq!(S::players_scroll(), 1);
        assert!(on_scroll(5.0));
        assert_eq!(S::players_scroll(), 0, "выше начала не уезжает");
        let mut r = Rec::default();
        draw(&mut r, (0, 0));
        assert!(!r.texts.iter().any(|s| s == "Вверх" || s == "Вниз"), "кнопок больше нет");
        lock().tab = Tab::Devices;
    }

    #[test]
    fn activations_tab_lists_server_activations() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        S::set_mic_picker(false);
        let act = |name: &str, weight: i32, distances: Vec<i32>| crate::plasmo::Activation {
            name: name.into(),
            translation: String::new(),
            id: crate::plasmo::activation_id(name),
            distances,
            default_distance: 16,
            proximity: name == "proximity",
            transitive: true,
            stereo: false,
            weight,
        };
        S::set_server_activations(vec![act("proximity", 1, vec![8, 16, 32]), act("groups", 5, vec![])]);
        on_open();
        lock().tab = Tab::Activation;
        lock().gui_h = 600;
        let mut r = Rec::default();
        draw(&mut r, (0, 0));
        let pos = |t: &str| r.texts.iter().position(|s| s == t);
        assert!(pos("Локальный").unwrap() < pos("Группы").unwrap(), "«Локальный» первым: {:?}", r.texts);
        assert!(r.texts.iter().any(|s| s == "Дистанция «Локальный»"));
        assert!(!r.texts.iter().any(|s| s == "Дистанция «Группы»"), "у групп дистанции нет");
        assert!(r.texts.iter().any(|s| s == "По клавише"), "группы по умолчанию по клавише");
        assert!(r.texts.iter().any(|s| s.starts_with("Нет")), "у групп клавиша не назначена");
        S::reset_activations();
        lock().gui_h = 240;
        lock().tab = Tab::Devices;
    }

    #[test]
    fn ptt_key_is_bound_by_pressing_it_in_the_menu() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        S::set_mic_picker(false);
        S::reset_activations();
        on_open();
        lock().tab = Tab::Activation;
        let mut r = Rec::default();
        draw(&mut r, (0, 0));
        assert!(!capture_key("key.keyboard.g", true), "без щелчка клавиши не забираем");
        lock().binding = Some(Bind::Slot(0));
        assert!(capture_key("key.keyboard.g", true));
        assert_eq!(S::slot_binding(0).as_deref(), Some("key.keyboard.g"));
        assert_eq!(S::slot_key_label(0), "П (G)");
        assert!(!capture_key("key.keyboard.h", true), "ждали только одну клавишу");
        lock().binding = Some(Bind::Slot(0));
        assert!(capture_key("key.keyboard.escape", true));
        assert_eq!(S::slot_binding(0).as_deref(), Some("key.keyboard.g"), "Esc — отмена");
        lock().binding = Some(Bind::Slot(0));
        assert!(capture_key("key.keyboard.backspace", true));
        assert_eq!(S::slot_binding(0).as_deref(), Some(S::UNBOUND_KEY), "Backspace — убрать");
        S::set_slot_binding(0, None);
        // действие: выключить микрофон
        lock().binding = Some(Bind::Action("mute"));
        assert!(capture_key("key.keyboard.b", true));
        assert_eq!(S::action_for_key("key.keyboard.b"), Some("mute"));
        // Backspace у меню возвращает клавишу из Options > Mods, а не оставляет без клавиши
        S::set_action_binding("menu", Some("key.keyboard.n"));
        lock().binding = Some(Bind::Action("menu"));
        assert!(capture_key("key.keyboard.backspace", true));
        assert_eq!(S::action_binding("menu"), None);
        S::set_action_binding("mute", None);
        lock().tab = Tab::Devices;
    }

    #[test]
    fn hotkeys_and_advanced_tabs_look_like_plasmo() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        S::set_mic_picker(false);
        on_open();
        lock().gui_h = 700;
        lock().tab = Tab::Controls;
        let mut r = Rec::default();
        draw(&mut r, (0, 0));
        for t in ["Основные", "Выключить микрофон", "Рация «Локальный»", "Увеличить дистанцию", "Кнопка действия"] {
            assert!(r.texts.iter().any(|s| s == t), "нет «{t}»: {:?}", r.texts);
        }
        lock().tab = Tab::Status;
        let mut r = Rec::default();
        draw(&mut r, (0, 0));
        for t in ["Визуальные и интерфейс", "Звуковой движок", "Громкость расстояния", "Состояние"] {
            assert!(r.texts.iter().any(|s| s == t), "нет «{t}»: {:?}", r.texts);
        }
        lock().gui_h = 240;
        lock().tab = Tab::Devices;
    }

    #[test]
    fn output_picker_lists_outputs() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        S::set_out_list(vec![("pulse:bluez_output.headset".to_string(), "Наушники".to_string())]);
        S::set_out_picker(true);
        lock().tab = Tab::Devices;
        let mut r = Rec::default();
        draw(&mut r, (100, 100));
        S::set_out_picker(false);
        assert!(r.texts.iter().any(|s| s == "Выберите устройство вывода"));
        assert!(r.texts.iter().any(|s| s == "Наушники"));
    }

    #[test]
    fn press_only_counts_while_drawn() {
        let mut r = Rec::default();
        draw(&mut r, (0, 0));
        assert!(press());
        release();
    }

    #[test]
    fn players_page_lists_players_with_volume() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        S::set_mic_picker(false);
        lock().tab = Tab::Players;
        let mut r = Rec::default();
        draw(&mut r, (100, 100));
        assert!(r.texts.iter().any(|s| s.contains("Громкость игроков")));
    }

    #[test]
    fn mute_button_remembers_volume() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(toggle_mute("steve", 140), Some(0));
        assert_eq!(toggle_mute("steve", 0), Some(140));
        assert_eq!(toggle_mute("steve", 0), Some(100), "запоминали только один раз");
    }

    #[test]
    fn search_takes_typed_keys_only_on_volume_tab() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        S::set_mic_picker(false);
        lock().tab = Tab::Players;
        on_open();
        let mut r = Rec::default();
        draw(&mut r, (0, 0));
        assert!(on_key("key.keyboard.s", true));
        assert!(on_key("key.keyboard.minus", true));
        assert!(on_key("key.keyboard.1", true));
        assert_eq!(lock().search, "s_1");
        assert!(on_key("key.keyboard.backspace", true));
        assert_eq!(lock().search, "s_");
        assert!(!on_key("key.keyboard.escape", true), "Esc остаётся игре");
        lock().tab = Tab::Devices;
        assert!(!on_key("key.keyboard.s", true));
        lock().search.clear();
    }

    #[test]
    fn volume_toast_draws_only_while_fresh() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let mut r = Rec::default();
        draw_volume_toast(&mut r, 427, 240);
        let before = r.texts.len();
        S::show_volume_toast("Steve", 120);
        draw_volume_toast(&mut r, 427, 240);
        assert_eq!(r.texts.len(), before + 1);
        assert!(r.texts.last().unwrap().contains("Steve"));
    }
}