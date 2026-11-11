//! Кто из игроков на сервере с голосом, кто говорит и у кого что выключено: по этому мод подписывает ники над головами.
//!
//! Данные приходят из TCP-пакетов Plasmo Voice: PlayerList (7), PlayerInfoUpdate (8) и SourceInfo (16).
//! Игроков ведём по нику в нижнем регистре: ник есть и в этих пакетах, и у сущности в игре.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::plasmo::Reader;

/// Сколько после последнего пакета голоса игрок ещё считается говорящим.
const TALK_HOLD: Duration = Duration::from_millis(350);
/// Как часто повторно спрашиваем у сервера, чей это источник.
const ASK_EVERY: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct VoicePlayer {
    /// UUID игрока: по нему сервер сообщает, что игрок вышел (PlayerDisconnect).
    pub uuid: u128,
    pub nick: String,
    /// Сервер запретил этому игроку говорить.
    pub muted: bool,
    /// Игрок выключил голосовой чат у себя.
    pub voice_disabled: bool,
    /// Игрок выключил свой микрофон.
    pub mic_muted: bool,
    /// Группа Simple Voice Chat, в которой игрок сейчас (у Plasmo не приходит).
    pub group: Option<u128>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Status {
    /// Ничего особенного: ник остаётся как в игре.
    Normal,
    Talking,
    Muted,
    Disabled,
    /// Список игроков с голосом получен, а этого игрока в нём нет.
    NoMod,
}

#[derive(Default)]
struct Roster {
    players: HashMap<String, VoicePlayer>,
    /// Пришёл ли полный список: пока нет, «без мода» не показываем, чтобы не ошибиться.
    listed: bool,
    /// Источник Plasmo (UUID) -> чей он и где звучит.
    sources: HashMap<u128, Src>,
    asked: HashMap<u128, Instant>,
    talking: HashMap<String, Instant>,
    /// Когда игрок последний раз говорил в группе (голос пришёл по линии группы).
    talking_group: HashMap<String, Instant>,
    /// Когда игрок последний раз говорил рядом (не в группу).
    talking_near: HashMap<String, Instant>,
    /// Наш UUID у Simple Voice Chat (из `voicechat:secret`): по нему узнаём свою группу.
    self_uuid: Option<u128>,
    /// Громкость голоса игрока прямо сейчас (RMS последнего пакета) и когда пришёл пакет.
    levels: HashMap<String, (f32, Instant)>,
}

static ROSTER: Mutex<Option<Roster>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Roster) -> R) -> Option<R> {
    let mut g = ROSTER.lock().ok()?;
    Some(f(g.get_or_insert_with(Roster::default)))
}

fn key(nick: &str) -> String {
    nick.to_lowercase()
}

pub fn reset() {
    if let Ok(mut g) = ROSTER.lock() {
        *g = None;
    }
}

fn read_uuid(r: &mut Reader) -> Option<u128> {
    Some(u128::from_be_bytes(r.bytes(16)?.try_into().ok()?))
}

/// VoicePlayerInfo: UUID игрока, ник, muted, voiceDisabled, microphoneMuted.
fn read_player(r: &mut Reader) -> Option<VoicePlayer> {
    let uuid = read_uuid(r)?;
    let nick = r.utf()?;
    Some(VoicePlayer { uuid, nick, muted: r.bool()?, voice_disabled: r.bool()?, mic_muted: r.bool()?, group: None })
}

/// PlayerListPacket (после байта типа): количество и игроки.
pub fn on_player_list(body: &[u8]) -> bool {
    let mut r = Reader::new(body);
    let Some(n) = r.i32().filter(|n| (0..=i32::from(i16::MAX)).contains(n)) else { return false };
    let mut list = Vec::new();
    for _ in 0..n {
        match read_player(&mut r) {
            Some(p) => list.push(p),
            None => return false,
        }
    }
    with(|ro| {
        ro.players = list.into_iter().map(|p| (key(&p.nick), p)).collect();
        ro.listed = true;
    });
    true
}

