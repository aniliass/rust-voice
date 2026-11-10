//! Ноты над пластинкой: пока играет звук мира (пластинка pv-addon-discs и другие звуки «в точке»), над
//! источником в такт музыке вылетают разноцветные ноты, как ванильные частицы нотного блока.
//!
//! Частицы мира клиентский мод на чужом сервере создать не может (`send_particles` есть только у сервера),
//! поэтому ноты рисуются на HUD: точка в мире проецируется камерой на экран (`nametag::project`), как значки
//! над никами. За стеной нот не видно (проверка луча раз в 150 мс), вдали они меньше и гаснут.
//!
//! Такт: голосовой поток отдаёт громкость каждого кадра (`on_audio`), здесь две огибающие — быстрая и
//! медленная. Когда быстрая резко выше медленной, это удар: вылетает нота. В тихих местах ноты идут реже.
//!
//! Нота, влетевшая в блок (крышка над проигрывателем, потолок), разбивается на цветные осколки, которые
//! разлетаются и падают. Каждую ноту и каждый осколок видно, только если до них прямой луч от камеры.
//!
//! Над проигрывателем висит табличка «что играет» (`draw_label`): крутящаяся пластинка, название трека
//! (его присылает сервер вместе со звуком) и полоски громкости. Её можно скрыть в «Оверлее».

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use horizon_client::mod_api::Host;

/// Сколько живёт нота (секунды), на сколько блоков поднимается и на сколько уходит в сторону.
const LIFE: f32 = 1.6;
const RISE: f64 = 1.15;
const DRIFT: f64 = 0.35;
/// Не больше нот на экране сразу.
const MAX_NOTES: usize = 64;
/// Удар: быстрая огибающая выше медленной во столько раз и не тише порога.
const BEAT_RATIO: f32 = 1.35;
const BEAT_FLOOR: f32 = 0.015;
/// Между нотами одного источника не меньше этого; в ровной музыке без ударов — нота раз в `IDLE_GAP`.
const MIN_GAP: Duration = Duration::from_millis(170);
const IDLE_GAP: Duration = Duration::from_millis(850);
/// Тише этого считаем тишиной: нот нет.
const SILENT: f32 = 0.004;
/// Источник считается играющим, пока звук приходил не позже этого.
const PLAYING_FOR: Duration = Duration::from_millis(600);
/// Размер ноты вблизи и вдали (GUI-пиксели), дальше `MAX_DIST` блоков нот не видно.
const NOTE_PX: f32 = 9.0;
const MIN_PX: f32 = 4.0;
const MAX_DIST: f64 = 32.0;
const FADE_DIST: f64 = 24.0;
/// Как часто проверять, не за стеной ли источник.
const LOS_EVERY: Duration = Duration::from_millis(150);

/// Нота 8×8: головка внизу слева, флажок вправо.
const GLYPH: [&str; 8] = [
    "...##...",
    "...###..",
    "...#.##.",
    "...#..#.",
    "...#....",
    ".###....",
    "####....",
    ".##.....",
];

struct Src {
    fast: f32,
    slow: f32,
    last_note: Option<Instant>,
    last_audio: Instant,
    /// Сколько нот ждёт появления (их ставит поток голоса, позицию знает HUD).
    pending: u32,
    /// Номер ноты: от него цвет (как высота у нотного блока).
    count: u32,
    vis: bool,
    vis_at: Option<Instant>,
}

struct Note {
    base: [f64; 3],
    born: Instant,
    /// Куда уходит в сторону (единичный вектор по XZ) и фаза покачивания.
    dir: [f64; 2],
    wobble: f32,
    color: u32,
    /// Последняя точка в воздухе: отсюда разлетаются осколки, если нота врежется в блок.
    last_free: [f64; 3],
    vis: bool,
    vis_at: Option<Instant>,
}

