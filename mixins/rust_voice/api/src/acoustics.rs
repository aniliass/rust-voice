//! Звук огибает углы: путь голоса от игрока до вас по воздуху, через двери, окна и проёмы.
//!
//! Поиск A* по клеткам-блокам вокруг слушателя (26 соседей: по прямой, по диагонали и по диагонали куба,
//! с честными длинами 1, √2, √3). Сквозь углы блоков по диагонали не протекает: для диагонального шага
//! все «промежуточные» клетки должны быть проходимы. Стекло и листва пропускают звук, но дороже и глуше.
//!
//! Откуда слышно: идём по найденному пути от вас и берём самую дальнюю точку, которую ещё видно из вашей
//! головы. Обычно это дверной проём: голос друга из соседней комнаты приходит со стороны двери.
//!
//! Тут нет API игры: блоки приходят через замыкание, поэтому всё проверяется обычными тестами.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Что за клетка для звука.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    /// Воздух и всё, что не целый блок (трава, факелы, заборы, полублоки, двери).
    Air,
    /// Целый, но прозрачный блок (стекло, листва): звук проходит, но тише и глуше.
    Thin,
    /// Сплошной блок или непрогруженный чанк.
    Solid,
}

/// Найденный путь звука.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Route {
    /// Откуда голос слышно (мировые координаты): дальняя видимая из головы точка пути.
    pub apparent: [f64; 3],
    /// Длина пути по воздуху, в блоках.
    pub length: f64,
    /// Сколько прозрачных блоков (стекло, листва) звук прошёл насквозь.
    pub thin: u32,
}

/// Радиус области поиска вокруг слушателя (в блоках): дальше голос всё равно почти не слышно.
pub const RADIUS: i32 = 24;
const SIDE: i32 = 2 * RADIUS + 1;
/// Не больше стольких раскрытых клеток на один путь: защита от лабиринтов и пещер.
const MAX_EXPAND: usize = 12_000;
/// Сколько «стоит» пройти сквозь прозрачный блок (в блоках пути).
const THIN_COST: f32 = 4.0;

/// Сосед клетки: смещение, длина шага и «промежуточные» клетки, которые должны быть не сплошными, чтобы
/// звук не протёк сквозь угол (у прямого шага их нет, у диагонали 2, у диагонали куба 6).
#[derive(Clone, Copy)]
struct Step {
    d: [i32; 3],
    len: f32,
    via: [[i32; 3]; 6],
    n_via: u8,
}

/// 26 соседей, посчитанные один раз.
fn neighbours() -> [Step; 26] {
    let mut out = [Step { d: [0; 3], len: 0.0, via: [[0; 3]; 6], n_via: 0 }; 26];
    let mut n = 0;
    for dx in -1..=1 {
        for dy in -1..=1 {
            for dz in -1..=1 {
                if (dx, dy, dz) == (0, 0, 0) {
                    continue;
                }
                let d = [dx, dy, dz];
                let mut st = Step { d, len: ((dx * dx + dy * dy + dz * dz) as f32).sqrt(), via: [[0; 3]; 6], n_via: 0 };
                // каждая непустая собственная часть шага: только по X, только по Z, по X и Y...
                for mask in 1u8..7 {
                    let part = [d[0] * (mask & 1) as i32, d[1] * ((mask >> 1) & 1) as i32, d[2] * ((mask >> 2) & 1) as i32];
                    if part != [0; 3] && part != d && (0..3).all(|a| part[a] == 0 || part[a] == d[a]) {
                        if !st.via[..st.n_via as usize].contains(&part) {
                            st.via[st.n_via as usize] = part;
                            st.n_via += 1;
                        }
                    }
                }
                out[n] = st;
                n += 1;
            }
        }
    }
    out
}

/// Сетка вокруг слушателя: клетки спрашиваем у игры лениво, только те, до которых дошёл поиск.
struct Grid<F: FnMut(i32, i32, i32) -> Cell> {
    origin: [i32; 3],
    /// 0 — ещё не спрашивали, иначе Cell + 1.
    cells: Vec<u8>,
    query: F,
}

