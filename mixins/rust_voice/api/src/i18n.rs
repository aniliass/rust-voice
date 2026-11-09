//! Язык интерфейса: русский, украинский или английский.
//!
//! Строки в коде пишутся по-русски, а перевод делается в одном месте, при выводе на экран: холст меню
//! (`lib.rs`, `Gui::text`) и ширина текста (`menu::text_width`) пропускают строку через [`tr`]. Поэтому ширина
//! и отрисовка всегда считаются по одной и той же строке. «Авто» берёт язык, выбранный в самой игре
//! (Настройки > Язык). Тест ниже проверяет, что у каждой строки меню есть перевод.

use std::borrow::Cow;
use std::sync::atomic::{AtomicU8, Ordering::Relaxed};

/// Язык, на котором показывается меню.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Ru,
    Uk,
    En,
}

impl Lang {
    /// Название языка на нём самом.
    pub fn native(self) -> &'static str {
        match self {
            Lang::Ru => "Русский",
            Lang::Uk => "Українська",
            Lang::En => "English",
        }
    }
}

/// Выбор языка в меню: авто (как в игре) или конкретный.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pref {
    Auto,
    Fixed(Lang),
}

static PREF: AtomicU8 = AtomicU8::new(0);

pub fn pref() -> Pref {
    match PREF.load(Relaxed) {
        1 => Pref::Fixed(Lang::Ru),
        2 => Pref::Fixed(Lang::En),
        3 => Pref::Fixed(Lang::Uk),
        _ => Pref::Auto,
    }
}

pub fn set_pref(p: Pref) {
    PREF.store(
        match p {
            Pref::Auto => 0,
            Pref::Fixed(Lang::Ru) => 1,
            Pref::Fixed(Lang::En) => 2,
            Pref::Fixed(Lang::Uk) => 3,
        },
        Relaxed,
    );
}

/// Следующий вариант по щелчку: авто, русский, украинский, английский и снова авто.
pub fn next_pref(p: Pref) -> Pref {
    match p {
        Pref::Auto => Pref::Fixed(Lang::Ru),
        Pref::Fixed(Lang::Ru) => Pref::Fixed(Lang::Uk),
        Pref::Fixed(Lang::Uk) => Pref::Fixed(Lang::En),
        Pref::Fixed(Lang::En) => Pref::Auto,
    }
}

/// Ключ для файла настроек.
pub fn pref_key(p: Pref) -> &'static str {
    match p {
        Pref::Auto => "auto",
        Pref::Fixed(Lang::Ru) => "ru",
        Pref::Fixed(Lang::Uk) => "uk",
        Pref::Fixed(Lang::En) => "en",
    }
}

pub fn pref_from_key(k: &str) -> Pref {
    match k {
        "ru" => Pref::Fixed(Lang::Ru),
        "uk" => Pref::Fixed(Lang::Uk),
        "en" => Pref::Fixed(Lang::En),
        _ => Pref::Auto,
    }
}

/// Язык по коду игры (`ru_ru`, `uk_ua`, `en_us`…): русский и близкие к нему — русский, украинский — украинский,
/// остальные — английский.
pub fn from_game_code(code: &str) -> Lang {
    let code = code.to_ascii_lowercase();
    if code.starts_with("uk") {
        Lang::Uk
    } else if ["ru", "be", "kk"].iter().any(|p| code.starts_with(p)) {
        Lang::Ru
    } else {
        Lang::En
    }
}

/// ЕДИНСТВЕННОЕ место, зависящее от того, как Horizon хранит язык игры (`horizon_client::lang`). Язык меняется
/// в настройках игры прямо во время игры, поэтому спрашиваем каждый раз (это чтение под замком, дёшево).
fn game_lang() -> Lang {
    if cfg!(test) {
        return Lang::Ru;
    }
    horizon_client::lang::current().map_or(Lang::En, |l| from_game_code(&l.code))
}

/// На каком языке меню сейчас.
pub fn lang() -> Lang {
    match pref() {
        Pref::Fixed(l) => l,
        Pref::Auto => game_lang(),
    }
}

/// Интерфейс сейчас на английском.
pub fn en() -> bool {
    lang() == Lang::En
}

