//! Какого игрока вы сейчас видите по центру экрана: по нему ПКМ + колесо меняют громкость.
//!
//! Позиции игроков записывает `render_name_tag` (`note`, раз в кадр у каждого, чей ник сейчас рисуется), а
//! `looked_at` бросает луч из камеры (`nametag::look_ray`) и берёт ближайшего игрока, в чью «коробку» он попал.
//! Коробка чуть шире и выше настоящей, чтобы не целиться в пиксель. Если между вами и игроком сплошной блок,
//! игрок не выбирается (`nametag::can_see`).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use horizon_client::mod_api::Host;

/// Позиция игрока считается свежей, если её записали не раньше этого срока назад.
const FRESH: Duration = Duration::from_millis(300);
/// Дальше этого расстояния (в блоках) игрока не выбираем.
const REACH: f64 = 64.0;
/// Половина ширины коробки игрока и её высота над ногами (у настоящего игрока 0.3 и 1.8).
const HALF_W: f64 = 0.55;
const HEIGHT: f64 = 2.0;

static SEEN: Mutex<Option<HashMap<String, (String, [f64; 3], Instant)>>> = Mutex::new(None);

/// Игрока нарисовали в этом кадре: запомнить, где он стоит (`pos` — у ног).
pub fn note(nick: &str, pos: [f64; 3]) {
    let now = Instant::now();
    let Ok(mut g) = SEEN.lock() else { return };
    let map = g.get_or_insert_with(HashMap::new);
    map.retain(|_, (_, _, at)| now.duration_since(*at) < Duration::from_secs(3));
    map.insert(nick.to_lowercase(), (nick.to_string(), pos, now));
}

/// Забыть всех (смена сервера).
pub fn reset() {
    if let Ok(mut g) = SEEN.lock() {
        *g = None;
    }
}

/// Расстояние вдоль луча до коробки игрока, если луч в неё попадает. `from` — глаза, `dir` — единичный взгляд.
fn hit_distance(from: [f64; 3], dir: [f64; 3], feet: [f64; 3]) -> Option<f64> {
    let min = [feet[0] - HALF_W, feet[1], feet[2] - HALF_W];
    let max = [feet[0] + HALF_W, feet[1] + HEIGHT, feet[2] + HALF_W];
    let (mut t_near, mut t_far) = (0.0f64, f64::INFINITY);
    for i in 0..3 {
        if dir[i].abs() < 1e-12 {
            if from[i] < min[i] || from[i] > max[i] {
                return None;
            }
        } else {
            let (a, b) = ((min[i] - from[i]) / dir[i], (max[i] - from[i]) / dir[i]);
            let (lo, hi) = if a < b { (a, b) } else { (b, a) };
            t_near = t_near.max(lo);
            t_far = t_far.min(hi);
            if t_near > t_far {
                return None;
            }
        }
    }
    Some(t_near)
}

/// Ближайший игрок на луче из `from` вдоль `dir`, если он в пределах `REACH`. `visible` — можно ли его видеть (нет стены).
fn pick(
    from: [f64; 3],
    dir: [f64; 3],
    players: &[(String, [f64; 3])],
    mut visible: impl FnMut([f64; 3]) -> bool,
) -> Option<String> {
    let mut best: Option<(f64, &String, [f64; 3])> = None;
    for (nick, pos) in players {
        if let Some(t) = hit_distance(from, dir, *pos) {
            if t <= REACH && best.is_none_or(|(bt, _, _)| t < bt) {
                best = Some((t, nick, *pos));
            }
        }
    }
    let (_, nick, pos) = best?;
    visible(pos).then(|| nick.clone())
}

/// Ник игрока, на которого вы сейчас смотрите (с правильным регистром), или None.
pub fn looked_at(host: &dyn Host) -> Option<String> {
    let (from, dir) = crate::nametag::look_ray(host)?;
    let now = Instant::now();
    let players: Vec<(String, [f64; 3])> = SEEN
        .lock()
        .ok()?
        .as_ref()?
        .values()
        .filter(|(_, _, at)| now.duration_since(*at) < FRESH)
        .map(|(nick, pos, _)| (nick.clone(), *pos))
        .collect();
    pick(from, dir, &players, |pos| crate::nametag::can_see(host, pos))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_hits_player_in_front_and_misses_sideways() {
        let eye = [0.0, 1.62, 0.0];
        let fwd = [0.0, 0.0, -1.0];
        // игрок в 5 блоках прямо по курсу
        let t = hit_distance(eye, fwd, [0.0, 0.0, -5.0]).unwrap();
        assert!((t - (5.0 - HALF_W)).abs() < 1e-9, "{t}");
        // в 2 блоках слева: мимо
        assert!(hit_distance(eye, fwd, [-2.0, 0.0, -5.0]).is_none());
        // за спиной: мимо
        assert!(hit_distance(eye, fwd, [0.0, 0.0, 5.0]).is_none());
        // чуть в стороне, но в пределах запаса коробки
        assert!(hit_distance(eye, fwd, [0.4, 0.0, -5.0]).is_some());
    }

    #[test]
    fn nearest_player_wins_and_walls_block() {
        let eye = [0.0, 1.62, 0.0];
        let fwd = [0.0, 0.0, -1.0];
        let players = vec![("Far".to_string(), [0.0, 0.0, -10.0]), ("Near".to_string(), [0.0, 0.0, -4.0])];
        assert_eq!(pick(eye, fwd, &players, |_| true).as_deref(), Some("Near"));
        assert_eq!(pick(eye, fwd, &players, |_| false), None, "стена между вами");
        let far = vec![("Far".to_string(), [0.0, 0.0, -(REACH + 5.0)])];
        assert_eq!(pick(eye, fwd, &far, |_| true), None, "дальше досягаемости");
    }

    #[test]
    fn looking_up_over_a_player_misses() {
        let eye = [0.0, 1.62, 0.0];
        let up = {
            let d = [0.0, 0.5, -1.0f64];
            let l = (d[1] * d[1] + d[2] * d[2]).sqrt();
            [0.0, d[1] / l, d[2] / l]
        };
        assert!(hit_distance(eye, up, [0.0, 0.0, -10.0]).is_none());
    }
}
