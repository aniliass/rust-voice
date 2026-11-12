//! Plasmo Voice v2: формат пакетов поверх плагин-канала `plasmo:voice/v2` (только та часть протокола,
//! которая нужна клиенту этого мода). Источник формата: github.com/plasmoapp/plasmo-voice, папка `protocol/`.
//! Здесь нет зависимостей от игры, поэтому этот файл проверяется обычными тестами.

pub const CHANNEL: &str = "plasmo:voice/v2";
pub const FLAG_CHANNEL: &str = "plasmo:voice/v2/installed";
pub const REGISTER_CHANNEL: &str = "minecraft:register";
/// Сервер проверяет только совпадение major (2) и минимальную версию.
pub const MOD_VERSION: &str = "2.1.17";
pub const MC_VERSION: &str = "26.3";

/// Номер TCP-пакета PlayerInfoPacket (serverbound) в реестре PacketTcpCodec.
pub const TCP_PLAYER_INFO: u8 = 10;

/// Тестовый публичный RSA-ключ (X.509 DER, 2048 бит) для проверки формата PlayerInfo. В игре не используется:
/// настоящий ключ создаётся заново на каждую сессию (`crypto::RsaKey::generate`).
#[cfg(test)]
pub const PROBE_PUBLIC_KEY: [u8; 294] = [
    0x30, 0x82, 0x01, 0x22, 0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01,
    0x01, 0x05, 0x00, 0x03, 0x82, 0x01, 0x0f, 0x00, 0x30, 0x82, 0x01, 0x0a, 0x02, 0x82, 0x01, 0x01,
    0x00, 0xb2, 0xbd, 0x7d, 0x84, 0x27, 0x88, 0xe8, 0x68, 0xc7, 0x6c, 0x93, 0x32, 0xbb, 0x72, 0x11,
    0x50, 0xd4, 0xcc, 0x0e, 0x2c, 0xaa, 0x0f, 0xb6, 0x71, 0xed, 0x28, 0xad, 0x74, 0x06, 0x76, 0xba,
    0x55, 0x92, 0x98, 0x27, 0x03, 0xcb, 0x5d, 0x89, 0xf0, 0xae, 0x2e, 0xa6, 0x81, 0x72, 0x0f, 0x53,
    0x43, 0xd7, 0xf9, 0x30, 0xdb, 0x7d, 0x40, 0x6a, 0x10, 0x3e, 0x7d, 0x7b, 0x61, 0x68, 0xd2, 0x1a,
    0x17, 0x08, 0x7b, 0x10, 0x4f, 0x6b, 0x1f, 0xb0, 0x4c, 0x52, 0x5c, 0x2c, 0xe7, 0x23, 0x75, 0x36,
    0x3d, 0x2c, 0x1e, 0x02, 0x31, 0x98, 0xf5, 0x39, 0x9e, 0xf1, 0xc0, 0x4c, 0xc9, 0x97, 0xc6, 0xae,
    0x5e, 0x52, 0x43, 0x08, 0xba, 0x98, 0xb4, 0xf0, 0xfc, 0x81, 0xc2, 0x33, 0xda, 0x64, 0x66, 0x97,
    0x42, 0x26, 0xd9, 0xf4, 0x10, 0x61, 0xe7, 0xe0, 0x87, 0x38, 0x21, 0x01, 0x7d, 0xdb, 0x47, 0x2f,
    0x1f, 0x35, 0xca, 0x75, 0x36, 0x36, 0x78, 0x2b, 0x80, 0x15, 0x84, 0xb4, 0xf2, 0x9f, 0xa0, 0x69,
    0x8a, 0xa8, 0x8c, 0xd4, 0x6f, 0x8b, 0x52, 0xb8, 0x5b, 0x24, 0xcb, 0xa9, 0x28, 0x3e, 0xce, 0x2b,
    0xde, 0x96, 0x99, 0xd0, 0xb3, 0xc5, 0x68, 0xa3, 0x25, 0x44, 0x6f, 0x77, 0x86, 0x1e, 0xf4, 0xb5,
    0x09, 0xcf, 0xf5, 0xe7, 0x5f, 0xc0, 0xd5, 0x1d, 0x9e, 0x42, 0xd9, 0x48, 0x73, 0x6f, 0x53, 0xa3,
    0x55, 0xe2, 0x22, 0xa3, 0xf7, 0x99, 0xee, 0x75, 0x7f, 0x2d, 0xf3, 0xae, 0x62, 0xac, 0xb2, 0x84,
    0x58, 0x61, 0xf0, 0xd1, 0x84, 0x51, 0x65, 0x88, 0x54, 0x21, 0x81, 0x6c, 0x87, 0x29, 0x7c, 0xec,
    0x51, 0x22, 0xb5, 0x5c, 0x2b, 0xbe, 0xd1, 0xbb, 0x30, 0x75, 0x3e, 0x4b, 0x1d, 0x9d, 0x06, 0x17,
    0x10, 0x95, 0xb5, 0xe2, 0x00, 0x02, 0xbb, 0x4a, 0x13, 0x98, 0xff, 0x56, 0x7b, 0xa0, 0xd2, 0x9b,
    0x45, 0x02, 0x03, 0x01, 0x00, 0x01,
];

