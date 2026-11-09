//! Simple Voice Chat: своя совместимая реализация протокола (каналы `voicechat:*` и UDP).
//!
//! Исходники SVC (github.com/henkelmax/simple-voice-chat) открыты только для чтения («All Rights Reserved»),
//! поэтому здесь нет ни их кода, ни их картинок: только формат пакетов, описанный своими словами.
//!
//! Как подключается клиент SVC:
//! 1. TCP (плагин-каналы): `voicechat:request_secret` с версией совместимости; сервер отвечает
//!    `voicechat:secret` (ключ, id соединения, порт и адрес UDP, кодек, дистанция...). Имена и состояния
//!    игроков приходят в `voicechat:states` / `voicechat:state` / `voicechat:remove_state`.
//! 2. UDP: каждый пакет — `0xFF`, id соединения (16 байт), длина (VarInt) и зашифрованное AES-128-GCM тело
//!    (IV 12 байт + шифртекст + метка 16 байт). Внутри — байт типа и поля пакета.
//!    Клиент раз в секунду шлёт Authenticate, пока не придёт AuthenticateAck (с nonce), потом
//!    ConnectionCheck(nonce), пока не придёт ConnectionCheckAck. Дальше сервер шлёт KeepAlive — отвечаем тем же.
//!
//! Здесь нет API игры: всё проверяется обычными тестами.

use aes::cipher::{BlockEncrypt, KeyInit};

pub const CH_REQUEST_SECRET: &str = "voicechat:request_secret";
pub const CH_SECRET: &str = "voicechat:secret";
pub const CH_STATES: &str = "voicechat:states";
pub const CH_STATE: &str = "voicechat:state";
pub const CH_REMOVE_STATE: &str = "voicechat:remove_state";
pub const CH_CLIENT_STATE: &str = "voicechat:client_state";
/// Каналы, о которых клиент сообщает серверу (`minecraft:register`): без этого Paper не шлёт по ним пакеты.
pub const CHANNELS: [&str; 6] = [CH_REQUEST_SECRET, CH_SECRET, CH_STATES, CH_STATE, CH_REMOVE_STATE, CH_CLIENT_STATE];
/// Версия совместимости SVC для Minecraft 26.x: сервер отдаёт ключ только клиенту с той же версией.
pub const COMPATIBILITY_VERSION: i32 = 21;

const MAGIC: u8 = 0xFF;

// ---------- байты в стиле FriendlyByteBuf ----------

fn varint(out: &mut Vec<u8>, mut v: u32) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

