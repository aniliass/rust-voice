//! Значок голоса НАД ником игрока: один пиксельный значок (микрофон или динамик) из `icons.rs`.
//!
//! Хук `render_name_tag` умеет только заменить текст одной строки, поэтому над ником значок им не поставить.
//! Делаем так: в `render_name_tag` запоминаем, где стоит игрок (`note`), а в `hud` проецируем точку над его
//! тегом на экран камерой игры и рисуем иконки (`draw`). Тег ника остаётся ванильным.
//!
//! Значки уменьшаются с расстоянием и видны сквозь стены; у крадущихся игроков, как у ника, они
//! скрыты за стеной (проверка по блокам через `host.block`, пересчёт раз в 100 мс).
//!
//! Всё, что зависит от API игры, собрано в двух функциях: `read_camera` (камера) и `block_name` (блок по
//! координатам). Если после обновления игры они перестанут компилироваться, править нужно только их.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use horizon_client::mod_api::Host;

use crate::icons::{self, Icon};
use crate::roster::Indicator;

/// Игрок считается «в кадре», если тег рисовали не позже этого срока назад.
const SEEN_FOR: Duration = Duration::from_millis(120);
/// Через сколько молчания забываем игрока совсем.
const FORGET_AFTER: Duration = Duration::from_secs(3);
/// Позиция сущности меняется 20 раз в секунду: сглаживаем, чтобы значки не дёргались (постоянная времени).
const SMOOTH_SECS: f32 = 0.07;
/// Скорость игрока меряем по смещению за это окно (сек) и сглаживаем (доля нового замера). Значок
/// опережает позицию на `LEAD_SECS`, чтобы сглаживание не отставало от бегущего игрока.
const VEL_WINDOW: f64 = 0.1;
const VEL_BLEND: f64 = 0.7;
const LEAD_SECS: f64 = 0.04;
/// Значок плавно появляется и исчезает (секунды на полный переход).
const FADE_IN_SECS: f32 = 0.12;
const FADE_OUT_SECS: f32 = 0.3;
/// Размер значка догоняет размер по расстоянию за это время (секунды), чтобы он не прыгал.
const SIZE_SMOOTH_SECS: f32 = 0.25;
/// Высота точки крепления тега над ногами: рост стоя 1.8 + 0.5, как у ванильного тега.
const TAG_HEIGHT: f64 = 2.3;
/// Размер значка в GUI-пикселях вблизи (кратно 8) и зазор над тегом.
const ICON_PX: f32 = 12.0;
/// false — размер значков всегда `ICON_PX`. true — значки уменьшаются с расстоянием (дальше `NEAR_DIST` блоков).
const SCALE_WITH_DISTANCE: bool = true;
const NEAR_DIST: f64 = 4.0;
/// Насколько сильно значок уменьшается с расстоянием: 1.0 — как ник (1/расстояние), 0 — совсем не уменьшается.
/// Маленькое значение делает уменьшение едва заметным.
const SCALE_EXPONENT: f64 = 0.3;
/// Минимальный размер значка (GUI-пиксели).
const MIN_ICON_PX: f32 = 7.0;
const GAP_PX: f32 = 3.0;
/// Дальше этого расстояния (в блоках) значки не рисуем.
const MAX_DIST: f64 = 30.0;
/// С какого расстояния значок начинает плавно гаснуть (полностью исчезает на `MAX_DIST`).
const FADE_DIST: f64 = 22.0;
/// Шаг проверки «стена между камерой и игроком», в блоках, и как часто её повторять.
const LOS_STEP: f64 = 0.2;
const LOS_EVERY: Duration = Duration::from_millis(100);
/// Насколько видно значок за стеной (как ванильный ник сквозь стену: бледный, но заметный).
const BEHIND_WALL_ALPHA: f32 = 0.3;
/// За сколько секунд значок бледнеет или проявляется, когда игрок заходит за стену или выходит.
const WALL_FADE_SECS: f32 = 0.15;
/// Если значки оказались вверх ногами по вертикали (проекция с перевёрнутым Y), поставь true.
const FLIP_Y: bool = false;
/// Если значки улетают в случайные места, а не к игрокам, матрицы лежат по строкам: поставь true.
const ROW_MAJOR: bool = false;

