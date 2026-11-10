//! Группы Plasmo Voice (аддон сервера pv-addon-groups) в меню мода.
//!
//! У самого Plasmo групп нет: их добавляет pv-addon-groups, и управляются они только командами чата
//! (`/groups create name: … password: …`, `/groups join <id> [пароль]`, `/groups leave`, `/groups invite <ник>`,
//! `/groups browse [страница]`). Меню клиента для них у авторов пока «coming soon», поэтому делаем своё:
//!
//! - список групп мод получает сам: шлёт `/groups browse` и читает ответ в чате (`on_chat`), а строки ответа
//!   прячет из чата. У каждой группы в ответе есть её UUID (клик по названию подставляет его), список
//!   участников (подсказка у числа игроков) и кнопка: «войти» (`/groups join <id>`; с паролем — подстановка
//!   `/groups join <id> ` вместо запуска) или «выйти» (`/groups leave` — значит, это ваша группа);
//! - аддон шлёт каждую группу несколькими сообщениями (название, игроки, кнопки) — их собирает `on_browse`;
//! - кнопки и поля меню (название, пароль) ставят команды в очередь, `tick` отправляет их на сервер (из меню
//!   отправлять нельзя: там нет изменяемого доступа к игре);
//! - ответ аддона на кнопку («Вы вошли в группу …», ошибка) меню показывает у себя, а из чата убирает, пока меню
//!   открыто.
//!
//! Тексты сервер переводит сам, поэтому разбор опирается не на слова, а на команды в кликах.
//!
//! На серверах Simple Voice Chat (и сервере голоса Horizon) группы — часть протокола: сервер сам присылает
//! группы (`voicechat:add_group` / `remove_group`) и где вы сейчас (`joined_group`), клиент шлёт «создать»,
//! «войти» и «выйти» (`svc.rs`). Меню то же, только вместо команд чата — эти пакеты (`Action::Svc`).

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering::Relaxed};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use horizon_client::text::{ClickEvent, Text};

/// Группа из ответа `/groups browse`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    /// UUID группы (как его ждёт `/groups join`).
    pub id: String,
    pub name: String,
    /// Текст сервера про игроков («3 игрока, владелец Alex»), как есть.
    pub info: String,
    /// Кто в группе (из подсказки), через запятую на сервере.
    pub members: Vec<String>,
    /// С паролем: вход через чат, чтобы ввести пароль.
    pub locked: bool,
    /// Ваша группа (у неё кнопка «выйти»).
    pub mine: bool,
}

/// Что сделать в `tick` (там есть доступ к игре на запись).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Отправить команду серверу (без `/`).
    Command(String),
    /// Пакет Simple Voice Chat: канал и данные.
    Svc(&'static str, Vec<u8>),
}

/// Чьи группы на сервере.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// pv-addon-groups (Plasmo Voice): команды чата.
    Plasmo,
    /// Simple Voice Chat: пакеты протокола.
    Svc,
}

fn backend() -> Backend {
    if crate::settings::protocol().contains("Simple Voice Chat") { Backend::Svc } else { Backend::Plasmo }
}

/// Группа SVC как её прислал сервер.
fn svc_id(id: &[u8; 16]) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}