pub fn varint(out: &mut Vec<u8>, mut v: u32) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Тело пакета CustomPayload без id: строка канала (VarInt длина + UTF-8) и данные.
pub fn payload_body(channel: &str, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(channel.len() + data.len() + 5);
    varint(&mut out, channel.len() as u32);
    out.extend_from_slice(channel.as_bytes());
    out.extend_from_slice(data);
    out
}

/// Данные пакета `minecraft:register`: имена каналов через NUL. Объявляем и Plasmo Voice, и Simple Voice
/// Chat: какой из них есть на сервере, тот и ответит.
pub fn register_data() -> Vec<u8> {
    let names: Vec<&str> = [CHANNEL, FLAG_CHANNEL].into_iter().chain(crate::svc::CHANNELS).collect();
    names.join("\0").into_bytes()
}

/// Java `DataOutput.writeUTF` для ASCII: u16 длина big-endian и байты.
fn write_utf(out: &mut Vec<u8>, s: &str) {
    assert!(s.is_ascii() && s.len() < 65536);
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
}

/// PlayerStatePacket (тип 11): клиент сообщает серверу, выключены ли у него звук и микрофон;
/// сервер рассылает это другим (PlayerInfoUpdate), и у них над ником появляется значок.
pub fn player_state_packet(voice_disabled: bool, microphone_muted: bool) -> Vec<u8> {
    vec![11, voice_disabled as u8, microphone_muted as u8]
}

/// PlayerInfoPacket: тип, два флага, версия Minecraft, версия мода, публичный ключ.
pub fn player_info_packet(public_key_der: &[u8], voice_disabled: bool, microphone_muted: bool) -> Vec<u8> {
    let mut out = vec![TCP_PLAYER_INFO, voice_disabled as u8, microphone_muted as u8];
    write_utf(&mut out, MC_VERSION);
    write_utf(&mut out, MOD_VERSION);
    out.extend_from_slice(&(public_key_der.len() as i32).to_be_bytes());
    out.extend_from_slice(public_key_der);
    out
}

/// Название TCP-пакета по номеру из PacketTcpCodec (для логов).
pub fn tcp_name(id: u8) -> &'static str {
    match id {
        1 => "Connection",
        2 => "PlayerInfoRequest",
        3 => "Config",
        4 => "ConfigPlayerInfo",
        5 => "LanguageRequest",
        6 => "Language",
        7 => "PlayerList",
        8 => "PlayerInfoUpdate",
        9 => "PlayerDisconnect",
        10 => "PlayerInfo",
        11 => "PlayerState",
        12 => "PlayerAudioEnd",
        13 => "PlayerActivationDistances",
        14 => "DistanceVisualize",
        15 => "SourceInfoRequest",
        16 => "SourceInfo",
        17 => "SelfSourceInfo",
        18 => "SourceAudioEnd",
        19 => "ActivationRegister",
        20 => "ActivationUnregister",
        21 => "SourceLineRegister",
        22 => "SourceLineUnregister",
        23 => "SourceLinePlayerAdd",
        24 => "SourceLinePlayerRemove",
        25 => "SourceLinePlayersList",
        26 => "AnimatedActionBar",
        _ => "?",
    }
}

pub struct ConnectionInfo {
    pub secret: [u8; 16],
    pub ip: String,
    pub port: i32,
}

/// Разбор ConnectionPacket (после байта типа): UUID (16 байт), UTF ip, int port.
/// Куда слать UDP Plasmo. Сервер присылает адрес из своего конфига; если там «слушать все адреса»
/// (`0.0.0.0`, `::`) или пусто, это не адрес для клиента — тогда, как настоящий клиент Plasmo, берём адрес
/// игрового сервера, на который зашли (`server` — как его ввёл игрок, можно с портом).
pub fn udp_host(ip: &str, server: &str) -> String {
    let ip = ip.trim();
    let any = ip.is_empty() || matches!(ip.trim_matches(['[', ']']), "0.0.0.0" | "::" | "0:0:0:0:0:0:0:0");
    if any { crate::svc::strip_port(server) } else { ip.to_string() }
}

pub fn parse_connection(body: &[u8]) -> Option<ConnectionInfo> {
    let secret = body.get(0..16)?;
    let len = u16::from_be_bytes([*body.get(16)?, *body.get(17)?]) as usize;
    let ip = std::str::from_utf8(body.get(18..18 + len)?).ok()?.to_string();
    let p = body.get(18 + len..22 + len)?;
    let port = i32::from_be_bytes([p[0], p[1], p[2], p[3]]);
    let mut raw = [0u8; 16];
    raw.copy_from_slice(secret);
    Some(ConnectionInfo {
        secret: raw,
        ip,
        port,
    })
}

/// Минимальный читатель в стиле Java DataInput (big-endian).
pub struct Reader<'a> {
    pub buf: &'a [u8],
    pub pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }
    pub fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.buf.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }
    pub fn u8(&mut self) -> Option<u8> {
        Some(self.bytes(1)?[0])
    }
    pub fn bool(&mut self) -> Option<bool> {
        Some(self.u8()? != 0)
    }
    pub fn i32(&mut self) -> Option<i32> {
        Some(i32::from_be_bytes(self.bytes(4)?.try_into().ok()?))
    }
    /// Пропускает строку `writeUTF`, не превращая её в `String`.
    pub fn skip_utf(&mut self) -> Option<()> {
        let n = u16::from_be_bytes(self.bytes(2)?.try_into().ok()?) as usize;
        self.bytes(n)?;
        Some(())
    }
    pub fn f64(&mut self) -> Option<f64> {
        Some(f64::from_be_bytes(self.bytes(8)?.try_into().ok()?))
    }
    pub fn utf(&mut self) -> Option<String> {
        let n = u16::from_be_bytes(self.bytes(2)?.try_into().ok()?) as usize;
        Some(java_utf(self.bytes(n)?))
    }
}