/// PlayerInfoUpdatePacket: один игрок, у которого что-то поменялось.
pub fn on_player_update(body: &[u8]) -> bool {
    let Some(p) = read_player(&mut Reader::new(body)) else { return false };
    with(|ro| {
        ro.players.insert(key(&p.nick), p);
    });
    true
}

/// PlayerDisconnectPacket (тип 9): игрок вышел. Убираем его отовсюду: из списка, «говорит», источников.
pub fn on_player_disconnect(body: &[u8]) -> bool {
    let Some(uuid) = read_uuid(&mut Reader::new(body)) else { return false };
    with(|ro| {
        let gone: Vec<String> = ro.players.iter().filter(|(_, p)| p.uuid == uuid).map(|(k, _)| k.clone()).collect();
        for k in &gone {
            ro.players.remove(k);
            ro.talking.remove(k);
        }
        ro.sources.retain(|_, s| !matches!(s, Src::Player { nick, .. } if gone.contains(&key(nick))));
    });
    true
}

/// Ник игрока по UUID (голос SVC приходит с UUID отправителя).
pub fn nick_by_uuid(uuid: u128) -> Option<String> {
    with(|ro| ro.players.values().find(|p| p.uuid == uuid).map(|p| p.nick.clone())).flatten()
}

fn from_svc(s: &crate::svc::PlayerState) -> VoicePlayer {
    VoicePlayer {
        uuid: u128::from_be_bytes(s.uuid),
        nick: s.name.clone(),
        muted: s.muted,
        voice_disabled: s.disabled,
        mic_muted: false,
        group: s.group.map(u128::from_be_bytes),
    }
}

/// SVC `voicechat:states`: весь список. Кто «не подключён» к голосу — для нас как без мода.
pub fn on_svc_states(list: &[crate::svc::PlayerState]) {
    with(|ro| {
        ro.players = list.iter().filter(|s| !s.disconnected).map(|s| (key(&s.name), from_svc(s))).collect();
        ro.listed = true;
    });
}

/// SVC `voicechat:state`: один игрок поменялся (или отключился от голоса).
pub fn on_svc_state(s: &crate::svc::PlayerState) {
    with(|ro| {
        if s.disconnected {
            ro.players.remove(&key(&s.name));
            ro.talking.remove(&key(&s.name));
        } else {
            ro.players.insert(key(&s.name), from_svc(s));
        }
    });
}

/// SVC `voicechat:remove_state`: игрок ушёл.
pub fn on_svc_remove(uuid: [u8; 16]) {
    let id = u128::from_be_bytes(uuid);
    with(|ro| {
        let gone: Vec<String> = ro.players.iter().filter(|(_, p)| p.uuid == id).map(|(k, _)| k.clone()).collect();
        for k in gone {
            ro.players.remove(&k);
            ro.talking.remove(&k);
        }
    });
}

/// Источник звука Plasmo: голос игрока или звук мира (пластинка, моб, объявление сервера).
#[derive(Clone, Debug, PartialEq)]
pub enum Src {
    /// Голос игрока; `group` — по линии групп.
    Player { nick: String, group: bool },
    /// Звук из точки мира (например, пластинка в проигрывателе, pv-addon-discs).
    Static { pos: [f64; 3], name: Option<String> },
    /// Звук от сущности (по её id в мире).
    Entity { id: u32, name: Option<String> },
    /// Звук «прямо в уши», без места в мире; `sender` — кто прислал, если сервер сказал.
    Direct { sender: Option<String> },
}

/// SourceInfoPacket: запоминаем, чей это источник и где он звучит.
pub fn on_source_info(body: &[u8]) -> bool {
    parse_source_info(body).is_some_and(|(src, kind, player)| {
        with(|ro| {
            ro.sources.insert(src, kind);
            if let Some(p) = player {
                ro.players.entry(key(&p.nick)).or_insert(p);
            }
        });
        true
    })
}