/// Перевод строки интерфейса. По-русски — как есть; на другом языке — из словаря или по шаблонам
/// («Рация 2», «12 дБ», «Ник: выкл»). Чего нет в словаре (ники, устройства), остаётся как есть.
pub fn tr(s: &str) -> Cow<'_, str> {
    let l = lang();
    if l == Lang::Ru || !s.chars().any(is_cyrillic) {
        return Cow::Borrowed(s);
    }
    match to_lang(s, l) {
        Some(t) => Cow::Owned(t),
        None => Cow::Borrowed(s),
    }
}

fn is_cyrillic(c: char) -> bool {
    matches!(c, 'А'..='я' | 'Ё' | 'ё')
}

/// Перевод на `l` (не русский): словарь, потом шаблоны. `None` — перевода нет.
fn to_lang(s: &str, l: Lang) -> Option<String> {
    let uk = l == Lang::Uk;
    if let Some((_, en, ukr)) = lookup(s) {
        return Some(if uk { ukr } else { en }.to_string());
    }
    let word = |name: &str| to_lang(name, l).unwrap_or_else(|| name.to_string());
    // «Дистанция «Локальный»: 32»
    if let Some(rest) = s.strip_prefix("Дистанция «") {
        if let Some((name, tail)) = rest.split_once('»') {
            return Some(if uk { format!("Дистанція «{}»{tail}", word(name)) } else { format!("{} distance{tail}", word(name)) });
        }
    }
    // «Рация «Группы»», «Рация 2»
    if let Some(rest) = s.strip_prefix("Рация «") {
        if let Some((name, tail)) = rest.split_once('»') {
            return Some(if uk { format!("Рація «{}»{tail}", word(name)) } else { format!("Push to talk ({}){tail}", word(name)) });
        }
    }
    if let Some(rest) = s.strip_prefix("Рация ") {
        return Some(if uk { format!("Рація {rest}") } else { format!("Push to talk {rest}") });
    }
    // «Молчите… 2.5 с»
    if let Some(rest) = s.strip_prefix("Молчите… ") {
        let n = rest.trim_end_matches(" с");
        return Some(if uk { format!("Мовчіть… {n} с") } else { format!("Stay quiet… {n} s") });
    }
    // «-42 дБ»
    if let Some(n) = s.strip_suffix(" дБ") {
        return Some(if uk { format!("{n} дБ") } else { format!("{n} dB") });
    }
    // «Ник: выкл» (громкость колесом)
    if let Some(nick) = s.strip_suffix(": выкл") {
        return Some(if uk { format!("{nick}: вимк") } else { format!("{nick}: off") });
    }
    None
}

fn lookup(s: &str) -> Option<(&'static str, &'static str, &'static str)> {
    DICT.binary_search_by(|(ru, _, _)| (*ru).cmp(s)).ok().map(|i| DICT[i])
}