pub struct Config {
    pub sample_rate: i32,
    pub mtu: i32,
    pub codec: String,
    pub codec_params: Vec<(String, String)>,
    pub aes_algorithm: Option<String>,
    /// AES-ключ, зашифрованный нашим публичным RSA-ключом.
    pub aes_key_encrypted: Vec<u8>,
    /// Дистанция по умолчанию у активации `proximity`, если удалось разобрать список.
    pub proximity_distance: Option<i32>,
    /// Имена активаций, которые успели разобрать (для лога).
    pub activation_names: Vec<String>,
    /// Активации сервера целиком: дистанции, вес, «прозрачность» (что из них можно выбрать в меню).
    pub activations: Vec<Activation>,
    /// `ok` или шаг, на котором разбор линий/активаций остановился (для лога).
    pub activations_note: &'static str,
    /// Имена линий источников сервера (`proximity`, `groups`, `discs`…): по ним видно, какие аддоны стоят.
    pub line_names: Vec<String>,
}

/// ConfigPacket (после байта типа): serverId, CaptureInfo, EncryptionInfo. Остальное нам не нужно.
pub fn parse_config(body: &[u8]) -> Option<Config> {
    let mut r = Reader::new(body);
    r.bytes(16)?; // serverId
    let sample_rate = r.i32()?;
    let mtu = r.i32()?;
    let (mut codec, mut codec_params) = (String::new(), Vec::new());
    if r.bool()? {
        codec = r.utf()?;
        let n = r.i32()?;
        if !(0..=128).contains(&n) {
            return None;
        }
        for _ in 0..n {
            let k = r.utf()?;
            let v = r.utf()?;
            codec_params.push((k, v));
        }
    }
    let (mut aes_algorithm, mut aes_key_encrypted) = (None, Vec::new());
    if r.bool()? {
        aes_algorithm = Some(r.utf()?);
        let n = r.i32()?;
        if !(1..=2048).contains(&n) {
            return None;
        }
        aes_key_encrypted = r.bytes(n as usize)?.to_vec();
    }
    let acts = parse_activations(&mut r);
    Some(Config {
        sample_rate,
        mtu,
        codec,
        codec_params,
        aes_algorithm,
        aes_key_encrypted,
        proximity_distance: acts.proximity_distance,
        activation_names: acts.list.iter().map(|a| a.name.clone()).collect(),
        activations: acts.list,
        activations_note: acts.note,
        line_names: acts.lines,
    })
}

fn skip_codec_info(r: &mut Reader) -> Option<()> {
    r.utf()?;
    let n = r.i32()?;
    if !(0..=128).contains(&n) {
        return None;
    }
    for _ in 0..n {
        r.utf()?;
        r.utf()?;
    }
    Some(())
}

struct Activations {
    lines: Vec<String>,
    list: Vec<Activation>,
    proximity_distance: Option<i32>,
    note: &'static str,
}

/// Активация Plasmo (`VoiceActivation`): куда идёт голос — «Локальный» (proximity), «Группы» и т.п.
#[derive(Clone, Debug, PartialEq)]
pub struct Activation {
    pub name: String,
    /// Ключ перевода (`pv.activation.proximity`): по нему подпись в меню.
    pub translation: String,
    /// UUID активации: `activation_id(name)`.
    pub id: [u8; 16],
    /// Какие дистанции можно выбрать: пусто — любые (дистанция не важна), `[-1, max]` — от 1 до max,
    /// иначе ровно эти значения.
    pub distances: Vec<i32>,
    pub default_distance: i32,
    /// Голос слышно вокруг игрока (как «Локальный»), а не только избранным (группа).
    pub proximity: bool,
    /// Сработав, НЕ глушит активации ниже себя по весу.
    pub transitive: bool,
    pub stereo: bool,
    /// Порядок: больше вес — выше в списке и важнее.
    pub weight: i32,
}

/// Дистанции, из которых можно выбрать. `[-1, max]` хранится диапазоном, а не списком: `max` присылает
/// сервер, и при `max = 2^31 - 1` список занял бы ~8 ГБ памяти.
#[derive(Clone, Debug, PartialEq)]
pub enum DistanceChoices {
    /// Любая от 1 до `max` включительно (`max` >= 1).
    Range(i32),
    /// Ровно эти значения (только положительные).
    List(Vec<i32>),
}

