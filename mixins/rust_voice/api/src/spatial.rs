//! Где звучит каждый игрок: игровой поток (`update` из `tick` и `hud`) записывает, где игрок относительно
//! вашей головы и закрыт ли он блоками, а звуковой поток (`params`) берёт это при смешивании.
//!
//! Позиции берём у всех игроков рядом (`Host::client_entities`), а не только у тех, чей ник сейчас виден.
//! Стены проверяет игра: `Host::client_visual_clear` пускает луч по настоящим формам блоков (стекло
//! прозрачное, полублоки — по своей форме).
//!
//! Если игрок за стеной и говорит, ищем, как его голос обходит препятствие (`acoustics.rs`): тогда звук
//! приходит со стороны двери или проёма, а громкость зависит от длины пути.
//!
//! Всё, что зависит от API игры, собрано в `read_listener`, `read_players` и `cell_at`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use horizon_client::mod_api::Host;

use crate::acoustics::{self, Cell};
use crate::dsp::{self, Params};

/// Чаще этого позиции не пересчитываем (hud зовёт каждый кадр).
const UPDATE_EVERY: Duration = Duration::from_millis(8);
/// Стены у одного игрока проверяем не чаще раза в 100 мс: лучи дороже позиций.
const OCC_EVERY: Duration = Duration::from_millis(100);
/// Позиция старше этого — игрок пропал из виду, звучит как есть.
const STALE: Duration = Duration::from_millis(600);
/// Рот игрока над ногами, в долях роста.
const MOUTH: f64 = 0.9;
/// Путь в обход ищем только тем, кто закрыт хотя бы на столько (из трёх лучей закрыты два).
const ROUTE_OCC: f32 = 0.6;
/// Найденный путь освежаем раз в 250 мс; если пути нет, пробуем снова через секунду (комната закрыта — она
/// и останется закрытой). За один вызов `update` ищем путь не больше чем одному игроку.
const ROUTE_EVERY: Duration = Duration::from_millis(250);
const NO_ROUTE_RETRY: Duration = Duration::from_millis(1000);

struct Entry {
    /// Относительно ваших глаз: (вправо, вверх, вперёд), в блоках.
    rel: [f64; 3],
    at: Instant,
    occ: f32,
    occ_at: Option<Instant>,
    /// Обход препятствия: откуда слышно (в мире), длина пути, сколько стёкол по пути; и когда искали.
    route: Option<acoustics::Route>,
    /// `route` пересчитанный в оси головы (обновляется каждый кадр вместе с поворотом).
    route_rel: [f64; 3],
    route_at: Option<Instant>,
}

static MAP: Mutex<Option<HashMap<String, Entry>>> = Mutex::new(None);
static LAST: Mutex<Option<Instant>> = Mutex::new(None);

pub fn reset() {
    if let Ok(mut g) = MAP.lock() {
        *g = None;
    }
}

/// Слушатель: глаза и поворот головы (рысканье в градусах, как в игре: 0 — на юг, +Z).
struct Listener {
    eye: [f64; 3],
    yaw_deg: f32,
}

/// ЕДИНСТВЕННОЕ место, зависящее от `Host::client_state`.
fn read_listener(host: &dyn Host) -> Option<Listener> {
    let s = host.client_state()?;
    Some(Listener { eye: s.eye_position?, yaw_deg: s.yaw })
}

/// ЕДИНСТВЕННОЕ место, зависящее от `Host::client_entities`: (ник, где рот).
fn read_players(host: &dyn Host) -> Vec<(String, [f64; 3])> {
    host.client_entities()
        .into_iter()
        .filter(|e| e.alive)
        .filter_map(|e| {
            let name = e.player_name?;
            let p = e.position;
            Some((name, [p[0], p[1] + e.height as f64 * MOUTH, p[2]]))
        })
        .collect()
}

/// ЕДИНСТВЕННОЕ место, зависящее от `Host::client_block_obstruction`: (целый блок, непрозрачный).
/// Целый непрозрачный — стена; целый прозрачный (стекло, листва) — пропускает, но глушит; остальное — воздух.
/// Непрогруженный чанк — стена.
fn cell_at(host: &dyn Host, x: i32, y: i32, z: i32) -> Cell {
    match host.client_block_obstruction([x, y, z]) {
        None => Cell::Solid,
        Some((true, true)) => Cell::Solid,
        Some((true, false)) => Cell::Thin,
        Some((false, _)) => Cell::Air,
    }
}