/// Осколок разбитой ноты: летит от точки удара и падает.
struct Shard {
    p0: [f64; 3],
    v: [f64; 3],
    born: Instant,
    color: u32,
    vis: bool,
    size: f32,
}

struct State {
    srcs: HashMap<String, Src>,
    notes: Vec<Note>,
    shards: Vec<Shard>,
    /// Название трека у источника (ключ звука мира -> имя от сервера).
    titles: HashMap<String, String>,
    seed: u64,
}

/// Сколько живёт осколок, их число на одну ноту и ускорение падения (блоков/с²).
const SHARD_LIFE: f32 = 0.5;
const SHARDS: usize = 7;
const GRAVITY: f64 = 9.0;
/// Не больше осколков сразу.
const MAX_SHARDS: usize = 160;
/// Как часто проверять, видна ли нота (луч от камеры).
const NOTE_LOS_EVERY: Duration = Duration::from_millis(90);

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    let mut g = STATE.lock().ok()?;
    let st = g.get_or_insert_with(|| State {
        srcs: HashMap::new(),
        notes: Vec::new(),
        shards: Vec::new(),
        titles: HashMap::new(),
        seed: 0x9E37_79B9_7F4A_7C15,
    });
    Some(f(st))
}

/// Решение для одного кадра звука: огибающие и «пора ли ноте».
fn step(src: &mut Src, rms: f32, now: Instant) -> bool {
    // быстрая догоняет за ~40 мс, медленная за ~0.6 с (кадр 20 мс)
    src.fast += (rms - src.fast) * if rms > src.fast { 0.6 } else { 0.25 };
    src.slow += (rms - src.slow) * 0.035;
    src.last_audio = now;
    if src.fast < SILENT {
        return false;
    }
    let since = src.last_note.map(|t| now.duration_since(t));
    let beat = src.fast > BEAT_FLOOR && src.fast > src.slow * BEAT_RATIO && since.is_none_or(|d| d >= MIN_GAP);
    let idle = since.is_none_or(|d| d >= IDLE_GAP);
    if beat || idle {
        src.last_note = Some(now);
        return true;
    }
    false
}

/// Кадр звука мира пришёл (поток голоса): `key` — ключ источника (`spatial::emitter_key`), `rms` — громкость.
pub fn on_audio(key: &str, rms: f32) {
    if !crate::settings::notes_on() || crate::settings::music_volume_pct() == 0 {
        return;
    }
    let now = Instant::now();
    with(|st| {
        let src = st.srcs.entry(key.to_string()).or_insert(Src {
            fast: 0.0,
            slow: 0.0,
            last_note: None,
            last_audio: now,
            pending: 0,
            count: 0,
            vis: true,
            vis_at: None,
        });
        if step(src, rms, now) {
            src.pending = (src.pending + 1).min(3);
        }
    });
}

/// Сервер сообщил, что играет источник: запомнить название трека для таблички.
pub fn set_title(key: &str, title: Option<&str>) {
    let Some(t) = title.map(str::trim).filter(|t| !t.is_empty()) else { return };
    with(|st| {
        if st.titles.get(key).map(String::as_str) != Some(t) {
            st.titles.insert(key.to_string(), t.to_string());
        }
    });
}

/// Клетка не пропускает ноту: полный блок (в том числе стекло) или непрогруженный чанк.
fn blocked(host: &dyn Host, p: [f64; 3]) -> bool {
    let c = [p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32];
    !matches!(host.client_block_obstruction(c), Some((false, _)))
}

/// Положение ноты через `t` (0..1 жизни): поднимается с замедлением, уходит в сторону и покачивается.
fn note_pos(n: &Note, t: f32) -> [f64; 3] {
    let rise = RISE * (1.0 - (1.0 - t as f64).powi(2));
    let side = DRIFT * t as f64;
    let sway = 0.08 * ((t * 9.0 + n.wobble).sin() as f64);
    [n.base[0] + n.dir[0] * side + n.dir[1] * sway, n.base[1] + rise, n.base[2] + n.dir[1] * side - n.dir[0] * sway]
}