fn svc_id_bytes(s: &str) -> Option<[u8; 16]> {
    if s.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

struct State {
    list: Vec<Group>,
    /// Пока открыто это окно, строки ответа `/groups browse` прячем из чата и собираем.
    window: Option<Instant>,
    /// Первая строка ответа уже пришла: дальше окно короткое и сдвигается с каждой строкой.
    answered: bool,
    collecting: Vec<Group>,
    /// Группа, которую собираем из строк ответа (аддон шлёт её несколькими сообщениями: название, игроки,
    /// кнопки). Готова, когда пришли кнопки.
    partial: Option<Group>,
    /// Какую страницу ответа ждём (по кнопкам листания).
    page: u32,
    /// В ответе уже был разделитель: аддон начинает им ответ, только после него «чужая» строка — ответ сервера
    /// («групп нет»), а не сообщение другого игрока.
    divider: bool,
    /// Прошлая строка ответа была разделителем: следующая без кнопок после групп — «Страница 1/1», конец ответа.
    after_divider: bool,
    /// Когда список последний раз пришёл (None — ещё ни разу).
    fetched: Option<Instant>,
    /// Попросить список заново на ближайшем `tick`.
    refresh_at: Option<Instant>,
    actions: Vec<Action>,
    /// После нашей команды (вошли, создали, вышли, позвали): первая строка ответа — это ответ аддона,
    /// её показываем в меню. Окно и уже взятый ответ.
    reply_window: Option<Instant>,
    reply_taken: bool,
    /// Последний ответ аддона для меню и когда пришёл.
    status: Option<(String, Instant)>,
    /// Группы SVC от сервера и в какой вы (по `joined_group`).
    svc_groups: Vec<crate::svc::SvcGroup>,
    svc_mine: Option<[u8; 16]>,
    /// Сервер хоть раз сказал, в какой мы группе (joined_group): дальше верим ему, а не списку игроков.
    svc_joined_known: bool,
    /// Пароли групп SVC, в которые входили из меню (чтобы вернуться после «Локального»).
    svc_pass: Vec<([u8; 16], String)>,
    /// Пароль группы, которую мы только что создали (id придёт в joined_group).
    svc_new_pass: Option<String>,
    /// «Локальный» в обычной группе SVC: временно вышли из группы, чтобы говорить только рядом.
    svc_detach: Detach,
    /// Ответ закончился, но аддон может дописать разделитель («групп нет» он обрамляет двумя): до этой
    /// минуты один разделитель ещё наш.
    tail_until: Option<Instant>,
    /// Приглашение в группу (pv-addon-groups), ещё не принятое и не отклонённое.
    invite: Option<Invite>,
    /// Последнее системное сообщение чата: у приглашения текст может прийти строкой раньше кнопки.
    last_sys: Option<(String, Instant)>,
    /// Когда последний раз просили список: опоздавший ответ (кнопки «Войти») — не приглашение.
    browse_at: Option<Instant>,
}

static STATE: Mutex<State> = Mutex::new(State {
    list: Vec::new(),
    window: None,
    answered: false,
    collecting: Vec::new(),
    partial: None,
    page: 1,
    divider: false,
    after_divider: false,
    fetched: None,
    refresh_at: None,
    actions: Vec::new(),
    reply_window: None,
    reply_taken: false,
    status: None,
    svc_groups: Vec::new(),
    svc_mine: None,
    svc_joined_known: false,
    svc_pass: Vec::new(),
    svc_new_pass: None,
    svc_detach: Detach::Attached,
    tail_until: None,
    invite: None,
    last_sys: None,
    browse_at: None,
});

/// Сколько ждём ответ на `/groups browse` (и его следующую страницу).
const WINDOW: Duration = Duration::from_millis(2500);
/// Сколько ждать первую строку ответа: сервер под нагрузкой отвечает не сразу.
const FIRST_REPLY: Duration = Duration::from_secs(8);
/// Сколько ждём ответ на команду кнопки.
const REPLY_WINDOW: Duration = Duration::from_millis(3000);
/// Сколько после конца ответа ждём хвостовой разделитель.
const TAIL: Duration = Duration::from_millis(800);
/// Сколько ответ виден в меню.
const STATUS_FOR: Duration = Duration::from_secs(6);
/// Больше страниц не листаем.
const MAX_PAGES: u32 = 5;
/// После действия (вошли, вышли, создали) список обновляется через это время: сервер успеет ответить.
const AFTER_ACTION: Duration = Duration::from_millis(900);
/// Длина названия и пароля в полях меню.
pub const NAME_MAX: usize = 24;
pub const PASS_MAX: usize = 24;

fn lock() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Группы SVC для меню: участники — по состояниям игроков от сервера.
fn svc_list(st: &State) -> Vec<Group> {
    let known = st.svc_joined_known || !matches!(st.svc_detach, Detach::Attached);
    let mine = st.svc_mine.or_else(|| if known { None } else { crate::roster::self_group().map(u128::to_be_bytes) });
    st.svc_groups
        .iter()
        .filter(|g| !g.hidden || Some(g.id) == mine)
        .map(|g| {
            let members = crate::roster::group_members(u128::from_be_bytes(g.id));
            let is_mine = Some(g.id) == mine;
            Group {
                id: svc_id(&g.id),
                name: g.name.clone(),
                info: members.join(", "),
                members,
                locked: g.has_password && !is_mine,
                mine: is_mine,
            }
        })
        .collect()
}

/// Группы на сервере (последний полученный список).
pub fn list() -> Vec<Group> {
    let st = lock();
    match backend() {
        Backend::Svc => svc_list(&st),
        Backend::Plasmo => st.list.clone(),
    }
}

/// Ваша группа, если вы в ней.
pub fn mine() -> Option<Group> {
    list().into_iter().find(|g| g.mine)
}

/// Можно выбрать тип группы при создании (SVC).
pub fn has_kinds() -> bool {
    backend() == Backend::Svc
}

/// Можно звать игроков в группу (у SVC такой команды нет: в группу входят сами).
pub fn can_invite() -> bool {
    backend() == Backend::Plasmo
}

/// Список нужно запрашивать (Plasmo) — у SVC сервер присылает его сам.
pub fn needs_refresh() -> bool {
    backend() == Backend::Plasmo
}

/// Список уже приходил хоть раз.
pub fn fetched() -> bool {
    backend() == Backend::Svc || lock().fetched.is_some()
}

/// Идёт запрос списка.
pub fn loading() -> bool {
    let st = lock();
    backend() == Backend::Plasmo && (st.window.is_some() || st.refresh_at.is_some())
}

/// SVC: сервер прислал группу (новую или изменённую).
pub fn on_svc_add_group(g: crate::svc::SvcGroup) {
    let mut st = lock();
    st.svc_groups.retain(|x| x.id != g.id);
    st.svc_groups.push(g);
}

/// SVC: группы больше нет.
pub fn on_svc_remove_group(id: [u8; 16]) {
    let mut st = lock();
    st.svc_groups.retain(|x| x.id != id);
    if st.svc_mine == Some(id) {
        st.svc_mine = None;
    }
}

/// SVC: в какой вы теперь группе (`wrong_password` — пароль не подошёл).
pub fn on_svc_joined(group: Option<[u8; 16]>, wrong_password: bool) {
    let mut st = lock();
    st.svc_joined_known = true;
    // ответы на наш временный выход ради «Локального» и на возвращение: в меню вы всё это время в группе
    match st.svc_detach {
        Detach::Leaving { id, .. } if group.is_none() => {
            st.svc_detach = Detach::Detached { id, released: None };
            SVC_ROUTE.store(ROUTE_LOCAL, Relaxed);
            return;
        }
        Detach::Rejoining { id, .. } => {
            st.svc_detach = Detach::Attached;
            SVC_ROUTE.store(ROUTE_NORMAL, Relaxed);
            if group == Some(id) {
                return;
            }
            if group.is_none() {
                st.svc_mine = None;
                st.status = Some(("Не удалось вернуться в группу".to_string(), Instant::now()));
                return;
            }
        }
        _ => {}
    }
    let was = st.svc_mine;
    st.svc_mine = group;
    if let (Some(id), Some(p)) = (group, st.svc_new_pass.take()) {
        st.svc_pass.retain(|(x, _)| *x != id);
        st.svc_pass.push((id, p));
    }
    let msg = if wrong_password {
        Some("Неверный пароль")
    } else if group.is_some() && group != was {
        Some("Вы вошли в группу")
    } else if group.is_none() && was.is_some() {
        Some("Вы вышли из группы")
    } else {
        None
    };
    if let Some(m) = msg {
        st.status = Some((m.to_string(), Instant::now()));
    }
}

// ---------- «Локальный» в обычной группе SVC ----------
//
// Сервер SVC отдаёт голос того, кто в группе, только группе, и спросить «это — только рядом» в протоколе
// нельзя. Поэтому в «обычной» группе, пока говорит «Локальный», мод на это время выходит из группы
// (leave_group) и потом возвращается (set_group с тем же паролем). Пока выход в пути, голос копится
// (voice.rs) и уходит, как только сервер подтвердит выход, — начало фразы не теряется.

/// Как voice.rs шлёт голос на сервер SVC.
pub const ROUTE_NORMAL: u8 = 0;
/// Выходим из группы или возвращаемся: голос ждёт (копится).
pub const ROUTE_TRANSIT: u8 = 1;
/// Временно вне группы: говорит «Локальный», только рядом.
pub const ROUTE_LOCAL: u8 = 2;
pub static SVC_ROUTE: AtomicU8 = AtomicU8::new(ROUTE_NORMAL);
/// voice.rs: в группе говорит «Локальный» (а «Группы» — нет).
pub static SVC_LOCAL_WANTED: AtomicBool = AtomicBool::new(false);
/// Из этой группы можно выйти на время «Локального».
pub static SVC_CAN_DETACH: AtomicBool = AtomicBool::new(false);

/// Замолчали в «Локальном» — через столько возвращаемся в группу (паузы в речи не дёргают группу).
const LOCAL_HANG: Duration = Duration::from_millis(1200);
/// Сервер не ответил на выход или возвращение за это время — сдаёмся.
const DETACH_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Detach {
    Attached,
    Leaving { id: [u8; 16], at: Instant },
    Detached { id: [u8; 16], released: Option<Instant> },
    Rejoining { id: [u8; 16], at: Instant },
}

fn svc_password(st: &State, id: &[u8; 16]) -> Option<String> {
    st.svc_pass.iter().find(|(g, _)| g == id).map(|(_, p)| p.clone())
}

/// Каждый тик (только SVC): выйти из группы на время «Локального» и вернуться.
pub fn svc_tick() {
    if backend() != Backend::Svc {
        SVC_ROUTE.store(ROUTE_NORMAL, Relaxed);
        SVC_CAN_DETACH.store(false, Relaxed);
        return;
    }
    let wanted = SVC_LOCAL_WANTED.load(Relaxed);
    let now = Instant::now();
    let mut st = lock();
    match st.svc_detach {
        Detach::Attached => {
            let group = st.svc_mine.and_then(|id| st.svc_groups.iter().find(|g| g.id == id).cloned());
            let Some(g) = group else {
                SVC_CAN_DETACH.store(false, Relaxed);
                return;
            };
            // только в «обычной» (в открытой вас и так слышат рядом, в изолированной рядом — никого);
            // вернуться можно, только зная пароль; кроме вас в группе никого — не выходим: она бы исчезла
            let normal = g.kind == crate::svc::GroupKind::Normal;
            let pass_known = !g.has_password || svc_password(&st, &g.id).is_some();
            let others = crate::roster::group_members(u128::from_be_bytes(g.id)).len() >= 2;
            let can = normal && pass_known && others;
            SVC_CAN_DETACH.store(can, Relaxed);
            if !can || !wanted {
                return;
            }
            st.actions.push(Action::Svc(crate::svc::CH_LEAVE_GROUP, Vec::new()));
            st.svc_detach = Detach::Leaving { id: g.id, at: now };
            SVC_ROUTE.store(ROUTE_TRANSIT, Relaxed);
        }
        Detach::Leaving { at, .. } | Detach::Rejoining { at, .. } if now.duration_since(at) > DETACH_TIMEOUT => {
            // сервер молчит: считаем, что мы там, где были
            st.svc_detach = Detach::Attached;
            SVC_ROUTE.store(ROUTE_NORMAL, Relaxed);
        }
        Detach::Detached { id, released } => {
            if wanted {
                st.svc_detach = Detach::Detached { id, released: None };
                return;
            }
            let since = released.unwrap_or(now);
            if released.is_none() {
                st.svc_detach = Detach::Detached { id, released: Some(now) };
            }
            if now.duration_since(since) >= LOCAL_HANG {
                let pass = svc_password(&st, &id);
                st.actions.push(Action::Svc(crate::svc::CH_SET_GROUP, crate::svc::set_group(&id, pass.as_deref())));
                st.svc_detach = Detach::Rejoining { id, at: now };
                SVC_ROUTE.store(ROUTE_TRANSIT, Relaxed);
            }
        }
        _ => {}
    }
}

/// Последний ответ аддона на кнопку меню («Вы вошли в группу …»), пока он свежий.
pub fn status() -> Option<String> {
    lock().status.as_ref().filter(|(_, t)| t.elapsed() < STATUS_FOR).map(|(s, _)| s.clone())
}

/// Обновить список (меню открыли или нажали «Обновить»).
pub fn refresh() {
    let mut st = lock();
    if st.window.is_none() {
        st.refresh_at = Some(Instant::now());
    }
}

/// Обновить, если список старше `age` (вкладку открыли снова).
pub fn refresh_if_older(age: Duration) {
    let stale = lock().fetched.is_none_or(|t| t.elapsed() >= age);
    if stale {
        refresh();
    }
}

fn command(st: &mut State, c: String) {
    st.actions.push(Action::Command(c));
    st.refresh_at = Some(Instant::now() + AFTER_ACTION);
}

/// Значение для команды аддона: без «:» (это разделитель флагов) и лишних пробелов, не длиннее `max`.
fn clean(s: &str, max: usize, spaces: bool) -> String {
    let s: String = s.chars().filter(|c| !c.is_control() && *c != ':' && (spaces || !c.is_whitespace())).take(max).collect();
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Войти в группу (`password` — для групп с паролем).
pub fn join(g: &Group, password: &str) {
    let mut st = lock();
    let pass = clean(password, PASS_MAX, false);
    if backend() == Backend::Svc {
        if let Some(id) = svc_id_bytes(&g.id) {
            let p = (!pass.is_empty()).then_some(pass.as_str());
            st.actions.push(Action::Svc(crate::svc::CH_SET_GROUP, crate::svc::set_group(&id, p)));
            st.svc_pass.retain(|(x, _)| *x != id);
            st.svc_pass.push((id, pass.clone()));
            st.svc_detach = Detach::Attached;
            SVC_ROUTE.store(ROUTE_NORMAL, Relaxed);
        }
        return;
    }
    let c = if g.locked && !pass.is_empty() { format!("groups join {} {pass}", g.id) } else { format!("groups join {}", g.id) };
    command(&mut st, c);
}

pub fn leave() {
    let mut st = lock();
    if backend() == Backend::Svc {
        // временно вне группы ради «Локального»: уже вышли, просто не возвращаемся
        if matches!(st.svc_detach, Detach::Detached { .. } | Detach::Leaving { .. }) {
            st.svc_detach = Detach::Attached;
            SVC_ROUTE.store(ROUTE_NORMAL, Relaxed);
            st.svc_mine = None;
            st.status = Some(("Вы вышли из группы".to_string(), Instant::now()));
            return;
        }
        st.svc_detach = Detach::Attached;
        SVC_ROUTE.store(ROUTE_NORMAL, Relaxed);
        st.actions.push(Action::Svc(crate::svc::CH_LEAVE_GROUP, Vec::new()));
        return;
    }
    command(&mut st, "groups leave".into());
}

/// Создать группу. Пустое название — не создаём (вернёт false).
/// `kind` — тип группы (только у SVC; у Plasmo аддон его не знает).
pub fn create(name: &str, password: &str, kind: crate::svc::GroupKind) -> bool {
    let name = clean(name, NAME_MAX, true);
    if name.is_empty() {
        return false;
    }
    let pass = clean(password, PASS_MAX, false);
    if backend() == Backend::Svc {
        let p = (!pass.is_empty()).then_some(pass.as_str());
        let data = crate::svc::create_group(&name, p, kind);
        let mut st = lock();
        st.actions.push(Action::Svc(crate::svc::CH_CREATE_GROUP, data));
        // пароль новой группы запомним, когда сервер скажет её id (joined_group)
        st.svc_new_pass = Some(pass.clone());
        return true;
    }
    let mut c = format!("groups create name: {name}");
    if !pass.is_empty() {
        c.push_str(&format!(" password: {pass}"));
    }
    let mut st = lock();
    command(&mut st, c);
    true
}

/// Позвать игрока в свою группу (только Plasmo).
pub fn invite(nick: &str) {
    if !valid_nick(nick) || !can_invite() {
        return;
    }
    let mut st = lock();
    st.actions.push(Action::Command(format!("groups invite {nick}")));
}

/// Ник Minecraft: латиница, цифры и «_», до 16 символов (в команду ничего лишнего не попадёт).
fn valid_nick(n: &str) -> bool {
    (1..=16).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Что сделать сейчас (`tick`): команды кнопок и запрос списка, когда пора.
pub fn take_actions() -> Vec<Action> {
    let mut st = lock();
    let now = Instant::now();
    if window_over(&st, now) {
        finish(&mut st);
    }
    let mut out = std::mem::take(&mut st.actions);
    if out.iter().any(|a| matches!(a, Action::Command(c) if !c.starts_with("groups browse"))) {
        st.reply_window = Some(now);
        st.reply_taken = false;
    }
    if st.window.is_none() && st.refresh_at.is_some_and(|t| now >= t) {
        st.refresh_at = None;
        st.window = Some(now);
        st.browse_at = Some(now);
        st.answered = false;
        st.collecting.clear();
        st.partial = None;
        st.page = 1;
        st.divider = false;
        st.after_divider = false;
        out.push(Action::Command("groups browse".into()));
    }
    out
}

/// Группа собрана (пришли кнопки): в список.
fn flush(st: &mut State) {
    if let Some(g) = st.partial.take() {
        st.collecting.retain(|x| x.id != g.id);
        st.collecting.push(g);
    }
}

/// Ответ закончился: список готов.
/// Окно ответа истекло: до первой строки ждём дольше, потом — WINDOW от последней.
fn window_over(st: &State, now: Instant) -> bool {
    let limit = if st.answered { WINDOW } else { FIRST_REPLY };
    st.window.is_some_and(|t| now.duration_since(t) >= limit)
}

fn finish(st: &mut State) {
    st.tail_until = Some(Instant::now() + TAIL);
    st.partial = None;
    st.window = None;
    st.list = std::mem::take(&mut st.collecting);
    st.fetched = Some(Instant::now());
}

/// Кнопка в строке группы.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Button {
    /// «Войти»: `locked` — с паролем (кнопка подставляет команду, а не запускает).
    Join { locked: bool },
    /// «Выйти»: это ваша группа.
    Leave,
}

/// Что есть в строке чата. Аддон шлёт группу несколькими сообщениями, поэтому строка может нести
/// только часть: название, или игроков, или кнопки.
#[derive(Debug, Default, PartialEq, Eq)]
struct Piece {
    /// UUID и название группы (клик по названию подставляет UUID).
    name: Option<(String, String)>,
    /// Перед названием значок с подсказкой (замок «защищена паролем»).
    icon: bool,
    /// Текст строки без названия и кнопок (например «Игроки: 1, владелец: Alex») и подсказка с участниками.
    info: String,
    members: Vec<String>,
    button: Option<Button>,
    /// Кнопки листания: `Some(next)` — есть следующая страница.
    pager: Option<Option<u32>>,
    /// Пустая или из чёрточек строка.
    divider: bool,
}

/// UUID в тексте: 36 символов, дефисы на своих местах.
/// UUID где угодно в сообщении: в кликах, в подсказке при наведении («Uuid: …») или в самом тексте.
fn find_uuid(t: &Text) -> Option<String> {
    let mut hay = String::new();
    for sp in &t.spans {
        hay.push(' ');
        hay.push_str(&sp.text);
        if let Some(c) = &sp.style.click {
            match c {
                ClickEvent::RunCommand(v) | ClickEvent::SuggestCommand(v) | ClickEvent::CopyToClipboard(v) => {
                    hay.push(' ');
                    hay.push_str(v);
                }
                _ => {}
            }
        }
        if let Some(h) = &sp.style.hover {
            hay.push(' ');
            hay.push_str(&plain(h));
        }
    }
    hay.split(|c: char| !(c.is_ascii_hexdigit() || c == '-')).find(|w| is_uuid(w)).map(str::to_string)
}

fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { *c == b'-' } else { c.is_ascii_hexdigit() })
}