impl DistanceChoices {
    pub fn len(&self) -> usize {
        match self {
            DistanceChoices::Range(max) => *max as usize,
            DistanceChoices::List(l) => l.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Дистанция под номером `i` (с нуля).
    pub fn get(&self, i: usize) -> Option<i32> {
        match self {
            DistanceChoices::Range(max) => (i < *max as usize).then(|| i as i32 + 1),
            DistanceChoices::List(l) => l.get(i).copied(),
        }
    }

    /// Номер дистанции `d` среди вариантов.
    pub fn position(&self, d: i32) -> Option<usize> {
        match self {
            DistanceChoices::Range(max) => (1..=*max).contains(&d).then(|| d as usize - 1),
            DistanceChoices::List(l) => l.iter().position(|x| *x == d),
        }
    }
}

impl Activation {
    /// Дистанции для ползунка в меню (пусто — ползунка нет).
    pub fn distance_choices(&self) -> DistanceChoices {
        match self.distances.as_slice() {
            [-1, max] if *max >= 1 => DistanceChoices::Range(*max),
            list => DistanceChoices::List(list.iter().copied().filter(|d| *d > 0).collect()),
        }
    }

    /// Дистанция, которую примет сервер: выбранная, если она разрешена, иначе по умолчанию
    /// (`Activation.calculateAllowedDistance`).
    pub fn allowed_distance(&self, wanted: i32) -> i32 {
        let ok = match self.distances.as_slice() {
            [] => true,
            [-1, max] => wanted >= 1 && wanted <= *max,
            list => list.contains(&wanted),
        };
        if ok { wanted } else { self.default_distance }
    }
}

/// Одна активация в формате `VoiceActivation.serialize` (Config и ActivationRegister).
fn read_activation(r: &mut Reader) -> Result<Activation, &'static str> {
    let name = r.utf().ok_or("имя активации")?;
    let translation = r.utf().ok_or("перевод активации")?;
    r.skip_utf().ok_or("иконка активации")?;
    let n = r.i32().ok_or("число дистанций")?;
    if !(0..=64).contains(&n) {
        return Err("число дистанций вне диапазона");
    }
    let mut distances = Vec::with_capacity(n as usize);
    for _ in 0..n {
        distances.push(r.i32().ok_or("дистанция")?);
    }
    let default_distance = r.i32().ok_or("дистанция по умолчанию")?;
    let proximity = r.bool().ok_or("proximity")?;
    let transitive = r.bool().ok_or("transitive")?;
    let stereo = r.bool().ok_or("stereo")?;
    if r.bool().ok_or("флаг кодека")? {
        skip_codec_info(r).ok_or("кодек активации")?;
    }
    let weight = r.i32().ok_or("вес активации")?;
    let id = activation_id(&name);
    Ok(Activation { name, translation, id, distances, default_distance, proximity, transitive, stereo, weight })
}

/// ActivationRegisterPacket (тип 19, после байта типа): сервер добавил активацию.
pub fn parse_activation_register(body: &[u8]) -> Option<Activation> {
    read_activation(&mut Reader::new(body)).ok()
}

/// ActivationUnregisterPacket (тип 20) и PlayerDisconnect: один UUID.
pub fn parse_uuid(body: &[u8]) -> Option<[u8; 16]> {
    body.get(..16)?.try_into().ok()
}

/// TCP PlayerActivationDistancesPacket (тип 13): какие дистанции вы выбрали для активаций.
pub fn activation_distances_packet(list: &[([u8; 16], i32)]) -> Vec<u8> {
    let mut out = vec![13u8];
    out.extend_from_slice(&(list.len() as i32).to_be_bytes());
    for (id, d) in list {
        out.extend_from_slice(id);
        out.extend_from_slice(&d.to_be_bytes());
    }
    out
}

/// Читает линии источников и активации. Не падает целиком: что успели разобрать, то и вернём,
/// а в `note` будет написано, на каком шаге остановились.
fn parse_activations(r: &mut Reader) -> Activations {
    let mut out = Activations { lines: Vec::new(), list: Vec::new(), proximity_distance: None, note: "ok" };
    if let Err(why) = read_activations(r, &mut out) {
        out.note = why;
    }
    out
}

fn read_activations(r: &mut Reader, out: &mut Activations) -> Result<(), &'static str> {
    let lines = r.i32().ok_or("число линий")?;
    if !(0..=127).contains(&lines) {
        return Err("число линий вне диапазона");
    }
    for _ in 0..lines {
        out.lines.push(r.utf().ok_or("имя линии")?);
        r.skip_utf().ok_or("перевод линии")?;
        r.skip_utf().ok_or("иконка линии")?;
        r.bytes(8).ok_or("громкость линии")?; // defaultVolume
        r.i32().ok_or("вес линии")?;
        if r.bool().ok_or("флаг игроков линии")? {
            // McGameProfileSerializer: UUID, имя, список свойств (3 строки каждое).
            let players = r.i32().ok_or("число игроков линии")?;
            if !(0..=32767).contains(&players) {
                return Err("число игроков линии вне диапазона");
            }
            for _ in 0..players {
                r.bytes(16).ok_or("uuid игрока линии")?;
                r.skip_utf().ok_or("имя игрока линии")?;
                let props = r.i32().ok_or("число свойств игрока")?;
                if !(0..=100).contains(&props) {
                    return Err("число свойств игрока вне диапазона");
                }
                for _ in 0..props {
                    r.skip_utf().ok_or("свойство игрока")?;
                    r.skip_utf().ok_or("значение свойства игрока")?;
                    r.skip_utf().ok_or("подпись свойства игрока")?;
                }
            }
        }
    }
    let acts = r.i32().ok_or("число активаций")?;
    if !(0..=127).contains(&acts) {
        return Err("число активаций вне диапазона");
    }
    for _ in 0..acts {
        let act = read_activation(r)?;
        if act.name == "proximity" {
            out.proximity_distance = Some(act.default_distance);
        }
        out.list.push(act);
    }
    Ok(())
}