/// Где осколок через `t` секунд.
fn shard_pos(s: &Shard, t: f64) -> [f64; 3] {
    [s.p0[0] + s.v[0] * t, s.p0[1] + s.v[1] * t - 0.5 * GRAVITY * t * t, s.p0[2] + s.v[2] * t]
}

/// Разбить ноту в точке `at`: осколки летят в стороны и чуть вверх, потом падают.
fn shatter(st: &mut State, at: [f64; 3], color: u32, vis: bool, now: Instant) {
    for i in 0..SHARDS {
        if st.shards.len() >= MAX_SHARDS {
            break;
        }
        let a = (i as f64 / SHARDS as f64 + rand(&mut st.seed) * 0.3) * std::f64::consts::TAU;
        let speed = 0.9 + rand(&mut st.seed) * 1.1;
        let up = -0.4 + rand(&mut st.seed) * 1.2;
        st.shards.push(Shard {
            p0: at,
            v: [a.cos() * speed, up, a.sin() * speed],
            born: now,
            color,
            vis,
            size: 1.0 + rand(&mut st.seed) as f32,
        });
    }
}

/// Цвет ноты как у ванильного нотного блока: высота 0..24 по кругу оттенков (`NoteParticle`).
pub fn note_color(n: u32) -> u32 {
    let f = (n % 25) as f32 / 24.0;
    let tau = std::f32::consts::TAU;
    let ch = |shift: f32| ((((f + shift) * tau).sin() * 0.65 + 0.35).clamp(0.0, 1.0) * 255.0).round() as u32;
    0xFF00_0000 | (ch(0.0) << 16) | (ch(1.0 / 3.0) << 8) | ch(2.0 / 3.0)
}

/// Откуда вылетают ноты: чуть выше крышки проигрывателя (его блок ищет `spatial::source_block`), а у звука
/// без блока — из самой точки.
fn source_top(p: [f64; 3], block: Option<[i32; 3]>) -> [f64; 3] {
    match block {
        Some(b) => [b[0] as f64 + 0.5, b[1] as f64 + 1.1, b[2] as f64 + 0.5],
        None => [p[0], p[1] + 0.1, p[2]],
    }
}

fn rand(seed: &mut u64) -> f64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    (*seed >> 11) as f64 / (1u64 << 53) as f64
}

