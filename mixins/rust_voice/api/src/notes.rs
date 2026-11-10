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
//!
//! По самому проигрывателю, пока он играет, пробегает белый блик, как по логотипу в меню (`draw_glow`): углы
//! блока проецируются на экран, по их выпуклой оболочке строками идёт диагональная полоса — раз в 20 секунд,
//! каждый раз из другого угла. Тоже отключается в «Оверлее».

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

/// Замочек 7×8 у закреплённой пластинки.
const LOCK: [&str; 8] = [
    "..###..",
    ".#...#.",
    ".#...#.",
    "#######",
    "###.###",
    "###.###",
    "#######",
    ".......",
];

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
    /// Виден ли сам блок проигрывателя (для блика) и когда проверяли.
    block_vis: bool,
    block_vis_at: Option<Instant>,
    /// Множитель громкости «по взгляду» (плавно идёт к 1 у выбранного и к `BACKGROUND` у остальных).
    focus_k: f32,
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
    /// Блок проигрывателя у источника: (точка, блок, когда искали) — чтобы не опрашивать мир каждый кадр.
    blocks: HashMap<String, ([f64; 3], Option<[i32; 3]>, Instant)>,
    /// Какой играющий проигрыватель сейчас громко (закреплённый, иначе тот, на который смотрим) и когда это
    /// обновлял HUD.
    focus: Option<(String, Instant)>,
    /// На какой проигрыватель смотрим прямо сейчас (для клавиши закрепления).
    look: Option<(String, Instant)>,
    /// Закреплённый клавишей проигрыватель и когда он последний раз звучал.
    pinned: Option<(String, Instant)>,
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
        blocks: HashMap::new(),
        focus: None,
        look: None,
        pinned: None,
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
            block_vis: false,
            block_vis_at: None,
            focus_k: 1.0,
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
    let glow = crate::settings::disc_glow_on();
    // выбор взглядом и закрепление тоже считаются здесь (нужна камера), поэтому не выходим, даже если ноты
    // и блик выключены: без играющих пластинок это почти ничего не стоит
    let Some(cam) = crate::nametag::camera(host) else { return };
    let now = Instant::now();
    // эффекты — только у нескольких ближайших играющих проигрывателей: их много в одном месте — остальные без нот,
    // блика и таблички (звук играет как обычно)
    let active = nearest_playing(host, &cam, now);
    // ничего не играет и ничего не догорает (ноты, осколки) — дальше ничего не считаем, даже сущности
    if active.is_empty() && (!crate::settings::notes_on() || with(|st| st.notes.is_empty() && st.shards.is_empty()).unwrap_or(true)) {
        return;
    }
    // сущности рядом (мобы, другие игроки): ноты, табличка и блик за ними не видны
    let occ = occluders(host, &cam, gui_w, gui_h);
    if glow {
        draw_glow(host, batch, &cam, &active, &occ, gui_w, gui_h);
    }
    if !crate::settings::notes_on() {
        return;
    }
    let positions: Vec<(String, [f64; 3])> = active.iter().map(|(k, p, _)| (k.clone(), *p)).collect();
    let block_of = |key: &str| active.iter().find(|(k, _, _)| k == key).and_then(|(_, _, b)| *b);
    // (точка, прозрачность, цвет, размер): размер > 0 — нота, < 0 — осколок (его сторона в GUI-пикселях)
    let mut draws: Vec<([f64; 3], f32, u32, f32)> = Vec::new();
    let mut labels: Vec<([f64; 3], String, f32, bool)> = Vec::new();
    with(|st| {
        st.srcs.retain(|_, s| now.duration_since(s.last_audio) < Duration::from_secs(10));
        for (key, pos) in &positions {
            let Some(src) = st.srcs.get_mut(key) else { continue };
            if now.duration_since(src.last_audio) > PLAYING_FOR {
                src.pending = 0;
                continue;
            }
            let top = source_top(*pos, block_of(key));
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
            // одна табличка: у того, на который смотрим, иначе у ближайшего (рядом стоящие не налезают друг на друга)
            let chosen = st
                .focus
                .as_ref()
                .map(|(k, _)| k.clone())
                .filter(|k| positions.iter().any(|(pk, _)| pk == k))
                .or_else(|| positions.first().map(|(k, _)| k.clone()));
            for (key, pos) in positions.iter().filter(|(k, _)| Some(k) == chosen.as_ref()) {
                let (Some(src), Some(title)) = (st.srcs.get(key), st.titles.get(key)) else { continue };
                if now.duration_since(src.last_audio) > PLAYING_FOR {
                    continue;
                }
                let top = source_top(*pos, block_of(key));
                let anchor = [top[0], top[1] + LABEL_UP, top[2]];
                if host.client_visual_clear(cam.pos, anchor) {
                    labels.push((anchor, title.clone(), src.fast, is_pinned(st, key)));
                }
            }
        }
    });
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_millis() % 3_600_000) as f32 / 1000.0)
        .unwrap_or(0.0);
    for (anchor, title, level, pinned) in labels {
        let Some((x, y, dist)) = crate::nametag::project(&cam, anchor, gui_w, gui_h) else { continue };
        if dist > LABEL_DIST {
            continue;
        }
        // сущности табличку не заслоняют: она всегда поверх, как ник игрока (иначе широкая табличка то бледнела,
        // то нет — смотря попала ли её середина за кого-то); прячут её только стены
        let fade = ((LABEL_DIST - dist) / 4.0).clamp(0.0, 1.0) as f32;
        draw_label(host, batch, x, y, label_scale(dist), &title, level, secs, fade, pinned);
    }
    for (p, alpha, color, size) in draws {
        if behind_entity(&occ, cam.pos, p) {
            continue;
        }
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

/// У скольких ближайших играющих проигрывателей рисовать ноты, блик и табличку; дальше `MAX_DIST` — ни у каких.
const EFFECTS_MAX: usize = 3;
/// Ближе этого (блоков) источник считаем в кадре всегда: ноты от него могут влететь в экран сбоку.
const BEHIND_NEAR: f64 = 2.5;
/// Косинус угла от взгляда, дальше которого — «за спиной» (около 78°: шире любого обычного обзора по
/// горизонтали с запасом на ноты, вылетающие вверх и в стороны).
const FRONT_COS: f64 = 0.2;

/// Источник впереди камеры (в пределах `FRONT_COS`), с поправкой на высоту нот над ним.
fn in_front(cam: [f64; 3], fwd: [f64; 3], p: [f64; 3], dist: f64) -> bool {
    let d = [p[0] - cam[0], p[1] + 0.8 - cam[1], p[2] - cam[2]];
    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
    let _ = dist;
    (d[0] * fwd[0] + d[1] * fwd[1] + d[2] * fwd[2]) / l > FRONT_COS
}

/// Как часто заново искать блок проигрывателя под звуком (если точка звука не сдвинулась).
const BLOCK_EVERY: Duration = Duration::from_secs(2);

/// Ближайшие к камере играющие источники (не больше `EFFECTS_MAX`, не дальше `MAX_DIST`) с их блоком.
fn nearest_playing(host: &dyn Host, camera: &crate::nametag::Cam, now: Instant) -> Vec<(String, [f64; 3], Option<[i32; 3]>)> {
    let positions = crate::spatial::emitter_positions();
    let all_pos = positions.clone();
    let cam = camera.pos;
    let fwd = crate::nametag::forward(camera);
    with(|st| {
        let mut list: Vec<(f64, String, [f64; 3])> = positions
            .into_iter()
            .filter(|(k, _)| st.srcs.get(k).is_some_and(|s| now.duration_since(s.last_audio) <= PLAYING_FOR))
            .map(|(k, p)| {
                let d = ((p[0] - cam[0]).powi(2) + (p[1] - cam[1]).powi(2) + (p[2] - cam[2]).powi(2)).sqrt();
                (d, k, p)
            })
            .filter(|(d, _, _)| *d <= MAX_DIST)
            // за спиной камеры (не в кадре) — пропускаем: ни лучей, ни нот; вплотную — оставляем
            .filter(|(d, _, p)| *d < BEHIND_NEAR || in_front(cam, fwd, *p, *d))
            .collect();
        list.sort_by(|a, b| a.0.total_cmp(&b.0));
        // на какой смотрим: луч взгляда в блок проигрывателя (ближайший), иначе почти по прицелу; только если
        // играют хотя бы два — один и так слышно как надо
        st.blocks.retain(|k, _| st.srcs.contains_key(k));
        // закреплённый: слетает, если отошли дальше `PIN_DIST`, или он молчит дольше `PIN_SILENCE`
        if let Some((k, heard)) = st.pinned.clone() {
            let src_heard = st.srcs.get(&k).map(|s| s.last_audio);
            let heard = src_heard.map_or(heard, |a| a.max(heard));
            let far = all_pos.iter().find(|(pk, _)| *pk == k).is_none_or(|(_, p)| {
                ((p[0] - cam[0]).powi(2) + (p[1] - cam[1]).powi(2) + (p[2] - cam[2]).powi(2)).sqrt() > PIN_DIST
            });
            if far || now.duration_since(heard) > PIN_SILENCE {
                st.pinned = None;
                crate::settings::notice("Пластинка откреплена");
            } else {
                st.pinned = Some((k, heard));
            }
        }
        let look = if !list.is_empty() {
            let mut best: Option<(f64, String)> = None;
            for (_, k, p) in &list {
                let b = cached_block(st, host, k, *p, now);
                let t = match b {
                    Some(b) => ray_box_t(cam, fwd, [b[0] as f64 - 0.1, b[1] as f64 - 0.1, b[2] as f64 - 0.1], [b[0] as f64 + 1.1, b[1] as f64 + 1.1, b[2] as f64 + 1.1]),
                    None => None,
                }
                .or_else(|| {
                    // нет блока или мимо него: почти по прицелу (в пределах ~6°) — тоже он
                    let d = [p[0] - cam[0], p[1] - cam[1], p[2] - cam[2]];
                    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
                    ((d[0] * fwd[0] + d[1] * fwd[1] + d[2] * fwd[2]) / l > FOCUS_COS).then_some(l + 0.5)
                });
                if let Some(t) = t {
                    if best.as_ref().is_none_or(|(bt, _)| t < *bt) {
                        best = Some((t, k.clone()));
                    }
                }
            }
            // сквозь стену не выбираем
            best.filter(|(t, _)| {
                let at = [cam[0] + fwd[0] * (t - 0.3).max(0.0), cam[1] + fwd[1] * (t - 0.3).max(0.0), cam[2] + fwd[2] * (t - 0.3).max(0.0)];
                host.client_visual_clear(cam, at)
            })
            .map(|(_, k)| k)
        } else {
            None
        };
        st.look = look.clone().map(|k| (k, now));
        let focus = match &st.pinned {
            Some((k, _)) => Some(k.clone()),
            None if list.len() >= 2 && crate::settings::disc_focus_on() => look,
            None => None,
        };
        st.focus = focus.clone().map(|k| (k, now));
        // выбранный — всегда среди тех, у кого эффекты
        if let Some(f) = &focus {
            if let Some(i) = list.iter().position(|(_, k, _)| k == f) {
                let item = list.remove(i);
                list.insert(0, item);
            }
        }
        list.truncate(EFFECTS_MAX);
        // те, что не попали в ближайшие, нот не копят: иначе подойдёшь — и вылетит пачка разом
        let keep: Vec<String> = list.iter().map(|(_, k, _)| k.clone()).collect();
        for (k, s) in st.srcs.iter_mut() {
            if !keep.contains(k) {
                s.pending = 0;
            }
        }
        list.into_iter()
            .map(|(_, k, p)| {
                let b = cached_block(st, host, &k, p, now);
                (k, p, b)
            })
            .collect()
    })
    .unwrap_or_default()
}

/// Блок проигрывателя у источника: из кэша (если точка не сдвинулась и прошло меньше `BLOCK_EVERY`) или из мира.
fn cached_block(st: &mut State, host: &dyn Host, k: &str, p: [f64; 3], now: Instant) -> Option<[i32; 3]> {
    let fresh = st.blocks.get(k).filter(|(at, _, t)| {
        (at[0] - p[0]).abs() < 0.3 && (at[1] - p[1]).abs() < 0.3 && (at[2] - p[2]).abs() < 0.3 && now.duration_since(*t) < BLOCK_EVERY
    });
    match fresh {
        Some((_, b, _)) => *b,
        None => {
            let b = crate::spatial::source_block(p, |x, y, z| crate::spatial::spot_of(host, x, y, z));
            st.blocks.insert(k.to_string(), (p, b, now));
            b
        }
    }
}

/// Где луч `o + t·d` (d — единичный) входит в коробку: `t`, или None — мимо (или дальше `MAX_DIST`).
fn ray_box_t(o: [f64; 3], d: [f64; 3], lo: [f64; 3], hi: [f64; 3]) -> Option<f64> {
    let (mut t0, mut t1) = (0.0f64, MAX_DIST);
    for i in 0..3 {
        if d[i].abs() < 1e-9 {
            if o[i] < lo[i] || o[i] > hi[i] {
                return None;
            }
            continue;
        }
        let (mut a, mut b) = ((lo[i] - o[i]) / d[i], (hi[i] - o[i]) / d[i]);
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        t0 = t0.max(a);
        t1 = t1.min(b);
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}

/// Выбор взглядом: «почти по прицелу» — косинус угла (около 6°); остальные проигрыватели тише во столько раз;
/// выбор держится, пока HUD его обновляет (дольше — значит, не рисуем кадры: выбора нет).
const FOCUS_COS: f64 = 0.9945;
const BACKGROUND: f32 = 0.25;
const FOCUS_FRESH: Duration = Duration::from_millis(400);

/// Множитель громкости звука мира `key` для выбора взглядом: вызывается на каждый кадр звука (20 мс) и плавно
/// (за ~0.2 с) ведёт к 1 у выбранного (или у всех, если не выбран никакой) и к `BACKGROUND` у остальных.
pub fn focus_gain(key: &str) -> f32 {
    let now = Instant::now();
    with(|st| {
        let focus = st.focus.as_ref().filter(|(_, at)| now.duration_since(*at) < FOCUS_FRESH).map(|(k, _)| k.clone());
        // «только закреплённая»: слышно лишь ту, что закреплена клавишей; ничего не закреплено — тишина
        let solo = crate::settings::pin_solo_on();
        let pinned = st.pinned.as_ref().map(|(k, _)| k.clone());
        let Some(src) = st.srcs.get_mut(key) else { return if solo { 0.0 } else { 1.0 } };
        let target = if solo {
            if pinned.as_deref() == Some(key) { 1.0 } else { 0.0 }
        } else {
            match focus {
                Some(f) if f != key => BACKGROUND,
                _ => 1.0,
            }
        };
        src.focus_k += (target - src.focus_k) * 0.1;
        src.focus_k
    })
    .unwrap_or(1.0)
}

/// Закрепление слетает дальше этого (блоков) или если пластинка молчит дольше `PIN_SILENCE`.
const PIN_DIST: f64 = 24.0;
const PIN_SILENCE: Duration = Duration::from_secs(3);

/// Клавиша «Закрепить пластинку»: смотрим на играющий — закрепить его (или снять, если он уже закреплён);
/// никуда не смотрим — снять закрепление.
pub fn toggle_pin() {
    let now = Instant::now();
    let msg = with(|st| {
        let look = st.look.as_ref().filter(|(_, at)| now.duration_since(*at) < FOCUS_FRESH).map(|(k, _)| k.clone());
        match (look, st.pinned.as_ref().map(|(k, _)| k.clone())) {
            (Some(l), Some(p)) if l == p => {
                st.pinned = None;
                "Пластинка откреплена"
            }
            (Some(l), _) => {
                st.pinned = Some((l, now));
                "Пластинка закреплена"
            }
            (None, Some(_)) => {
                st.pinned = None;
                "Пластинка откреплена"
            }
            (None, None) => "Посмотрите на играющий проигрыватель",
        }
    });
    if let Some(m) = msg {
        crate::settings::notice(m);
    }
}

/// Закреплён ли этот проигрыватель (для замочка на табличке).
fn is_pinned(st: &State, key: &str) -> bool {
    st.pinned.as_ref().is_some_and(|(k, _)| k == key)
}

/// Блик: дальше этого (блоков) не рисуем, с `GLOW_FADE` гаснет; полоса пробегает за `GLOW_SWEEP` с,
/// потом пауза до `GLOW_PERIOD`; ширина полосы — доля диагонали блока на экране.
const GLOW_DIST: f64 = 28.0;
const GLOW_FADE: f64 = 20.0;
const GLOW_SWEEP: f32 = 1.1;
const GLOW_PERIOD: f32 = 20.0;
const GLOW_WIDTH: f32 = 0.16;
/// Наклон полосы: на сколько сдвигается по X на каждый пиксель вниз.
const GLOW_SLANT: f32 = 0.7;

/// Белый блик по играющему проигрывателю.
fn draw_glow(
    host: &dyn Host,
    batch: &mut horizon_client::hud::HudBatch,
    cam: &crate::nametag::Cam,
    active: &[(String, [f64; 3], Option<[i32; 3]>)],
    occ: &[Occluder],
    gui_w: f32,
    gui_h: f32,
) {
    let now = Instant::now();
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_millis() % 3_600_000) as f32 / 1000.0)
        .unwrap_or(0.0);
    let sweep_t = |phase: f32| ((secs / GLOW_PERIOD + phase).fract() * GLOW_PERIOD) / GLOW_SWEEP;
    let mut blocks: Vec<([i32; 3], f32, f32)> = Vec::new(); // блок, громкость, фаза (свой сдвиг у каждого)
    with(|st| {
        for (key, _, b) in active {
            let Some(b) = *b else { continue };
            let phase = (b[0].wrapping_mul(7) ^ b[2].wrapping_mul(13)).rem_euclid(10) as f32 * 0.037;
            // блик идёт 1 с из 20: в остальное время ни лучей, ни проекции — ничего не считаем
            if sweep_t(phase) > 1.0 + GLOW_WIDTH {
                continue;
            }
            let Some(src) = st.srcs.get_mut(key) else { continue };
            if src.block_vis_at.is_none_or(|t| now.duration_since(t) >= LOS_EVERY) {
                // виден ли блок: луч до точки чуть перед ним со стороны камеры, и до верха
                let c = [b[0] as f64 + 0.5, b[1] as f64 + 0.5, b[2] as f64 + 0.5];
                let d = [cam.pos[0] - c[0], cam.pos[1] - c[1], cam.pos[2] - c[2]];
                let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-3);
                let front = [c[0] + d[0] / l * 0.62, c[1] + d[1] / l * 0.62, c[2] + d[2] / l * 0.62];
                src.block_vis = host.client_visual_clear(cam.pos, front) || host.client_visual_clear(cam.pos, [c[0], c[1] + 0.62, c[2]]);
                src.block_vis_at = Some(now);
            }
            if src.block_vis {
                blocks.push((b, src.fast, phase));
            }
        }
    });
    for (b, level, phase) in blocks {
        // восемь углов на экран; если хоть один за камерой — не рисуем (стоим вплотную)
        let mut pts: Vec<(f32, f32)> = Vec::with_capacity(8);
        let mut dist = 0.0f64;
        for i in 0..8 {
            let p = [b[0] as f64 + (i & 1) as f64, b[1] as f64 + ((i >> 1) & 1) as f64, b[2] as f64 + ((i >> 2) & 1) as f64];
            let Some((x, y, d)) = crate::nametag::project(cam, p, gui_w, gui_h) else { break };
            dist = dist.max(d);
            pts.push((x, y));
        }
        if pts.len() < 8 || dist > GLOW_DIST {
            continue;
        }
        let far = ((GLOW_DIST - dist) / (GLOW_DIST - GLOW_FADE)).clamp(0.0, 1.0) as f32;
        // кто стоит между камерой и проигрывателем: его силуэт вырезаем из блика
        let block_near = box_distance(cam.pos, [b[0] as f64, b[1] as f64, b[2] as f64], [b[0] as f64 + 1.0, b[1] as f64 + 1.0, b[2] as f64 + 1.0]);
        let front: Vec<&Occluder> = occ.iter().filter(|o| o.hull.len() >= 3 && o.near < block_near).collect();
        let hull = convex_hull(&pts);
        if hull.len() < 3 {
            continue;
        }
        let (x0, x1) = hull.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
        let (y0, y1) = hull.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
        if x1 < 0.0 || y1 < 0.0 || x0 > gui_w || y0 > gui_h || (y1 - y0) > gui_h * 2.0 {
            continue;
        }
        // где полоса: 0..1 по диагонали оболочки, за краями — ждёт; раз в `GLOW_PERIOD`, и каждый раз
        // с другой стороны (из одного из четырёх углов)
        let _ = level;
        let span = (x1 - x0) + (y1 - y0) * GLOW_SLANT;
        let cycle_f = secs / GLOW_PERIOD + phase;
        let t = sweep_t(phase);
        if t > 1.0 + GLOW_WIDTH {
            continue;
        }
        let corner = glow_corner(cycle_f.floor() as u32);
        let (flip_x, flip_y) = (corner & 1 == 1, corner & 2 == 2);
        let center = -GLOW_WIDTH + t * (1.0 + 2.0 * GLOW_WIDTH);
        // строки по 1 GUI-пикселю: в каждой — отрезок оболочки и в нём полоса (мягкие края: 4 ступени)
        let step = 1.0f32;
        let mut y = y0.max(0.0).floor();
        while y < y1.min(gui_h) {
            let yc = y + step * 0.5;
            if let Some((l, r)) = hull_span(&hull, yc) {
                let shift = if flip_y { y1 - yc } else { yc - y0 } * GLOW_SLANT;
                for k in 0..4 {
                    let half = GLOW_WIDTH * (1.0 - k as f32 * 0.22);
                    let a = 0.12 * far;
                    // отрезок полосы вдоль «своей» оси x (от левого или правого края)
                    let (u0, u1) = ((center - half) * span - shift, (center + half) * span - shift);
                    let (bl, br) = if flip_x { (x1 - u1, x1 - u0) } else { (x0 + u0, x0 + u1) };
                    let (bl, br) = (bl.max(l), br.min(r));
                    if br > bl {
                        for (sl, sr) in cut_spans(bl, br, front.iter().filter_map(|o| hull_span(&o.hull, yc))) {
                            host.fill(&mut *batch, [sl, y, sr - sl, step], with_alpha(0xFFFF_FFFF, a));
                        }
                    }
                }
            }
            y += step;
        }
    }
}