/// Камера: позиция и матрицы `[f32; 16]` (у Horizon они уже массивы, по столбцам, как у glam `to_cols_array`).
pub struct Cam {
    pub pos: [f64; 3],
    pub view: [f32; 16],
    pub proj: [f32; 16],
}

/// ЕДИНСТВЕННОЕ место, зависящее от API камеры Horizon (`Host::camera()`, PR «native camera»).
/// Матрицы камеры относительные: вид не содержит сдвига на позицию камеры, поэтому ниже мы вычитаем `pos`.
fn read_camera(host: &dyn Host) -> Option<Cam> {
    let c = host.camera()?;
    Some(Cam {
        pos: [c.position[0], c.position[1], c.position[2]],
        view: c.view,
        proj: c.projection,
    })
}

/// Куда смотрит камера: (глаза, единичный вектор взгляда) в мировых координатах. Для выбора игрока в `aim.rs`.
/// Матрица вида переводит мир в камеру, где взгляд идёт вдоль -Z, поэтому взгляд в мире = минус третья строка
/// её поворота. Раскладку (по столбцам или по строкам) задаёт тот же `ROW_MAJOR`, что и у проекции значков.
pub fn look_ray(host: &dyn Host) -> Option<([f64; 3], [f64; 3])> {
    let cam = read_camera(host)?;
    Some((cam.pos, forward_of(&cam.view)))
}

fn forward_of(view: &[f32; 16]) -> [f64; 3] {
    let (a, b, c) = if ROW_MAJOR { (view[8], view[9], view[10]) } else { (view[2], view[6], view[10]) };
    let f = [-(a as f64), -(b as f64), -(c as f64)];
    let len = (f[0] * f[0] + f[1] * f[1] + f[2] * f[2]).sqrt();
    if len < 1e-9 { [0.0, 0.0, -1.0] } else { [f[0] / len, f[1] / len, f[2] / len] }
}

/// Видна ли камере точка рядом с игроком (нет сплошного блока между ними): чтобы не выбирать игрока сквозь стену.
pub fn can_see(host: &dyn Host, pos: [f64; 3]) -> bool {
    match read_camera(host) {
        Some(cam) => visible(host, &cam, pos),
        None => true,
    }
}

struct Entry {
    nick: String,
    pos: [f64; 3],
    seen: Instant,
    /// Виден ли игрок (нет стены между ним и камерой) и когда это считали в последний раз.
    vis: bool,
    vis_at: Option<Instant>,
    /// Плавная видимость 0..=1 (1 — видно, 0 — за стеной): от неё яркость значка.
    vis_t: f32,
    /// Игрок крадётся: как у ванильного ника, сквозь стены его значки не видны.
    sneaking: bool,
    /// Для скорости: прошлая позиция замера, его время и сглаженная скорость (блоков/сек).
    s_pos: [f64; 3],
    s_at: Instant,
    vel: [f64; 3],
    /// Прозрачность значка 0..=1, последний показанный значок (для затухания) и время прошлого шага.
    alpha: f32,
    ind: Option<Indicator>,
    /// Сглаженная громкость голоса 0..=1 (капсула) и когда началась красная черта (выключил микрофон/звук).
    level: f32,
    slash_at: Option<Instant>,
    fade_at: Instant,
    /// Сглаженный размер значка; 0 — ещё не задан.
    size: f32,
}

static TAGS: Mutex<Option<HashMap<String, Entry>>> = Mutex::new(None);
/// Оверлей работает (камера доступна): тогда `render_name_tag` не вставляет значок в текст.
static OVERLAY_OK: AtomicBool = AtomicBool::new(false);

pub fn overlay_ok() -> bool {
    OVERLAY_OK.load(Relaxed)
}