fn click_command(c: &ClickEvent) -> Option<&str> {
    match c {
        ClickEvent::RunCommand(s) | ClickEvent::SuggestCommand(s) => Some(s.trim_start_matches('/')),
        _ => None,
    }
}

pub fn plain(t: &Text) -> String {
    t.spans.iter().map(|s| s.text.as_str()).collect()
}

/// Разобрать строку чата (`page` — какую страницу листаем сейчас).
/// Участники из подсказки: «Alex, Steve».
fn members_of(hover: &str) -> Vec<String> {
    hover.split(',').map(|m| m.trim().to_string()).filter(|m| !m.is_empty()).collect()
}

fn parse(t: &Text, page: u32) -> Piece {
    let mut p = Piece::default();
    let mut before_name: Option<(String, String)> = None;
    for s in &t.spans {
        let click = s.style.click.as_ref();
        if let Some(ClickEvent::SuggestCommand(v)) = click {
            if is_uuid(v.trim()) {
                let n = p.name.get_or_insert_with(|| (v.trim().to_string(), String::new()));
                n.1.push_str(&s.text);
                continue;
            }
        }
        match click.and_then(click_command) {
            Some(c) if c.starts_with("groups join ") => {
                p.button = Some(Button::Join { locked: matches!(click, Some(ClickEvent::SuggestCommand(_))) });
                continue;
            }
            Some("groups leave") => {
                p.button = Some(Button::Leave);
                continue;
            }
            Some(c) if c == "groups browse" || c.starts_with("groups browse ") => {
                let n = c.strip_prefix("groups browse").and_then(|n| n.trim().parse::<u32>().ok()).unwrap_or(1);
                let next = p.pager.get_or_insert(None);
                if n > page {
                    *next = Some(n);
                }
                continue;
            }
            Some(c) if c.starts_with("groups ") => continue, // «пригласить» и другие кнопки
            _ => {}
        }
        if let Some(h) = &s.style.hover {
            let hv = plain(h);
            if p.name.is_none() && !s.text.trim().is_empty() && p.info.trim().is_empty() {
                // возможно, значок перед названием (замок «защищена паролем»): решим, когда увидим, есть ли название
                before_name = Some((s.text.clone(), hv));
                continue;
            }
            if p.members.is_empty() && !hv.trim().is_empty() && !hv.contains('/') {
                p.members = members_of(&hv);
            }
        }
        if p.name.is_some() || !s.text.trim().is_empty() {
            p.info.push_str(&s.text);
        }
    }
    if let Some((text, hv)) = before_name {
        if p.name.is_some() {
            p.icon = true;
        } else {
            // названия нет: это строка игроков («Игроки: 1, владелец: Alex» с подсказкой-списком)
            p.info = format!("{text}{}", p.info);
            if p.members.is_empty() && !hv.contains('/') {
                p.members = members_of(&hv);
            }
        }
    }
    if let Some(n) = &mut p.name {
        n.1 = n.1.trim().to_string();
    }
    p.info = p.info.trim().to_string();
    let text = plain(t);
    p.divider = p.name.is_none()
        && p.button.is_none()
        && p.pager.is_none()
        && text.trim().chars().all(|c| c.is_whitespace() || "-—–─━=_▬·•~*".contains(c));
    p
}