/// Из какого угла бежит блик в этот раз (0..4: биты — по X справа, по Y снизу); соседние разы — разные углы.
fn glow_corner(cycle: u32) -> u32 {
    let h = cycle.wrapping_mul(0x9E37_79B9) >> 29; // 0..8
    let prev = cycle.wrapping_sub(1).wrapping_mul(0x9E37_79B9) >> 29;
    let (a, b) = (h % 4, prev % 4);
    if a == b { (a + 1 + (h & 1) * 2) % 4 } else { a }
}

/// Сущность, за которой эффектов не видно: её коробка в мире (в этом кадре), ближайшее до неё расстояние
/// и силуэт на экране (пусто, если часть её позади камеры).
pub(crate) struct Occluder {
    lo: [f64; 3],
    hi: [f64; 3],
    near: f64,
    hull: Vec<(f32, f32)>,
}

/// Камера дальше этого от наших глаз — значит, вид от третьего лица (F5), и наше тело видно в кадре.
const SELF_CAM_GAP: f64 = 0.6;

/// Дальше этого (блоков) сущности эффекты не заслоняют: эффектов там уже нет.
const OCCLUDE_DIST: f64 = 40.0;

/// Сущности вокруг (не мы сами: себя игра в списке не даёт) — коробки по положению этого кадра.
fn occluders(host: &dyn Host, cam: &crate::nametag::Cam, gui_w: f32, gui_h: f32) -> Vec<Occluder> {
    let mut boxes: Vec<([f64; 3], [f64; 3])> = Vec::new();
    for e in host.client_entities() {
        if e.invisible || e.spectator || !e.alive {
            continue;
        }
        let now = e.position_at(cam.partial as f64);
        let off = [now[0] - e.position[0], now[1] - e.position[1], now[2] - e.position[2]];
        boxes.push((
            [e.bounds[0][0] + off[0], e.bounds[0][1] + off[1], e.bounds[0][2] + off[2]],
            [e.bounds[1][0] + off[0], e.bounds[1][1] + off[1], e.bounds[1][2] + off[2]],
        ));
    }
    // мы сами: игра в списке сущностей нас не даёт, а в F5 камера вне головы — тогда мы тоже заслоняем
    if let Some(eye) = host.client_state().and_then(|s| s.eye_position) {
        let d = ((eye[0] - cam.pos[0]).powi(2) + (eye[1] - cam.pos[1]).powi(2) + (eye[2] - cam.pos[2]).powi(2)).sqrt();
        if d > SELF_CAM_GAP {
            boxes.push(([eye[0] - 0.3, eye[1] - 1.62, eye[2] - 0.3], [eye[0] + 0.3, eye[1] + 0.2, eye[2] + 0.3]));
        }
    }
    let mut out = Vec::new();
    for (lo, hi) in boxes {
        let near = box_distance(cam.pos, lo, hi);
        if near > OCCLUDE_DIST {
            continue;
        }
        let mut pts = Vec::with_capacity(8);
        for i in 0..8 {
            let p = [if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] }];
            match crate::nametag::project(cam, p, gui_w, gui_h) {
                Some((x, y, _)) => pts.push((x, y)),
                None => {
                    pts.clear();
                    break;
                }
            }
        }
        let hull = if pts.len() == 8 { convex_hull(&pts) } else { Vec::new() };
        out.push(Occluder { lo, hi, near, hull });
    }
    out
}