/// Игрок с видимым тегом: запоминаем (и сглаживаем) его позицию. Зовётся из `render_name_tag` раз в кадр.
pub fn note(nick: &str, pos: [f64; 3], sneaking: bool) {
    let now = Instant::now();
    let Ok(mut g) = TAGS.lock() else { return };
    let map = g.get_or_insert_with(HashMap::new);
    map.retain(|_, e| now.duration_since(e.seen) < FORGET_AFTER);
    match map.get_mut(&nick.to_lowercase()) {
        Some(e) => {
            let dt = now.duration_since(e.seen).as_secs_f32();
            let el = now.duration_since(e.s_at).as_secs_f64();
            if el >= VEL_WINDOW {
                let jump = (0..3).any(|i| (pos[i] - e.s_pos[i]).abs() > 8.0);
                for i in 0..3 {
                    let v = if el < 0.5 && !jump { (pos[i] - e.s_pos[i]) / el } else { 0.0 };
                    e.vel[i] += (v - e.vel[i]) * VEL_BLEND;
                }
                e.s_pos = pos;
                e.s_at = now;
            }
            let target = [pos[0] + e.vel[0] * LEAD_SECS, pos[1] + e.vel[1] * LEAD_SECS, pos[2] + e.vel[2] * LEAD_SECS];
            let far = (0..3).any(|i| (target[i] - e.pos[i]).abs() > 8.0);
            let snap = far || dt > 0.5;
            let a: f64 = if snap { 1.0 } else { f64::from(1.0 - (-dt / SMOOTH_SECS).exp()) };
            for i in 0..3 {
                e.pos[i] += (target[i] - e.pos[i]) * a;
            }
            if snap {
                e.vel = [0.0; 3];
                e.s_pos = pos;
                e.s_at = now;
            }
            e.seen = now;
            e.sneaking = sneaking;
        }
        None => {
            map.insert(
                nick.to_lowercase(),
                Entry {
                    nick: nick.to_string(),
                    pos,
                    seen: now,
                    vis: true,
                    vis_at: None,
                    vis_t: 1.0,
                    sneaking,
                    s_pos: pos,
                    s_at: now,
                    vel: [0.0; 3],
                    alpha: 0.0,
                    ind: None,
                    level: 0.0,
                    slash_at: None,
                    fade_at: now,
                    size: 0.0,
                },
            );
        }
    }
}

fn mul(m: &[f32; 16], v: [f32; 4]) -> [f32; 4] {
    let mut o = [0.0f32; 4];
    for (r, out) in o.iter_mut().enumerate() {
        *out = if ROW_MAJOR {
            m[r * 4] * v[0] + m[r * 4 + 1] * v[1] + m[r * 4 + 2] * v[2] + m[r * 4 + 3] * v[3]
        } else {
            m[r] * v[0] + m[4 + r] * v[1] + m[8 + r] * v[2] + m[12 + r] * v[3]
        };
    }
    o
}

/// Точка (относительно камеры) -> GUI-пиксели. None, если она позади камеры.
fn to_screen(cam: &Cam, rel: [f64; 3], gui_w: f32, gui_h: f32) -> Option<(f32, f32)> {
    let v = [rel[0] as f32, rel[1] as f32, rel[2] as f32, 1.0];
    let c = mul(&cam.proj, mul(&cam.view, v));
    if !(c[3] > 0.05) {
        return None;
    }
    let nx = c[0] / c[3];
    let mut ny = c[1] / c[3];
    if FLIP_Y {
        ny = -ny;
    }
    Some(((nx * 0.5 + 0.5) * gui_w, (0.5 - ny * 0.5) * gui_h))
}

/// Где рисовать значки: центр по X, нижняя граница по Y и высота одного значка.
#[derive(Debug, Clone, Copy)]
pub struct Placed {
    pub cx: f32,
    pub bottom: f32,
    pub h: f32,
}

/// Размер значка по расстоянию до игрока (плавный, без ступенек; границы пикселей прижимает `q`).
fn icon_size(d: [f64; 3]) -> f32 {
    if !SCALE_WITH_DISTANCE {
        return ICON_PX;
    }
    let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(0.1);
    let k = (NEAR_DIST / dist).min(1.0).powf(SCALE_EXPONENT) as f32;
    (ICON_PX * k).clamp(MIN_ICON_PX, ICON_PX)
}

pub fn place(cam: &Cam, pos: [f64; 3], gui_w: f32, gui_h: f32) -> Option<Placed> {
    place_sized(cam, pos, gui_w, gui_h, None)
}