/// Строка для лога о звуке мира из SourceInfo (`None` — это голос игрока).
pub fn world_source_note(body: &[u8]) -> Option<String> {
    match parse_source_info(body)?.1 {
        Src::Player { .. } => None,
        Src::Static { pos, name } => Some(format!(
            "звук мира в точке {:.0} {:.0} {:.0}{}",
            pos[0],
            pos[1],
            pos[2],
            name.map(|n| format!(" («{n}»)")).unwrap_or_default()
        )),
        Src::Entity { id, name } => Some(format!("звук сущности #{id}{}", name.map(|n| format!(" («{n}»)")).unwrap_or_default())),
        Src::Direct { sender } => Some(format!("прямой звук{}", sender.map(|n| format!(" от {n}")).unwrap_or_default())),
    }
}

/// Что известно об источнике (если сервер уже прислал SourceInfo).
pub fn source(src: [u8; 16]) -> Option<Src> {
    let id = u128::from_be_bytes(src);
    with(|ro| ro.sources.get(&id).cloned()).flatten()
}

fn parse_source_info(body: &[u8]) -> Option<(u128, Src, Option<VoicePlayer>)> {
    let mut r = Reader::new(body);
    let ty = r.utf()?;
    if !matches!(ty.as_str(), "PLAYER" | "STATIC" | "ENTITY" | "DIRECT") {
        return None;
    }
    r.skip_utf()?; // addonId
    let id = read_uuid(&mut r)?;
    let name = if r.bool()? { Some(r.utf()?) } else { None };
    r.u8()?; // state
    if r.bool()? {
        // CodecInfo: имя и параметры
        r.skip_utf()?;
        let n = r.i32().filter(|n| (0..=128).contains(n))?;
        for _ in 0..n {
            r.skip_utf()?;
            r.skip_utf()?;
        }
    }
    r.bool()?; // stereo
    let line = r.bytes(16)?; // lineId: голос группы идёт по линии `groups`
    let group = line == &crate::plasmo::line_id(GROUPS_LINE)[..];
    r.bool()?; // iconVisible
    r.i32()?; // angle
    let pos3 = |r: &mut Reader| Some([r.f64()?, r.f64()?, r.f64()?]);
    match ty.as_str() {
        "PLAYER" => {
            let p = read_player(&mut r)?;
            Some((id, Src::Player { nick: p.nick.clone(), group }, Some(p)))
        }
        "STATIC" => {
            let pos = pos3(&mut r)?;
            pos3(&mut r)?; // куда «смотрит» источник
            Some((id, Src::Static { pos, name }, None))
        }
        "ENTITY" => Some((id, Src::Entity { id: r.i32()? as u32, name }, None)),
        _ => {
            // DIRECT: профиль отправителя (uuid, ник, свойства), смещение, относительно ли камеры
            let sender = if r.bool()? {
                read_uuid(&mut r)?;
                let nick = r.utf()?;
                let n = r.i32().filter(|n| (0..=100).contains(n))?;
                for _ in 0..n * 3 {
                    r.skip_utf()?;
                }
                Some(nick)
            } else {
                None
            };
            Some((id, Src::Direct { sender }, None))
        }
    }
}

/// Линия Plasmo, по которой идёт голос групп (аддон pv-addon-groups).
pub const GROUPS_LINE: &str = "groups";

/// Наш UUID у Simple Voice Chat (из `voicechat:secret`).
pub fn set_self_uuid(uuid: u128) {
    with(|ro| ro.self_uuid = Some(uuid));
}

/// Группа Simple Voice Chat, в которой мы сейчас (если сервер её сообщил).
/// Ники игроков в группе SVC `group` (по их состояниям от сервера), по алфавиту.
pub fn group_members(group: u128) -> Vec<String> {
    let mut v: Vec<String> =
        with(|ro| ro.players.values().filter(|p| p.group == Some(group)).map(|p| p.nick.clone()).collect()).unwrap_or_default();
    v.sort_by_key(|n| n.to_lowercase());
    v
}