impl<F: FnMut(i32, i32, i32) -> Cell> Grid<F> {
    fn index(&self, p: [i32; 3]) -> Option<usize> {
        let l = [p[0] - self.origin[0] + RADIUS, p[1] - self.origin[1] + RADIUS, p[2] - self.origin[2] + RADIUS];
        if l.iter().any(|&v| v < 0 || v >= SIDE) {
            return None;
        }
        Some(((l[0] * SIDE + l[1]) * SIDE + l[2]) as usize)
    }

    fn pos(&self, i: usize) -> [i32; 3] {
        let i = i as i32;
        let (x, rest) = (i / (SIDE * SIDE), i % (SIDE * SIDE));
        let (y, z) = (rest / SIDE, rest % SIDE);
        [x + self.origin[0] - RADIUS, y + self.origin[1] - RADIUS, z + self.origin[2] - RADIUS]
    }

    fn get(&mut self, p: [i32; 3]) -> Cell {
        let Some(i) = self.index(p) else { return Cell::Solid };
        match self.cells[i] {
            0 => {
                let c = (self.query)(p[0], p[1], p[2]);
                self.cells[i] = c as u8 + 1;
                c
            }
            1 => Cell::Air,
            2 => Cell::Thin,
            _ => Cell::Solid,
        }
    }

    /// Можно ли сделать шаг из `p`, не протекая сквозь углы блоков.
    fn can_step(&mut self, p: [i32; 3], st: &Step) -> bool {
        st.via[..st.n_via as usize].iter().all(|v| self.get([p[0] + v[0], p[1] + v[1], p[2] + v[2]]) != Cell::Solid)
    }

    /// Видна ли точка `to` из `from` (ни сплошного, ни прозрачного блока на линии).
    fn line_clear(&mut self, from: [f64; 3], to: [f64; 3]) -> bool {
        let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let steps = (len / 0.25).ceil() as i32;
        for s in 1..steps {
            let t = s as f64 / steps as f64;
            let p = [
                (from[0] + d[0] * t).floor() as i32,
                (from[1] + d[1] * t).floor() as i32,
                (from[2] + d[2] * t).floor() as i32,
            ];
            if self.get(p) != Cell::Air {
                return false;
            }
        }
        true
    }
}

fn block_of(p: [f64; 3]) -> [i32; 3] {
    [p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32]
}