/// MD5 (RFC 1321). Нужен только для UUID активации: в Java это `UUID.nameUUIDFromBytes`.
pub fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
        14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15,
        21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32).collect();
    let (mut a0, mut b0, mut c0, mut d0) = (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_le_bytes());
    for chunk in msg.chunks(64) {
        let m: Vec<u32> = chunk.chunks(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f2 = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f2.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

/// Java `UUID.nameUUIDFromBytes` (UUID версии 3).
pub fn name_uuid(name: &[u8]) -> [u8; 16] {
    let mut h = md5(name);
    h[6] = (h[6] & 0x0f) | 0x30;
    h[8] = (h[8] & 0x3f) | 0x80;
    h
}

/// Id активации по её имени: `VoiceActivation.generateId`.
pub fn activation_id(name: &str) -> [u8; 16] {
    name_uuid(format!("{name}_activation").as_bytes())
}

/// UUID линии источников Plasmo по имени (`VoiceSourceLine.generateId`: имя + `_line`).
pub fn line_id(name: &str) -> [u8; 16] {
    name_uuid(format!("{name}_line").as_bytes())
}

pub const UDP_MAGIC: i32 = 0x4e9004e9;
pub const UDP_PING: u8 = 1;

/// UDP-пакет PingPacket: магия, тип, секрет, время, затем тело (время и необязательные ip:port).
pub fn udp_ping(secret: &[u8; 16], now_ms: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(37);
    out.extend_from_slice(&UDP_MAGIC.to_be_bytes());
    out.push(UDP_PING);
    out.extend_from_slice(secret);
    out.extend_from_slice(&(now_ms as i64).to_be_bytes());
    out.extend_from_slice(&(now_ms as i64).to_be_bytes());
    out
}

fn udp_header(out: &mut Vec<u8>, ty: u8, secret: &[u8; 16], now_ms: u64) {
    out.extend_from_slice(&UDP_MAGIC.to_be_bytes());
    out.push(ty);
    out.extend_from_slice(secret);
    out.extend_from_slice(&(now_ms as i64).to_be_bytes());
}

/// Строка Java `writeUTF` (modified UTF-8): эмодзи и прочее вне BMP там — пара суррогатов по 3 байта,
/// а NUL — `C0 80`. Обычный UTF-8 такое не принимает, и из-за одного 🐰 в названии пластинки
/// терялся весь пакет. Склеиваем пары; что не разобрать — заменяем на «�», но строку не теряем.
pub fn java_utf(b: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(b) {
        return s.to_string();
    }
    let mut units: Vec<u16> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let (u, n) = if c < 0x80 {
            (c as u16, 1)
        } else if c & 0xE0 == 0xC0 && i + 1 < b.len() {
            ((((c & 0x1F) as u16) << 6) | (b[i + 1] & 0x3F) as u16, 2)
        } else if c & 0xF0 == 0xE0 && i + 2 < b.len() {
            ((((c & 0x0F) as u16) << 12) | (((b[i + 1] & 0x3F) as u16) << 6) | (b[i + 2] & 0x3F) as u16, 3)
        } else if c & 0xF8 == 0xF0 && i + 3 < b.len() {
            // настоящий 4-байтный UTF-8 вперемешку: раскладываем на пару
            let cp = (((c & 0x07) as u32) << 18) | (((b[i + 1] & 0x3F) as u32) << 12) | (((b[i + 2] & 0x3F) as u32) << 6) | (b[i + 3] & 0x3F) as u32;
            let mut buf = [0u16; 2];
            units.extend_from_slice(char::from_u32(cp).unwrap_or('\u{FFFD}').encode_utf16(&mut buf));
            i += 4;
            continue;
        } else {
            (0xFFFD, 1)
        };
        units.push(u);
        i += n;
    }
    String::from_utf16_lossy(&units)
}

/// UDP PlayerAudioPacket: seq, данные (уже зашифрованные), activationId, дистанция, stereo.
pub fn udp_player_audio(
    secret: &[u8; 16],
    now_ms: u64,
    seq: i64,
    data: &[u8],
    activation: &[u8; 16],
    distance: i16,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 64);
    udp_header(&mut out, 2, secret, now_ms);
    out.extend_from_slice(&seq.to_be_bytes());
    out.extend_from_slice(&(data.len() as i32).to_be_bytes());
    out.extend_from_slice(data);
    out.extend_from_slice(activation);
    out.extend_from_slice(&distance.to_be_bytes());
    out.push(0); // stereo = false
    out
}

pub struct SourceAudio {
    pub seq: i64,
    pub data: Vec<u8>,
    pub source_id: [u8; 16],
    /// Поле протокола Plasmo (состояние источника). Читаем, чтобы не сбить разбор, но пока не используем.
    #[allow(dead_code)]
    pub state: u8,
    pub distance: i16,
}

/// Разбор целого UDP-датаграмма: возвращает (тип, тело после заголовка 29 байт).
pub fn udp_split(datagram: &[u8]) -> Option<(u8, &[u8])> {
    if datagram.len() < 29 || datagram[0..4] != UDP_MAGIC.to_be_bytes() {
        return None;
    }
    Some((datagram[4], &datagram[29..]))
}