pub fn self_group() -> Option<u128> {
    with(|ro| ro.self_uuid.and_then(|me| ro.players.values().find(|p| p.uuid == me).and_then(|p| p.group))).flatten()
}

/// Игрок только что говорил рядом (обычным голосом).
pub fn mark_talking_near(nick: &str) {
    with(|ro| {
        ro.talking_near.insert(key(nick), Instant::now());
    });
}

/// Игрок только что говорил в группе.
pub fn mark_talking_group(nick: &str) {
    with(|ro| {
        ro.talking_group.insert(key(nick), Instant::now());
    });
}

/// Пора ли спросить у сервера про этот источник (не чаще раза в несколько секунд).
pub fn should_ask(src: [u8; 16]) -> bool {
    let id = u128::from_be_bytes(src);
    with(|ro| {
        let now = Instant::now();
        if ro.asked.get(&id).is_some_and(|t| now.duration_since(*t) < ASK_EVERY) {
            return false;
        }
        ro.asked.insert(id, now);
        true
    })
        .unwrap_or(false)
}

/// Игрок только что говорил (пришёл пакет его голоса).
pub fn mark_talking(nick: &str) {
    with(|ro| {
        ro.talking.insert(key(nick), Instant::now());
    });
}

/// Сколько после последнего голоса в группе игрок ещё считается «в группе», если сервер сам не сообщает.
const GROUP_HOLD: Duration = Duration::from_secs(30);

/// Громкость только что пришедшего голоса игрока (RMS, 0..1): для капсулы-индикатора над ником.
pub fn mark_level(nick: &str, rms: f32) {
    with(|ro| {
        ro.levels.insert(key(nick), (rms, Instant::now()));
    });
}

/// Громкость голоса игрока сейчас 0..=1 (шкала по децибелам: -50 дБ — пусто, -12 дБ — доверху).
/// Пакетов нет дольше 120 мс — 0.
pub fn voice_level(nick: &str) -> f32 {
    with(|ro| {
        ro.levels
            .get(&key(nick))
            .filter(|(_, at)| at.elapsed() < Duration::from_millis(120))
            .map_or(0.0, |(rms, _)| level_fraction(*rms))
    })
    .unwrap_or(0.0)
}

/// RMS -> доля 0..=1 по децибелам.
pub fn level_fraction(rms: f32) -> f32 {
    let db = 20.0 * rms.max(1e-6).log10();
    ((db + 50.0) / 38.0).clamp(0.0, 1.0)
}

/// Говорит ли игрок прямо сейчас (пришёл пакет его голоса недавно).
pub fn is_talking(nick: &str) -> bool {
    with(|ro| ro.talking.get(&key(nick)).is_some_and(|t| t.elapsed() < TALK_HOLD)).unwrap_or(false)
}

pub fn status(nick: &str) -> Status {
    with(|ro| {
        let k = key(nick);
        if ro.talking.get(&k).is_some_and(|t| t.elapsed() < TALK_HOLD) {
            return Status::Talking;
        }
        match ro.players.get(&k) {
            Some(p) if p.muted || p.mic_muted => Status::Muted,
            Some(p) if p.voice_disabled => Status::Disabled,
            Some(_) => Status::Normal,
            None if ro.listed => Status::NoMod,
            None => Status::Normal,
        }
    })
        .unwrap_or(Status::Normal)
}

/// Что рисовать над ником (два пиксельных значка, см. `nametag.rs`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Indicator {
    /// Микрофон и динамик: что у игрока выключено и говорит ли он сейчас.
    Voice { mic_off: bool, spk_off: bool, talking: bool },
    /// Список игроков с голосом получен, а этого игрока в нём нет.
    NoMod,
}