/// Нарисовать ноты (каждый кадр HUD). Позиции источников — из `spatial` (там же их видит звук).
pub fn draw(host: &dyn Host, batch: &mut horizon_client::hud::HudBatch, gui_w: f32, gui_h: f32) {
    if !crate::settings::notes_on() {
        return;
    }
    let Some(cam) = crate::nametag::camera(host) else { return };
    let now = Instant::now();
    let positions = crate::spatial::emitter_positions();
    // (точка, прозрачность, цвет, размер): размер > 0 — нота, < 0 — осколок (его сторона в GUI-пикселях)
    let mut draws: Vec<([f64; 3], f32, u32, f32)> = Vec::new();
    let mut labels: Vec<([f64; 3], String, f32)> = Vec::new();
    with(|st| {
        st.srcs.retain(|_, s| now.duration_since(s.last_audio) < Duration::from_secs(10));
        for (key, pos) in &positions {
            let Some(src) = st.srcs.get_mut(key) else { continue };
            if now.duration_since(src.last_audio) > PLAYING_FOR {
                src.pending = 0;
                continue;
            }
            let top = source_top(*pos, crate::spatial::source_block(*pos, |x, y, z| crate::spatial::spot_of(host, x, y, z)));
            if src.vis_at.is_none_or(|t| now.duration_since(t) >= LOS_EVERY) {
                src.vis = host.client_visual_clear(cam.pos, top) || host.client_visual_clear(cam.pos, [top[0], top[1] + 0.8, top[2]]);
                src.vis_at = Some(now);
            }
            let mut spawned = Vec::new();
            while src.pending > 0 && st.notes.len() < MAX_NOTES {
                src.pending -= 1;
                src.count = src.count.wrapping_add(7); // шаг по кругу оттенков: соседние ноты разного цвета
                spawned.push(note_color(src.count));
            }
            for color in spawned {
                let a = rand(&mut st.seed) * std::f64::consts::TAU;
                let off = 0.15 + rand(&mut st.seed) * 0.15;
                let base = [top[0] + a.cos() * off, top[1], top[2] + a.sin() * off];
                if blocked(host, base) {
                    // над проигрывателем блок: нота разбивается о его низ сразу
                    let at = [base[0], base[1].floor() - 0.04, base[2]];
                    let vis = host.client_visual_clear(cam.pos, at);
                    shatter(st, at, color, vis, now);
                    continue;
                }
                st.notes.push(Note {
                    base,
                    born: now,
                    dir: [a.cos(), a.sin()],
                    wobble: (rand(&mut st.seed) * 6.28) as f32,
                    color,
                    last_free: base,
                    vis: true,
                    vis_at: None,
                });
            }
        }
        // ноты летят; врезалась в блок — разбивается
        let mut hits = Vec::new();
        st.notes.retain_mut(|n| {
            let t = now.duration_since(n.born).as_secs_f32() / LIFE;
            if t >= 1.0 {
                return false;
            }
            let p = note_pos(n, t);
            if blocked(host, p) {
                hits.push((n.last_free, n.color, n.vis));
                return false;
            }
            n.last_free = p;
            if n.vis_at.is_none_or(|at| now.duration_since(at) >= NOTE_LOS_EVERY) {
                n.vis = host.client_visual_clear(cam.pos, p);
                n.vis_at = Some(now);
            }
            if n.vis {
                let alpha = (t / 0.12).min(1.0) * (1.0 - ((t - 0.6) / 0.4).clamp(0.0, 1.0));
                draws.push((p, alpha, n.color, NOTE_PX));
            }
            true
        });
        for (at, color, vis) in hits {
            shatter(st, at, color, vis, now);
        }
        // осколки падают и гаснут; упав на блок, исчезают
        st.shards.retain(|sh| {
            let t = now.duration_since(sh.born).as_secs_f32();
            if t >= SHARD_LIFE {
                return false;
            }
            let p = shard_pos(sh, t as f64);
            if blocked(host, p) {
                return false;
            }
            if sh.vis {
                let k = t / SHARD_LIFE;
                draws.push((p, 1.0 - k * k, sh.color, -sh.size));
            }
            true
        });
        // таблички «что играет»: над каждым играющим проигрывателем с названием
        if crate::settings::disc_label_on() {
            for (key, pos) in &positions {
                let (Some(src), Some(title)) = (st.srcs.get(key), st.titles.get(key)) else { continue };
                if now.duration_since(src.last_audio) > PLAYING_FOR {
                    continue;
                }
                let top = source_top(*pos, crate::spatial::source_block(*pos, |x, y, z| crate::spatial::spot_of(host, x, y, z)));
                let anchor = [top[0], top[1] + LABEL_UP, top[2]];
                if host.client_visual_clear(cam.pos, anchor) {
                    labels.push((anchor, title.clone(), src.fast));
                }
            }
        }
    });
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_millis() % 3_600_000) as f32 / 1000.0)
        .unwrap_or(0.0);
    for (anchor, title, level) in labels {
        let Some((x, y, dist)) = crate::nametag::project(&cam, anchor, gui_w, gui_h) else { continue };
        if dist > LABEL_DIST {
            continue;
        }
        let fade = ((LABEL_DIST - dist) / 4.0).clamp(0.0, 1.0) as f32;
        draw_label(host, batch, x, y, label_scale(dist), &title, level, secs, fade);
    }
    for (p, alpha, color, size) in draws {
        let Some((x, y, dist)) = crate::nametag::project(&cam, p, gui_w, gui_h) else { continue };
        if dist > MAX_DIST {
            continue;
        }
        let far = ((MAX_DIST - dist) / (MAX_DIST - FADE_DIST)).clamp(0.0, 1.0) as f32;
        let a = alpha * far;
        if a < 0.02 {
            continue;
        }
        let k = (4.0 / dist.max(4.0)).powf(0.45) as f32;
        if size > 0.0 {
            let px = (size * k).clamp(MIN_PX, NOTE_PX);
            draw_glyph(host, batch, x, y, px, color, a);
        } else {
            let px = (-size * 1.4 * k).max(1.0);
            host.fill(&mut *batch, [x - px / 2.0, y - px / 2.0, px, px], with_alpha(color, a));
        }
    }
}