impl Piece {
    /// В строке есть что-то от группы.
    fn is_group_part(&self) -> bool {
        self.name.is_some() || self.button.is_some()
    }
}

/// Строка в окне ответа на `/groups browse`. true — наша, спрятать.
fn on_browse(st: &mut State, p: Piece) -> bool {
    let after_divider = std::mem::replace(&mut st.after_divider, p.divider);
    // «Страница 1/1» (одна страница — без кнопок листания): после групп, сразу за разделителем
    if after_divider
        && !p.divider
        && !p.is_group_part()
        && p.pager.is_none()
        && st.partial.is_none()
        && !st.collecting.is_empty()
    {
        finish(st);
        return true;
    }
    if let Some((id, name)) = p.name.clone() {
        // новая группа: прошлая, если её кнопки так и не пришли, не считается
        st.partial = Some(Group { id, name, info: String::new(), members: Vec::new(), locked: p.icon, mine: false });
    }
    if let Some(g) = st.partial.as_mut() {
        if !p.info.is_empty() && p.name.is_none() && g.info.is_empty() {
            g.info = p.info.clone();
        } else if p.name.is_some() && !p.info.is_empty() {
            g.info = p.info.clone();
        }
        if g.members.is_empty() {
            g.members = p.members.clone();
        }
        match p.button {
            Some(Button::Join { locked }) => {
                g.locked |= locked;
                flush(st);
                return true;
            }
            Some(Button::Leave) => {
                g.mine = true;
                g.locked = false;
                flush(st);
                return true;
            }
            None => {}
        }
        if p.name.is_some() || !p.info.is_empty() || !p.members.is_empty() {
            return true;
        }
    }
    if p.divider {
        st.partial = None;
        st.divider = true;
        return true;
    }
    match p.pager {
        Some(Some(next)) if next <= MAX_PAGES => {
            // ещё страница: просим её на ближайшем tick, окно продлеваем
            st.page = next;
            st.window = Some(Instant::now());
            st.browse_at = Some(Instant::now());
            st.answered = false;
            st.actions.push(Action::Command(format!("groups browse {next}")));
            return true;
        }
        Some(_) => {
            finish(st);
            return true;
        }
        None => {}
    }
    // «групп нет»: идёт сразу после разделителя, групп ещё не было. Иначе это чужое сообщение — не трогаем.
    if st.collecting.is_empty() && st.partial.is_none() && st.divider {
        finish(st);
        return true;
    }
    false
}

