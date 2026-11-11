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
    ("(если вы в группе один — только группе).", "(if you're alone in the group — group only).", "(якщо ви в групі самі — лише групі)."),
    ("360° в наушниках: голос проходит через записанные отклики ушей (KEMAR, MIT) — слышно не только слева-справа, но и спереди-сзади, сверху-снизу", "360° in headphones: voices go through recorded ear responses (KEMAR, MIT), so you hear not just left and right but also front and back, above and below", "360° у навушниках: голос проходить через записані відгуки вух (KEMAR, MIT) — чутно не лише ліворуч-праворуч, а й спереду-ззаду, згори-знизу"),
    ("< Назад", "< Back", "< Назад"),
    ("Esc — отмена, Backspace — убрать клавишу", "Esc to cancel, Backspace to unbind", "Esc — скасувати, Backspace — прибрати клавішу"),
    ("RNNoise, как в Plasmo Voice: лучше с клавиатурой и щелчками. Выключено — своё спектральное (можно запомнить шум)", "RNNoise, as in Plasmo Voice: better with keyboard and clicks. Off: the built-in spectral one (can learn your noise)", "RNNoise, як у Plasmo Voice: краще з клавіатурою й клацаннями. Вимкнено — власне спектральне (можна запам'ятати шум)"),
    ("RNNoise: лёгкое, задержка 10 мс", "RNNoise: light, 10 ms delay", "RNNoise: легке, затримка 10 мс"),
    ("«Группы» — группе, «Локальный» — только рядом; пока говорите рядом, группу не слышно. Если вы в группе один, голос идёт только группе", "«Groups» talks to the group, «Local» only nearby; while talking nearby you don't hear the group. If you're alone in the group, your voice goes to the group only", "«Групи» — групі, «Локальний» — лише поруч; поки говорите поруч, групу не чути. Якщо ви в групі самі, голос іде лише групі"),
    ("Авто", "Auto", "Авто"),
    ("Авто: микрофон включается, когда нейросеть слышит речь (клавиатура и кулер не включают). Порог: по громкости, ручку ставите сами", "Auto: the mic turns on when the neural net hears speech (keyboard and fans don't trigger it). Threshold: by loudness, you set the handle", "Авто: мікрофон вмикається, коли нейромережа чує мову (клавіатура й кулер не вмикають). Поріг: за гучністю, ручку ставите самі"),
    ("Автогромкость", "Auto gain", "Автогучність"),
    ("Активации", "Activations", "Активації"),
    ("Активации и дистанции задаёт сервер.", "The server sets activations and distances.", "Активації та дистанції задає сервер."),
    ("Активация выключена: включить", "Activation is off: turn on", "Активацію вимкнено: увімкнути"),
    ("Активация нейросетью", "Neural activation", "Активація нейромережею"),
    ("Без шумоподавления", "No noise suppression", "Без шумозаглушення"),
    ("Блик на проигрывателе", "Jukebox shine", "Відблиск на програвачі"),
    ("В обычной группе: «Группы» — группе, «Локальный» — рядом", "In a normal group: «Groups» to the group, «Local» nearby", "У звичайній групі: «Групи» — групі, «Локальний» — поруч"),
    ("Вас приглашают в группу", "You're invited to a group", "Вас запрошують до групи"),
    ("Вас слышит группа и те, кто рядом; вы слышите всех", "The group and players nearby hear you; you hear everyone", "Вас чує група і ті, хто поруч; ви чуєте всіх"),
    ("Вас слышно", "You are heard", "Вас чути"),
    ("Визуальные и интерфейс", "Visuals and interface", "Візуальне та інтерфейс"),
    ("Вкл", "On", "Увімк"),
    ("Вкл/выкл заглушение", "Toggle muffling", "Увімк/вимк приглушення"),
    ("Включается нейросеть: когда слышит речь (цифра — её уверенность). Порог здесь не нужен", "The neural net turns it on when it hears speech (the number is its confidence). No threshold needed", "Вмикає нейромережа, коли чує мову (цифра — її впевненість). Поріг тут не потрібен"),
    ("Включается, когда громче порога", "Turns on when louder than the threshold", "Вмикається, коли гучніше за поріг"),
    ("Включение микрофона", "Microphone activation", "Увімкнення мікрофона"),
    ("Включить звук", "Unmute", "Увімкнути звук"),
    ("Включить или выключить весь голосовой чат", "Turn the whole voice chat on or off", "Увімкнути або вимкнути весь голосовий чат"),
    ("Включить или выключить заглушение через блоки", "Turn muffling through blocks on or off", "Увімкнути або вимкнути приглушення крізь блоки"),
    ("Включить или выключить микрофон (в игре, не в чате)", "Turn the microphone on or off (in game, not in chat)", "Увімкнути або вимкнути мікрофон (у грі, не в чаті)"),
    ("Войдите в группу из списка или создайте свою.", "Join one from the list or create your own.", "Увійдіть у групу зі списку або створіть свою."),
    ("Войти", "Join", "Увійти"),
    ("Войти в группу", "Join the group", "Увійти в групу"),
    ("Войти…", "Join…", "Увійти…"),
    ("Вы в группе", "You're in", "Ви в групі"),
    ("Вы вошли в группу", "You joined the group", "Ви увійшли до групи"),
    ("Вы вышли из группы", "You left the group", "Ви вийшли з групи"),
    ("Вы не в группе.", "You're not in a group.", "Ви не в групі."),
    ("Выберите микрофон", "Choose a microphone", "Оберіть мікрофон"),
    ("Выберите список и щёлкните угол.", "Pick a list, then click a corner.", "Оберіть список і клацніть кут."),
    ("Выберите устройство вывода", "Choose an output device", "Оберіть пристрій виведення"),
    ("Выбрать микрофон", "Choose microphone", "Обрати мікрофон"),
    ("Выбрать наушники или колонки", "Choose headphones or speakers", "Обрати навушники або колонки"),
    ("Вывод звука", "Sound output", "Виведення звуку"),
    ("Выйти", "Leave", "Вийти"),
    ("Выкл", "Off", "Вимк"),
    ("Выключить", "Turn off", "Вимкнути"),
    ("Выключить голосовой чат", "Turn voice chat off", "Вимкнути голосовий чат"),
    ("Выключить звук", "Mute", "Вимкнути звук"),
    ("Выключить микрофон", "Turn microphone off", "Вимкнути мікрофон"),
    ("Выключить эту активацию", "Turn this activation off", "Вимкнути цю активацію"),
    ("Где список говорящих", "Where the talking list goes", "Де список тих, хто говорить"),
    ("Говорит", "Talking", "Говорить"),
    ("Говорит, выключен микрофон, выключен звук или у игрока нет голосового мода", "Talking, mic off, sound off, or the player has no voice mod", "Говорить, вимкнено мікрофон, вимкнено звук або в гравця немає голосового мода"),
    ("Говорите, пока зажата (если у активации тип «По клавише»)", "Talk while held (if the activation type is Push to talk)", "Говоріть, поки затиснута (якщо тип активації «За клавішею»)"),
    ("Говорите: вы слышите себя, другие — нет", "Talk: you hear yourself, others don't", "Говоріть: ви чуєте себе, інші — ні"),
    ("Говорящих в вашей группе — в своём углу", "Talkers in your group get their own corner", "Ті, хто говорить у вашій групі, — у своєму куті"),
    ("Головы и ники тех, кто сейчас говорит", "Heads and nicks of those talking right now", "Голови й ніки тих, хто зараз говорить"),
    ("Голос", "Voice", "Голос"),
    ("Голос звучит оттуда, где стоит игрок: за стеной глуше, из соседней комнаты — со стороны двери, в наушниках слышно и спереди-сзади", "Voices come from where players stand: muffled behind walls, from the door side when they're in the next room, and front or back in headphones", "Голос звучить звідти, де стоїть гравець: за стіною глухіше, із сусідньої кімнати — з боку дверей, у навушниках чутно й спереду-ззаду"),
    ("Голос из-за стены идёт в обход: через двери, окна и проёмы, и слышно его со стороны проёма", "A voice behind a wall goes around: through doors, windows and openings, and you hear it from that side", "Голос з-за стіни йде в обхід: через двері, вікна й прорізи, і чути його з боку прорізу"),
    ("Голос слышно с той стороны, где стоит игрок", "You hear a voice from where the player stands", "Голос чути з того боку, де стоїть гравець"),
    ("Голосовой чат включён", "Voice chat on", "Голосовий чат увімкнено"),
    ("Голосовой чат выключен", "Voice chat off", "Голосовий чат вимкнено"),
    ("Голосовой чат, совместимый с Plasmo Voice и Simple Voice Chat. Меню: клавиша ниже, /voice или /vm.", "Voice chat compatible with Plasmo Voice and Simple Voice Chat. Menu: the key below, /voice or /vm.", "Голосовий чат, сумісний з Plasmo Voice і Simple Voice Chat. Меню: клавіша нижче, /voice або /vm."),
    ("Голосовой чат: ВКЛ", "Voice chat: ON", "Голосовий чат: УВІМК"),
    ("Голосовой чат: ВЫКЛ", "Voice chat: OFF", "Голосовий чат: ВИМК"),
    ("Громкость", "Volume", "Гучність"),
    ("Громкость голосов рядом", "Nearby voices volume", "Гучність голосів поруч"),
    ("Громкость игроков", "Player volume", "Гучність гравців"),
    ("Громкость источников", "Source volume", "Гучність джерел"),
    ("Громкость колесом", "Volume with the wheel", "Гучність коліщатком"),
    ("Громкость микрофона", "Microphone volume", "Гучність мікрофона"),
    ("Громкость расстояния", "Distance fade", "Гучність від відстані"),
    ("Громче тот, на который смотришь", "Louder where you look", "Гучніше той, на який дивишся"),
    ("Группа", "Group", "Група"),
    ("Группа отдельным списком", "Group in its own list", "Група окремим списком"),
    ("Группа с паролем", "Password-protected group", "Група з паролем"),
    ("Группа с паролем: откроется чат, допишите пароль и нажмите Enter", "Password-protected: chat opens, type the password and press Enter", "Група з паролем: відкриється чат, допишіть пароль і натисніть Enter"),
    ("Группа:", "Group:", "Група:"),
    ("Группы", "Groups", "Групи"),
    ("Группы Plasmo Voice даёт аддон сервера pv-addon-groups.", "Plasmo Voice groups come from the pv-addon-groups server add-on.", "Групи Plasmo Voice додає аддон сервера pv-addon-groups."),
    ("Группы выключены в настройках Simple Voice Chat на сервере.", "Groups are turned off in the server's Simple Voice Chat settings.", "Групи вимкнено в налаштуваннях Simple Voice Chat на сервері."),
    ("Группы на сервере", "Groups on this server", "Групи на сервері"),
    ("Да", "Yes", "Так"),
    ("Дальше игрок — тише его голос. Выключите — всех слышно одинаково", "The farther a player, the quieter. Off: everyone is equally loud", "Що далі гравець — то тихіше його голос. Вимкніть — всіх чути однаково"),
    ("Дальше — вас слышно дальше. Варианты задаёт сервер", "Farther: you are heard farther. The server sets the options", "Далі — вас чути далі. Варіанти задає сервер"),
    ("Дистанция голоса «Локальный»", "Proximity voice distance", "Дистанція голосу «Локальний»"),
    ("Других групп пока нет.", "No other groups yet.", "Інших груп поки немає."),
    ("За стеной голос не только тише, но и глуше, как в жизни. Стекло звук пропускает", "Behind a wall a voice is quieter and duller, as in life. Glass lets sound through", "За стіною голос не лише тихіший, а й глухіший, як у житті. Скло звук пропускає"),
    ("Забыть шум", "Forget noise", "Забути шум"),
    ("Заглушение через блоки", "Muffling through blocks", "Приглушення крізь блоки"),
    ("Заглушение через блоки включено", "Muffling through blocks on", "Приглушення крізь блоки увімкнено"),
    ("Заглушение через блоки выключено", "Muffling through blocks off", "Приглушення крізь блоки вимкнено"),
    ("Загружаем…", "Loading…", "Завантажуємо…"),
    ("Загружаю список групп…", "Loading groups…", "Завантажую список груп…"),
    ("Закрепить пластинку", "Pin disc", "Закріпити платівку"),
    ("Закреплено", "Pinned", "Закріплено"),
    ("Записать 3 с тишины", "Record 3 s of silence", "Записати 3 с тиші"),
    ("Запомнен ✓ (ещё раз)", "Learned ✓ (again)", "Запам'ятовано ✓ (ще раз)"),
    ("Запомнить шум", "Learn noise", "Запам'ятати шум"),
    ("Затухание звука через блоки", "Muffling through blocks", "Приглушення звуку крізь блоки"),
    ("Звук огибает углы", "Sound goes around corners", "Звук огинає кути"),
    ("Звуковой движок", "Sound engine", "Звуковий рушій"),
    ("Значки", "Icons", "Значки"),
    ("Значки над никами", "Badges over names", "Значки над ніками"),
    ("Значки над никами игроков", "Icons above player nicks", "Значки над ніками гравців"),
    ("Игроков с таким ником нет.", "No players with that name.", "Гравців з таким ніком немає."),
    ("Изолированная", "Isolated", "Ізольована"),
    ("Как «Локальный»", "Same as Proximity", "Як «Локальний»"),
    ("Клавиша рации", "Push-to-talk key", "Клавіша рації"),
    ("Клавиши", "Hotkeys", "Клавіші"),
    ("Кнопка действия", "Action button", "Кнопка дії"),
    ("Когда рядом играют несколько пластинок, та, на которую смотришь, звучит громко, остальные — тихо", "When several discs play nearby, the one you look at is loud and the rest are quiet", "Коли поруч грають кілька платівок, та, на яку дивишся, звучить гучно, решта — тихо"),
    ("Команды в чате", "Chat commands", "Команди в чаті"),
    ("Кто говорит", "Who's talking", "Хто говорить"),
    ("Кто говорит в вашей группе, отмечен фиолетовой рамкой.", "Players talking in your group have a purple frame.", "Хто говорить у вашій групі, позначений фіолетовою рамкою."),
    ("Локальный", "Proximity", "Локальний"),
    ("Лучше в наушниках", "Better with headphones", "Краще в навушниках"),
    ("Максимум", "Maximum", "Максимум"),
    ("Микрофон", "Microphone", "Мікрофон"),
    ("Микрофон включается, когда нейросеть слышит речь: клавиатура, щелчки и кулер его не включают, а тихий голос — включает. Порог настраивать не нужно", "The microphone turns on when the neural net hears speech: keyboard, clicks and fans don't trigger it, a quiet voice does. No threshold to tune", "Мікрофон вмикається, коли нейромережа чує мову: клавіатура, клацання й кулер його не вмикають, а тихий голос — вмикає. Поріг налаштовувати не треба"),
    ("Микрофон включён", "Microphone on", "Мікрофон увімкнено"),
    ("Микрофон выключен", "Microphone off", "Мікрофон вимкнено"),
    ("Микрофон или динамик над головой: говорит, выключен, без мода", "Microphone or speaker over the head: talking, muted, no mod", "Мікрофон або динамік над головою: говорить, вимкнено, без мода"),
    ("Микрофон не доступен", "Microphone unavailable", "Мікрофон недоступний"),
    ("Микрофон отключён", "Microphone disabled", "Мікрофон відключено"),
    ("Микрофон открылся", "The microphone opened", "Мікрофон відкрився"),
    ("Микрофон, пока вы говорите (фиолетовый — в группу); перечёркнутые микрофон или наушники — выключены", "Microphone while you talk (purple — to the group); crossed-out mic or headphones — turned off", "Мікрофон, поки ви говорите (фіолетовий — у групу); перекреслені мікрофон чи навушники — вимкнено"),
    ("Микрофон: ВКЛ", "Microphone: ON", "Мікрофон: УВІМК"),
    ("Микрофон: ВЫКЛ", "Microphone: OFF", "Мікрофон: ВИМК"),
    ("Микрофон: тонкая настройка", "Microphone: fine tuning", "Мікрофон: тонке налаштування"),
    ("Мод сам понимает, что стоит на сервере: Plasmo Voice или Simple Voice Chat", "The mod detects what the server runs: Plasmo Voice or Simple Voice Chat", "Мод сам визначає, що стоїть на сервері: Plasmo Voice чи Simple Voice Chat"),
    ("Моно", "Mono", "Моно"),
    ("Музыка и звуки", "Music and sounds", "Музика та звуки"),
    ("Музыка с пластинок не слышна, пока не закрепите проигрыватель клавишей «Закрепить пластинку» (вкладка «Клавиши»); слышно только его", "Disc music stays silent until you pin a jukebox with the «Pin disc» key (Keys tab); then only that one plays", "Музику з платівок не чути, доки не закріпите програвач клавішею «Закріпити платівку» (вкладка «Клавіші»); чути лише його"),
    ("На этом сервере нет групп голосового чата.", "This server has no voice chat groups.", "На цьому сервері немає груп голосового чату."),
    ("Наведитесь на игрока, зажмите ПКМ и крутите колесо — его громкость", "Look at a player, hold RMB and scroll: their volume", "Наведіться на гравця, затисніть ПКМ і крутіть коліщатко — його гучність"),
    ("Наведитесь на игрока, зажмите ПКМ и крутите колесо — меняется его громкость", "Look at a player, hold RMB and scroll to change their volume", "Наведіться на гравця, затисніть ПКМ і крутіть коліщатко — змінюється його гучність"),
    ("Нажмите клавишу", "Press a key", "Натисніть клавішу"),
    ("Нажмите клавишу…", "Press a key…", "Натисніть клавішу…"),
    ("Название", "Name", "Назва"),
    ("Название трека над пластинкой", "Track name over the disc", "Назва треку над платівкою"),
    ("Найти группу или игрока в ней…", "Find a group or a player in it…", "Знайти групу або гравця в ній…"),
    ("Найти игрока…", "Find a player…", "Знайти гравця…"),
    ("Направленные источники", "Directional sources", "Спрямовані джерела"),
    ("Направленный звук и HRTF работают только в стерео", "Directional sound and HRTF need stereo", "Спрямований звук і HRTF працюють лише в стерео"),
    ("Насколько нейросеть уверена, что вы говорите. Скажите что-нибудь — полоса станет зелёной", "How sure the neural net is that you're speaking. Say something and the bar turns green", "Наскільки нейромережа впевнена, що ви говорите. Скажіть щось — смуга стане зеленою"),
    ("Не доступен", "Unavailable", "Недоступний"),
    ("Не доступно", "Unavailable", "Недоступно"),
    ("Не нашёл эту группу — войдите из списка", "Couldn't find that group — join it from the list", "Не знайшов цю групу — увійдіть зі списку"),
    ("Не показывать приглашения вовсе (и в чат они не пишутся)", "Don't show invitations at all (they don't go to chat either)", "Не показувати запрошення взагалі (і в чат вони не пишуться)"),
    ("Не удалось вернуться в группу", "Couldn't return to the group", "Не вдалося повернутися до групи"),
    ("Неверный пароль", "Wrong password", "Неправильний пароль"),
    ("Нейросеть RNNoise (как в Plasmo Voice): убирает и ровный шум, и клавиатуру, щелчки, шорох. Меньше 100% — подмешивает исходный звук", "RNNoise neural net (as in Plasmo Voice): removes steady noise as well as keyboard, clicks and rustle. Below 100% it mixes the original sound back in", "Нейромережа RNNoise (як у Plasmo Voice): прибирає і рівний шум, і клавіатуру, клацання, шурхіт. Менше 100% — підмішує початковий звук"),
    ("Нейросеть RNNoise, как в Plasmo Voice: лёгкая, задержка 10 мс", "RNNoise neural net, as in Plasmo Voice: light, 10 ms delay", "Нейромережа RNNoise, як у Plasmo Voice: легка, затримка 10 мс"),
    ("Нейросеть решает, говорите ли вы", "The neural net decides whether you're speaking", "Нейромережа вирішує, чи ви говорите"),
    ("Нет", "No", "Ні"),
    ("Нет групп с таким названием или игроком.", "No groups with that name or player.", "Немає груп з такою назвою чи гравцем."),
    ("Нет подключения к голосу.", "Not connected to voice.", "Немає підключення до голосу."),
    ("Нет — щёлкните", "None: click", "Немає — клацніть"),
    ("Ни от кого", "From nobody", "Ні від кого"),
    ("Ноты над играющей пластинкой", "Notes over a playing disc", "Ноти над платівкою, що грає"),
    ("Ноты разбиваются о блоки и сквозь стены не видны", "Notes shatter on blocks and aren't visible through walls", "Ноти розбиваються об блоки й крізь стіни не видно"),
    ("Нужно подключение к голосу и микрофон", "Needs a voice connection and a microphone", "Потрібне підключення до голосу та мікрофон"),
    ("Нужного нет? Подключите его и откройте список заново.", "Not listed? Plug it in and open the list again.", "Потрібного немає? Підключіть його й відкрийте список знову."),
    ("Обновить", "Refresh", "Оновити"),
    ("Обновляю…", "Refreshing…", "Оновлюю…"),
    ("Объёмный звук", "Spatial sound", "Об'ємний звук"),
    ("Объёмный звук: тонкая настройка", "Spatial sound: fine tuning", "Об'ємний звук: тонке налаштування"),
    ("Обычная", "Normal", "Звичайна"),
    ("Обычное", "Normal", "Звичайне"),
    ("Обычные", "Nearby", "Звичайні"),
    ("Обычные:", "Nearby:", "Звичайні:"),
    ("Оверлей", "Overlay", "Оверлей"),
    ("Основные", "General", "Основні"),
    ("Остальные активации появятся после подключения к голосу.", "Other activations appear once connected to voice.", "Інші активації з'являться після підключення до голосу."),
    ("От всех, кроме", "From all except", "Від усіх, крім"),
    ("Отдельные рации для групп и других активаций появятся,", "Push to talk for groups and other activations", "Окремі рації для груп та інших активацій з'являться,"),
    ("Отказаться", "Decline", "Відмовитися"),
    ("Отключено", "Disabled", "Відключено"),
    ("Откреплено", "Unpinned", "Відкріплено"),
    ("Откроется чат с командой: допишите название, пароль — по желанию (password: …)", "Chat opens with the command: type a name, a password if you want (password: …)", "Відкриється чат з командою: допишіть назву, пароль — за бажанням (password: …)"),
    ("Открытая", "Open", "Відкрита"),
    ("Открыть и закрыть это меню. Без своей клавиши — из Options > Mods", "Open and close this menu. Without its own key: from Options > Mods", "Відкрити й закрити це меню. Без своєї клавіші — з Options > Mods"),
    ("Открыть меню", "Open menu", "Відкрити меню"),
    ("Открыть меню голоса", "Open voice menu", "Відкрити меню голосу"),
    ("Отметьте, от кого не принимать:", "Check whose invitations to ignore:", "Позначте, від кого не приймати:"),
    ("Отметьте, от кого принимать:", "Check whose invitations to accept:", "Позначте, від кого приймати:"),
    ("ПКМ", "RMB", "ПКМ"),
    ("Пароль (необязательно)", "Password (optional)", "Пароль (необов'язково)"),
    ("Пароль группы", "Group password", "Пароль групи"),
    ("Пластинки", "Discs", "Платівки"),
    ("Пластинки и другие звуки из мира, которые шлёт сервер (например, pv-addon-discs)", "Discs and other world sounds the server sends (e.g. pv-addon-discs)", "Платівки та інші звуки світу, які надсилає сервер (наприклад, pv-addon-discs)"),
    ("По клавише", "Push to talk", "За клавішею"),
    ("Подключение к голосу", "Voice connection", "Підключення до голосу"),
    ("Позвать", "Invite", "Покликати"),
    ("Позвать в группу", "Invite to your group", "Покликати в групу"),
    ("Поиск игрока…", "Find a player…", "Пошук гравця…"),
    ("Пока играет пластинка, по проигрывателю пробегает белый блик", "While a disc plays, a white shine runs over the jukebox", "Поки грає платівка, програвачем пробігає білий відблиск"),
    ("Пока никого: игроки с голосом появятся здесь.", "Nobody yet: players with voice will appear here.", "Поки нікого: гравці з голосом з'являться тут."),
    ("Показывать приглашения от всех, кроме отмеченных ниже", "Show invitations from everyone except those checked below", "Показувати запрошення від усіх, крім позначених нижче"),
    ("Показывать приглашения только от отмеченных ниже", "Show invitations only from those checked below", "Показувати запрошення лише від позначених нижче"),
    ("Помолчите 3 секунды: шумодав запомнит ваш фон и не будет глушить долгую речь", "Stay quiet for 3 seconds: the suppressor learns your background and won't cut long speech", "Помовчіть 3 секунди: шумодав запам'ятає ваш фон і не глушитиме довге мовлення"),
    ("Порог", "Threshold", "Поріг"),
    ("Порог активации", "Activation threshold", "Поріг активації"),
    ("Посмотрите на играющий проигрыватель", "Look at a playing jukebox", "Подивіться на програвач, що грає"),
    ("Предыдущая дистанция из разрешённых сервером", "Previous distance allowed by the server", "Попередня дистанція з дозволених сервером"),
    ("Приглашение в группу", "Group invitation", "Запрошення до групи"),
    ("Приглашения", "Invitations", "Запрошення"),
    ("Принять", "Accept", "Прийняти"),
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
    ("Рядом нет игроков с голосовым чатом, кого можно позвать.", "No players with voice chat to invite.", "Немає гравців з голосовим чатом, кого можна покликати."),
    ("Рядом нет игроков с голосовым чатом.", "No players with voice chat nearby.", "Поруч немає гравців з голосовим чатом."),
    ("Сбросить (100%)", "Reset (100%)", "Скинути (100%)"),
    ("Сбросить (вкл)", "Reset (on)", "Скинути (увімк)"),
    ("Сбросить (выкл)", "Reset (off)", "Скинути (вимк)"),
    ("Сбросить громкость", "Reset volume", "Скинути гучність"),
    ("Сбросить дистанцию", "Reset distance", "Скинути дистанцію"),
    ("Сбросить клавишу", "Reset key", "Скинути клавішу"),
    ("Сбросить настройки", "Reset settings", "Скинути налаштування"),
    ("Сбросить порог", "Reset threshold", "Скинути поріг"),
    ("Сбросить тип", "Reset type", "Скинути тип"),
    ("Свой значок над хотбаром", "Your icon above the hotbar", "Свій значок над хотбаром"),
    ("Своё спектральное: убирает ровный шум (кулер, улица). Сильнее — чище, но голос «плоский»", "Our own spectral filter: removes steady noise (fans, street). Stronger is cleaner, but the voice gets flat", "Власне спектральне: прибирає рівний шум (кулер, вулиця). Сильніше — чистіше, але голос «плаский»"),
    ("Сейчас играет", "Now playing", "Зараз грає"),
    ("Сервер голоса не отвечает: порт UDP закрыт?", "Voice server isn't answering: UDP port closed?", "Сервер голосу не відповідає: порт UDP закрито?"),
    ("Сервер не даёт менять дистанцию", "The server doesn't allow changing the distance", "Сервер не дає змінювати дистанцію"),
    ("Сервер принял ваш голосовой клиент", "The server accepted your voice client", "Сервер прийняв ваш голосовий клієнт"),
    ("Сила шумоподавления", "Noise suppression strength", "Сила шумозаглушення"),
    ("Системный вывод", "System output", "Системне виведення"),
    ("Системный микрофон", "System microphone", "Системний мікрофон"),
    ("Системный по умолчанию", "System default", "Системний за замовчуванням"),
    ("Слева сверху", "Top left", "Зліва зверху"),
    ("Слева снизу", "Bottom left", "Зліва знизу"),
    ("Следующая дистанция из разрешённых сервером", "Next distance allowed by the server", "Наступна дистанція з дозволених сервером"),
    ("Слушать только закреплённую", "Hear only the pinned disc", "Слухати лише закріплену"),
    ("Слышу речь", "Hearing speech", "Чую мову"),
    ("Сменить: вкладка «Активации»", "Change it on the Activations tab", "Змінити: вкладка «Активації»"),
    ("Смотрите на проигрыватель и нажмите: он звучит громко, куда бы вы ни смотрели. Ещё раз — снять", "Look at a jukebox and press: it stays loud wherever you look. Press again to unpin", "Дивіться на програвач і натисніть: він звучить гучно, куди б ви не дивилися. Ще раз — зняти"),
    ("Создать", "Create", "Створити"),
    ("Создать группу", "Create a group", "Створити групу"),
    ("Создать группу…", "Create a group…", "Створити групу…"),
    ("Состояние", "Status", "Стан"),
    ("Спасибо проекту", "Thanks to the", "Дякуємо проєкту"),
    ("Список говорящих на экране", "Talker list on screen", "Список тих, хто говорить, на екрані"),
    ("Справа сверху", "Top right", "Справа зверху"),
    ("Справа снизу", "Bottom right", "Справа знизу"),
    ("Срезает всё ниже 80 Гц: гул, вентилятор, стук по столу. Голос не трогает", "Cuts everything below 80 Hz: hum, fans, desk knocks. Leaves the voice alone", "Зрізає все нижче 80 Гц: гул, вентилятор, стукіт по столу. Голос не чіпає"),
    ("Стерео", "Stereo", "Стерео"),
    ("Табличка с названием над проигрывателем", "A name tag above the jukebox", "Табличка з назвою над програвачем"),
    ("Так выглядит оверлей:", "The overlay looks like this:", "Так виглядає оверлей:"),
    ("Тип активации", "Activation type", "Тип активації"),
    ("Тихий голос подтягивается сам, громкий смех не хрипит. Подстраивается только пока вы говорите", "A quiet voice is raised, loud laughter doesn't clip. Adjusts only while you talk", "Тихий голос підтягується сам, гучний сміх не хрипить. Підлаштовується лише поки ви говорите"),
    ("Тишина", "Silence", "Тиша"),
    ("Только группа: ни вас, ни вам — никого рядом", "Group only: nobody nearby hears you, and you hear nobody nearby", "Лише група: ні вас, ні вам — нікого поруч"),
    ("Только от", "Only from", "Лише від"),
    ("Трек не найден", "Track not found", "Трек не знайдено"),
    ("Увеличить дистанцию", "Increase distance", "Збільшити дистанцію"),
    ("Угол экрана:", "Screen corner:", "Кут екрана:"),
    ("Уменьшить дистанцию", "Decrease distance", "Зменшити дистанцію"),
    ("Устройства", "Devices", "Пристрої"),
    ("Устройство ввода", "Input device", "Пристрій введення"),
    ("Устройство вывода", "Output device", "Пристрій виведення"),
    ("Устройство вывода моно: направление не слышно", "Output device is mono: no direction", "Пристрій виведення моно: напрямок не чути"),
    ("Фильтр гула", "Hum filter", "Фільтр гулу"),
    ("Шумодав-нейросеть", "Neural noise suppression", "Шумодав-нейромережа"),
    ("Шумоподавление", "Noise suppression", "Шумозаглушення"),
    ("Шёпот", "Whisper", "Шепіт"),
    ("Щёлкните другой угол на схеме.", "Click another corner on the map.", "Клацніть інший кут на схемі."),
    ("Щёлкните и нажмите клавишу (работает в режиме «По клавише»)", "Click and press a key (works in Push to talk mode)", "Клацніть і натисніть клавішу (працює в режимі «За клавішею»)"),
    ("Щёлкните и нажмите клавишу: говорите, пока она зажата", "Click and press a key: talk while it is held", "Клацніть і натисніть клавішу: говоріть, поки вона затиснута"),
    ("Щёлкните, чтобы сменить", "Click to change", "Клацніть, щоб змінити"),
    ("Язык", "Language", "Мова"),
    ("Язык меню: авто — как в игре", "Menu language: Auto follows the game language", "Мова меню: авто — як у грі"),
    ("выкл", "off", "вимк"),
    ("голосовой чат вкл/выкл", "voice chat on/off", "голосовий чат увімк/вимк"),
    ("если сервер их поддерживает.", "appears when the server has them.", "якщо сервер їх підтримує."),
    ("за поддержку ♥", "project for the support ♥", "за підтримку ♥"),
    ("микрофон вкл/выкл", "microphone on/off", "мікрофон увімк/вимк"),
    ("открыть меню (или /vm)", "open the menu (or /vm)", "відкрити меню (або /vm)"),
    ("речь", "speech", "мова"),
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
        // строки лога (многострочный format!), в игре не показываются
        "сервер Horizon",
        "SVC secret ({kind}): голос {vhost}:{port}, дистанция {}, кодек {}, mtu {}, keep-alive {} мс, группы {}",
        "хендшейк #{n}: minecraft:register {}, PlayerInfo {}, SVC request_secret v{ver} {}, Horizon {}",
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