/// Табличка над проигрывателем: на сколько выше места, откуда вылетают ноты, и с какого расстояния видна.
const LABEL_UP: f64 = 0.75;
const LABEL_DIST: f64 = 20.0;
/// Самое длинное название (символов), дальше «…».
const TITLE_MAX: usize = 32;

/// Название покороче: длинное обрезаем с «…».
fn short_title(t: &str) -> String {
    if t.chars().count() <= TITLE_MAX {
        t.to_string()
    } else {
        let mut s: String = t.chars().take(TITLE_MAX - 1).collect();
        s.push('…');
        s
    }
}

/// Табличка «что играет»: крутящаяся пластинка, название и полоски громкости. `x, y` — центр низа таблички,
/// `scale` — размер (1 — вблизи, меньше — дальше, как ник игрока). Рисуется одним вызовом `gui_paint` под своим
/// сдвигом и масштабом, поэтому уменьшается целиком, вместе с текстом.
#[allow(clippy::too_many_arguments)]
fn draw_label(host: &dyn Host, batch: &mut horizon_client::hud::HudBatch, x: f32, y: f32, scale: f32, title: &str, level: f32, secs: f32, fade: f32) {
    use horizon_client::hud_layout::{GuiDraw, PoseOp};
    let title = short_title(title);
    let tw = crate::menu::text_width(&title);
    let (pad, disc, gap, bars) = (4, 9, 4, 11);
    let w = pad + disc + gap + tw + gap + bars + pad;
    let h = 15;
    let a = |argb: u32| with_alpha(argb, fade);
    let mut d: Vec<GuiDraw> = Vec::with_capacity(48);
    d.push(GuiDraw::Push(vec![PoseOp::Translate(x - w as f32 * scale / 2.0, y - h as f32 * scale), PoseOp::Scale(scale, scale)]));
    let mut rect = |x: i32, y: i32, w: i32, h: i32, c: u32| d.push(GuiDraw::Fill { x0: x, y0: y, x1: x + w, y1: y + h, color: a(c) });
    // фон и фиолетовая рамка
    rect(0, 0, w, h, 0xB80A_080E);
    rect(0, 0, w, 1, 0xFFA8_55F7);
    rect(0, h - 1, w, 1, 0xFF7E_3BC4);
    rect(0, 0, 1, h, 0xFFA8_55F7);
    rect(w - 1, 0, 1, h, 0xFFA8_55F7);
    // пластинка 9×9: тёмный диск, светлая дорожка, по кругу бежит блик, в центре фиолетовая наклейка
    let (dx, dy) = (pad, 3);
    for (row, line) in DISC.iter().enumerate() {
        for (col, ch) in line.bytes().enumerate() {
            let c = match ch {
                b'#' => 0xFF1C_1C22,
                b'=' => 0xFF3A_3A44,
                b'o' => 0xFFA8_55F7,
                _ => continue,
            };
            rect(dx + col as i32, dy + row as i32, 1, 1, c);
        }
    }
    let ang = secs * std::f32::consts::TAU * 0.8;
    rect(dx + 4 + (ang.cos() * 3.0).round() as i32, dy + 4 + (ang.sin() * 3.0).round() as i32, 1, 1, 0xFFE4_E4EC);
    // полоски громкости: высота по громкости звука, у каждой своя фаза
    let bx = dx + disc + gap + tw + gap;
    for i in 0..4 {
        let wob = 0.55 + 0.45 * (secs * 9.0 + i as f32 * 1.7).sin().abs();
        let bh = (1.0 + level * 60.0 * wob).clamp(1.0, 9.0).round() as i32;
        rect(bx + i * 3, 12 - bh, 2, bh, 0xFFA8_55F7);
    }
    // название
    d.push(GuiDraw::Text { text: title.as_str().into(), x: dx + disc + gap, y: 4, color: a(0xFFF2_EEFC), shadow: true });
    d.push(GuiDraw::Pop);
    host.gui_paint(&mut *batch, &d);
}