/// Ближайшее расстояние от точки до коробки (0 — внутри).
fn box_distance(p: [f64; 3], lo: [f64; 3], hi: [f64; 3]) -> f64 {
    let d = |i: usize| (lo[i] - p[i]).max(0.0).max(p[i] - hi[i]);
    (d(0).powi(2) + d(1).powi(2) + d(2).powi(2)).sqrt()
}

/// Заслоняет ли какая-то сущность точку `p` от камеры: луч камера → точка проходит через её коробку.
fn behind_entity(occ: &[Occluder], cam: [f64; 3], p: [f64; 3]) -> bool {
    occ.iter().any(|o| ray_hits_box(cam, p, o.lo, o.hi))
}

/// Отрезок a → b пересекает коробку (не считая самого конца: точка у самой коробки видна).
fn ray_hits_box(a: [f64; 3], b: [f64; 3], lo: [f64; 3], hi: [f64; 3]) -> bool {
    let (mut t0, mut t1) = (0.0f64, 0.98f64);
    for i in 0..3 {
        let d = b[i] - a[i];
        if d.abs() < 1e-9 {
            if a[i] < lo[i] || a[i] > hi[i] {
                return false;
            }
            continue;
        }
        let (mut ta, mut tb) = ((lo[i] - a[i]) / d, (hi[i] - a[i]) / d);
        if ta > tb {
            std::mem::swap(&mut ta, &mut tb);
        }
        t0 = t0.max(ta);
        t1 = t1.min(tb);
        if t0 > t1 {
            return false;
        }
    }
    true
}