/// Словарь: русская строка -> английская и украинская. Отсортирован по русской строке (поиск двоичный, тест
/// проверяет).
static DICT: &[(&str, &str, &str)] = &[
    ("< Назад", "< Back", "< Назад"),
    ("Esc — отмена, Backspace — убрать клавишу", "Esc to cancel, Backspace to unbind", "Esc — скасувати, Backspace — прибрати клавішу"),
    ("Авто", "Auto", "Авто"),
    ("Автогромкость", "Auto gain", "Автогучність"),
    ("Активации", "Activations", "Активації"),
    ("Активации и дистанции задаёт сервер.", "The server sets activations and distances.", "Активації та дистанції задає сервер."),
    ("Активация выключена: включить", "Activation is off: turn on", "Активацію вимкнено: увімкнути"),
    ("Вас слышно", "You are heard", "Вас чути"),
    ("Визуальные и интерфейс", "Visuals and interface", "Візуальне та інтерфейс"),
    ("Вкл", "On", "Увімк"),
    ("Вкл/выкл заглушение", "Toggle muffling", "Увімк/вимк приглушення"),
    ("Включить звук", "Unmute", "Увімкнути звук"),
    ("Включить или выключить весь голосовой чат", "Turn the whole voice chat on or off", "Увімкнути або вимкнути весь голосовий чат"),
    ("Включить или выключить заглушение через блоки", "Turn muffling through blocks on or off", "Увімкнути або вимкнути приглушення крізь блоки"),
    ("Включить или выключить микрофон (в игре, не в чате)", "Turn the microphone on or off (in game, not in chat)", "Увімкнути або вимкнути мікрофон (у грі, не в чаті)"),
    ("Включить оверлей", "Show overlay", "Показувати оверлей"),
    ("Выберите микрофон", "Choose a microphone", "Оберіть мікрофон"),
    ("Выберите список и щёлкните угол.", "Pick a list, then click a corner.", "Оберіть список і клацніть кут."),
    ("Выберите устройство вывода", "Choose an output device", "Оберіть пристрій виведення"),
    ("Выбрать микрофон", "Choose microphone", "Обрати мікрофон"),
    ("Выбрать наушники или колонки", "Choose headphones or speakers", "Обрати навушники або колонки"),
    ("Вывод звука", "Sound output", "Виведення звуку"),
    ("Выкл", "Off", "Вимк"),
    ("Выключить", "Turn off", "Вимкнути"),
    ("Выключить голосовой чат", "Turn voice chat off", "Вимкнути голосовий чат"),
    ("Выключить звук", "Mute", "Вимкнути звук"),
    ("Выключить микрофон", "Turn microphone off", "Вимкнути мікрофон"),
    ("Выключить эту активацию", "Turn this activation off", "Вимкнути цю активацію"),
    ("Где список говорящих", "Where the talking list goes", "Де список тих, хто говорить"),
    ("Говорит", "Talking", "Говорить"),
    ("Говорите в группу", "Talking to your group", "Говорите в групу"),
    ("Говорите рядом (локальный голос)", "Talking nearby (proximity voice)", "Говорите поруч (локальний голос)"),
    ("Говорите, пока зажата (если у активации тип «По клавише»)", "Talk while held (if the activation type is Push to talk)", "Говоріть, поки затиснута (якщо тип активації «За клавішею»)"),
    ("Говорите: вы слышите себя", "Talk: you hear yourself", "Говоріть: ви чуєте себе"),
    ("Голос", "Voice", "Голос"),
    ("Голос из-за стены идёт в обход: через двери, окна и проёмы, и слышно его со стороны проёма", "A voice behind a wall goes around: through doors, windows and openings, and you hear it from that side", "Голос з-за стіни йде в обхід: через двері, вікна й прорізи, і чути його з боку прорізу"),
    ("Голос слышно с той стороны, где стоит игрок", "You hear a voice from where the player stands", "Голос чути з того боку, де стоїть гравець"),
    ("Голосовой чат включён", "Voice chat on", "Голосовий чат увімкнено"),
    ("Голосовой чат выключен", "Voice chat off", "Голосовий чат вимкнено"),
    ("Голосовой чат выключен: вы никого не слышите, и микрофон тоже молчит", "Voice chat off: you hear nobody, and your microphone is off too", "Голосовий чат вимкнено: ви нікого не чуєте, і мікрофон теж мовчить"),
    ("Голосовой чат, совместимый с Plasmo Voice и Simple Voice Chat. Меню: клавиша ниже, /voice или /vm.", "Voice chat compatible with Plasmo Voice and Simple Voice Chat. Menu: the key below, /voice or /vm.", "Голосовий чат, сумісний з Plasmo Voice і Simple Voice Chat. Меню: клавіша нижче, /voice або /vm."),
    ("Голосовой чат: ВКЛ", "Voice chat: ON", "Голосовий чат: УВІМК"),
    ("Голосовой чат: ВЫКЛ", "Voice chat: OFF", "Голосовий чат: ВИМК"),
    ("Громкость", "Volume", "Гучність"),
    ("Громкость голосового чата", "Voice chat volume", "Гучність голосового чату"),
    ("Громкость игроков", "Player volume", "Гучність гравців"),
    ("Громкость источников", "Source volume", "Гучність джерел"),
    ("Громкость колесом", "Volume with the wheel", "Гучність коліщатком"),
    ("Громкость микрофона", "Microphone volume", "Гучність мікрофона"),
    ("Громкость расстояния", "Distance fade", "Гучність від відстані"),
    ("Группа", "Group", "Група"),
    ("Группа отдельным списком", "Group in its own list", "Група окремим списком"),
    ("Группа:", "Group:", "Група:"),
    ("Группы", "Groups", "Групи"),
    ("Да", "Yes", "Так"),
    ("Дальше игрок — тише его голос. Выключите — всех слышно одинаково", "The farther a player, the quieter. Off: everyone is equally loud", "Що далі гравець — то тихіше його голос. Вимкніть — всіх чути однаково"),
    ("Дальше — вас слышно дальше. Варианты задаёт сервер", "Farther: you are heard farther. The server sets the options", "Далі — вас чути далі. Варіанти задає сервер"),
    ("Держите клавишу рации", "Hold the push-to-talk key", "Тримайте клавішу рації"),
    ("Дистанция голоса «Локальный»", "Proximity voice distance", "Дистанція голосу «Локальний»"),
    ("За стеной голос не только тише, но и глуше, как в жизни. Стекло звук пропускает", "Behind a wall a voice is quieter and duller, as in life. Glass lets sound through", "За стіною голос не лише тихіший, а й глухіший, як у житті. Скло звук пропускає"),
    ("Забыть шум", "Forget noise", "Забути шум"),
    ("Заглушение через блоки", "Muffling through blocks", "Приглушення крізь блоки"),
    ("Заглушение через блоки включено", "Muffling through blocks on", "Приглушення крізь блоки увімкнено"),
    ("Заглушение через блоки выключено", "Muffling through blocks off", "Приглушення крізь блоки вимкнено"),
    ("Записать 3 с тишины", "Record 3 s of silence", "Записати 3 с тиші"),
    ("Запомнен ✓ (ещё раз)", "Learned ✓ (again)", "Запам'ятовано ✓ (ще раз)"),
    ("Запомнить шум", "Learn noise", "Запам'ятати шум"),
    ("Затухание звука через блоки", "Muffling through blocks", "Приглушення звуку крізь блоки"),
    ("Звук огибает углы", "Sound goes around corners", "Звук огинає кути"),
    ("Звуковой движок", "Sound engine", "Звуковий рушій"),
    ("Значки над никами", "Badges over names", "Значки над ніками"),
    ("Значок статуса над ником игрока", "Status badge over player names", "Значок стану над ніком гравця"),
    ("Значок у хотбара, пока вы говорите:", "Icon by the hotbar while you talk:", "Значок біля хотбара, поки ви говорите:"),
    ("Игроков с таким ником нет.", "No players with that name.", "Гравців з таким ніком немає."),
    ("Как «Локальный»", "Same as Proximity", "Як «Локальний»"),
    ("Клавиша рации", "Push-to-talk key", "Клавіша рації"),
    ("Клавиши", "Hotkeys", "Клавіші"),
    ("Кнопка действия", "Action button", "Кнопка дії"),
    ("Команды в чате", "Chat commands", "Команди в чаті"),
    ("Кто говорит в вашей группе, отмечен фиолетовой рамкой.", "Players talking in your group have a purple frame.", "Хто говорить у вашій групі, позначений фіолетовою рамкою."),
    ("Локальный", "Proximity", "Локальний"),
    ("Лучше в наушниках", "Better with headphones", "Краще в навушниках"),
    ("Микрофон", "Microphone", "Мікрофон"),
    ("Микрофон включён", "Microphone on", "Мікрофон увімкнено"),
    ("Микрофон выключен", "Microphone off", "Мікрофон вимкнено"),
    ("Микрофон закрывается совсем: система не видит, что он занят. Слушать других можно", "The microphone is fully closed: the system doesn't see it in use. You can still listen", "Мікрофон закривається повністю: система не бачить, що він зайнятий. Слухати інших можна"),
    ("Микрофон или динамик над головой: говорит, выключен, без мода", "Microphone or speaker over the head: talking, muted, no mod", "Мікрофон або динамік над головою: говорить, вимкнено, без мода"),
    ("Микрофон не доступен", "Microphone unavailable", "Мікрофон недоступний"),
    ("Микрофон отключён", "Microphone disabled", "Мікрофон відключено"),
    ("Микрофон открылся", "The microphone opened", "Мікрофон відкрився"),
    ("Микрофон: ВКЛ", "Microphone: ON", "Мікрофон: УВІМК"),
    ("Микрофон: ВЫКЛ", "Microphone: OFF", "Мікрофон: ВИМК"),
    ("Мод сам понимает, что стоит на сервере: Plasmo Voice или Simple Voice Chat", "The mod detects what the server runs: Plasmo Voice or Simple Voice Chat", "Мод сам визначає, що стоїть на сервері: Plasmo Voice чи Simple Voice Chat"),
    ("Моно", "Mono", "Моно"),
    ("Наведитесь на игрока, зажмите ПКМ и крутите колесо — его громкость", "Look at a player, hold RMB and scroll: their volume", "Наведіться на гравця, затисніть ПКМ і крутіть коліщатко — його гучність"),
    ("Наведитесь на игрока, зажмите ПКМ и крутите колесо — меняется его громкость", "Look at a player, hold RMB and scroll to change their volume", "Наведіться на гравця, затисніть ПКМ і крутіть коліщатко — змінюється його гучність"),
    ("Нажмите клавишу", "Press a key", "Натисніть клавішу"),
    ("Нажмите клавишу…", "Press a key…", "Натисніть клавішу…"),
    ("Направленные источники", "Directional sources", "Спрямовані джерела"),
    ("Направленный звук и HRTF работают только в стерео", "Directional sound and HRTF need stereo", "Спрямований звук і HRTF працюють лише в стерео"),
    ("Не доступен", "Unavailable", "Недоступний"),
    ("Не доступно", "Unavailable", "Недоступно"),
    ("Нет", "No", "Ні"),
    ("Нет подключения к голосу.", "Not connected to voice.", "Немає підключення до голосу."),
    ("Нет — щёлкните", "None: click", "Немає — клацніть"),
    ("Нужно подключение к голосу", "Needs a voice connection", "Потрібне підключення до голосу"),
    ("Нужно подключение к голосу и микрофон", "Needs a voice connection and a microphone", "Потрібне підключення до голосу та мікрофон"),
    ("Нужного нет? Подключите его и откройте список заново.", "Not listed? Plug it in and open the list again.", "Потрібного немає? Підключіть його й відкрийте список знову."),
    ("Объём в наушниках: задержка между ушами и «тень головы», сзади голос глуше, чем спереди", "Depth in headphones: delay between the ears and head shadow, voices behind you are duller", "Об'єм у навушниках: затримка між вухами й «тінь голови», ззаду голос глухіший, ніж спереду"),
    ("Обычные", "Nearby", "Звичайні"),
    ("Обычные:", "Nearby:", "Звичайні:"),
    ("Оверлей", "Overlay", "Оверлей"),
    ("Основные", "General", "Основні"),
    ("Остальные активации появятся после подключения к голосу.", "Other activations appear once connected to voice.", "Інші активації з'являться після підключення до голосу."),
    ("Отключено", "Disabled", "Відключено"),
    ("Отключить устройство", "Disable device", "Відключити пристрій"),
    ("Открыть и закрыть это меню. Без своей клавиши — из Options > Mods", "Open and close this menu. Without its own key: from Options > Mods", "Відкрити й закрити це меню. Без своєї клавіші — з Options > Mods"),
    ("Открыть меню", "Open menu", "Відкрити меню"),
    ("Открыть меню голоса", "Open voice menu", "Відкрити меню голосу"),
    ("ПКМ", "RMB", "ПКМ"),
    ("По клавише", "Push to talk", "За клавішею"),
    ("Подключение к голосу", "Voice connection", "Підключення до голосу"),
    ("Поиск игрока…", "Find a player…", "Пошук гравця…"),
    ("Пока никого: игроки с голосом появятся здесь.", "Nobody yet: players with voice will appear here.", "Поки нікого: гравці з голосом з'являться тут."),
    ("Помолчите 3 секунды: шумодав запомнит ваш фон и не будет глушить долгую речь", "Stay quiet for 3 seconds: the suppressor learns your background and won't cut long speech", "Помовчіть 3 секунди: шумодав запам'ятає ваш фон і не глушитиме довге мовлення"),
    ("Порог активации", "Activation threshold", "Поріг активації"),
    ("Предыдущая дистанция из разрешённых сервером", "Previous distance allowed by the server", "Попередня дистанція з дозволених сервером"),
    ("Приоритет", "Priority", "Пріоритет"),
    ("Проверка звука", "Sound check", "Перевірка звуку"),
    ("Проверка звука: ВКЛ", "Sound check: ON", "Перевірка звуку: УВІМК"),
    ("Продвинутые", "Advanced", "Розширені"),
    ("Протокол", "Protocol", "Протокол"),
    ("Расположение оверлея", "Overlay position", "Розташування оверлея"),
    ("Рация", "Push to talk", "Рація"),
    ("Рация 2: вторая активация сервера (например, группы)", "Push to talk 2: the server's second activation (e.g. groups)", "Рація 2: друга активація сервера (наприклад, групи)"),
    ("Рация 3: третья активация сервера", "Push to talk 3: the server's third activation", "Рація 3: третя активація сервера"),
    ("Рация: говорить по клавише (Я)", "Push to talk", "Рація: говорити за клавішею (Я)"),
    ("Ручка — порог: полоса должна выходить за неё, когда вы говорите", "The handle is the threshold: the bar should pass it when you talk", "Ручка — поріг: смуга має виходити за неї, коли ви говорите"),
    ("Сбросить (100%)", "Reset (100%)", "Скинути (100%)"),
    ("Сбросить (вкл)", "Reset (on)", "Скинути (увімк)"),
    ("Сбросить (выкл)", "Reset (off)", "Скинути (вимк)"),
    ("Сбросить громкость", "Reset volume", "Скинути гучність"),
    ("Сбросить дистанцию", "Reset distance", "Скинути дистанцію"),
    ("Сбросить клавишу", "Reset key", "Скинути клавішу"),
    ("Сбросить настройки", "Reset settings", "Скинути налаштування"),
    ("Сбросить порог", "Reset threshold", "Скинути поріг"),
    ("Сбросить тип", "Reset type", "Скинути тип"),
    ("Своё спектральное: убирает ровный шум (кулер, улица). Сильнее — чище, но голос «плоский»", "Our own spectral filter: removes steady noise (fans, street). Stronger is cleaner, but the voice gets flat", "Власне спектральне: прибирає рівний шум (кулер, вулиця). Сильніше — чистіше, але голос «плаский»"),
    ("Сейчас:", "Now:", "Зараз:"),
    ("Сервер не даёт менять дистанцию", "The server doesn't allow changing the distance", "Сервер не дає змінювати дистанцію"),
    ("Сервер принял ваш голосовой клиент", "The server accepted your voice client", "Сервер прийняв ваш голосовий клієнт"),
    ("Системный вывод", "System output", "Системне виведення"),
    ("Системный микрофон", "System microphone", "Системний мікрофон"),
    ("Системный по умолчанию", "System default", "Системний за замовчуванням"),
    ("Сколько голосов играет прямо сейчас", "How many voices are playing right now", "Скільки голосів звучить просто зараз"),
    ("Сколько игроков сейчас слышно через двери и проёмы", "How many players you hear through doors and openings now", "Скільки гравців зараз чути через двері й прорізи"),
    ("Слева сверху", "Top left", "Зліва зверху"),
    ("Слева снизу", "Bottom left", "Зліва знизу"),
    ("Следующая дистанция из разрешённых сервером", "Next distance allowed by the server", "Наступна дистанція з дозволених сервером"),
    ("Слышно в обход стен", "Heard around walls", "Чути в обхід стін"),
    ("Слышно источников", "Voices heard", "Чути джерел"),
    ("Сменить: вкладка «Активации»", "Change it on the Activations tab", "Змінити: вкладка «Активації»"),
    ("Состояние", "Status", "Стан"),
    ("Справа сверху", "Top right", "Справа зверху"),
    ("Справа снизу", "Bottom right", "Справа знизу"),
    ("Срезает всё ниже 80 Гц: гул, вентилятор, стук по столу. Голос не трогает", "Cuts everything below 80 Hz: hum, fans, desk knocks. Leaves the voice alone", "Зрізає все нижче 80 Гц: гул, вентилятор, стукіт по столу. Голос не чіпає"),
    ("Стерео", "Stereo", "Стерео"),
    ("Так выглядит оверлей:", "The overlay looks like this:", "Так виглядає оверлей:"),
    ("Тип активации", "Activation type", "Тип активації"),
    ("Тихий голос подтягивается сам, громкий смех не хрипит. Подстраивается только пока вы говорите", "A quiet voice is raised, loud laughter doesn't clip. Adjusts only while you talk", "Тихий голос підтягується сам, гучний сміх не хрипить. Підлаштовується лише поки ви говорите"),
    ("Тишина", "Silence", "Тиша"),
    ("Увеличить дистанцию", "Increase distance", "Збільшити дистанцію"),
    ("Уменьшить дистанцию", "Decrease distance", "Зменшити дистанцію"),
    ("Устройства", "Devices", "Пристрої"),
    ("Устройство ввода", "Input device", "Пристрій введення"),
    ("Устройство вывода", "Output device", "Пристрій виведення"),
    ("Устройство вывода моно: направление не слышно", "Output device is mono: no direction", "Пристрій виведення моно: напрямок не чути"),
    ("Фильтр гула", "Hum filter", "Фільтр гулу"),
    ("Шумоподавление", "Noise suppression", "Шумозаглушення"),
    ("Шёпот", "Whisper", "Шепіт"),
    ("Щёлкните и нажмите клавишу (работает в режиме «По клавише»)", "Click and press a key (works in Push to talk mode)", "Клацніть і натисніть клавішу (працює в режимі «За клавішею»)"),
    ("Щёлкните и нажмите клавишу: говорите, пока она зажата", "Click and press a key: talk while it is held", "Клацніть і натисніть клавішу: говоріть, поки вона затиснута"),
    ("Щёлкните, чтобы сменить", "Click to change", "Клацніть, щоб змінити"),
    ("Язык", "Language", "Мова"),
    ("Язык меню: авто — как в игре", "Menu language: Auto follows the game language", "Мова меню: авто — як у грі"),
    ("в группу", "to group", "в групу"),
    ("голосовой чат вкл/выкл", "voice chat on/off", "голосовий чат увімк/вимк"),
    ("звук выкл", "sound off", "звук вимк"),
    ("микрофон вкл/выкл", "microphone on/off", "мікрофон увімк/вимк"),
    ("микрофон выкл", "mic off", "мікрофон вимк"),
    ("открыть меню (или /vm)", "open the menu (or /vm)", "відкрити меню (або /vm)"),
    ("рядом", "nearby", "поруч"),
    ("свободных клавиш нет", "no free keys", "вільних клавіш немає"),
];