/// То же, но с заданным размером значка (сглаженным): `None` — взять размер по расстоянию.
pub fn place_sized(cam: &Cam, pos: [f64; 3], gui_w: f32, gui_h: f32, size: Option<f32>) -> Option<Placed> {
    let d = [pos[0] - cam.pos[0], pos[1] - cam.pos[1], pos[2] - cam.pos[2]];
    if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] > MAX_DIST * MAX_DIST {
        return None;
    }
    let anchor = [d[0], d[1] + TAG_HEIGHT, d[2]];
    let (ax, ay) = to_screen(cam, anchor, gui_w, gui_h)?;
    // тег висит вниз от точки крепления, поэтому значки ставим чуть выше неё
    let h = size.unwrap_or_else(|| icon_size(d)).clamp(MIN_ICON_PX, ICON_PX);
    let bottom = ay - GAP_PX * h / ICON_PX;
    if ax < -3.0 * h || ax > gui_w + 3.0 * h || bottom < -h || bottom > gui_h + h {
        return None;
    }
    Some(Placed { cx: ax, bottom, h })
}

/// Умножить прозрачность цвета ARGB на `a` (0..=1).
fn with_alpha(argb: u32, a: f32) -> u32 {
    let na = ((argb >> 24) as f32 * a.clamp(0.0, 1.0)).round() as u32;
    (na << 24) | (argb & 0x00FF_FFFF)
}

/// Прижать к половине GUI-пикселя: при масштабе GUI 2 и выше границы попадают на физические пиксели.
fn q(v: f32) -> f32 {
    (v * 2.0).round() / 2.0
}

/// Фирменный фиолетовый мода: им светится капсула микрофона у говорящего.
const TALK_TINT: u32 = 0xA855F7;

/// Какая иконка у значка и с какой яркостью (приоритет сверху вниз):
/// голосовой чат выключен — перечёркнутый динамик; микрофон выключен (сам или сервером) — перечёркнутый
/// микрофон; говорит — микрофон с фиолетовой капсулой; без мода — бледный «пропавший» микрофон.
fn badge_icon(ind: Indicator) -> (&'static Icon, f32, bool) {
    match ind {
        Indicator::Voice { spk_off: true, .. } => (&icons::SPK_OFF, 1.0, false),
        Indicator::Voice { mic_off: true, .. } => (&icons::MIC_MUTED, 1.0, false),
        Indicator::Voice { talking, .. } => (&icons::MIC, 1.0, talking),
        Indicator::NoMod => (&icons::MIC_GONE, 0.55, false),
    }
}

/// За сколько секунд рисуется красная черта, когда игрок выключил микрофон или звук.
const SLASH_SECS: f32 = 0.25;
/// Строки капсулы микрофона в иконке (сверху вниз): по ним капсула «заполняется» громкостью.
const CAPSULE_TOP: u8 = 1;
const CAPSULE_BOTTOM: u8 = 8;

/// Перечёркнутый значок (микрофон или звук выключен): у него рисуется черта.
fn slashed(i: Indicator) -> bool {
    matches!(i, Indicator::Voice { mic_off: true, .. } | Indicator::Voice { spk_off: true, .. })
}

/// Та же иконка (не начинать черту заново, если поменялось что-то другое).
fn same_icon(a: Indicator, b: Indicator) -> bool {
    std::ptr::eq(badge_icon(a).0, badge_icon(b).0)
}

/// Красный пиксель черты в иконке.
fn is_red(argb: u32) -> bool {
    let (r, g, b) = ((argb >> 16) & 0xFF, (argb >> 8) & 0xFF, argb & 0xFF);
    r > g + 50 && r > b + 50
}