/// Отрезок [l, r] без кусков `cuts` (силуэтов сущностей в этой строке).
fn cut_spans(l: f32, r: f32, cuts: impl Iterator<Item = (f32, f32)>) -> Vec<(f32, f32)> {
    let mut parts = vec![(l, r)];
    for (cl, cr) in cuts {
        let mut next = Vec::with_capacity(parts.len() + 1);
        for (a, b) in parts {
            if cr <= a || cl >= b {
                next.push((a, b));
                continue;
            }
            if cl > a {
                next.push((a, cl));
            }
            if cr < b {
                next.push((cr, b));
            }
        }
        parts = next;
    }
    parts
}

/// Выпуклая оболочка точек (обход Эндрю), по часовой или против — неважно.
fn convex_hull(pts: &[(f32, f32)]) -> Vec<(f32, f32)> {
    let mut p: Vec<(f32, f32)> = pts.to_vec();
    p.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
    let cross = |o: (f32, f32), a: (f32, f32), b: (f32, f32)| (a.0 - o.0) * (b.1 - o.1) - (a.1 - o.1) * (b.0 - o.0);
    let mut lower: Vec<(f32, f32)> = Vec::new();
    for &q in &p {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], q) <= 0.0 {
            lower.pop();
        }
        lower.push(q);
    }
    let mut upper: Vec<(f32, f32)> = Vec::new();
    for &q in p.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], q) <= 0.0 {
            upper.pop();
        }
        upper.push(q);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// Где строка `y` пересекает выпуклый многоугольник: (левый, правый) край.