/// Перевести точку мира в оси головы: (вправо, вверх, вперёд). В игре при рысканье 0 взгляд на +Z,
/// а правая рука смотрит на -X.
fn to_head(eye: [f64; 3], yaw_deg: f32, p: [f64; 3]) -> [f64; 3] {
    let d = [p[0] - eye[0], p[1] - eye[1], p[2] - eye[2]];
    let (s, c) = (yaw_deg as f64).to_radians().sin_cos();
    let fwd = [-s, c];
    let right = [-c, -s];
    [d[0] * right[0] + d[2] * right[1], d[1], d[0] * fwd[0] + d[2] * fwd[1]]
}

/// Позиции всех игроков рядом. Зовётся из `tick` и `hud`.
pub fn update(host: &dyn Host) {
    let now = Instant::now();
    {
        let Ok(mut last) = LAST.lock() else { return };
        if last.is_some_and(|t| now.duration_since(t) < UPDATE_EVERY) {
            return;
        }
        *last = Some(now);
    }
    let Some(me) = read_listener(host) else { return };
    let players = read_players(host);
    let occlusion = crate::settings::occlusion_on();
    let routes = occlusion && crate::settings::routes_on();
    // путь голоса не длиннее полутора дистанции сервера: дальше его всё равно не слышно
    let max_len = 1.5 * f64::from(crate::settings::distance().max(16));
    let mut route_budget = 1;
    let Ok(mut g) = MAP.lock() else { return };
    let map = g.get_or_insert_with(HashMap::new);
    map.retain(|_, e| now.duration_since(e.at) < STALE * 4);
    for (nick, mouth) in players {
        let rel = to_head(me.eye, me.yaw_deg, mouth);
        let e = map.entry(nick.to_lowercase()).or_insert(Entry {
            rel,
            at: now,
            occ: 0.0,
            occ_at: None,
            route: None,
            route_rel: rel,
            route_at: None,
        });
        e.rel = rel;
        e.at = now;
        if !occlusion {
            e.occ = 0.0;
            e.occ_at = None;
            e.route = None;
            continue;
        }
        if e.occ_at.is_none_or(|t| now.duration_since(t) >= OCC_EVERY) {
            e.occ_at = Some(now);
            // три луча: в рот, в грудь и чуть в сторону. Сколько закрыто — настолько и глуше.
            let targets = [mouth, [mouth[0], mouth[1] - 0.6, mouth[2]], [mouth[0] + 0.35, mouth[1], mouth[2] + 0.35]];
            let shut = targets.iter().filter(|t| !host.client_visual_clear(me.eye, **t)).count();
            e.occ = shut as f32 / targets.len() as f32;
        }
        // обход препятствия: только если игрок за стеной и сейчас говорит (молчащих не слышно — незачем)
        if !routes || e.occ < ROUTE_OCC || !crate::roster::is_talking(&nick) {
            e.route = None;
            e.route_at = None;
        } else {
            let wait = if e.route.is_some() { ROUTE_EVERY } else { NO_ROUTE_RETRY };
            if route_budget > 0 && e.route_at.is_none_or(|t| now.duration_since(t) >= wait) {
                route_budget -= 1;
                e.route_at = Some(now);
                e.route = acoustics::find_route(me.eye, mouth, max_len, |x, y, z| cell_at(host, x, y, z));
            }
        }
        if let Some(r) = e.route {
            e.route_rel = to_head(me.eye, me.yaw_deg, r.apparent);
        }
    }
}

/// Сколько игроков сейчас слышно в обход стен (для вкладки «Продвинутые»).
pub fn routed_count() -> usize {
    MAP.lock().ok().and_then(|g| g.as_ref().map(|m| m.values().filter(|e| e.route.is_some() && e.at.elapsed() < STALE).count())).unwrap_or(0)
}

/// Как сейчас звучит игрок `nick`, если голос слышен до `max_dist` блоков. None — его нет рядом
/// (или он далеко за краем прогрузки): тогда голос играет как есть, по центру.
pub fn params(nick: &str, max_dist: f32) -> Option<Params> {
    let g = MAP.lock().ok()?;
    let e = g.as_ref()?.get(&nick.to_lowercase())?;
    if e.at.elapsed() >= STALE {
        return None;
    }
    let direct = from_rel(e.rel, e.occ, max_dist);
    Some(match e.route {
        Some(r) => choose(direct, e.rel, e.route_rel, r, max_dist),
        None => direct,
    })
}