/// Читатель FriendlyByteBuf: big-endian числа, VarInt-длины, UUID как два long.
pub struct Buf<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Buf<'a> {
    pub fn new(b: &'a [u8]) -> Buf<'a> {
        Buf { b, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn bool(&mut self) -> Option<bool> {
        Some(self.u8()? != 0)
    }
    fn i32(&mut self) -> Option<i32> {
        Some(i32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }
    fn i64(&mut self) -> Option<i64> {
        Some(i64::from_be_bytes(self.take(8)?.try_into().ok()?))
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }
    fn f64(&mut self) -> Option<f64> {
        Some(f64::from_be_bytes(self.take(8)?.try_into().ok()?))
    }
    fn uuid(&mut self) -> Option<[u8; 16]> {
        self.take(16)?.try_into().ok()
    }
    fn varint(&mut self) -> Option<u32> {
        let mut v = 0u32;
        for i in 0..5 {
            let b = self.u8()?;
            v |= ((b & 0x7f) as u32) << (7 * i);
            if b & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }
    fn bytes(&mut self, max: usize) -> Option<&'a [u8]> {
        let n = self.varint()? as usize;
        if n > max {
            return None;
        }
        self.take(n)
    }
    fn utf(&mut self, max: usize) -> Option<String> {
        String::from_utf8(self.bytes(max * 4)?.to_vec()).ok()
    }
}

// ---------- AES-128-GCM (своя, поверх крейта aes: в Cargo.lock Horizon нет aes-gcm) ----------

fn gf_mul(x: u128, y: u128) -> u128 {
    const R: u128 = 0xE1 << 120;
    let (mut z, mut v) = (0u128, y);
    for i in 0..128 {
        if (x >> (127 - i)) & 1 == 1 {
            z ^= v;
        }
        v = if v & 1 == 1 { (v >> 1) ^ R } else { v >> 1 };
    }
    z
}

struct Gcm {
    aes: aes::Aes128,
    h: u128,
}

impl Gcm {
    fn new(key: &[u8; 16]) -> Gcm {
        let aes = aes::Aes128::new(key.into());
        let mut zero = [0u8; 16].into();
        aes.encrypt_block(&mut zero);
        Gcm { aes, h: u128::from_be_bytes(zero.into()) }
    }

    fn block(&self, counter: u128) -> [u8; 16] {
        let mut b = counter.to_be_bytes().into();
        self.aes.encrypt_block(&mut b);
        b.into()
    }

    /// Счётчик: шифрование и расшифровка одинаковы. `j0` — начальный блок (IV || 1).
    fn ctr(&self, j0: u128, data: &mut [u8]) {
        let mut ctr = j0;
        for chunk in data.chunks_mut(16) {
            // увеличиваем младшие 32 бита
            ctr = (ctr & !0xFFFF_FFFFu128) | ((ctr as u32).wrapping_add(1) as u128);
            let ks = self.block(ctr);
            for (d, k) in chunk.iter_mut().zip(ks) {
                *d ^= k;
            }
        }
    }

    fn tag(&self, j0: u128, ct: &[u8]) -> [u8; 16] {
        let mut y = 0u128;
        for chunk in ct.chunks(16) {
            let mut b = [0u8; 16];
            b[..chunk.len()].copy_from_slice(chunk);
            y = gf_mul(y ^ u128::from_be_bytes(b), self.h);
        }
        let lens = (ct.len() as u128) * 8; // длина дополнительных данных 0, шифртекста — в битах
        y = gf_mul(y ^ lens, self.h);
        (u128::from_be_bytes(self.block(j0)) ^ y).to_be_bytes()
    }

    fn j0(iv: &[u8; 12]) -> u128 {
        let mut b = [0u8; 16];
        b[..12].copy_from_slice(iv);
        b[15] = 1;
        u128::from_be_bytes(b)
    }

    /// IV (12) + шифртекст + метка (16), как `Secret.encrypt` у SVC.
    fn seal(&self, iv: &[u8; 12], plain: &[u8]) -> Vec<u8> {
        let j0 = Gcm::j0(iv);
        let mut ct = plain.to_vec();
        self.ctr(j0, &mut ct);
        let tag = self.tag(j0, &ct);
        [iv.as_slice(), &ct, &tag].concat()
    }

    fn open(&self, payload: &[u8]) -> Option<Vec<u8>> {
        if payload.len() < 12 + 16 {
            return None;
        }
        let iv: [u8; 12] = payload[..12].try_into().ok()?;
        let (ct, tag) = payload[12..].split_at(payload.len() - 12 - 16);
        let j0 = Gcm::j0(&iv);
        // сравнение метки без раннего выхода
        let want = self.tag(j0, ct);
        if want.iter().zip(tag).fold(0u8, |acc, (a, b)| acc | (a ^ b)) != 0 {
            return None;
        }
        let mut plain = ct.to_vec();
        self.ctr(j0, &mut plain);
        Some(plain)
    }
}

// ---------- TCP: плагин-каналы ----------

/// `voicechat:request_secret`: версия совместимости.
pub fn request_secret(version: i32) -> Vec<u8> {
    version.to_be_bytes().to_vec()
}

/// `voicechat:client_state`: выключен ли у вас микрофон и голосовой чат.
pub fn client_state(muted: bool, disabled: bool) -> Vec<u8> {
    vec![muted as u8, disabled as u8]
}

/// Что сервер прислал в `voicechat:secret`.
#[derive(Clone, Debug, PartialEq)]
pub struct Secret {
    pub key: [u8; 16],
    pub connection_id: [u8; 16],
    pub port: i32,
    pub player: [u8; 16],
    /// 0 — VOIP, 1 — AUDIO, 2 — RESTRICTED_LOWDELAY (как приложение Opus).
    pub codec: u8,
    pub mtu: i32,
    pub distance: f64,
    /// Через сколько мс сервер шлёт KeepAlive.
    pub keep_alive: i32,
    pub groups: bool,
    /// Отдельный адрес голоса: пусто — адрес игрового сервера и `port`; число — другой порт; иначе хост[:порт].
    pub voice_host: String,
}

pub fn parse_secret(data: &[u8]) -> Option<Secret> {
    let mut b = Buf::new(data);
    Some(Secret {
        key: b.uuid()?,
        connection_id: b.uuid()?,
        port: b.i32()?,
        player: b.uuid()?,
        codec: b.u8()?,
        mtu: b.i32()?,
        distance: b.f64()?,
        keep_alive: b.i32()?,
        groups: b.bool()?,
        voice_host: b.utf(32767)?,
    })
}

/// Куда слать UDP: (хост, порт). `server` — адрес игрового сервера, как игрок его ввёл (`хост` или `хост:порт`).
pub fn voice_address(s: &Secret, server: &str) -> (String, u16) {
    let server_host = strip_port(server);
    let host = s.voice_host.trim();
    let port = s.port.clamp(0, 65535) as u16;
    if host.is_empty() {
        return (server_host, port);
    }
    if let Ok(p) = host.parse::<u16>() {
        return (server_host, p);
    }
    match host.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') || h.starts_with('[') => match p.parse::<u16>() {
            Ok(p) => (h.trim_matches(['[', ']']).to_string(), p),
            Err(_) => (host.to_string(), port),
        },
        _ => (host.to_string(), port),
    }
}

fn strip_port(addr: &str) -> String {
    let a = addr.trim();
    if let Some(rest) = a.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest).to_string();
    }
    match a.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') && p.parse::<u16>().is_ok() => h.to_string(),
        _ => a.to_string(),
    }
}