/// None — ничего особенного (молчит, всё включено): ник остаётся без значков.
pub fn indicator(nick: &str) -> Option<Indicator> {
    with(|ro| {
        let k = key(nick);
        let talking = ro.talking.get(&k).is_some_and(|t| t.elapsed() < TALK_HOLD);
        match ro.players.get(&k) {
            Some(p) => {
                let mic_off = p.muted || p.mic_muted;
                let spk_off = p.voice_disabled;
                (talking || mic_off || spk_off).then_some(Indicator::Voice { mic_off, spk_off, talking })
            }
            None if talking => Some(Indicator::Voice { mic_off: false, spk_off: false, talking: true }),
            None if ro.listed => Some(Indicator::NoMod),
            None => None,
        }
    })
        .flatten()
}

/// Строка списка игроков в меню.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuPlayer {
    pub nick: String,
    pub talking: bool,
    /// В нашей группе: говорил по линии группы недавно или SVC сказал, что он в той же группе, что и мы.
    pub group: bool,
    /// Прямо сейчас говорит рядом / в группу (бывает и то и другое сразу).
    pub talk_near: bool,
    pub talk_group: bool,
    /// Игрок выключил микрофон (или его заглушил сервер).
    pub mic_off: bool,
}

/// Игроки с голосом для вкладки «Игроки», по алфавиту. Порядок не зависит от того, кто говорит: иначе строка
/// ушла бы из-под мыши, пока тянешь ползунок.
pub fn menu_players() -> Vec<MenuPlayer> {
    let mut list = with(|ro| {
        let mine = ro.self_uuid.and_then(|me| ro.players.values().find(|p| p.uuid == me).and_then(|p| p.group));
        ro.players
            .iter()
            // кого уже нет в списке игроков сервера (вышел, а PlayerDisconnect не дошёл), не показываем
            .filter(|(_, p)| crate::faces::online(&p.nick) != Some(false))
            .map(|(k, p)| MenuPlayer {
                nick: p.nick.clone(),
                talking: ro.talking.get(k).is_some_and(|t| t.elapsed() < TALK_HOLD),
                group: ro.talking_group.get(k).is_some_and(|t| t.elapsed() < GROUP_HOLD)
                    || (mine.is_some() && p.group == mine),
                talk_near: ro.talking_near.get(k).is_some_and(|t| t.elapsed() < TALK_HOLD),
                talk_group: ro.talking_group.get(k).is_some_and(|t| t.elapsed() < TALK_HOLD),
                mic_off: p.muted || p.mic_muted,
            })
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
    list.sort_by_key(|p| p.nick.to_lowercase());
    list
}

/// Значок над ником: символ из шрифта игры и его цвет (ARGB). Цвет передаём отдельно, коды `§` тут не нужны.
pub fn badge(status: Status) -> Option<(&'static str, u32)> {
    match status {
        Status::Normal => None,
        Status::Talking => Some(("♪", 0xFF55_FF55)),
        Status::Muted => Some(("✖", 0xFFFF_5555)),
        Status::Disabled => Some(("✖", 0xFFFF_AA00)),
        Status::NoMod => Some(("?", 0xFFAA_AAAA)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LockExt;
    use std::sync::Mutex as TestLock;

    /// Хранилище глобальное, поэтому тесты идут по очереди.
    static ONE_AT_A_TIME: TestLock<()> = TestLock::new(());

    fn utf(out: &mut Vec<u8>, s: &str) {
        out.extend_from_slice(&(s.len() as u16).to_be_bytes());
        out.extend_from_slice(s.as_bytes());
    }
    fn player(out: &mut Vec<u8>, nick: &str, flags: [bool; 3]) {
        out.extend_from_slice(&[7u8; 16]);
        utf(out, nick);
        out.extend(flags.iter().map(|&b| b as u8));
    }

    #[test]
    fn list_marks_missing_players_as_no_mod() {
        let _g = ONE_AT_A_TIME.lock_ok();
        reset();
        assert_eq!(status("Steve"), Status::Normal, "до списка ничего не утверждаем");
        let mut b = 2i32.to_be_bytes().to_vec();
        player(&mut b, "Steve", [false, false, false]);
        player(&mut b, "Alex", [true, false, false]);
        assert!(on_player_list(&b));
        assert_eq!(status("steve"), Status::Normal);
        assert_eq!(status("ALEX"), Status::Muted);
        assert_eq!(status("Herobrine"), Status::NoMod);
    }

    #[test]
    fn update_changes_flags() {
        let _g = ONE_AT_A_TIME.lock_ok();
        reset();
        let mut b = Vec::new();
        player(&mut b, "Steve", [false, true, false]);
        assert!(on_player_update(&b));
        assert_eq!(status("Steve"), Status::Disabled);
    }

    #[test]
    fn source_info_maps_source_to_player_and_talking_expires() {
        let _g = ONE_AT_A_TIME.lock_ok();
        reset();
        let mut b = Vec::new();
        utf(&mut b, "PLAYER");
        utf(&mut b, "pv"); // addonId
        let src = [9u8; 16];
        b.extend_from_slice(&src);
        b.push(0); // имя не задано
        b.push(0); // state
        b.push(1); // есть CodecInfo
        utf(&mut b, "opus");
        b.extend_from_slice(&1i32.to_be_bytes());
        utf(&mut b, "mode");
        utf(&mut b, "VOIP");
        b.push(0); // stereo
        b.extend_from_slice(&[1u8; 16]); // lineId
        b.push(1); // iconVisible
        b.extend_from_slice(&0i32.to_be_bytes()); // angle
        player(&mut b, "Steve", [false, false, false]);
        assert!(on_source_info(&b));
        assert!(matches!(source(src), Some(Src::Player { ref nick, .. }) if nick == "Steve"));
        mark_talking("Steve");
        assert_eq!(status("steve"), Status::Talking);
        assert!(should_ask([5u8; 16]));
        assert!(!should_ask([5u8; 16]), "спрашиваем не чаще раза в несколько секунд");
    }

    #[test]
    fn group_line_marks_group_voice() {
        let _g = ONE_AT_A_TIME.lock_ok();
        reset();
        let info = |src: [u8; 16], line: [u8; 16], nick: &str| {
            let mut b = Vec::new();
            utf(&mut b, "PLAYER");
            utf(&mut b, "pv");
            b.extend_from_slice(&src);
            b.push(0); // имя не задано
            b.push(0); // state
            b.push(0); // без CodecInfo
            b.push(0); // stereo
            b.extend_from_slice(&line);
            b.push(1); // iconVisible
            b.extend_from_slice(&0i32.to_be_bytes());
            player(&mut b, nick, [false, false, false]);
            b
        };
        assert!(on_source_info(&info([1; 16], crate::plasmo::line_id(GROUPS_LINE), "Alex")));
        assert!(on_source_info(&info([2; 16], crate::plasmo::line_id("proximity"), "Steve")));
        assert!(matches!(source([1; 16]), Some(Src::Player { group: true, .. })));
        assert!(matches!(source([2; 16]), Some(Src::Player { group: false, .. })));
        mark_talking("Alex");
        mark_talking_group("Alex");
        mark_talking("Steve");
        let list = menu_players();
        let g = |n: &str| list.iter().find(|p| p.nick == n).map(|p| p.group);
        assert_eq!(g("Alex"), Some(true), "говорил в группе");
        assert_eq!(g("Steve"), Some(false));
    }

    #[test]
    fn world_sources_are_parsed() {
        let _g = ONE_AT_A_TIME.lock_ok();
        reset();
        let head = |ty: &str, src: u8, name: Option<&str>| {
            let mut b = Vec::new();
            utf(&mut b, ty);
            utf(&mut b, "pv-addon-discs");
            b.extend_from_slice(&[src; 16]);
            match name {
                Some(n) => {
                    b.push(1);
                    utf(&mut b, n);
                }
                None => b.push(0),
            }
            b.push(0); // state
            b.push(0); // без CodecInfo
            b.push(1); // stereo
            b.extend_from_slice(&crate::plasmo::line_id("discs"));
            b.push(1); // iconVisible
            b.extend_from_slice(&0i32.to_be_bytes());
            b
        };
        // пластинка в проигрывателе: точка + направление
        let mut st = head("STATIC", 1, Some("Track"));
        for v in [10.5f64, 64.0, -3.5, 0.0, 0.0, 1.0] {
            st.extend_from_slice(&v.to_be_bytes());
        }
        assert!(on_source_info(&st));
        assert_eq!(source([1; 16]), Some(Src::Static { pos: [10.5, 64.0, -3.5], name: Some("Track".into()) }));
        // сущность
        let mut en = head("ENTITY", 2, None);
        en.extend_from_slice(&77i32.to_be_bytes());
        assert!(on_source_info(&en));
        assert_eq!(source([2; 16]), Some(Src::Entity { id: 77, name: None }));
        // прямой: отправитель с одним свойством, без смещения
        let mut di = head("DIRECT", 3, None);
        di.push(1);
        di.extend_from_slice(&[9u8; 16]);
        utf(&mut di, "Admin");
        di.extend_from_slice(&1i32.to_be_bytes());
        for t in ["textures", "v", "sig"] {
            utf(&mut di, t);
        }
        di.push(0); // без смещения
        di.push(0); // не относительно камеры
        assert!(on_source_info(&di));
        assert_eq!(source([3; 16]), Some(Src::Direct { sender: Some("Admin".into()) }));
        assert!(!on_source_info(&head("UNKNOWN", 4, None)));
    }

    #[test]
    fn svc_same_group_counts_as_group() {
        let _g = ONE_AT_A_TIME.lock_ok();
        reset();
        let st = |uuid: u8, name: &str, group: Option<[u8; 16]>| crate::svc::PlayerState {
            uuid: [uuid; 16],
            name: name.to_string(),
            muted: false,
            disabled: false,
            disconnected: false,
            group,
        };
        on_svc_states(&[st(1, "me", Some([7; 16])), st(2, "Alex", Some([7; 16])), st(3, "Steve", None), st(4, "Bob", Some([8; 16]))]);
        assert_eq!(self_group(), None, "пока не знаем свой UUID");
        set_self_uuid(u128::from_be_bytes([1; 16]));
        assert_eq!(self_group(), Some(u128::from_be_bytes([7; 16])));
        let list = menu_players();
        let g = |n: &str| list.iter().find(|p| p.nick == n).map(|p| p.group);
        assert_eq!(g("Alex"), Some(true));
        assert_eq!(g("Steve"), Some(false));
        assert_eq!(g("Bob"), Some(false), "чужая группа — не наша");
    }

    #[test]
    fn broken_packets_are_rejected() {
        let _g = ONE_AT_A_TIME.lock_ok();
        assert!(!on_player_list(&[0, 0, 0, 5, 1]));
        assert!(!on_player_update(&[1, 2, 3]));
        assert!(!on_source_info(&[0, 6, b'P']));
    }

    #[test]
    fn indicator_shows_mic_and_speaker_states() {
        let _g = ONE_AT_A_TIME.lock_ok();
        reset();
        assert_eq!(indicator("Steve"), None, "до списка ничего не утверждаем");
        let mut b = 3i32.to_be_bytes().to_vec();
        player(&mut b, "Steve", [false, false, false]);
        player(&mut b, "Alex", [false, false, true]);
        player(&mut b, "Bob", [false, true, false]);
        assert!(on_player_list(&b));
        assert_eq!(indicator("steve"), None, "всё включено и молчит: значков нет");
        assert_eq!(indicator("ALEX"), Some(Indicator::Voice { mic_off: true, spk_off: false, talking: false }));
        assert_eq!(indicator("bob"), Some(Indicator::Voice { mic_off: false, spk_off: true, talking: false }));
        assert_eq!(indicator("Herobrine"), Some(Indicator::NoMod));
        mark_talking("Steve");
        assert_eq!(indicator("Steve"), Some(Indicator::Voice { mic_off: false, spk_off: false, talking: true }));
    }

    #[test]
    fn disconnect_removes_player() {
        let _g = ONE_AT_A_TIME.lock_ok();
        reset();
        let mut b = 2i32.to_be_bytes().to_vec();
        player(&mut b, "Steve", [false, false, false]);
        b.extend_from_slice(&[8u8; 16]); // у Alex другой UUID
        utf(&mut b, "Alex");
        b.extend_from_slice(&[0, 0, 0]);
        assert!(on_player_list(&b));
        mark_talking("Alex");
        assert!(on_player_disconnect(&[8u8; 16]));
        let names: Vec<String> = menu_players().into_iter().map(|p| p.nick).collect();
        assert_eq!(names, ["Steve"]);
        assert!(!is_talking("Alex"));
        assert!(!on_player_disconnect(&[1, 2]), "битый пакет");
    }

    #[test]
    fn voice_level_follows_loudness_and_expires() {
        let _g = ONE_AT_A_TIME.lock_ok();
        reset();
        assert_eq!(voice_level("Steve"), 0.0);
        mark_level("Steve", 0.01); // шёпот, около -40 дБ
        let quiet = voice_level("steve");
        mark_level("Steve", 0.3); // крик
        let loud = voice_level("STEVE");
        assert!(quiet > 0.0 && quiet < 0.3 && loud > 0.95, "{quiet} {loud}");
        assert_eq!(level_fraction(0.0), 0.0);
    }

    #[test]
    fn svc_states_fill_the_roster() {
        use crate::svc::PlayerState;
        let _g = ONE_AT_A_TIME.lock_ok();
        reset();
        let st = |name: &str, id: u8, muted: bool, disabled: bool, disconnected: bool| PlayerState {
            uuid: [id; 16],
            name: name.into(),
            muted,
            disabled,
            disconnected,
            group: None,
        };
        on_svc_states(&[st("Steve", 1, true, false, false), st("Alex", 2, false, true, false), st("Bob", 3, false, false, true)]);
        assert_eq!(status("steve"), Status::Muted);
        assert_eq!(status("alex"), Status::Disabled);
        assert_eq!(status("bob"), Status::NoMod, "не подключён к голосу — как без мода");
        assert_eq!(nick_by_uuid(u128::from_be_bytes([1; 16])).as_deref(), Some("Steve"));
        on_svc_state(&st("Bob", 3, false, false, false));
        assert_eq!(status("bob"), Status::Normal, "подключился");
        on_svc_remove([1; 16]);
        assert_eq!(status("steve"), Status::NoMod, "ушёл");
    }

    #[test]
    fn badges() {
        assert_eq!(badge(Status::Normal), None);
        assert_eq!(badge(Status::Talking), Some(("♪", 0xFF55_FF55)));
    }

    #[test]
    fn menu_players_are_alphabetical_and_stable_while_talking() {
        let _g = ONE_AT_A_TIME.lock_ok();
        reset();
        let mut b = 3i32.to_be_bytes().to_vec();
        player(&mut b, "zed", [false, false, false]);
        player(&mut b, "Alex", [false, false, true]);
        player(&mut b, "bob", [false, false, false]);
        assert!(on_player_list(&b));
        let names: Vec<String> = menu_players().into_iter().map(|p| p.nick).collect();
        assert_eq!(names, ["Alex", "bob", "zed"]);
        mark_talking("zed");
        let list = menu_players();
        assert_eq!(list.iter().map(|p| p.nick.as_str()).collect::<Vec<_>>(), ["Alex", "bob", "zed"]);
        assert!(list[2].talking);
        assert!(list.iter().find(|p| p.nick == "Alex").unwrap().mic_off);
    }
}