/// Размер таблички по расстоянию: полный до `LABEL_NEAR` блоков, дальше уменьшается почти как ник игрока.
fn label_scale(dist: f64) -> f32 {
    ((LABEL_NEAR / dist.max(LABEL_NEAR)).powf(0.85) as f32).clamp(0.3, 1.0)
}

/// До скольких блоков табличка полного размера.
const LABEL_NEAR: f64 = 3.0;

/// Пластинка 9×9 для таблички.
const DISC: [&str; 9] = [
    "..#####..",
    ".#==#==#.",
    "#=#####=#",
    "#=##o##=#",
    "##=ooo=##",
    "#=##o##=#",
    "#=#####=#",
    ".#==#==#.",
    "..#####..",
];

fn with_alpha(argb: u32, a: f32) -> u32 {
    let na = ((argb >> 24) as f32 * a.clamp(0.0, 1.0)).round() as u32;
    (na << 24) | (argb & 0x00FF_FFFF)
}

/// Нота с тёмной тенью (читается на светлом небе и на тёмной пещере). `x, y` — центр.
fn draw_glyph(host: &dyn Host, batch: &mut horizon_client::hud::HudBatch, x: f32, y: f32, size: f32, color: u32, a: f32) {
    let s = size / 8.0;
    let (x0, y0) = (x - size / 2.0, y - size / 2.0);
    for (layer, (dx, dy, argb)) in [(s * 0.6, s * 0.6, with_alpha(0xAA10_0814, a)), (0.0, 0.0, with_alpha(color, a))].into_iter().enumerate() {
        let _ = layer;
        for (row, line) in GLYPH.iter().enumerate() {
            let mut col = 0;
            let b = line.as_bytes();
            while col < b.len() {
                if b[col] != b'#' {
                    col += 1;
                    continue;
                }
                let start = col;
                while col < b.len() && b[col] == b'#' {
                    col += 1;
                }
                let r = [x0 + dx + start as f32 * s, y0 + dy + row as f32 * s, (col - start) as f32 * s, s];
                host.fill(&mut *batch, r, argb);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(now: Instant) -> Src {
        Src { fast: 0.0, slow: 0.0, last_note: None, last_audio: now, pending: 0, count: 0, vis: true, vis_at: None }
    }

    #[test]
    fn beats_make_notes_and_silence_does_not() {
        let t0 = Instant::now();
        let mut s = src(t0);
        let frame = |i: u32| t0 + Duration::from_millis(20 * i as u64);
        // тишина: ни одной ноты
        assert!((0..100).all(|i| !step(&mut s, 0.0, frame(i))));
        // ровный тихий фон, потом удары каждые 500 мс: ноты на ударах, не на каждом кадре
        let mut notes = 0;
        for i in 100..400 {
            let loud = (i - 100) % 25 < 2;
            if step(&mut s, if loud { 0.3 } else { 0.03 }, frame(i)) {
                notes += 1;
            }
        }
        assert!((10..=15).contains(&notes), "нота на удар (12 ударов), а не на кадр: {notes}");
    }

    #[test]
    fn steady_music_still_gets_a_note_now_and_then() {
        let t0 = Instant::now();
        let mut s = src(t0);
        let n = (0..250).filter(|&i| step(&mut s, 0.1, t0 + Duration::from_millis(20 * i))).count();
        // 5 секунд ровного звука: на старте трека несколько нот подряд (медленная огибающая догоняет),
        // дальше примерно раз в IDLE_GAP; не каждый кадр
        assert!((5..=10).contains(&n), "{n}");
    }

    #[test]
    fn colors_follow_the_note_block_palette() {
        assert_eq!(note_color(0) >> 24, 0xFF);
        assert_ne!(note_color(0), note_color(12));
        assert_eq!(note_color(3), note_color(28), "круг из 25 высот");
        // высота 0 у нотного блока — зелёная
        let g = note_color(0);
        assert!(((g >> 8) & 0xFF) > ((g >> 16) & 0xFF));
    }

    #[test]
    fn notes_start_above_the_jukebox() {
        use crate::spatial::{source_block, Spot};
        // проигрыватель (10, 64, -4) на полу; pv-addon-discs шлёт (10.5, 65.5, -3.5)
        let world = |cover: bool| {
            move |x: i32, y: i32, z: i32| match (x, y, z) {
                (10, 64, -4) => Spot::Jukebox,
                (10, 65, -4) if cover => Spot::Solid,
                (_, 63, _) => Spot::Solid,
                _ => Spot::Air,
            }
        };
        let b = source_block([10.5, 65.5, -3.5], world(false));
        assert_eq!(b, Some([10, 64, -4]));
        assert_eq!(source_top([10.5, 65.5, -3.5], b), [10.5, 65.1, -3.5]);
        // другой аддон шлёт центр самого проигрывателя — не путаем его с полом
        assert_eq!(source_block([10.5, 64.5, -3.5], world(false)), Some([10, 64, -4]));
        // накрыт сверху: всё равно проигрыватель, а не крышка
        assert_eq!(source_block([10.5, 65.5, -3.5], world(true)), Some([10, 64, -4]));
        // в воздухе (рог у игрока) — из самой точки
        assert_eq!(source_block([1.2, 70.3, 4.0], |_, _, _| Spot::Air), None);
    }

    #[test]
    fn shards_fly_out_and_fall() {
        let sh = Shard { p0: [0.0, 10.0, 0.0], v: [1.0, 0.5, 0.0], born: Instant::now(), color: 0, vis: true, size: 1.0 };
        let a = shard_pos(&sh, 0.1);
        let b = shard_pos(&sh, 0.4);
        assert!(b[0] > a[0], "летит в сторону");
        assert!(b[1] < 10.0 && b[1] < a[1], "падает");
    }

    #[test]
    fn long_titles_are_cut() {
        assert_eq!(short_title("Temporary"), "Temporary");
        let long = "Очень длинное название трека с YouTube (Official Video)";
        let s = short_title(long);
        assert_eq!(s.chars().count(), TITLE_MAX);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn notes_rise_from_the_spawn_point() {
        let n = Note { base: [0.0, 65.0, 0.0], born: Instant::now(), dir: [1.0, 0.0], wobble: 0.0, color: 0, last_free: [0.0; 3], vis: true, vis_at: None };
        assert_eq!(note_pos(&n, 0.0)[1], 65.0);
        assert!(note_pos(&n, 1.0)[1] > 66.0);
    }

    #[test]
    fn label_shrinks_with_distance() {
        assert_eq!(label_scale(2.0), 1.0);
        assert!(label_scale(8.0) < 0.5 && label_scale(8.0) > 0.4);
        assert!(label_scale(16.0) < label_scale(8.0));
        assert_eq!(label_scale(100.0), 0.3, "не меньше трети: название ещё читается");
    }
}