/// Пришла строка чата. true — это ответ аддона на наш запрос или кнопку: её из чата убрать.
/// `menu_open` — меню видно: ответы на кнопки показываем в нём, а из чата убираем.
pub fn on_chat(t: &Text, menu_open: bool) -> bool {
    let mut st = lock();
    let now = Instant::now();
    let p = parse(t, st.page);
    let prev_sys = st.last_sys.replace((plain(t).trim().to_string(), now));
    // Приглашение в группу (pv-addon-groups): разделитель, «X пригласил(а) вас в группу [название]»
    // (название — с UUID в клике), иногда кнопка «Присоединиться», разделитель. Когда мы сами ничего не
    // спрашивали, весь этот блок прячем, а вместо него — плашка «Принять» / «Отказаться» (`draw_invite`).
    let ours_now = st.window.is_some()
        || st.browse_at.is_some_and(|b| now.duration_since(b) < LATE_BROWSE)
        || st.reply_window.is_some_and(|w| now.duration_since(w) < REPLY_WINDOW);
    if !ours_now {
        // рамка: строка из чёрточек или зачёркнутая (как `&8&m-----` у аддона); пустые строки других плагинов не трогаем
        let dashes = p.divider && (!plain(t).trim().is_empty() || t.spans.iter().any(|s| s.style.strikethrough == Some(true)));
        if dashes {
            // разделители печатает только аддон групп: вне наших запросов они — рамка приглашения
            st.tail_until = Some(now + INVITE_TAIL);
            return true;
        }
        // строка внутри рамки (сразу после спрятанного разделителя) или с названием группы — текст приглашения
        let in_frame = st.tail_until.is_some_and(|u| now < u) && !p.divider && st.invite.is_none();
        let named = p.name.is_some() && !p.info.trim().is_empty() && p.members.is_empty();
        if (named || in_frame) && !plain(t).trim().is_empty() {
            let text = plain(t).trim().to_string();
            let from = inviter(&text);
            let id = p.name.as_ref().map(|(id, _)| id.clone()).or_else(|| find_uuid(t));
            let cmd = join_command(t).or_else(|| id.as_ref().map(|id| format!("groups join {id}")));
            let id = id.unwrap_or_default();
            let cmd = cmd.unwrap_or_default();
            st.tail_until = Some(now + INVITE_TAIL);
            if crate::settings::invite_accepted(from.as_deref()) {
                let group_id = (!id.is_empty()).then_some(id);
                st.invite = Some(Invite { command: cmd, group_id, text, from, at: now });
            }
            return true;
        }
        if let Some(cmd) = join_command(t).filter(|_| p.name.is_none()) {
            // кнопка «Присоединиться» отдельной строкой: к уже показанному приглашению или своё
            let tail = st.tail_until.is_some_and(|u| now < u);
            match st.invite.as_mut().filter(|_| tail) {
                Some(inv) => inv.command = cmd,
                None if tail => {} // приглашение заблокировано: кнопку тоже прячем
                None => {
                    let text = prev_sys.filter(|(_, at)| now.duration_since(*at) < INVITE_TEXT_GAP).map(|(s, _)| s).unwrap_or_default();
                    let from = inviter(&text);
                    let id = cmd.split_whitespace().nth(2).filter(|s| is_uuid(s)).map(str::to_string);
                    if crate::settings::invite_accepted(from.as_deref()) {
                        st.invite = Some(Invite { command: cmd, group_id: id, text, from, at: now });
                    }
                }
            }
            st.tail_until = Some(now + INVITE_TAIL);
            return true;
        }
    }
    // хвостовой разделитель ответа, который уже закончился
    if st.window.is_none() && p.divider && st.tail_until.is_some_and(|u| now < u) {
        st.tail_until = None;
        return true;
    }
    // ответ на кнопку («Вы вошли в группу …»): первая строка после команды, не часть списка
    if st.reply_window.is_some_and(|w| now.duration_since(w) < REPLY_WINDOW) {
        if !st.reply_taken && !p.divider && !p.is_group_part() && p.pager.is_none() && !p.info.is_empty() {
            st.reply_taken = true;
            st.status = Some((plain(t).trim().to_string(), now));
            return menu_open;
        }
        // создание группы следом печатает её саму: это тоже ответ кнопке, а не список
        if st.window.is_none() && (p.divider || p.is_group_part() || (!p.info.is_empty() && !p.members.is_empty())) {
            return menu_open;
        }
    }
    if st.window.is_none() {
        return false;
    }
    if window_over(&st, now) {
        finish(&mut st);
        return false;
    }
    let page = st.page;
    let ours = on_browse(&mut st, p);
    // строка наша: окно отсчитываем от неё (кроме запроса следующей страницы — там снова ждём первую)
    if ours && st.window.is_some() && st.page == page {
        st.answered = true;
        st.window = Some(now);
    }
    ours
}

