//! Скины игроков для голов в меню (вкладка «Громкость»).
//!
//! Игра отдаёт список игроков со скинами (`Host::client_profiles`): имя текстуры вроде `skins/<sha1>`
//! (скачанный скин) или `entity/player/wide/steve` (скин по умолчанию, пока скачивается настоящий).
//! `lib.rs` раз в секунду кладёт этот список сюда в `tick`, а меню берёт скин по нику при рисовании:
//! рисовать у него есть только `&dyn Host`, а `client_profiles` просит изменяемый.

use std::collections::HashMap;
use std::sync::Mutex;

static SKINS: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn lock() -> std::sync::MutexGuard<'static, Option<HashMap<String, String>>> {
    SKINS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Заменить весь список: (ник, имя текстуры скина).
pub fn set(list: impl IntoIterator<Item = (String, String)>) {
    let map: HashMap<String, String> = list.into_iter().map(|(n, s)| (n.to_lowercase(), s)).collect();
    *lock() = Some(map);
}

/// Имя текстуры скина игрока, если игра его уже назвала.
pub fn skin(nick: &str) -> Option<String> {
    lock().as_ref()?.get(&nick.to_lowercase()).cloned()
}

/// Есть ли игрок в списке игроков сервера (таб). None — список ещё не пришёл (или пустой): не знаем.
pub fn online(nick: &str) -> Option<bool> {
    let g = lock();
    let map = g.as_ref().filter(|m| !m.is_empty())?;
    Some(map.contains_key(&nick.to_lowercase()))
}

/// Смена сервера: список прошлого сервера больше не нужен.
pub fn reset() {
    *lock() = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skin_is_found_by_nick_in_any_case() {
        set([("Steve".to_string(), "skins/abc".to_string())]);
        assert_eq!(skin("sTEVE").as_deref(), Some("skins/abc"));
        assert_eq!(skin("Alex"), None);
        assert_eq!(online("steve"), Some(true));
        assert_eq!(online("Herobrine"), Some(false), "вышел");
        reset();
        assert_eq!(skin("Steve"), None);
        assert_eq!(online("Steve"), None, "списка нет — не знаем");
    }
}
