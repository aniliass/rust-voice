//! RSA (генерация ключа, расшифровка PKCS#1 v1.5) и AES-CBC для Plasmo Voice.
//! Крейта `rsa` в Horizon нет, поэтому RSA написан поверх num-bigint.

use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
use num_bigint::BigUint;
use num_integer::Integer;
use num_traits::{One, Zero};

pub fn random_bytes(buf: &mut [u8]) {
    getrandom::getrandom(buf).expect("нет источника случайности ОС");
}

/// Случайное число ровно `bits` бит, старшие два бита и младший включены (для p·q ровно 2·bits).
fn random_odd(bits: usize) -> BigUint {
    let mut buf = vec![0u8; bits.div_ceil(8)];
    random_bytes(&mut buf);
    let mut n = BigUint::from_bytes_be(&buf);
    n >>= buf.len() * 8 - bits;
    n.set_bit(bits as u64 - 1, true);
    n.set_bit(bits as u64 - 2, true);
    n.set_bit(0, true);
    n
}

fn random_below(max: &BigUint) -> BigUint {
    let mut buf = vec![0u8; (max.bits() as usize).div_ceil(8) + 8];
    random_bytes(&mut buf);
    BigUint::from_bytes_be(&buf) % max
}

const SMALL_PRIMES: [u32; 24] = [
    3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53, 59, 61, 67, 71, 73, 79, 83, 89, 97,
];

fn is_probably_prime(n: &BigUint, rounds: usize) -> bool {
    for p in SMALL_PRIMES {
        let p = BigUint::from(p);
        if *n == p {
            return true;
        }
        if (n % &p).is_zero() {
            return false;
        }
    }
    let one = BigUint::one();
    let n_1 = n - &one;
    let s = n_1.trailing_zeros().unwrap_or(0);
    let d = &n_1 >> s;
    'witness: for _ in 0..rounds {
        let a = random_below(&(n - BigUint::from(3u32))) + BigUint::from(2u32);
        let mut x = a.modpow(&d, n);
        if x == one || x == n_1 {
            continue;
        }
        for _ in 0..s.saturating_sub(1) {
            x = x.modpow(&BigUint::from(2u32), n);
            if x == n_1 {
                continue 'witness;
            }
        }
        return false;
    }
    true
}

fn gen_prime(bits: usize) -> BigUint {
    loop {
        let mut c = random_odd(bits);
        for _ in 0..2000 {
            if is_probably_prime(&c, 24) {
                return c;
            }
            c += 2u32;
        }
    }
}

pub struct RsaKey {
    pub n: BigUint,
    pub e: BigUint,
    pub d: BigUint,
}

impl RsaKey {
    pub fn generate(bits: usize) -> RsaKey {
        let e = BigUint::from(65537u32);
        loop {
            let p = gen_prime(bits / 2);
            let q = gen_prime(bits / 2);
            if p == q {
                continue;
            }
            let n = &p * &q;
            if n.bits() as usize != bits {
                continue;
            }
            let phi = (&p - 1u32) * (&q - 1u32);
            if !e.gcd(&phi).is_one() {
                continue;
            }
            let Some(d) = mod_inverse(&e, &phi) else { continue };
            return RsaKey { n, e, d };
        }
    }

    #[cfg(test)]
    pub fn from_nd(n: &[u8], d: &[u8]) -> RsaKey {
        RsaKey { n: BigUint::from_bytes_be(n), e: BigUint::from(65537u32), d: BigUint::from_bytes_be(d) }
    }

    /// X.509 SubjectPublicKeyInfo (DER), как у Java `PublicKey.getEncoded()`.
    pub fn public_der(&self) -> Vec<u8> {
        let n = der_uint(&self.n);
        let e = der_uint(&self.e);
        let rsa_pub = der(0x30, &[n, e].concat());
        let mut bitstr = vec![0u8];
        bitstr.extend_from_slice(&rsa_pub);
        let alg = [0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01, 0x05, 0x00];
        der(0x30, &[alg.to_vec(), der(0x03, &bitstr)].concat())
    }