#[cfg(test)]
mod tests {
    use super::*;

    static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn dictionary_is_sorted_and_unique() {
        for w in DICT.windows(2) {
            assert!(w[0].0 < w[1].0, "словарь не отсортирован: {:?} и {:?}", w[0].0, w[1].0);
        }
        assert!(DICT.iter().all(|(_, en, uk)| !en.is_empty() && !uk.is_empty()));
    }

    /// Каждая русская строка меню (кроме тестов, комментариев и лога) переводится: словарём или шаблоном.
    #[test]
    fn every_menu_string_has_a_translation() {
        let mut missing = Vec::new();
        for (file, src) in [
            ("menu.rs", include_str!("menu.rs")),
            ("settings.rs", include_str!("settings.rs")),
            ("lib.rs", include_str!("lib.rs")),
        ] {
            let code = src.split("#[cfg(test)]").next().unwrap_or("");
            for line in code.lines() {
                let t = line.trim_start();
                if t.starts_with("//") || line.contains(".log(") || line.contains("log(&format!") || line.contains("eprintln!") {
                    continue;
                }
                let mut rest = line;
                while let Some(start) = rest.find('"') {
                    let after = &rest[start + 1..];
                    let Some(end) = after.find('"') else { break };
                    let lit = &after[..end];
                    rest = &after[end + 1..];
                    if !lit.chars().any(is_cyrillic) || ALLOWED.contains(&lit) {
                        continue;
                    }
                    // шаблоны format!: подставим пример вместо {…}
                    let sample = lit
                        .replace("{left:.1}", "2.5")
                        .replace("{title}", "Локальный")
                        .replace("{n}", "Группы")
                        .replace("{nick}", "Steve")
                        .replace("{d}", "32")
                        .replace("{}", "2");
                    for l in [Lang::En, Lang::Uk] {
                        if to_lang(&sample, l).is_none() {
                            missing.push(format!("{file} ({l:?}): {lit}"));
                        }
                    }
                }
            }
        }
        assert!(missing.is_empty(), "нет перевода:\n{}", missing.join("\n"));
    }