fn hull_span(hull: &[(f32, f32)], y: f32) -> Option<(f32, f32)> {
    let (mut l, mut r) = (f32::MAX, f32::MIN);
    for i in 0..hull.len() {
        let (a, b) = (hull[i], hull[(i + 1) % hull.len()]);
        if (a.1 <= y && b.1 > y) || (b.1 <= y && a.1 > y) {
            let x = a.0 + (y - a.1) / (b.1 - a.1) * (b.0 - a.0);
            l = l.min(x);
            r = r.max(x);
        }
    }
    (r > l).then_some((l, r))
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
#[allow(clippy::too_many_arguments)]
fn draw_label(host: &dyn Host, batch: &mut horizon_client::hud::HudBatch, x: f32, y: f32, scale: f32, title: &str, level: f32, secs: f32, fade: f32, pinned: bool) {
    use horizon_client::hud_layout::{GuiDraw, PoseOp};
    let title = short_title(title);
    let tw = crate::menu::text_width(&title);
    let (pad, disc, gap, bars) = (4, 9, 4, 11);
    // закреплённый: справа золотой замочек 7×8
    let lock_w = if pinned { 7 + gap } else { 0 };
    let w = pad + disc + gap + tw + gap + bars + lock_w + pad;
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
    if pinned {
        let lx = bx + bars + gap;
        for (row, line) in LOCK.iter().enumerate() {
            for (col, ch) in line.bytes().enumerate() {
                if ch == b'#' {
                    rect(lx + col as i32, 3 + row as i32, 1, 1, 0xFFFF_D25A);
                }
            }
        }
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
        Src { fast: 0.0, slow: 0.0, last_note: None, last_audio: now, pending: 0, count: 0, vis: true, vis_at: None, block_vis: false, block_vis_at: None, focus_k: 1.0 }
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

    #[test]
    fn glow_hull_of_a_cube_and_its_rows() {
        // проекция куба: квадрат с вершиной-«выступом», внутренняя точка в оболочку не попадает
        let pts = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0), (5.0, 5.0), (12.0, 5.0)];
        let h = convex_hull(&pts);
        assert_eq!(h.len(), 5);
        assert!(!h.contains(&(5.0, 5.0)));
        let (l, r) = hull_span(&h, 5.0).unwrap();
        assert!((l - 0.0).abs() < 1e-4 && (r - 12.0).abs() < 1e-4);
        assert!(hull_span(&h, 11.0).is_none());
    }


    #[test]
    fn sources_behind_the_camera_are_skipped() {
        let cam = [0.0, 64.0, 0.0];
        let fwd = [0.0, 0.0, -1.0]; // смотрим на север
        assert!(in_front(cam, fwd, [0.0, 63.5, -10.0], 10.0), "впереди");
        assert!(!in_front(cam, fwd, [0.0, 63.5, 10.0], 10.0), "за спиной");
        assert!(in_front(cam, fwd, [6.0, 63.5, -10.0], 11.0), "сбоку в кадре");
        assert!(!in_front(cam, fwd, [10.0, 63.5, 1.0], 10.0), "строго сбоку — уже не в кадре");
    }


    #[test]
    fn entities_hide_what_is_behind_them() {
        let cam = [0.0, 1.6, 0.0];
        // игрок в 3 блоках впереди, нота за ним в 6
        let (lo, hi) = ([-0.3, 0.0, -3.3], [0.3, 1.8, -2.7]);
        assert!(ray_hits_box(cam, [0.0, 1.5, -6.0], lo, hi));
        assert!(!ray_hits_box(cam, [2.0, 1.5, -6.0], lo, hi), "сбоку от него — видно");
        assert!(!ray_hits_box(cam, [0.0, 1.5, -2.0], lo, hi), "перед ним — видно");
        assert!(!ray_hits_box(cam, [0.0, 3.5, -6.0], lo, hi), "выше головы — видно");
        assert_eq!(cut_spans(0.0, 10.0, [(3.0, 5.0)].into_iter()), vec![(0.0, 3.0), (5.0, 10.0)]);
        assert_eq!(cut_spans(0.0, 10.0, [(-1.0, 11.0)].into_iter()), vec![]);
        assert!((box_distance([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 1.0, 1.0]) - 1.0).abs() < 1e-9);
    }


    #[test]
    fn look_ray_picks_the_jukebox() {
        let cam = [0.5, 65.6, 4.0];
        let fwd = [0.0, -0.3, -1.0];
        let l = (0.09f64 + 1.0).sqrt();
        let fwd = [fwd[0] / l, fwd[1] / l, fwd[2] / l];
        assert!(ray_box_t(cam, fwd, [0.0, 64.0, 0.0], [1.0, 65.0, 1.0]).is_some(), "смотрим на него");
        assert!(ray_box_t(cam, fwd, [2.0, 64.0, 0.0], [3.0, 65.0, 1.0]).is_none(), "соседний справа — мимо");
    }

}