// ---------- приглашения ----------

/// Приглашение в группу от pv-addon-groups.
#[derive(Clone, Debug)]
struct Invite {
    /// Команда кнопки «Присоединиться» (без `/`), как её прислал сервер.
    command: String,
    group_id: Option<String>,
    /// Текст приглашения от сервера («AniLiaSS пригласил(а) вас в группу тестер»), на языке сервера.
    text: String,
    /// Кто позвал (ник из текста), если узнали.
    from: Option<String>,
    at: Instant,
}

/// Рамка приглашения (разделители и кнопка) приходит следом за текстом: столько её ещё прячем.
const INVITE_TAIL: Duration = Duration::from_millis(1200);

/// Кто позвал: ник игрока с голосом из текста приглашения (самый длинный, что нашёлся), иначе первое слово.
fn inviter(text: &str) -> Option<String> {
    let low = text.to_lowercase();
    let me = crate::settings::self_name().to_lowercase();
    let known = crate::roster::menu_players()
        .into_iter()
        .map(|p| p.nick)
        .filter(|n| n.to_lowercase() != me && low.contains(&n.to_lowercase()))
        .max_by_key(|n| n.len());
    known.or_else(|| {
        text.split_whitespace()
            .next()
            .map(|w| w.trim_matches(|c: char| !(c.is_alphanumeric() || c == '_')).to_string())
            .filter(|w| (3..=16).contains(&w.len()) && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
    })
}

/// Сколько висит приглашение, если на него не ответили.
pub const INVITE_FOR: Duration = Duration::from_secs(30);
/// После запроса списка столько строк с «Войти» считаем опоздавшим ответом, а не приглашением.
const LATE_BROWSE: Duration = Duration::from_secs(10);
/// Текст приглашения может прийти отдельной строкой прямо перед кнопкой.
const INVITE_TEXT_GAP: Duration = Duration::from_millis(1500);

/// Команда `groups join …` из кликов сообщения.
fn join_command(t: &Text) -> Option<String> {
    t.spans.iter().find_map(|s| {
        s.style.click.as_ref().and_then(click_command).filter(|c| c.starts_with("groups join ")).map(|c| c.trim().to_string())
    })
}

/// Текущее приглашение: (текст, название группы, если известно, сколько уже висит 0..1).
pub fn pending_invite() -> Option<(String, Option<String>, f32)> {
    let mut st = lock();
    let inv = st.invite.clone()?;
    let age = inv.at.elapsed();
    if age >= INVITE_FOR || !crate::settings::invites_on() {
        st.invite = None;
        return None;
    }
    let name = inv.group_id.as_ref().and_then(|id| st.list.iter().find(|g| &g.id == id).map(|g| g.name.clone()));
    Some((inv.text, name, age.as_secs_f32() / INVITE_FOR.as_secs_f32()))
}

/// Кто позвал в текущем приглашении.
pub fn invite_from() -> Option<String> {
    lock().invite.as_ref().and_then(|i| i.from.clone())
}


/// Принять приглашение: та же команда, что у кнопки в чате.
pub fn accept_invite() {
    let mut st = lock();
    let Some(inv) = st.invite.take() else { return };
    let mut cmd = inv.command;
    if cmd.is_empty() {
        // сервер не дал UUID: ищем группу по названию из текста («… в группу [название]») в списке
        let name = inv.text.rsplit_once('[').and_then(|(_, r)| r.split_once(']')).map(|(n, _)| n.trim().to_lowercase());
        if let Some(g) = name.and_then(|n| st.list.iter().find(|g| g.name.to_lowercase() == n).cloned()) {
            cmd = format!("groups join {}", g.id);
        }
    }
    if cmd.is_empty() {
        st.status = Some(("Не нашёл эту группу — войдите из списка".to_string(), Instant::now()));
        return;
    }
    command(&mut st, cmd);
}

pub fn decline_invite() {
    lock().invite = None;
}

/// Сервер сменился: старый список не годится.
pub fn reset() {
    let mut st = lock();
    st.invite = None;
    st.last_sys = None;
    st.browse_at = None;
    st.list.clear();
    st.collecting.clear();
    st.partial = None;
    st.window = None;
    st.fetched = None;
    st.refresh_at = None;
    st.actions.clear();
    st.reply_window = None;
    st.status = None;
    st.tail_until = None;
    st.svc_groups.clear();
    st.svc_mine = None;
    st.svc_joined_known = false;
    st.svc_pass.clear();
    st.svc_new_pass = None;
    st.svc_detach = Detach::Attached;
    SVC_ROUTE.store(ROUTE_NORMAL, Relaxed);
    SVC_LOCAL_WANTED.store(false, Relaxed);
    SVC_CAN_DETACH.store(false, Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_client::text::{Span, Style};

    fn span(text: &str, click: Option<ClickEvent>, hover: Option<&str>) -> Span {
        Span {
            text: text.into(),
            style: Style { click, hover: hover.map(|h| Box::new(Text::literal(h))), ..Style::default() },
        }
    }

    fn text(spans: Vec<Span>) -> Text {
        let mut t = Text::literal("");
        t.spans = spans;
        t
    }

    const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
    const ID2: &str = "1b4e28ba-2fa1-11d2-883f-0016d3cca427";

    /// Тесты с общим состоянием групп идут по одному.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static ONE: Mutex<()> = Mutex::new(());
        ONE.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn divider() -> Text {
        text(vec![span("                                        ", None, None)])
    }

    /// Группа так, как её шлёт pv-addon-groups: три сообщения (название, игроки, кнопки).
    fn group_lines(id: &str, name: &str, locked: bool, mine: bool) -> Vec<Text> {
        let mut first = Vec::new();
        if locked {
            first.push(span("🔒 ", None, Some("Защищена паролем")));
        }
        first.push(span(name, Some(ClickEvent::SuggestCommand(id.into())), Some("UUID группы")));
        let players = text(vec![span("Игроки: 1, владелец: AniLiaSS", None, Some("AniLiaSS"))]);
        let buttons = if mine {
            text(vec![
                span("←Выйти", Some(ClickEvent::RunCommand("/groups leave".into())), Some("/groups leave")),
                span("  ", None, None),
                span("✉ Пригласить", Some(ClickEvent::SuggestCommand("/groups invite ".into())), Some("/groups invite")),
            ])
        } else if locked {
            text(vec![span("→Войти", Some(ClickEvent::SuggestCommand(format!("/groups join {id} "))), None)])
        } else {
            text(vec![span("→Войти", Some(ClickEvent::RunCommand(format!("/groups join {id}"))), None)])
        };
        vec![text(first), players, buttons]
    }

    fn pager() -> Text {
        text(vec![span("Страница 1/1", None, None)])
    }

    fn browse(lines: Vec<Text>) -> Vec<bool> {
        refresh();
        assert_eq!(take_actions(), [Action::Command("groups browse".into())]);
        lines.iter().map(|l| on_chat(l, true)).collect()
    }

    #[test]
    fn multi_line_answer_as_the_addon_sends_it() {
        let _one = serial();
        reset();
        let mut lines = vec![divider()];
        lines.extend(group_lines(ID, "Тайная", true, false));
        lines.push(divider());
        lines.extend(group_lines(ID2, "Моя", false, true));
        lines.push(divider());
        lines.push(pager());
        let hidden = browse(lines);
        assert!(hidden.iter().all(|h| *h), "весь ответ спрятан, и «Страница 1/1» тоже: {hidden:?}");
        assert!(!loading(), "«Страница 1/1» закончила ответ");
        let l = list();
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].name.as_str(), l[0].locked, l[0].mine), ("Тайная", true, false));
        assert_eq!((l[1].name.as_str(), l[1].locked, l[1].mine), ("Моя", false, true));
        assert_eq!(l[1].info, "Игроки: 1, владелец: AniLiaSS");
        assert_eq!(l[1].members, ["AniLiaSS"]);
        assert_eq!(mine().map(|g| g.id), Some(ID2.to_string()));
        reset();
    }

    #[test]
    fn page_line_ends_the_answer() {
        let _one = serial();
        reset();
        let mut lines = vec![divider()];
        lines.extend(group_lines(ID, "Одна", false, false));
        lines.push(divider());
        let next = text(vec![span("Страница 1/1 →", Some(ClickEvent::RunCommand("/groups browse 1".into())), None)]);
        lines.push(next);
        assert!(browse(lines).iter().all(|h| *h));
        assert!(!loading());
        assert_eq!(list().len(), 1);
        // после ответа обычный чат снова не трогаем
        assert!(!on_chat(&pager(), true));
        reset();
    }

    #[test]
    fn single_line_groups_still_work() {
        let _one = serial();
        reset();
        let one = text(vec![
            span("Строители", Some(ClickEvent::SuggestCommand(ID.into())), None),
            span(" 2 игрока", None, Some("Alex, Steve")),
            span("[Войти]", Some(ClickEvent::RunCommand(format!("groups join {ID}"))), None),
        ]);
        assert!(browse(vec![divider(), one]).iter().all(|h| *h));
        finish(&mut lock());
        assert_eq!(list()[0].members, ["Alex", "Steve"]);
        reset();
    }

    #[test]
    fn someone_elses_chat_is_not_eaten_while_we_wait() {
        let _one = serial();
        reset();
        refresh();
        let _ = take_actions();
        assert!(!on_chat(&text(vec![span("<Alex> привет", None, None)]), true), "до разделителя — чужое");
        assert!(on_chat(&divider(), true));
        assert!(on_chat(&text(vec![span("Групп пока нет", None, None)]), true), "после разделителя — «групп нет»");
        assert!(list().is_empty() && fetched());
        assert!(on_chat(&divider(), true), "и закрывающий разделитель тоже наш");
        assert!(!on_chat(&divider(), true), "но только один");
        reset();
    }

    #[test]
    fn slow_server_answer_is_still_ours() {
        let _one = serial();
        reset();
        refresh();
        let _ = take_actions();
        // сервер лагает: ответ пришёл через 4 секунды после запроса
        lock().window = Some(Instant::now() - Duration::from_secs(4));
        let _ = take_actions();
        assert!(loading(), "окно ещё ждёт первую строку");
        let none = text(vec![span("Нет групп. Создайте группу с помощью \"/groups create\"", None, None)]);
        assert!(on_chat(&divider(), true));
        assert!(on_chat(&none, true));
        assert!(on_chat(&divider(), true));
        assert!(list().is_empty() && fetched());
        reset();
    }

    #[test]
    fn button_reply_goes_to_the_menu() {
        let _one = serial();
        reset();
        let g = Group { id: ID.into(), name: "x".into(), info: String::new(), members: vec![], locked: false, mine: false };
        join(&g, "");
        let _ = take_actions();
        let reply = text(vec![span("Вы вошли в группу x", None, None)]);
        assert!(on_chat(&reply, true), "ответ спрятан, пока меню открыто");
        assert_eq!(status().as_deref(), Some("Вы вошли в группу x"));
        // следующее сообщение — уже не ответ
        assert!(!on_chat(&text(vec![span("Очистка дропа через 10 секунд!", None, None)]), true));
        reset();
        join(&g, "");
        let _ = take_actions();
        assert!(!on_chat(&reply, false), "меню закрыто: ответ остаётся в чате");
        reset();
    }

    #[test]
    fn actions_build_the_addon_commands() {
        let _one = serial();
        reset();
        let g = Group { id: ID.into(), name: "x".into(), info: String::new(), members: vec![], locked: false, mine: false };
        join(&g, "");
        join(&Group { locked: true, ..g.clone() }, "qwe 123");
        leave();
        assert!(create("  Мои  друзья ", "p:ss", crate::svc::GroupKind::Open));
        assert!(!create("   ", "", crate::svc::GroupKind::Open));
        invite("Alex_1");
        invite("bad nick; /op me");
        let acts: Vec<Action> = take_actions().into_iter().filter(|a| !matches!(a, Action::Command(c) if c == "groups browse")).collect();
        assert_eq!(
            acts,
            [
                Action::Command(format!("groups join {ID}")),
                Action::Command(format!("groups join {ID} qwe123")),
                Action::Command("groups leave".into()),
                Action::Command("groups create name: Мои друзья password: pss".into()),
                Action::Command("groups invite Alex_1".into()),
            ]
        );
        reset();
    }

    #[test]
    fn uuid_check() {
        assert!(is_uuid(ID));
        assert!(!is_uuid("not-a-uuid"));
        assert!(!is_uuid("0f8fad5bxd9cb-469f-a165-70867728950e"));
    }

    #[test]
    fn svc_ids_round_trip() {
        let id = [0xAB; 16];
        assert_eq!(svc_id_bytes(&svc_id(&id)), Some(id));
        assert_eq!(svc_id_bytes("xyz"), None);
    }

    #[test]
    fn invite_becomes_a_card_not_chat() {
        let _one = serial();
        reset();
        // текст приглашения строкой раньше, кнопка «Присоединиться» следом
        let msg = text(vec![span("AniLiaSS пригласил(а) вас в группу тестер", None, None)]);
        assert!(!on_chat(&msg, false), "текст сам по себе — обычный чат");
        let id = "123e4567-e89b-12d3-a456-426614174000";
        let button = text(vec![span("→ Присоединиться", Some(ClickEvent::RunCommand(format!("/groups join {id}"))), None)]);
        let _ = id;
        assert!(on_chat(&button, false), "кнопку забирает плашка");
        let (t, _, age) = pending_invite().expect("приглашение есть");
        assert!(t.contains("тестер") && age < 0.1);
        accept_invite();
        assert!(pending_invite().is_none());
        let cmds: Vec<String> = take_actions().into_iter().filter_map(|a| match a {
            Action::Command(c) => Some(c),
            _ => None,
        }).collect();
        assert_eq!(cmds.first().map(String::as_str), Some(format!("groups join {id}").as_str()));
        reset();
    }

    #[test]
    fn whole_invite_block_is_hidden_and_filtered_by_sender() {
        let _one = serial();
        reset();
        crate::settings::set_invite_mode(crate::settings::InviteMode::AllExcept);
        // как на сервере: разделитель, «X пригласил(а) вас в группу [название]» (название с UUID), разделитель
        let line = text(vec![
            span("Steve пригласил(а) вас в группу ", None, None),
            span("[ек]", Some(ClickEvent::SuggestCommand(ID.into())), None),
        ]);
        let frame = text(vec![span("-----------------", None, None)]);
        assert!(on_chat(&frame, false), "верхний разделитель спрятан");
        assert!(on_chat(&line, false), "текст приглашения спрятан");
        assert!(on_chat(&frame, false), "нижний разделитель спрятан");
        let (t, _, _) = pending_invite().expect("плашка есть");
        assert!(t.contains("[ек]"));
        assert_eq!(invite_from().as_deref(), Some("Steve"));
        // «от всех, кроме Steve»: следующее от него не показывается
        crate::settings::set_invite_blocked("Steve", true);
        decline_invite();
        std::thread::sleep(INVITE_TAIL);
        assert!(on_chat(&line, false), "строка всё равно не в чате");
        assert!(pending_invite().is_none(), "от заблокированного — без плашки");
        crate::settings::set_invite_blocked("Steve", false);
        // «только от»: Steve не в списке — не показываем
        crate::settings::set_invite_mode(crate::settings::InviteMode::OnlyFrom);
        std::thread::sleep(INVITE_TAIL);
        assert!(on_chat(&line, false));
        assert!(pending_invite().is_none());
        crate::settings::set_invite_allowed("Steve", true);
        std::thread::sleep(INVITE_TAIL);
        assert!(on_chat(&line, false));
        assert!(pending_invite().is_some(), "а когда отмечен — показываем");
        decline_invite();
        crate::settings::set_invite_allowed("Steve", false);
        // ни от кого
        crate::settings::set_invite_mode(crate::settings::InviteMode::Nobody);
        std::thread::sleep(INVITE_TAIL);
        assert!(on_chat(&line, false));
        assert!(pending_invite().is_none());
        crate::settings::set_invite_mode(crate::settings::InviteMode::AllExcept);
        // принять — команда входа по UUID из названия
        std::thread::sleep(INVITE_TAIL);
        assert!(on_chat(&line, false));
        accept_invite();
        let cmds: Vec<String> = take_actions().into_iter().filter_map(|a| match a {
            Action::Command(c) => Some(c),
            _ => None,
        }).collect();
        assert_eq!(cmds.first().cloned(), Some(format!("groups join {ID}")));
        reset();
    }

    #[test]
    fn invite_with_uuid_only_in_hover() {
        let _one = serial();
        reset();
        crate::settings::set_invite_mode(crate::settings::InviteMode::AllExcept);
        let frame = text(vec![span("-----------------", None, None)]);
        let line = text(vec![
            span("Player78306 пригласил(а) вас в группу ", None, None),
            span("[343434]", None, Some(&format!("Uuid: {ID}"))),
        ]);
        assert!(on_chat(&frame, false));
        assert!(on_chat(&line, false), "строка приглашения внутри рамки спрятана");
        assert!(on_chat(&frame, false));
        assert!(pending_invite().is_some());
        accept_invite();
        let cmds: Vec<String> = take_actions().into_iter().filter_map(|a| match a {
            Action::Command(c) => Some(c),
            _ => None,
        }).collect();
        assert_eq!(cmds.first().cloned(), Some(format!("groups join {ID}")), "UUID взят из подсказки");
        reset();
    }
}