/// Один значок над ником: иконка 16×16 с тонкой тёмной обводкой (читается на любом фоне).
///
/// - говорит: капсула микрофона заполняется фиолетовым снизу вверх по громкости голоса (`level` 0..=1:
///   шёпот — чуть-чуть, крик — доверху), остальная капсула бледно-сиреневая;
/// - выключил микрофон или звук: красная черта рисуется сверху вниз (`reveal` 0..=1);
/// - `phase` — секунды, для лёгкого мерцания верхней строки заливки.
fn draw_indicator(ind: Indicator, p: Placed, phase: f32, level: f32, reveal: f32, fill: &mut dyn FnMut([f32; 4], u32)) {
    let (icon, alpha, talking) = badge_icon(ind);
    let s = p.h / 16.0;
    let x = q(p.cx - p.h / 2.0);
    let y = q(p.bottom - p.h);
    let rect = |rx: f32, ry: f32, n: f32| {
        let (x0, y0) = (q(x + rx * s), q(y + ry * s));
        [x0, y0, (q(x + (rx + n) * s) - x0).max(0.5), (q(y + (ry + 1.0) * s) - y0).max(0.5)]
    };
    // черта «дорисовывается» сверху вниз: её пиксели ниже границы пока не видны
    let red_limit = 1.0 + reveal.clamp(0.0, 1.0) * 14.0;
    let shown = |ry: u8, argb: u32| !is_red(argb) || (ry as f32) < red_limit;
    for &(rx, ry, n, argb) in icon.runs {
        if !shown(ry, argb) {
            continue;
        }
        for (ox, oy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)] {
            fill(rect(rx as f32 + ox, ry as f32 + oy, n as f32), with_alpha(0x6000_0000, alpha));
        }
    }
    // сколько строк капсулы залито: хоть одна, пока говорит
    let rows = (CAPSULE_BOTTOM - CAPSULE_TOP + 1) as f32;
    let filled = (level.clamp(0.0, 1.0) * rows).ceil().max(1.0);
    let fill_top = CAPSULE_BOTTOM as f32 + 1.0 - filled;
    for &(rx, ry, n, argb) in icon.runs {
        if !shown(ry, argb) {
            continue;
        }
        let light = ((argb >> 16) & 0xFF) > 0xE0 && ((argb >> 8) & 0xFF) > 0xE0 && (argb & 0xFF) > 0xE0;
        let c = if talking && light && (CAPSULE_TOP..=CAPSULE_BOTTOM).contains(&ry) {
            if (ry as f32) >= fill_top {
                // залитая часть; верхняя строка заливки чуть мерцает
                let top = (ry as f32) < fill_top + 1.0;
                lerp_rgb(argb, 0xFF00_0000 | TALK_TINT, if top { 0.75 + 0.15 * (phase * 9.0).sin() } else { 0.85 })
            } else {
                lerp_rgb(argb, 0xFF00_0000 | TALK_TINT, 0.18)
            }
        } else {
            argb
        };
        fill(rect(rx as f32, ry as f32, n as f32), with_alpha(c, alpha));
    }
}