/// Сквозь стену или в обход: звук идёт обоими путями, слышим тот, что громче.
fn choose(direct: Params, rel: [f64; 3], around_rel: [f64; 3], r: acoustics::Route, max_dist: f32) -> Params {
    let straight = (rel[0] * rel[0] + rel[1] * rel[1] + rel[2] * rel[2]).sqrt();
    // чем сильнее звук огибает угол (и чем больше стёкол прошёл), тем он чуть глуше
    let bend = ((r.length - straight) / (straight + 4.0)).clamp(0.0, 1.0) as f32;
    let occ = (0.35 * bend + 0.3 * r.thin as f32).min(1.0);
    let around = from_rel_len(around_rel, r.length as f32, occ, max_dist);
    let loud = |p: &Params| p.gain * (1.0 - 0.5 * p.occ);
    if loud(&around) > loud(&direct) { around } else { direct }
}

fn from_rel(rel: [f64; 3], occ: f32, max_dist: f32) -> Params {
    let dist = (rel[0] * rel[0] + rel[1] * rel[1] + rel[2] * rel[2]).sqrt() as f32;
    from_rel_len(rel, dist, occ, max_dist)
}

/// Направление берём из `rel`, а громкость — по длине пути `dist` (в обход она длиннее прямой).
fn from_rel_len(rel: [f64; 3], dist: f32, occ: f32, max_dist: f32) -> Params {
    let flat = (rel[0] * rel[0] + rel[2] * rel[2]).sqrt();
    // совсем рядом или прямо над головой направление не слышно: держим по центру
    let (side, front) = if flat < 0.3 { (0.0, 1.0) } else { ((rel[0] / flat) as f32, (rel[2] / flat) as f32) };
    let near = (flat as f32 / 1.5).min(1.0);
    let fade = crate::settings::dist_fade_on();
    let gain = if fade { dsp::distance_gain(dist, max_dist) } else { 1.0 };
    let far = if fade && max_dist > 0.0 { (dist / max_dist).clamp(0.0, 1.0) } else { 0.0 };
    Params { gain, side: side * near, front: lerp1(front, near), occ, far }
}

fn lerp1(front: f32, t: f32) -> f32 {
    1.0 + (front - 1.0) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-9)
    }

    #[test]
    fn head_axes_follow_minecraft_yaw() {
        let eye = [0.0, 64.0, 0.0];
        // рысканье 0: смотрим на юг (+Z), справа запад (-X)
        assert!(near(to_head(eye, 0.0, [0.0, 64.0, 5.0]), [0.0, 0.0, 5.0]));
        assert!(near(to_head(eye, 0.0, [-3.0, 64.0, 0.0]), [3.0, 0.0, 0.0]));
        // рысканье 90: смотрим на запад (-X)
        assert!(near(to_head(eye, 90.0, [-5.0, 66.0, 0.0]), [0.0, 2.0, 5.0]));
        // рысканье 180: на север (-Z), справа восток (+X)
        assert!(near(to_head(eye, 180.0, [4.0, 64.0, 0.0]), [4.0, 0.0, 0.0]));
    }

    #[test]
    fn params_from_position() {
        let right = from_rel([6.0, 0.0, 0.0], 0.0, 32.0);
        assert!((right.side - 1.0).abs() < 1e-6 && right.front.abs() < 1e-6);
        let behind = from_rel([0.0, 0.0, -6.0], 0.0, 32.0);
        assert!((behind.front + 1.0).abs() < 1e-6);
        let far = from_rel([0.0, 0.0, 40.0], 0.0, 32.0);
        assert_eq!(far.gain, 0.0, "за дистанцией тишина");
        let close = from_rel([0.1, 1.0, 0.0], 0.0, 32.0);
        assert_eq!((close.side, close.front, close.gain), (0.0, 1.0, 1.0), "над головой — по центру");
    }

    #[test]
    fn door_route_wins_over_wall_and_points_to_the_door() {
        // друг в 6 блоках прямо, за стеной; дверь справа, путь 10 блоков
        let rel = [0.0, 0.0, 6.0];
        let direct = from_rel(rel, 1.0, 32.0);
        let r = acoustics::Route { apparent: [0.0; 3], length: 10.0, thin: 0 };
        let p = choose(direct, rel, [4.0, 0.0, 3.0], r, 32.0);
        assert!(p.side > 0.5, "слышно справа, со стороны двери: {p:?}");
        assert!(p.occ < 0.5 && p.gain < direct.gain, "чище, чем сквозь стену, но тише, чем по прямой");
        // слишком длинный обход проигрывает стене
        let far = acoustics::Route { apparent: [0.0; 3], length: 30.0, thin: 0 };
        assert_eq!(choose(direct, rel, [4.0, 0.0, 3.0], far, 32.0), direct);
    }
}