    /// Строки, которые не показываются в интерфейсе, собираются по частям (их части переведены отдельно)
    /// или пишутся на своём языке (названия языков).
    const ALLOWED: &[&str] = &[
        "йцукенгшщзхъфывапролджэячсмитьбю",
        "йцукенгшщзхїфівапролджєячсмитьбю",
        "выключен",
        "включён",
        "включено",
        "выключено",
        "Микрофон {}",
        "Голосовой чат {}",
        "Заглушение через блоки {s}",
        "ушёл",
        "не ушёл",
        "Русский",
        "Українська",
        "{} ({})",
        "SVC secret: голос {vhost}:{port}, дистанция {}, кодек {}, mtu {}, keep-alive {} мс, группы {}",
        "хендшейк #{n}: minecraft:register {}, PlayerInfo {}, SVC request_secret {}",
        "Config: {} Гц, mtu {}, кодек {} {:?}, шифрование {:?}, proximity {:?}",
    ];

    #[test]
    fn patterns_and_languages() {
        let _g = ONE.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(to_lang("Дистанция «Локальный»: 32", Lang::En).as_deref(), Some("Proximity distance: 32"));
        assert_eq!(to_lang("Дистанция «Локальный»: 32", Lang::Uk).as_deref(), Some("Дистанція «Локальний»: 32"));
        assert_eq!(to_lang("Рация «Группы»", Lang::En).as_deref(), Some("Push to talk (Groups)"));
        assert_eq!(to_lang("Рация 3", Lang::Uk).as_deref(), Some("Рація 3"));
        assert_eq!(to_lang("Молчите… 2.5 с", Lang::En).as_deref(), Some("Stay quiet… 2.5 s"));
        assert_eq!(to_lang("-42 дБ", Lang::En).as_deref(), Some("-42 dB"));
        assert_eq!(to_lang("Steve: выкл", Lang::Uk).as_deref(), Some("Steve: вимк"));
        assert_eq!(to_lang("Микрофон", Lang::Uk).as_deref(), Some("Мікрофон"));
        assert_eq!(to_lang("Steve", Lang::En), None);
        set_pref(Pref::Fixed(Lang::Uk));
        assert_eq!(tr("Громкость"), "Гучність");
        assert_eq!(tr("Steve"), "Steve");
        set_pref(Pref::Fixed(Lang::Ru));
        assert_eq!(tr("Микрофон"), "Микрофон");
        set_pref(Pref::Auto);
        assert_eq!(lang(), Lang::Ru, "в тестах «авто» — русский");
    }

    #[test]
    fn game_codes() {
        assert_eq!(from_game_code("ru_ru"), Lang::Ru);
        assert_eq!(from_game_code("uk_ua"), Lang::Uk);
        assert_eq!(from_game_code("be_by"), Lang::Ru);
        assert_eq!(from_game_code("en_us"), Lang::En);
        assert_eq!(from_game_code("de_de"), Lang::En);
        for p in [Pref::Auto, Pref::Fixed(Lang::Ru), Pref::Fixed(Lang::Uk), Pref::Fixed(Lang::En)] {
            assert_eq!(pref_from_key(pref_key(p)), p);
        }
        assert_eq!(next_pref(next_pref(next_pref(next_pref(Pref::Auto)))), Pref::Auto);
    }
}