    /// Расшифровка RSA/ECB/PKCS1Padding (то, что делает Java `Cipher.getInstance("RSA")`).
    pub fn decrypt_pkcs1(&self, ct: &[u8]) -> Option<Vec<u8>> {
        let k = (self.n.bits() as usize).div_ceil(8);
        if ct.len() != k {
            return None;
        }
        let m = BigUint::from_bytes_be(ct).modpow(&self.d, &self.n).to_bytes_be();
        let mut em = vec![0u8; k - m.len()];
        em.extend_from_slice(&m);
        // 00 02 PS(не нули, >=8) 00 M
        if em[0] != 0 || em[1] != 2 {
            return None;
        }
        let sep = em[2..].iter().position(|&b| b == 0)? + 2;
        if sep < 10 {
            return None;
        }
        Some(em[sep + 1..].to_vec())
    }

    #[cfg(test)]
    pub fn n_bytes(&self) -> Vec<u8> {
        self.n.to_bytes_be()
    }
    #[cfg(test)]
    pub fn d_bytes(&self) -> Vec<u8> {
        self.d.to_bytes_be()
    }
}

fn mod_inverse(a: &BigUint, m: &BigUint) -> Option<BigUint> {
    use num_bigint::BigInt;
    let (a, m) = (BigInt::from(a.clone()), BigInt::from(m.clone()));
    let e = a.extended_gcd(&m);
    if !e.gcd.is_one() {
        return None;
    }
    let x = ((e.x % &m) + &m) % &m;
    x.to_biguint()
}

fn der_len(len: usize) -> Vec<u8> {
    if len < 128 {
        vec![len as u8]
    } else if len < 256 {
        vec![0x81, len as u8]
    } else {
        vec![0x82, (len >> 8) as u8, len as u8]
    }
}

fn der(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut v = vec![tag];
    v.extend(der_len(body.len()));
    v.extend_from_slice(body);
    v
}

fn der_uint(n: &BigUint) -> Vec<u8> {
    let mut b = n.to_bytes_be();
    if b[0] & 0x80 != 0 {
        b.insert(0, 0);
    }
    der(0x02, &b)
}

type Enc = cbc::Encryptor<aes::Aes128>;
type Dec = cbc::Decryptor<aes::Aes128>;

/// AES/CBC/PKCS5Padding: результат = IV(16) + шифртекст.
pub fn aes_encrypt(key: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let mut iv = [0u8; 16];
    random_bytes(&mut iv);
    let ct = Enc::new(key.into(), (&iv).into()).encrypt_padded_vec_mut::<Pkcs7>(data);
    [iv.to_vec(), ct].concat()
}

pub fn aes_decrypt(key: &[u8; 16], data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 32 || data.len() % 16 != 0 {
        return None;
    }
    let (iv, ct) = data.split_at(16);
    Dec::new(key.into(), iv.into()).decrypt_padded_vec_mut::<Pkcs7>(ct).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aes_roundtrip() {
        let key = [9u8; 16];
        let msg = b"opus frame bytes";
        let enc = aes_encrypt(&key, msg);
        assert_eq!(enc.len(), 16 + 32);
        assert_eq!(aes_decrypt(&key, &enc).unwrap(), msg);
        assert!(aes_decrypt(&[1u8; 16], &enc).map(|d| d != msg).unwrap_or(true));
    }

    #[test]
    fn rsa_generates_valid_key() {
        let k = RsaKey::generate(1024);
        assert_eq!(k.n.bits(), 1024);
        let m = BigUint::from(123456789u64);
        assert_eq!(m.modpow(&k.e, &k.n).modpow(&k.d, &k.n), m);
        let der = k.public_der();
        assert_eq!(der[0], 0x30);
    }

    /// Файлы кладёт скрипт interop.sh: ключ n/d от Rust и шифртекст от OpenSSL.
    #[test]
    #[ignore]
    fn openssl_interop() {
        let n = std::fs::read("/tmp/interop/n.bin").unwrap();
        let d = std::fs::read("/tmp/interop/d.bin").unwrap();
        let ct = std::fs::read("/tmp/interop/ct.bin").unwrap();
        let k = RsaKey::from_nd(&n, &d);
        assert_eq!(k.decrypt_pkcs1(&ct).unwrap(), b"0123456789abcdef");
    }

    #[test]
    #[ignore]
    fn dump_key() {
        let k = RsaKey::generate(2048);
        std::fs::create_dir_all("/tmp/interop").unwrap();
        std::fs::write("/tmp/interop/n.bin", k.n_bytes()).unwrap();
        std::fs::write("/tmp/interop/d.bin", k.d_bytes()).unwrap();
        std::fs::write("/tmp/interop/pub.der", k.public_der()).unwrap();
    }
}
