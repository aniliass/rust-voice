# Changelog

## 0.1.46

**Русский**

- Группы Simple Voice Chat в меню: список, участники, вход с паролем, выход, создание группы с типом (обычная, открытая, изолированная).
- Обычная группа SVC: «Группы» — группе, «Локальный» — только рядом (мод на это время выходит из группы и возвращается, начало фразы копится и не теряется; если вы в группе один — не выходит).
- Отдельная рация «Группы» на серверах Simple Voice Chat.
- Plasmo Voice: другие игроки теперь видят, что у вас выключен микрофон или звук.
- Значки над никами: перечёркнутые наушники (звук выключен), серый знак «нет» (нет голосового мода); значок стоит точно на нике и не отстаёт при беге и прыжках.
- Громкость игрока (ПКМ + колесо): маленькая фиолетовая плашка над его ником; пока она видна, значок над ником скрыт.
- Simple Voice Chat: мод больше не пересоздаёт сессию, если UDP молчит (сервер не выдаёт ключ второй раз), а подсказывает, что порт закрыт; сообщение «incompatible_version» в чате скрывается.
- Ответ pv-addon-groups на медленном сервере больше не попадает в чат.
- Вкладка «Оверлей» разбита на разделы, строки не вылезают за окно при прокрутке; свой значок над хотбаром можно выключить; подсказки не обрезаются.
- Из «Продвинутых» убраны отладочные счётчики.
- Приглашения в группу (Plasmo, pv-addon-groups): вместо текста в чате — компактная полоска сверху, Y — принять, N — отказаться; в меню выбор, от кого принимать: от всех, кроме отмеченных; только от отмеченных; ни от кого.
- Пластинки: исправлены названия с эмодзи (раньше такая пластинка играла «в уши», без расстояния, стен и нот); заодно правильно читаются ники и надписи с эмодзи.
- Пластинки: по играющему проигрывателю раз в 20 секунд пробегает белый блик (можно выключить).
- Пластинки: громче тот проигрыватель, на который смотрите, остальные рядом — тихо фоном; табличка с названием одна — над выбранным.
- Клавиша «Закрепить пластинку»: проигрыватель звучит громко, куда бы вы ни смотрели (замочек на табличке); снимается клавишей, дальше 24 блоков или когда пластинка доиграла. Режим «Слушать только закреплённую» — без закрепления музыки с пластинок не слышно.
- Ноты и блик прячутся за мобами и игроками (и за вами в F5); эффекты — только у трёх ближайших проигрывателей в кадре, за спиной ничего не считается.
- Проверка микрофона без сервера; поиск групп по названию и нику; значки и головы участников во вкладке «Группы».
- Секрет на логотипе.

**English**

- Simple Voice Chat groups in the menu: list, members, joining with a password, leaving, creating a group of a chosen type (normal, open, isolated).
- Normal SVC group: *Groups* talks to the group, *Local* only nearby (the mod steps out of the group meanwhile and comes back; the start of the phrase is buffered, not lost; it never leaves a group you're alone in).
- A separate *Groups* push-to-talk on Simple Voice Chat servers.
- Plasmo Voice: other players now see when your microphone or sound is off.
- Badges over names: crossed-out headphones (sound off), a grey "no" sign (no voice mod); the badge sits exactly on the name and doesn't lag when players run or jump.
- Player volume (right mouse button + wheel): a small purple tag over their name; their badge is hidden while it shows.
- Simple Voice Chat: the mod no longer restarts the session when UDP is silent (the server won't send a second key) and hints that the port is closed instead; the "incompatible_version" chat message is hidden.
- The pv-addon-groups answer no longer leaks into chat on a slow server.
- The Overlay tab is split into sections and no longer draws past the window when scrolled; your own hotbar badge can be turned off; tooltips aren't cut off.
- Debug counters removed from the Advanced tab.
- Group invitations (Plasmo, pv-addon-groups): a compact strip at the top instead of chat text, Y to accept, N to decline; the menu chooses whose to accept: everyone except the checked ones, only the checked ones, or nobody.
- Discs: titles with emoji fixed (such a disc used to play "in your ears", ignoring distance, walls and notes); names and texts with emoji are read correctly too.
- Discs: a white shine runs over a playing jukebox every 20 seconds (can be turned off).
- Discs: the jukebox you look at is louder, the others nearby play quietly; a single title label, over the chosen one.
- *Pin disc* key: the jukebox stays loud wherever you look (a lock on its label); unpinned by the key, beyond 24 blocks or when the disc ends. *Hear only the pinned disc* mode: no disc music until you pin one.
- Notes and the shine hide behind mobs and players (and behind you in F5); effects only for the three nearest jukeboxes in view, nothing is computed behind you.
- Microphone test without a server; group search by name and member; icons and member heads in the Groups tab.
- A secret on the logo.