/// Состояние игрока SVC: ник и что у него выключено.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerState {
    pub uuid: [u8; 16],
    pub name: String,
    pub muted: bool,
    pub disabled: bool,
    /// Не подключён к голосу (мод есть, но UDP не дошёл, или вышел).
    pub disconnected: bool,
    pub group: Option<[u8; 16]>,
}

fn read_state(b: &mut Buf) -> Option<PlayerState> {
    let flags = b.u8()?;
    let bit = |i: u8| flags & (1 << i) != 0;
    let uuid = b.uuid()?;
    let name = b.utf(256)?;
    let group = if bit(5) { Some(b.uuid()?) } else { None };
    Some(PlayerState { uuid, name, muted: bit(0) || bit(1), disabled: bit(2) || bit(3), disconnected: bit(4), group })
}

/// `voicechat:states`: все игроки.
pub fn parse_states(data: &[u8]) -> Option<Vec<PlayerState>> {
    let mut b = Buf::new(data);
    let n = b.i32()?;
    if !(0..=10_000).contains(&n) {
        return None;
    }
    (0..n).map(|_| read_state(&mut b)).collect()
}

/// `voicechat:state`: один игрок.
pub fn parse_state(data: &[u8]) -> Option<PlayerState> {
    read_state(&mut Buf::new(data))
}

/// `voicechat:remove_state`: UUID игрока, который ушёл.
pub fn parse_remove_state(data: &[u8]) -> Option<[u8; 16]> {
    Buf::new(data).uuid()
}

// ---------- UDP ----------

pub const T_MIC: u8 = 0x1;
pub const T_PLAYER_SOUND: u8 = 0x2;
pub const T_GROUP_SOUND: u8 = 0x3;
pub const T_LOCATION_SOUND: u8 = 0x4;
pub const T_AUTHENTICATE: u8 = 0x5;
pub const T_AUTHENTICATE_ACK: u8 = 0x6;
pub const T_PING: u8 = 0x7;
pub const T_KEEP_ALIVE: u8 = 0x8;
pub const T_CONNECTION_CHECK: u8 = 0x9;
pub const T_CONNECTION_CHECK_ACK: u8 = 0xA;

/// Ключи UDP-сессии SVC.
pub struct Session {
    gcm: Gcm,
    pub connection_id: [u8; 16],
}

impl Session {
    pub fn new(key: &[u8; 16], connection_id: [u8; 16]) -> Session {
        Session { gcm: Gcm::new(key), connection_id }
    }

    /// Готовый датаграмм клиента: магия, id соединения, длина и зашифрованное (тип + тело).
    pub fn wrap(&self, ty: u8, body: &[u8]) -> Vec<u8> {
        let mut iv = [0u8; 12];
        crate::crypto::random_bytes(&mut iv);
        self.wrap_with_iv(ty, body, &iv)
    }

    fn wrap_with_iv(&self, ty: u8, body: &[u8], iv: &[u8; 12]) -> Vec<u8> {
        let plain = [&[ty][..], body].concat();
        let sealed = self.gcm.seal(iv, &plain);
        let mut out = Vec::with_capacity(sealed.len() + 22);
        out.push(MAGIC);
        out.extend_from_slice(&self.connection_id);
        varint(&mut out, sealed.len() as u32);
        out.extend_from_slice(&sealed);
        out
    }