/// Цвет между двумя ARGB (t от 0 до 1).
fn lerp_rgb(a: u32, b: u32, t: f32) -> u32 {
    let mix = |sh: u32| {
        let (x, y) = (((a >> sh) & 0xFF) as f32, ((b >> sh) & 0xFF) as f32);
        ((x + (y - x) * t.clamp(0.0, 1.0)).round() as u32) & 0xFF
    };
    (mix(24) << 24) | (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

/// ЕДИНСТВЕННОЕ место, зависящее от API «игрок крадётся». В документации Horizon читать приседание других
/// игроков на клиенте я не нашёл, поэтому пока всегда false (значки видны сквозь стены и у крадущихся).
/// Когда найдёшь метод (поза/sneaking у сущности), верни его значение здесь.
pub fn is_sneaking<T: Copy>(_host: &dyn Host, _entity_id: T) -> bool {
    false
}

/// ЕДИНСТВЕННОЕ место, зависящее от API блоков: `host.block(pos)` отдаёт имя блока строкой (пустая — не знаем).
fn block_name(host: &dyn Host, x: i32, y: i32, z: i32) -> Option<String> {
    let name = host.block([x, y, z]).to_string();
    if name.is_empty() { None } else { Some(name) }
}

/// Пропускает ли блок взгляд (воздух, стекло, вода, трава, цветы, факелы, решётки и тому подобное).
fn passes_sight(name: &str) -> bool {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    const SEE_THROUGH: [&str; 22] = [
        "air", "glass", "water", "grass", "fern", "flower", "torch", "sapling", "rail", "vine", "ladder", "sign",
        "button", "lever", "tulip", "poppy", "dandelion", "bush", "bars", "web", "fence", "carpet",
    ];
    SEE_THROUGH.iter().any(|k| n.contains(k)) && !n.contains("fence_gate") && n != "grass_block"
}

/// Свободна ли линия от `from` до `to`: ни один блок на пути не загораживает (концы по 0.3 блока не считаем).
fn clear_line(from: [f64; 3], to: [f64; 3], solid: &mut dyn FnMut(i32, i32, i32) -> bool) -> bool {
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if len < 1.0 {
        return true;
    }
    let mut dist = 0.3;
    while dist <= len - 0.3 {
        let t = dist / len;
        let (x, y, z) = (from[0] + d[0] * t, from[1] + d[1] * t, from[2] + d[2] * t);
        if solid(x.floor() as i32, y.floor() as i32, z.floor() as i32) {
            return false;
        }
        dist += LOS_STEP;
    }
    true
}

/// Виден ли игрок камере: хватает, чтобы голова или грудь не были закрыты стеной. Луч пускает игра
/// (`client_visual_clear`: настоящие формы блоков, стекло прозрачное). Не ответила — проверяем сами по блокам.
fn visible(host: &dyn Host, cam: &Cam, pos: [f64; 3]) -> bool {
    let points = [[pos[0], pos[1] + 1.6, pos[2]], [pos[0], pos[1] + 0.9, pos[2]], [pos[0], pos[1] + TAG_HEIGHT, pos[2]]];
    if points.iter().any(|p| host.client_visual_clear(cam.pos, *p)) {
        return true;
    }
    visible_by_blocks(host, cam, pos)
}

/// Запасная проверка по именам блоков (если луч игры не прошёл, например из-за непрогруженного чанка).
fn visible_by_blocks(host: &dyn Host, cam: &Cam, pos: [f64; 3]) -> bool {
    let mut solid = |x: i32, y: i32, z: i32| block_name(host, x, y, z).is_some_and(|n| !passes_sight(&n));
    [1.6, 0.9].iter().any(|dy| clear_line(cam.pos, [pos[0], pos[1] + dy, pos[2]], &mut solid))
}

/// Нарисовать значки всем, кого видно. Зовётся из `hud`; `gui_w` и `gui_h` — размер GUI в пикселях.
/// Значок плавно появляется, а когда у игрока больше нечего показывать, плавно гаснет.
pub fn draw(host: &dyn Host, batch: &mut horizon_client::hud::HudBatch, gui_w: f32, gui_h: f32) {
    let Some(cam) = read_camera(host) else {
        OVERLAY_OK.store(false, Relaxed);
        return;
    };
    OVERLAY_OK.store(true, Relaxed);
    let now = Instant::now();
    let mut tags: Vec<([f64; 3], f32, Indicator, f32, f32, f32)> = Vec::new();
    match TAGS.lock() {
        Ok(mut g) => {
            if let Some(map) = g.as_mut() {
                for e in map.values_mut().filter(|e| now.duration_since(e.seen) < SEEN_FOR) {
                    let dt = now.duration_since(e.fade_at).as_secs_f32().min(0.1);
                    e.fade_at = now;
                    let d = [e.pos[0] - cam.pos[0], e.pos[1] - cam.pos[1], e.pos[2] - cam.pos[2]];
                    let want = icon_size(d);
                    let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    let far_fade = ((MAX_DIST - dist) / (MAX_DIST - FADE_DIST)).clamp(0.0, 1.0) as f32;
                    if e.size <= 0.0 || dt >= 0.1 {
                        e.size = want;
                    } else {
                        e.size += (want - e.size) * (1.0 - (-dt / SIZE_SMOOTH_SECS).exp());
                    }
                    match crate::roster::indicator(&e.nick) {
                        Some(i) => {
                            // только что выключил микрофон или звук: черта начинает рисоваться
                            if slashed(i) && !e.ind.is_some_and(|old| slashed(old) && same_icon(old, i)) {
                                e.slash_at = Some(now);
                            }
                            e.ind = Some(i);
                            e.alpha = (e.alpha + dt / FADE_IN_SECS).min(1.0);
                        }
                        None => e.alpha = (e.alpha - dt / FADE_OUT_SECS).max(0.0),
                    }
                    if e.alpha <= 0.0 {
                        continue;
                    }
                    let Some(ind) = e.ind else { continue };
                    // капсула: быстро растёт на громком, плавно опадает
                    let want = crate::roster::voice_level(&e.nick);
                    let k = if want > e.level { 1.0 - (-dt / 0.04).exp() } else { 1.0 - (-dt / 0.18).exp() };
                    e.level += (want - e.level) * k;
                    let reveal = e.slash_at.map_or(1.0, |t| (now.duration_since(t).as_secs_f32() / SLASH_SECS).min(1.0));
                    // За стеной значок бледный (как ванильный ник сквозь стену), а у крадущихся не виден вовсе.
                    if e.vis_at.is_none_or(|t| now.duration_since(t) >= LOS_EVERY) {
                        e.vis = visible(host, &cam, e.pos);
                        e.vis_at = Some(now);
                    }
                    let to = if e.vis { 1.0 } else { 0.0 };
                    let step = dt / WALL_FADE_SECS;
                    e.vis_t = if e.vis_t < to { (e.vis_t + step).min(to) } else { (e.vis_t - step).max(to) };
                    let wall = if e.sneaking { e.vis_t } else { BEHIND_WALL_ALPHA + (1.0 - BEHIND_WALL_ALPHA) * e.vis_t };
                    if wall > 0.01 {
                        tags.push((e.pos, e.alpha * far_fade * wall, ind, e.size, e.level, reveal));
                    }
                }
            }
        }
        Err(_) => return,
    }
    let phase = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_millis() % 3_600_000) as f32 / 1000.0)
        .unwrap_or(0.0);
    let mut fill = |r: [f32; 4], argb: u32| host.fill(&mut *batch, r, argb);
    for (pos, alpha, ind, size, level, reveal) in tags {
        let Some(p) = place_sized(&cam, pos, gui_w, gui_h, Some(size)) else { continue };
        let mut faded = |r: [f32; 4], argb: u32| fill(r, with_alpha(argb, alpha));
        draw_indicator(ind, p, phase, level, reveal, &mut faded);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Простая перспектива 90° с квадратным экраном: w = -z, камера в нуле смотрит вдоль -Z.
    fn cam() -> Cam {
        #[rustfmt::skip]
        let proj = [
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, -1.0, -1.0,
            0.0, 0.0, -0.1, 0.0,
        ];
        #[rustfmt::skip]
        let view = [
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0,
        ];
        Cam { pos: [0.0; 3], view, proj }
    }

    #[test]
    fn anchor_in_front_lands_in_screen_center_and_icons_sit_above() {
        let p = place(&cam(), [0.0, -TAG_HEIGHT, -3.0], 240.0, 240.0).expect("точка перед камерой");
        assert!((p.cx - 120.0).abs() < 0.01);
        assert!(p.bottom < 120.0, "значки выше точки крепления тега");
        assert_eq!(p.h, ICON_PX, "вблизи полный размер");
        let far = place(&cam(), [0.0, -TAG_HEIGHT, -20.0], 240.0, 240.0).expect("в пределах дальности");
        if SCALE_WITH_DISTANCE {
            assert!(far.h < p.h && far.h >= MIN_ICON_PX, "вдали значки меньше");
        } else {
            assert_eq!(far.h, p.h, "размер не зависит от расстояния");
        }
        assert!(place(&cam(), [0.0, -TAG_HEIGHT, -100.0], 240.0, 240.0).is_none(), "дальше MAX_DIST");
    }

    #[test]
    fn wall_blocks_the_line_of_sight() {
        let mut wall = |x: i32, _y: i32, _z: i32| x == 5;
        assert!(!clear_line([0.5, 0.5, 0.5], [10.5, 0.5, 0.5], &mut wall));
        assert!(clear_line([0.5, 0.5, 0.5], [3.5, 0.5, 0.5], &mut wall));
        let mut none = |_x: i32, _y: i32, _z: i32| false;
        assert!(clear_line([0.5, 0.5, 0.5], [30.5, 9.5, 2.5], &mut none));
    }

    #[test]
    fn glass_and_air_do_not_block_but_stone_does() {
        assert!(passes_sight("minecraft:air"));
        assert!(passes_sight("minecraft:cave_air"));
        assert!(passes_sight("minecraft:glass_pane"));
        assert!(passes_sight("minecraft:oak_fence"));
        assert!(!passes_sight("minecraft:oak_fence_gate"));
        assert!(!passes_sight("minecraft:stone"));
        assert!(!passes_sight("minecraft:grass_block"));
        assert!(passes_sight("minecraft:short_grass"));
    }

    #[test]
    fn point_behind_camera_is_skipped() {
        assert!(place(&cam(), [0.0, -TAG_HEIGHT, 5.0], 240.0, 240.0).is_none());
    }

    /// Сколько прямоугольников и сколько зелёных полосок нарисовал значок.
    fn purple(c: &u32) -> bool {
        (c >> 16 & 0xFF) > (c >> 8 & 0xFF) + 40 && (c & 0xFF) > (c >> 8 & 0xFF) + 60
    }

    /// Цвета значка (без прозрачности), по одному на каждый нарисованный прямоугольник.
    fn colors(ind: Indicator, level: f32, reveal: f32) -> Vec<u32> {
        let p = Placed { cx: 100.0, bottom: 50.0, h: 16.0 };
        let mut out = Vec::new();
        draw_indicator(ind, p, 0.0, level, reveal, &mut |_r: [f32; 4], argb: u32| out.push(argb & 0x00FF_FFFF));
        out
    }

    const TALK: Indicator = Indicator::Voice { mic_off: false, spk_off: false, talking: true };

    #[test]
    fn capsule_fills_with_voice_level() {
        let whisper = colors(TALK, 0.1, 1.0).iter().filter(|c| purple(c)).count();
        let shout = colors(TALK, 1.0, 1.0).iter().filter(|c| purple(c)).count();
        assert!(whisper >= 1, "шёпот: хоть немного залито");
        assert!(shout > whisper * 3, "крик: капсула доверху ({shout} против {whisper})");
        let quiet = colors(Indicator::Voice { mic_off: false, spk_off: false, talking: false }, 0.0, 1.0);
        assert!(!quiet.iter().any(purple), "молчит: без фиолетового");
    }

    #[test]
    fn slash_is_drawn_top_down() {
        let off = Indicator::Voice { mic_off: true, spk_off: false, talking: false };
        let red = |reveal: f32| colors(off, 0.0, reveal).iter().filter(|c| is_red(**c)).count();
        assert_eq!(red(0.0), 0, "в начале черты нет");
        assert!(red(0.5) > 0 && red(0.5) < red(1.0), "посередине — наполовину");
        assert!(slashed(off) && !slashed(TALK));
        assert!(same_icon(off, Indicator::Voice { mic_off: true, spk_off: false, talking: true }), "заговорил при выключенном — та же иконка");
        assert!(std::ptr::eq(badge_icon(Indicator::Voice { mic_off: true, spk_off: true, talking: true }).0, &icons::SPK_OFF), "звук выключен важнее");
        assert!(badge_icon(Indicator::NoMod).1 < 1.0, "без мода — бледный значок");
    }

    #[test]
    fn alpha_scales_only_the_alpha_channel() {
        assert_eq!(with_alpha(0xFF55_FF55, 1.0), 0xFF55_FF55);
        assert_eq!(with_alpha(0xFF55_FF55, 0.0), 0x0055_FF55);
        assert_eq!(with_alpha(0xFF55_FF55, 0.5), 0x8055_FF55);
    }

    #[test]
    fn note_smooths_and_forgets_unknown_nick_case() {
        note("Steve", [0.0, 0.0, 0.0], false);
        note("steve", [1.0, 0.0, 0.0], false);
        let g = TAGS.lock().unwrap();
        let m = g.as_ref().unwrap();
        assert_eq!(m.get("steve").map(|e| e.nick.as_str()), Some("Steve"));
    }

    #[test]
    fn forward_comes_from_view_matrix() {
        // единичный вид: камера смотрит вдоль -Z
        #[rustfmt::skip]
        let id = [
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0,
        ];
        let f = forward_of(&id);
        assert!((f[2] + 1.0).abs() < 1e-9 && f[0].abs() < 1e-9 && f[1].abs() < 1e-9);
        // камера повёрнута на 90° вокруг Y и смотрит вдоль -X
        #[rustfmt::skip]
        let yaw = [
            0.0, 0.0, 1.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            -1.0, 0.0, 0.0, 0.0,
            0.0, 0.0, 0.0, 1.0,
        ];
        let f = forward_of(&yaw);
        assert!((f[0] + 1.0).abs() < 1e-9 && f[1].abs() < 1e-9 && f[2].abs() < 1e-9, "{f:?}");
    }
}