fn center(p: [i32; 3]) -> [f64; 3] {
    [p[0] as f64 + 0.5, p[1] as f64 + 0.5, p[2] as f64 + 0.5]
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Путь голоса из `from` (ваши глаза) к `to` (рот игрока) не длиннее `max_len` блоков, или None, если
/// по воздуху не дойти (закрытая комната, слишком далеко, за краем области поиска).
pub fn find_route(from: [f64; 3], to: [f64; 3], max_len: f64, query: impl FnMut(i32, i32, i32) -> Cell) -> Option<Route> {
    let start = block_of(from);
    let goal = block_of(to);
    let mut g = Grid { origin: start, cells: vec![0u8; (SIDE * SIDE * SIDE) as usize], query };
    let (si, gi) = (g.index(start)?, g.index(goal)?);
    let n = g.cells.len();
    let mut cost = vec![f32::INFINITY; n];
    let mut parent = vec![u32::MAX; n];
    let goal_c = center(goal);
    let h = |p: [i32; 3]| dist(center(p), goal_c) as f32;
    let nb = neighbours();
    let mut heap = BinaryHeap::new();
    cost[si] = 0.0;
    heap.push(Reverse((h(start).to_bits(), si as u32)));
    let mut expanded = 0usize;
    let mut found = false;
    while let Some(Reverse((f_bits, i))) = heap.pop() {
        let i = i as usize;
        let p = g.pos(i);
        if f32::from_bits(f_bits) > cost[i] + h(p) + 1e-3 {
            continue; // устаревшая запись
        }
        if i == gi {
            found = true;
            break;
        }
        expanded += 1;
        if expanded > MAX_EXPAND || cost[i] as f64 > max_len {
            break;
        }
        for st in &nb {
            let (d, step) = (st.d, st.len);
            let q = [p[0] + d[0], p[1] + d[1], p[2] + d[2]];
            let Some(qi) = g.index(q) else { continue };
            // клетку игрока считаем проходимой, даже если он стоит в полублоке или по горло в листве
            let c = if qi == gi { Cell::Air } else { g.get(q) };
            if c == Cell::Solid || !g.can_step(p, st) {
                continue;
            }
            let nc = cost[i] + step + if c == Cell::Thin { THIN_COST } else { 0.0 };
            if nc < cost[qi] {
                cost[qi] = nc;
                parent[qi] = i as u32;
                heap.push(Reverse(((nc + h(q)).to_bits(), qi as u32)));
            }
        }
    }
    if !found {
        return None;
    }

    // путь от вас к игроку
    let mut path = vec![gi];
    while let Some(&last) = path.last() {
        if last == si {
            break;
        }
        path.push(parent[last] as usize);
    }
    path.reverse();
    let mut length = 0.0;
    let mut thin = 0;
    let mut prev = from;
    for (k, &i) in path.iter().enumerate().skip(1) {
        let p = if k == path.len() - 1 { to } else { center(g.pos(i)) };
        length += dist(prev, p);
        prev = p;
        if k < path.len() - 1 && g.get(g.pos(i)) == Cell::Thin {
            thin += 1;
        }
    }
    if length > max_len {
        return None;
    }

    // откуда слышно: самая дальняя точка пути, которую ещё видно из головы
    let mut apparent = center(g.pos(path[0]));
    for &i in path.iter().skip(1) {
        let c = center(g.pos(i));
        if g.line_clear(from, c) {
            apparent = c;
        } else {
            break;
        }
    }
    Some(Route { apparent, length, thin })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Стена x = 5 от пола до потолка, в ней дверь на z = 6..=7 (высота 2), пол y = 0 и потолок y = 4.
    fn room(door: bool, glass: bool) -> impl FnMut(i32, i32, i32) -> Cell {
        move |x, y, z| {
            if y <= 0 || y >= 4 {
                return Cell::Solid;
            }
            if x == 5 {
                if door && (6..=7).contains(&z) && y <= 2 {
                    return Cell::Air;
                }
                if glass && z == 0 && y == 2 {
                    return Cell::Thin;
                }
                return Cell::Solid;
            }
            Cell::Air
        }
    }

    const ME: [f64; 3] = [2.5, 1.6, 0.5];
    const FRIEND: [f64; 3] = [8.5, 1.6, 0.5];

    #[test]
    fn voice_goes_around_through_the_door() {
        let r = find_route(ME, FRIEND, 48.0, room(true, false)).expect("через дверь можно дойти");
        let direct = dist(ME, FRIEND);
        assert!(r.length > direct + 4.0 && r.length < direct + 16.0, "путь {} при прямой {direct}", r.length);
        assert_eq!(r.thin, 0);
        // слышно со стороны двери: справа по +Z, у стены
        assert!(r.apparent[2] >= 5.0 && r.apparent[0] <= 6.5, "{:?}", r.apparent);
    }

    #[test]
    fn closed_room_has_no_route() {
        assert_eq!(find_route(ME, FRIEND, 48.0, room(false, false)), None);
    }

    #[test]
    fn too_long_detour_is_not_taken() {
        assert_eq!(find_route(ME, FRIEND, 8.0, room(true, false)), None);
    }

    #[test]
    fn window_glass_lets_voice_through_but_counts() {
        let r = find_route(ME, FRIEND, 48.0, room(false, true)).expect("сквозь стекло");
        assert_eq!(r.thin, 1);
        assert!(r.length < 8.0, "прямо сквозь окно, без обхода: {}", r.length);
    }

    #[test]
    fn no_leaking_through_block_corners() {
        // две сплошные клетки стоят «шахматкой»: по диагонали между ними не пролезть
        let mut q = |x: i32, y: i32, z: i32| {
            if y != 1 {
                return Cell::Solid;
            }
            if (x, z) == (1, 0) || (x, z) == (0, 1) {
                return Cell::Solid;
            }
            if (x, z) == (0, 0) || (x, z) == (1, 1) { Cell::Air } else { Cell::Solid }
        };
        assert_eq!(find_route([0.5, 1.5, 0.5], [1.5, 1.5, 1.5], 10.0, &mut q), None);
    }

    #[test]
    fn open_air_route_is_straight() {
        let r = find_route(ME, FRIEND, 48.0, |_, y, _| if y <= 0 { Cell::Solid } else { Cell::Air }).unwrap();
        assert!((r.length - dist(ME, FRIEND)).abs() < 1.5, "{}", r.length);
        assert!(dist(r.apparent, FRIEND) < 1.5, "в чистом поле слышно прямо от игрока");
    }
}