    /// Датаграмм от сервера: магия, длина и зашифрованное тело. Возвращает (тип, тело).
    pub fn unwrap(&self, datagram: &[u8]) -> Option<(u8, Vec<u8>)> {
        let mut b = Buf::new(datagram);
        if b.u8()? != MAGIC {
            return None;
        }
        let sealed = b.bytes(64 * 1024)?;
        let plain = self.gcm.open(sealed)?;
        let (&ty, body) = plain.split_first()?;
        Some((ty, body.to_vec()))
    }
}

/// Тело MicPacket: Opus, номер, шёпот.
pub fn mic(opus: &[u8], seq: i64, whispering: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(opus.len() + 12);
    varint(&mut out, opus.len() as u32);
    out.extend_from_slice(opus);
    out.extend_from_slice(&seq.to_be_bytes());
    out.push(whispering as u8);
    out
}

/// Тело ConnectionCheck: nonce из AuthenticateAck.
pub fn connection_check(nonce: &[u8; 16]) -> Vec<u8> {
    nonce.to_vec()
}

pub fn parse_auth_ack(body: &[u8]) -> Option<[u8; 16]> {
    body.get(..16)?.try_into().ok()
}

/// Чужой голос.
#[derive(Clone, Debug, PartialEq)]
pub struct Sound {
    /// Поток голоса (у игрока свой): ключ очереди воспроизведения.
    pub channel: [u8; 16],
    /// Кто говорит (UUID игрока; у звука из точки — источник).
    pub sender: [u8; 16],
    pub opus: Vec<u8>,
    pub seq: i64,
    /// До скольких блоков слышно (у группы 0 — без расстояния).
    pub distance: f32,
    pub kind: SoundKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SoundKind {
    /// Голос игрока рядом: ставим в мир по его позиции.
    Player { whisper: bool },
    /// Голос из группы: без места в мире.
    Group,
    /// Звук из точки мира (аддоны).
    Location([f64; 3]),
}

/// Разобрать звук (типы 2, 3, 4).
pub fn parse_sound(ty: u8, body: &[u8]) -> Option<Sound> {
    let mut b = Buf::new(body);
    let channel = b.uuid()?;
    let sender = b.uuid()?;
    let loc = if ty == T_LOCATION_SOUND { Some([b.f64()?, b.f64()?, b.f64()?]) } else { None };
    let opus = b.bytes(4096)?.to_vec();
    let seq = b.i64()?;
    let (distance, kind) = match ty {
        T_PLAYER_SOUND => {
            let d = b.f32()?;
            let flags = b.u8()?;
            (d, SoundKind::Player { whisper: flags & 1 != 0 })
        }
        T_GROUP_SOUND => {
            b.u8()?;
            (0.0, SoundKind::Group)
        }
        T_LOCATION_SOUND => {
            let d = b.f32()?;
            b.u8()?;
            (d, SoundKind::Location(loc?))
        }
        _ => return None,
    };
    Some(Sound { channel, sender, opus, seq, distance, kind })
}

/// Пинг сервера: отвечаем тем же телом.
pub fn is_ping(ty: u8) -> bool {
    ty == T_PING
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// Эталоны из спецификации GCM (McGrew–Viega), тест-кейсы 2 и 3.
    #[test]
    fn gcm_matches_reference_vectors() {
        let g = Gcm::new(&[0u8; 16]);
        let out = g.seal(&[0u8; 12], &[0u8; 16]);
        assert_eq!(&out[12..28], hex("0388dace60b6a392f328c2b971b2fe78").as_slice());
        assert_eq!(&out[28..], hex("ab6e47d42cec13bdf53a67b21257bddf").as_slice());

        let key: [u8; 16] = hex("feffe9928665731c6d6a8f9467308308").try_into().unwrap();
        let iv: [u8; 12] = hex("cafebabefacedbaddecaf888").try_into().unwrap();
        let p = hex("d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b391aafd255");
        let g = Gcm::new(&key);
        let out = g.seal(&iv, &p);
        assert_eq!(&out[12..12 + p.len()], hex("42831ec2217774244b7221b784d0d49ce3aa212f2c02a4e035c17e2329aca12e21d514b25466931c7d8f6a5aac84aa051ba30b396a0aac973d58e091473f5985").as_slice());
        assert_eq!(&out[12 + p.len()..], hex("4d5c2af327cd64a62cf35abd2ba6fab4").as_slice());
        assert_eq!(g.open(&out).unwrap(), p);
        let mut bad = out.clone();
        bad[20] ^= 1;
        assert!(g.open(&bad).is_none(), "подделку не принимаем");
    }

    #[test]
    fn udp_roundtrip_and_framing() {
        let s = Session::new(&[7u8; 16], [9u8; 16]);
        let dg = s.wrap(T_MIC, &mic(&[1, 2, 3], 42, false));
        assert_eq!(dg[0], 0xFF);
        assert_eq!(&dg[1..17], &[9u8; 16]);
        // ответ сервера — без id соединения: снимем его и разберём
        let server_style = [&dg[..1], &dg[17..]].concat();
        let (ty, body) = s.unwrap(&server_style).unwrap();
        assert_eq!(ty, T_MIC);
        assert_eq!(body, mic(&[1, 2, 3], 42, false));
        assert!(s.unwrap(&[0x00, 1, 2]).is_none());
    }

    #[test]
    fn sound_packets_parse() {
        let mut body = Vec::new();
        body.extend_from_slice(&[1u8; 16]);
        body.extend_from_slice(&[2u8; 16]);
        varint(&mut body, 3);
        body.extend_from_slice(&[5, 6, 7]);
        body.extend_from_slice(&77i64.to_be_bytes());
        body.extend_from_slice(&48.0f32.to_be_bytes());
        body.push(0b1); // шёпот
        let s = parse_sound(T_PLAYER_SOUND, &body).unwrap();
        assert_eq!((s.channel, s.sender, s.opus.as_slice(), s.seq, s.distance), ([1u8; 16], [2u8; 16], &[5u8, 6, 7][..], 77, 48.0));
        assert_eq!(s.kind, SoundKind::Player { whisper: true });

        let mut g = Vec::new();
        g.extend_from_slice(&[1u8; 16]);
        g.extend_from_slice(&[3u8; 16]);
        varint(&mut g, 1);
        g.push(9);
        g.extend_from_slice(&5i64.to_be_bytes());
        g.push(0);
        assert_eq!(parse_sound(T_GROUP_SOUND, &g).unwrap().kind, SoundKind::Group);
    }

    #[test]
    fn secret_and_states_parse() {
        let mut d = Vec::new();
        d.extend_from_slice(&[1u8; 16]); // ключ
        d.extend_from_slice(&[2u8; 16]); // соединение
        d.extend_from_slice(&24454i32.to_be_bytes());
        d.extend_from_slice(&[3u8; 16]); // игрок
        d.push(0);
        d.extend_from_slice(&1024i32.to_be_bytes());
        d.extend_from_slice(&48.0f64.to_be_bytes());
        d.extend_from_slice(&1000i32.to_be_bytes());
        d.push(1);
        varint(&mut d, 0);
        d.push(0); // allowRecording
        let s = parse_secret(&d).unwrap();
        assert_eq!((s.port, s.mtu, s.distance, s.keep_alive, s.groups), (24454, 1024, 48.0, 1000, true));
        assert_eq!(voice_address(&s, "mc.jelecraft.su"), ("mc.jelecraft.su".to_string(), 24454));
        assert_eq!(voice_address(&s, "mc.example.org:25570"), ("mc.example.org".to_string(), 24454));
        let custom = Secret { voice_host: "voice.example.org:30000".into(), ..s.clone() };
        assert_eq!(voice_address(&custom, "mc.example.org"), ("voice.example.org".to_string(), 30000));
        let port_only = Secret { voice_host: "25000".into(), ..s };
        assert_eq!(voice_address(&port_only, "1.2.3.4:25565"), ("1.2.3.4".to_string(), 25000));

        let mut st = 2i32.to_be_bytes().to_vec();
        st.push(0b0000_0001); // заглушён
        st.extend_from_slice(&[4u8; 16]);
        varint(&mut st, 5);
        st.extend_from_slice(b"Steve");
        st.push(0b0010_0100); // выключил голос + в группе
        st.extend_from_slice(&[5u8; 16]);
        varint(&mut st, 4);
        st.extend_from_slice(b"Alex");
        st.extend_from_slice(&[6u8; 16]);
        let list = parse_states(&st).unwrap();
        assert_eq!(list[0].name, "Steve");
        assert!(list[0].muted && !list[0].disabled);
        assert!(list[1].disabled && list[1].group == Some([6u8; 16]));
        assert_eq!(parse_remove_state(&[4u8; 16]), Some([4u8; 16]));
        assert_eq!(request_secret(21), [0, 0, 0, 21]);
    }
}