pub fn parse_source_audio(body: &[u8]) -> Option<SourceAudio> {
    let mut r = Reader::new(body);
    let seq = i64::from_be_bytes(r.bytes(8)?.try_into().ok()?);
    let n = r.i32()?;
    if !(1..=2048).contains(&n) {
        return None;
    }
    let data = r.bytes(n as usize)?.to_vec();
    let mut source_id = [0u8; 16];
    source_id.copy_from_slice(r.bytes(16)?);
    let state = r.u8()?;
    let distance = i16::from_be_bytes(r.bytes(2)?.try_into().ok()?);
    Some(SourceAudio { seq, data, source_id, state, distance })
}

/// TCP PlayerAudioEndPacket (тип 12): конец реплики.
pub fn player_audio_end(seq: i64, activation: &[u8; 16], distance: i16) -> Vec<u8> {
    let mut out = vec![12u8];
    out.extend_from_slice(&seq.to_be_bytes());
    out.extend_from_slice(activation);
    out.extend_from_slice(&distance.to_be_bytes());
    out
}

pub fn hex_prefix(data: &[u8], max: usize) -> String {
    data.iter().take(max).map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_matches_minecraft() {
        let mut v = Vec::new();
        varint(&mut v, 127);
        varint(&mut v, 128);
        varint(&mut v, 300);
        assert_eq!(v, [0x7f, 0x80, 0x01, 0xac, 0x02]);
    }

    #[test]
    fn payload_has_channel_then_data() {
        let b = payload_body("minecraft:register", &[1, 2, 3]);
        assert_eq!(b[0] as usize, "minecraft:register".len());
        assert_eq!(&b[1..19], "minecraft:register".as_bytes());
        assert_eq!(&b[19..], [1, 2, 3]);
    }

    #[test]
    fn java_utf_reads_emoji_and_nul() {
        // 🐰 = U+1F430 = D83D DC30 → ED A0 BD ED B0 B0
        assert_eq!(java_utf(&[b'H', b'i', 0xED, 0xA0, 0xBD, 0xED, 0xB0, 0xB0]), "Hi🐰");
        assert_eq!(java_utf(&[b'a', 0xC0, 0x80, b'b']), "a\0b");
        assert_eq!(java_utf("Жми ♥ 🐰".as_bytes()), "Жми ♥ 🐰");
        let mut r = Reader::new(&[0, 6, 0xED, 0xA0, 0xBD, 0xED, 0xB0, 0xB0]);
        assert_eq!(r.utf().as_deref(), Some("🐰"));
    }

    #[test]
    fn register_lists_both_channels() {
        let d = register_data();
        let s = String::from_utf8(d).unwrap();
        assert!(s.starts_with("plasmo:voice/v2\0plasmo:voice/v2/installed\0"), "{s}");
        assert!(s.contains("voicechat:secret") && s.contains("voicechat:request_secret"), "и каналы SVC");
    }

    #[test]
    fn player_info_layout() {
        let p = player_info_packet(&PROBE_PUBLIC_KEY, false, true);
        assert_eq!(p[0], 10);
        assert_eq!((p[1], p[2]), (0, 1));
        // writeUTF("26.3")
        assert_eq!(&p[3..5], [0, 4]);
        assert_eq!(&p[5..9], b"26.3");
        assert_eq!(&p[9..11], [0, 6]);
        assert_eq!(&p[11..17], b"2.1.17");
        assert_eq!(i32::from_be_bytes([p[17], p[18], p[19], p[20]]), 294);
        assert_eq!(&p[21..], PROBE_PUBLIC_KEY);
    }

    #[test]
    fn key_is_x509_rsa() {
        // SEQUENCE, затем OID rsaEncryption 1.2.840.113549.1.1.1
        assert_eq!(PROBE_PUBLIC_KEY[0], 0x30);
        assert!(PROBE_PUBLIC_KEY.windows(9).any(|w| w == [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01]));
    }

    #[test]
    fn config_parses() {
        let mut b = vec![0x11u8; 16];
        b.extend_from_slice(&48000i32.to_be_bytes());
        b.extend_from_slice(&1024i32.to_be_bytes());
        b.push(1);
        b.extend_from_slice(&[0, 4]);
        b.extend_from_slice(b"opus");
        b.extend_from_slice(&1i32.to_be_bytes());
        b.extend_from_slice(&[0, 4]);
        b.extend_from_slice(b"mode");
        b.extend_from_slice(&[0, 4]);
        b.extend_from_slice(b"VOIP");
        b.push(1);
        b.extend_from_slice(&[0, 20]);
        b.extend_from_slice(b"AES/CBC/PKCS5Padding");
        b.extend_from_slice(&256i32.to_be_bytes());
        b.extend_from_slice(&[0xaa; 256]);
        let c = parse_config(&b).unwrap();
        assert_eq!((c.sample_rate, c.mtu), (48000, 1024));
        assert_eq!(c.codec, "opus");
        assert_eq!(c.codec_params, [("mode".to_string(), "VOIP".to_string())]);
        assert_eq!(c.aes_algorithm.as_deref(), Some("AES/CBC/PKCS5Padding"));
        assert_eq!(c.aes_key_encrypted.len(), 256);
    }

    fn put_utf(b: &mut Vec<u8>, s: &str) {
        b.extend_from_slice(&(s.len() as u16).to_be_bytes());
        b.extend_from_slice(s.as_bytes());
    }

    /// Config с линией, у которой есть список игроков (раньше разбор на этом сдавался и давал `proximity None`).
    fn config_with_lines_and_activations() -> Vec<u8> {
        let mut b = vec![0x11u8; 16];
        b.extend_from_slice(&48000i32.to_be_bytes());
        b.extend_from_slice(&1000i32.to_be_bytes());
        b.push(1);
        put_utf(&mut b, "opus");
        b.extend_from_slice(&0i32.to_be_bytes());
        b.push(1);
        put_utf(&mut b, "AES/CBC/PKCS5Padding");
        b.extend_from_slice(&256i32.to_be_bytes());
        b.extend_from_slice(&[0xaa; 256]);
        // линии: одна, с двумя игроками (у одного два свойства)
        b.extend_from_slice(&1i32.to_be_bytes());
        put_utf(&mut b, "vip");
        put_utf(&mut b, "pv.line.vip");
        put_utf(&mut b, "plasmovoice:textures/addon/vip.png");
        b.extend_from_slice(&1.0f64.to_be_bytes());
        b.extend_from_slice(&5i32.to_be_bytes());
        b.push(1);
        b.extend_from_slice(&2i32.to_be_bytes());
        b.extend_from_slice(&[0x01; 16]);
        put_utf(&mut b, "Steve");
        b.extend_from_slice(&2i32.to_be_bytes());
        for _ in 0..2 {
            put_utf(&mut b, "textures");
            put_utf(&mut b, "dmFsdWU=");
            put_utf(&mut b, "sig");
        }
        b.extend_from_slice(&[0x02; 16]);
        put_utf(&mut b, "Alex");
        b.extend_from_slice(&0i32.to_be_bytes());
        // активации: proximity и ptt без кодека/с кодеком
        b.extend_from_slice(&2i32.to_be_bytes());
        for (name, with_codec) in [("proximity", false), ("groups", true)] {
            put_utf(&mut b, name);
            put_utf(&mut b, "pv.activation");
            put_utf(&mut b, "icon");
            b.extend_from_slice(&3i32.to_be_bytes());
            for d in [8i32, 16, 32] {
                b.extend_from_slice(&d.to_be_bytes());
            }
            b.extend_from_slice(&16i32.to_be_bytes());
            b.extend_from_slice(&[1, 0, 1]);
            if with_codec {
                b.push(1);
                put_utf(&mut b, "opus");
                b.extend_from_slice(&0i32.to_be_bytes());
            } else {
                b.push(0);
            }
            b.extend_from_slice(&1i32.to_be_bytes());
        }
        b
    }

    #[test]
    fn config_survives_player_lines_and_finds_proximity() {
        let c = parse_config(&config_with_lines_and_activations()).unwrap();
        assert_eq!(c.activations_note, "ok");
        assert_eq!(c.activation_names, ["proximity", "groups"]);
        assert_eq!(c.proximity_distance, Some(16));
        assert_eq!(c.line_names, ["vip"]);
    }

    #[test]
    fn activations_keep_distances_weight_and_flags() {
        let c = parse_config(&config_with_lines_and_activations()).unwrap();
        let prox = &c.activations[0];
        assert_eq!(prox.id, activation_id("proximity"));
        assert_eq!(prox.distances, [8, 16, 32]);
        assert_eq!((prox.default_distance, prox.proximity, prox.transitive, prox.weight), (16, true, false, 1));
        assert_eq!(prox.distance_choices(), DistanceChoices::List(vec![8, 16, 32]));
        assert_eq!(prox.allowed_distance(32), 32);
        assert_eq!(prox.allowed_distance(20), 16, "чего нет в списке — по умолчанию");
        // ActivationRegister — та же запись отдельно
        let mut body = Vec::new();
        put_utf(&mut body, "groups");
        put_utf(&mut body, "pv.activation.groups");
        put_utf(&mut body, "icon");
        body.extend_from_slice(&2i32.to_be_bytes());
        body.extend_from_slice(&(-1i32).to_be_bytes());
        body.extend_from_slice(&64i32.to_be_bytes());
        body.extend_from_slice(&0i32.to_be_bytes());
        body.extend_from_slice(&[0, 1, 0, 0]); // не proximity, transitive, без стерео, без кодека
        body.extend_from_slice(&5i32.to_be_bytes());
        let g = parse_activation_register(&body).unwrap();
        assert_eq!(g.distance_choices().len(), 64, "[-1, 64] — любая от 1 до 64");
        assert_eq!(g.distance_choices().get(0), Some(1));
        assert_eq!(g.distance_choices().get(63), Some(64));
        assert_eq!(g.distance_choices().get(64), None);
        assert_eq!(g.distance_choices().position(40), Some(39));
        assert_eq!(g.distance_choices().position(65), None);
        assert_eq!(g.allowed_distance(40), 40);
        assert_eq!(g.allowed_distance(0), 0);
        assert!(g.transitive && !g.proximity);
        assert_eq!(parse_uuid(&g.id), Some(g.id));
        let pkt = activation_distances_packet(&[(g.id, 40)]);
        assert_eq!(pkt.len(), 1 + 4 + 16 + 4);
        assert_eq!(pkt[0], 13);
    }

    #[test]
    fn truncated_activations_say_where_they_stopped() {
        let full = config_with_lines_and_activations();
        let c = parse_config(&full[..full.len() - 30]).unwrap();
        assert_ne!(c.activations_note, "ok");
        assert_eq!(c.proximity_distance, Some(16)); // proximity успели прочитать до обрыва
    }

    #[test]
    fn md5_known_vectors() {
        let hex = |d: [u8; 16]| d.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(hex(md5(b"")), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(hex(md5(b"abc")), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(hex(md5(&[b'a'; 100])), "36a92cc94a9e0fa21f625f8bfb007adf");
    }

    #[test]
    fn proximity_activation_id() {
        // эталон посчитан python: uuid.UUID(bytes=md5("proximity_activation") с version=3)
        let id = activation_id("proximity");
        let hex = id.iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert_eq!(hex, "4aec07bad10933459a0a92022a116cd0");
    }

    #[test]
    fn player_audio_roundtrips_as_source_audio_shape() {
        let s = [3u8; 16];
        let a = [4u8; 16];
        let pkt = udp_player_audio(&s, 5, 7, &[9, 9, 9], &a, 32);
        let (ty, body) = udp_split(&pkt).unwrap();
        assert_eq!(ty, 2);
        // PlayerAudio: seq, len, data, activation, distance, stereo
        assert_eq!(i64::from_be_bytes(body[0..8].try_into().unwrap()), 7);
        assert_eq!(&body[12..15], [9, 9, 9]);
        assert_eq!(&body[15..31], a);
        assert_eq!(i16::from_be_bytes(body[31..33].try_into().unwrap()), 32);
        assert_eq!(body.len(), 34);
    }

    #[test]
    fn source_audio_parses() {
        let mut b = Vec::new();
        b.extend_from_slice(&42i64.to_be_bytes());
        b.extend_from_slice(&3i32.to_be_bytes());
        b.extend_from_slice(&[1, 2, 3]);
        b.extend_from_slice(&[8u8; 16]);
        b.push(0);
        b.extend_from_slice(&16i16.to_be_bytes());
        let s = parse_source_audio(&b).unwrap();
        assert_eq!((s.seq, s.data.as_slice(), s.distance), (42, &[1u8, 2, 3][..], 16));
        assert_eq!(s.source_id, [8u8; 16]);
        assert!(parse_source_audio(&b[..20]).is_none());
    }

    #[test]
    fn audio_end_layout() {
        let p = player_audio_end(5, &[1u8; 16], 48);
        assert_eq!(p.len(), 1 + 8 + 16 + 2);
        assert_eq!(p[0], 12);
    }

    #[test]
    fn udp_ping_layout() {
        let s = [7u8; 16];
        let p = udp_ping(&s, 1_700_000_000_000);
        assert_eq!(&p[0..4], [0x4e, 0x90, 0x04, 0xe9]);
        assert_eq!(p[4], 1);
        assert_eq!(&p[5..21], s);
        assert_eq!(p.len(), 37);
        assert_eq!(i64::from_be_bytes(p[21..29].try_into().unwrap()), 1_700_000_000_000);
    }

    #[test]
    fn huge_server_distance_range_is_not_allocated() {
        // сервер прислал [-1, 2^31 - 1]: список занял бы ~8 ГБ, диапазон — ничего
        let a = Activation {
            name: "proximity".into(),
            translation: String::new(),
            id: activation_id("proximity"),
            distances: vec![-1, i32::MAX],
            default_distance: 16,
            proximity: true,
            transitive: false,
            stereo: false,
            weight: 0,
        };
        let c = a.distance_choices();
        assert_eq!(c, DistanceChoices::Range(i32::MAX));
        assert_eq!(c.len(), i32::MAX as usize);
        assert_eq!(c.get(c.len() - 1), Some(i32::MAX));
        assert_eq!(c.get(c.len()), None);
        assert_eq!(c.position(i32::MAX), Some(c.len() - 1));
        assert_eq!(c.position(0), None);
        assert_eq!(c.position(-5), None);
        // [-1, 0] и прочие пустые варианты — ползунка нет
        let none = Activation { distances: vec![-1, 0], ..a.clone() };
        assert!(none.distance_choices().is_empty());
    }

    #[test]
    fn connection_parses() {
        let mut b = vec![0xab; 16];
        b.extend_from_slice(&[0, 9]);
        b.extend_from_slice(b"127.0.0.1");
        b.extend_from_slice(&25565i32.to_be_bytes());
        let c = parse_connection(&b).unwrap();
        assert_eq!(c.ip, "127.0.0.1");
        assert_eq!(c.port, 25565);
        assert_eq!(c.secret.len(), 16);
        assert!(parse_connection(&b[..10]).is_none());
    }

    /// Сервер слушает все адреса (0.0.0.0): голос идёт на адрес игрового сервера, как у клиента Plasmo.
    #[test]
    fn any_address_means_the_game_server() {
        assert_eq!(udp_host("0.0.0.0", "5.83.140.202:25765"), "5.83.140.202");
        assert_eq!(udp_host("", "mc.example.org"), "mc.example.org");
        assert_eq!(udp_host("::", "[2001:db8::1]:25565"), "2001:db8::1");
        assert_eq!(udp_host("voice.example.org", "mc.example.org"), "voice.example.org", "свой адрес не трогаем");
        assert_eq!(udp_host("10.0.0.5", "mc.example.org"), "10.0.0.5");
    }
}
