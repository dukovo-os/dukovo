// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Kerberos's encryption types: AES in CBC mode with ciphertext stealing
//! and HMAC-SHA1 (RFC 3962, types 17 and 18), and with HMAC-SHA2 (RFC
//! 8009, types 19 and 20). Older types (DES, RC4) are left out on purpose:
//! they are broken, and every KDC of this century offers AES.
//!
//! Each message is encrypted under keys made for its use (RFC 3961 3):
//! a "key usage" number, such as 3 for the AS reply, names the use, and
//! [`Key::encrypt`] derives the encryption and integrity keys for it from
//! the base key, so that a ciphertext made for one use is refused in
//! another. RFC 3961's n-fold, DK and DR make those keys for types 17
//! and 18, RFC 8009's KDF-HMAC-SHA2 for types 19 and 20.
//!
//! The tests hold the RFCs' own vectors. Where a KDC refuses a password
//! that is right, compare [`Key::from_password`] with MIT's
//! `ktutil` (`addent -password`, then `list -k`), which prints the key
//! it makes from the same password and salt.

use aes::cipher::{BlockCipherDecrypt, BlockCipherEncrypt, KeyInit};
use aes::{Aes128, Aes256};
use hmac::{Hmac, Mac};
use ring::rand::{SecureRandom, SystemRandom};
use sha1::Sha1;
use sha2::{Sha256, Sha384};

use crate::{Error, Result};

/// AES's block, in bytes.
const BLOCK: usize = 16;

/// An encryption type that Dukovo speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Enctype {
    /// aes128-cts-hmac-sha1-96 (RFC 3962).
    Aes128Sha1 = 17,
    /// aes256-cts-hmac-sha1-96 (RFC 3962).
    Aes256Sha1 = 18,
    /// aes128-cts-hmac-sha256-128 (RFC 8009).
    Aes128Sha256 = 19,
    /// aes256-cts-hmac-sha384-192 (RFC 8009).
    Aes256Sha384 = 20,
}

/// Every type, strongest first: what a request offers unless krb5.conf
/// says otherwise.
pub const ALL: [Enctype; 4] =
    [Enctype::Aes256Sha384, Enctype::Aes128Sha256, Enctype::Aes256Sha1, Enctype::Aes128Sha1];

impl Enctype {
    /// The type of its number, if Dukovo speaks it.
    pub fn from_number(number: i32) -> Option<Enctype> {
        ALL.into_iter().find(|enctype| enctype.number() == number)
    }

    /// Its number in messages.
    pub fn number(self) -> i32 {
        self as i32
    }

    /// Its name, as krb5.conf and klist write it.
    pub fn name(self) -> &'static str {
        match self {
            Enctype::Aes128Sha1 => "aes128-cts-hmac-sha1-96",
            Enctype::Aes256Sha1 => "aes256-cts-hmac-sha1-96",
            Enctype::Aes128Sha256 => "aes128-cts-hmac-sha256-128",
            Enctype::Aes256Sha384 => "aes256-cts-hmac-sha384-192",
        }
    }

    /// The type a name in krb5.conf gives: its full name, or MIT's short
    /// ones (`aes256-cts`, `aes128-sha2` and the like). `None` for a name
    /// of a type Dukovo does not speak, which a list of types skips.
    pub fn from_name(name: &str) -> Option<Enctype> {
        let name = name.to_ascii_lowercase();
        match name.as_str() {
            "aes128-cts" | "aes128-cts-hmac-sha1-96" | "aes128" => Some(Enctype::Aes128Sha1),
            "aes256-cts" | "aes256-cts-hmac-sha1-96" | "aes256" => Some(Enctype::Aes256Sha1),
            "aes128-sha2" | "aes128-cts-hmac-sha256-128" => Some(Enctype::Aes128Sha256),
            "aes256-sha2" | "aes256-cts-hmac-sha384-192" => Some(Enctype::Aes256Sha384),
            _ => None,
        }
    }

    /// Its keys' length, in bytes.
    pub fn key_len(self) -> usize {
        match self {
            Enctype::Aes128Sha1 | Enctype::Aes128Sha256 => 16,
            Enctype::Aes256Sha1 | Enctype::Aes256Sha384 => 32,
        }
    }

    /// The number of its keyed checksum (RFC 3961 and 8009).
    pub fn checksum_type(self) -> i32 {
        match self {
            Enctype::Aes128Sha1 => 15,
            Enctype::Aes256Sha1 => 16,
            Enctype::Aes128Sha256 => 19,
            Enctype::Aes256Sha384 => 20,
        }
    }

    /// The length of its checksums and of the integrity check after each
    /// ciphertext, in bytes.
    pub fn mac_len(self) -> usize {
        match self {
            Enctype::Aes128Sha1 | Enctype::Aes256Sha1 => 12,
            Enctype::Aes128Sha256 => 16,
            Enctype::Aes256Sha384 => 24,
        }
    }

    /// The PBKDF2 rounds a password takes when the KDC names none.
    fn default_rounds(self) -> u32 {
        match self {
            Enctype::Aes128Sha1 | Enctype::Aes256Sha1 => 4096,
            Enctype::Aes128Sha256 | Enctype::Aes256Sha384 => 32_768,
        }
    }

    /// Whether it is one of RFC 8009's.
    fn sha2(self) -> bool {
        matches!(self, Enctype::Aes128Sha256 | Enctype::Aes256Sha384)
    }

    /// How much a ciphertext adds to its plaintext: the confounder and
    /// the integrity check.
    pub fn overhead(self) -> usize {
        BLOCK + self.mac_len()
    }
}

/// The most PBKDF2 rounds a KDC may ask for: past this, a request would
/// make kinit work for minutes, which no KDC means.
const MOST_ROUNDS: u32 = 1 << 24;

/// A key of one of [`Enctype`]'s types, wiped when dropped.
#[derive(Clone, PartialEq, Eq)]
pub struct Key {
    /// Its type.
    pub enctype: Enctype,
    /// Its bytes, [`Enctype::key_len`] of them.
    bytes: Vec<u8>,
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the bytes, which would put a key in a log.
        f.debug_struct("Key").field("enctype", &self.enctype).finish_non_exhaustive()
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        wipe(&mut self.bytes);
    }
}

/// Zeroes `bytes`, in a way the compiler keeps.
pub fn wipe(bytes: &mut [u8]) {
    bytes.fill(0);
    std::hint::black_box(&bytes);
}

/// `count` random bytes from the system.
pub fn random(count: usize) -> Result<Vec<u8>> {
    let mut bytes = vec![0; count];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| Error::Other("the system gave no random numbers".into()))?;
    Ok(bytes)
}

/// The last 4 bytes of a derivation's input for `usage` (RFC 3961 3):
/// the usage, then which key: 0x99 checksums, 0xaa encrypts, 0x55 checks
/// integrity.
fn usage_label(usage: u32, which: u8) -> [u8; 5] {
    let mut label = [0; 5];
    label[..4].copy_from_slice(&usage.to_be_bytes());
    label[4] = which;
    label
}

impl Key {
    /// A key of `enctype` from its bytes.
    pub fn new(enctype: Enctype, bytes: &[u8]) -> Result<Key> {
        if bytes.len() != enctype.key_len() {
            return Err(Error::Malformed(format!(
                "a key of {} bytes for {}, which takes {}",
                bytes.len(),
                enctype.name(),
                enctype.key_len()
            )));
        }
        Ok(Key { enctype, bytes: bytes.to_vec() })
    }

    /// A new random key of `enctype`.
    pub fn random(enctype: Enctype) -> Result<Key> {
        Ok(Key { enctype, bytes: random(enctype.key_len())? })
    }

    /// Its bytes.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The key of `password` (RFC 3962 4 and RFC 8009 4): PBKDF2 over the
    /// password and `salt`, by default the realm then the name's parts,
    /// for as many rounds as `params` says (4 bytes, big-endian), or the
    /// type's default, then derived once more with "kerberos".
    pub fn from_password(
        enctype: Enctype,
        password: &[u8],
        salt: &[u8],
        params: Option<&[u8]>,
    ) -> Result<Key> {
        let rounds = match params {
            None => enctype.default_rounds(),
            Some(&[a, b, c, d]) => u32::from_be_bytes([a, b, c, d]),
            Some(other) => {
                return Err(Error::Malformed(format!(
                    "the KDC named the rounds for a password in {} bytes, not 4",
                    other.len()
                )));
            }
        };
        if rounds == 0 || rounds > MOST_ROUNDS {
            return Err(Error::Other(format!(
                "the KDC asked for {rounds} PBKDF2 rounds, outside 1 to {MOST_ROUNDS}; ask its \
                 administrator to check the principal's salt and rounds"
            )));
        }
        let mut made = vec![0; enctype.key_len()];
        let key = match enctype {
            Enctype::Aes128Sha1 | Enctype::Aes256Sha1 => {
                pbkdf2::pbkdf2_hmac::<Sha1>(password, salt, rounds, &mut made);
                let made = Key { enctype, bytes: made };
                Key { enctype, bytes: made.derive_dk(b"kerberos") }
            }
            Enctype::Aes128Sha256 | Enctype::Aes256Sha384 => {
                // RFC 8009 puts the type's name and a zero before the salt.
                let mut salted = enctype.name().as_bytes().to_vec();
                salted.push(0);
                salted.extend_from_slice(salt);
                if enctype == Enctype::Aes128Sha256 {
                    pbkdf2::pbkdf2_hmac::<Sha256>(password, &salted, rounds, &mut made);
                } else {
                    pbkdf2::pbkdf2_hmac::<Sha384>(password, &salted, rounds, &mut made);
                }
                let made = Key { enctype, bytes: made };
                Key { enctype, bytes: made.kdf(b"kerberos", enctype.key_len()) }
            }
        };
        Ok(key)
    }

    /// AES under this key's bytes, or under `bytes`.
    fn aes(bytes: &[u8]) -> Aes {
        match bytes.len() {
            16 => Aes::A128(Box::new(Aes128::new_from_slice(bytes).expect("16 bytes"))),
            _ => Aes::A256(Box::new(Aes256::new_from_slice(bytes).expect("32 bytes"))),
        }
    }

    /// RFC 3961's DR then DK, for types 17 and 18: the n-fold of
    /// `constant` encrypted, then each block encrypted again, until there
    /// are a key's bytes. AES's keys are any bytes, so DK is DR.
    fn derive_dk(&self, constant: &[u8]) -> Vec<u8> {
        let aes = Key::aes(&self.bytes);
        let mut block = [0u8; BLOCK];
        block.copy_from_slice(&nfold(constant, BLOCK));
        let mut out = Vec::with_capacity(self.bytes.len());
        while out.len() < self.bytes.len() {
            aes.encrypt(&mut block);
            let need = (self.bytes.len() - out.len()).min(BLOCK);
            out.extend_from_slice(&block[..need]);
        }
        out
    }

    /// RFC 8009's KDF-HMAC-SHA2 for types 19 and 20: `bytes` bytes of
    /// HMAC over 1, `label`, a zero and the length in bits.
    fn kdf(&self, label: &[u8], bytes: usize) -> Vec<u8> {
        let mut input = 1u32.to_be_bytes().to_vec();
        input.extend_from_slice(label);
        input.push(0);
        input.extend_from_slice(&((bytes * 8) as u32).to_be_bytes());
        let mut out = self.hmac(&self.bytes, &[&input]);
        out.truncate(bytes);
        out
    }

    /// The key for `usage` and `which` of 0x99, 0xaa or 0x55: for types
    /// 19 and 20 the encryption key is the type's whole length, the
    /// others its checksum's.
    fn derived(&self, usage: u32, which: u8) -> Vec<u8> {
        let label = usage_label(usage, which);
        if !self.enctype.sha2() {
            return self.derive_dk(&label);
        }
        let len = match (self.enctype, which) {
            (Enctype::Aes256Sha384, 0xaa) => 32,
            (Enctype::Aes256Sha384, _) => 24,
            _ => 16,
        };
        self.kdf(&label, len)
    }

    /// HMAC under `key` of `parts`, with this type's hash.
    fn hmac(&self, key: &[u8], parts: &[&[u8]]) -> Vec<u8> {
        macro_rules! run {
            ($hash:ty) => {{
                let mut mac = <Hmac<$hash> as hmac::KeyInit>::new_from_slice(key)
                    .expect("HMAC takes any key");
                for part in parts {
                    mac.update(part);
                }
                mac.finalize().into_bytes().to_vec()
            }};
        }
        match self.enctype {
            Enctype::Aes128Sha1 | Enctype::Aes256Sha1 => run!(Sha1),
            Enctype::Aes128Sha256 => run!(Sha256),
            Enctype::Aes256Sha384 => run!(Sha384),
        }
    }

    /// `plain` encrypted for `usage`, with a random confounder before it
    /// and the integrity check after.
    pub fn encrypt(&self, usage: u32, plain: &[u8]) -> Result<Vec<u8>> {
        let confounder = random(BLOCK)?;
        Ok(self.encrypt_with(usage, &confounder, plain))
    }

    /// [`Key::encrypt`] with a confounder of the caller's, for the tests'
    /// vectors.
    fn encrypt_with(&self, usage: u32, confounder: &[u8], plain: &[u8]) -> Vec<u8> {
        let mut data = confounder.to_vec();
        data.extend_from_slice(plain);
        let mut ke = self.derived(usage, 0xaa);
        let mut ki = self.derived(usage, 0x55);
        let aes = Key::aes(&ke);
        // RFC 3962 checks the plaintext, RFC 8009 the ciphertext after
        // the cipher state, which starts as zeros.
        let mut mac = if self.enctype.sha2() { Vec::new() } else { self.hmac(&ki, &[&data]) };
        let mut out = cts_encrypt(&aes, &data);
        if self.enctype.sha2() {
            mac = self.hmac(&ki, &[&[0u8; BLOCK], &out]);
        }
        out.extend_from_slice(&mac[..self.enctype.mac_len()]);
        wipe(&mut data);
        wipe(&mut ke);
        wipe(&mut ki);
        out
    }

    /// The plaintext of `cipher`, encrypted for `usage`; refused when its
    /// integrity check fails, as with the wrong key.
    pub fn decrypt(&self, usage: u32, cipher: &[u8]) -> Result<Vec<u8>> {
        let mac_len = self.enctype.mac_len();
        if cipher.len() < BLOCK + mac_len {
            return Err(Error::Malformed(format!(
                "a ciphertext of {} bytes, shorter than its confounder and check",
                cipher.len()
            )));
        }
        let (body, mac) = cipher.split_at(cipher.len() - mac_len);
        let mut ke = self.derived(usage, 0xaa);
        let mut ki = self.derived(usage, 0x55);
        if self.enctype.sha2() && !same(&self.hmac(&ki, &[&[0u8; BLOCK], body])[..mac_len], mac) {
            wipe(&mut ke);
            wipe(&mut ki);
            return Err(Error::Integrity);
        }
        let mut data = cts_decrypt(&Key::aes(&ke), body);
        let checked = self.enctype.sha2() || same(&self.hmac(&ki, &[&data])[..mac_len], mac);
        wipe(&mut ke);
        wipe(&mut ki);
        if !checked {
            wipe(&mut data);
            return Err(Error::Integrity);
        }
        let plain = data[BLOCK..].to_vec();
        wipe(&mut data);
        Ok(plain)
    }

    /// The keyed checksum of `data` for `usage`.
    pub fn checksum(&self, usage: u32, data: &[u8]) -> Vec<u8> {
        let mut kc = self.derived(usage, 0x99);
        let mut mac = self.hmac(&kc, &[data]);
        wipe(&mut kc);
        mac.truncate(self.enctype.mac_len());
        mac
    }

    /// Whether `mac` is the keyed checksum of `data` for `usage`.
    pub fn verify(&self, usage: u32, data: &[u8], mac: &[u8]) -> bool {
        same(&self.checksum(usage, data), mac)
    }
}

/// Whether `a` and `b` are the same, in a time that does not tell where
/// they differ.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

/// AES-128 or AES-256.
enum Aes {
    /// Under a 16-byte key.
    A128(Box<Aes128>),
    /// Under a 32-byte key.
    A256(Box<Aes256>),
}

impl Aes {
    /// Encrypts one block in place.
    fn encrypt(&self, block: &mut [u8; BLOCK]) {
        let mut b = aes::Block::from(*block);
        match self {
            Aes::A128(aes) => aes.encrypt_block(&mut b),
            Aes::A256(aes) => aes.encrypt_block(&mut b),
        }
        block.copy_from_slice(&b);
    }

    /// Decrypts one block in place.
    fn decrypt(&self, block: &mut [u8; BLOCK]) {
        let mut b = aes::Block::from(*block);
        match self {
            Aes::A128(aes) => aes.decrypt_block(&mut b),
            Aes::A256(aes) => aes.decrypt_block(&mut b),
        }
        block.copy_from_slice(&b);
    }
}

/// `a` xored into `into`.
fn xor(into: &mut [u8; BLOCK], a: &[u8]) {
    for (to, from) in into.iter_mut().zip(a) {
        *to ^= from;
    }
}

/// CBC with ciphertext stealing from a zero cipher state (RFC 3962 5,
/// CBC-CS3): CBC over the blocks, the last padded with zeros, then the
/// last two ciphertext blocks swapped and the output cut to the input's
/// length. `data` is at least a block long, as with a confounder before
/// it always is.
fn cts_encrypt(aes: &Aes, data: &[u8]) -> Vec<u8> {
    let blocks = data.len().div_ceil(BLOCK);
    let mut out: Vec<[u8; BLOCK]> = Vec::with_capacity(blocks);
    let mut previous = [0u8; BLOCK];
    for chunk in data.chunks(BLOCK) {
        let mut block = previous;
        xor(&mut block, chunk);
        aes.encrypt(&mut block);
        out.push(block);
        previous = block;
    }
    if blocks > 1 {
        out.swap(blocks - 2, blocks - 1);
    }
    let mut flat: Vec<u8> = out.concat();
    flat.truncate(data.len());
    flat
}

/// The inverse of [`cts_encrypt`]. `data` is at least a block long:
/// [`Key::decrypt`] checks that first.
fn cts_decrypt(aes: &Aes, data: &[u8]) -> Vec<u8> {
    let blocks = data.len().div_ceil(BLOCK);
    let block_at = |i: usize| -> [u8; BLOCK] {
        let mut block = [0u8; BLOCK];
        block.copy_from_slice(&data[i * BLOCK..(i + 1) * BLOCK]);
        block
    };
    if blocks == 1 {
        let mut block = block_at(0);
        aes.decrypt(&mut block);
        return block.to_vec();
    }
    let mut out = Vec::with_capacity(data.len());
    let mut previous = [0u8; BLOCK];
    for i in 0..blocks - 2 {
        let cipher = block_at(i);
        let mut plain = cipher;
        aes.decrypt(&mut plain);
        xor(&mut plain, &previous);
        out.extend_from_slice(&plain);
        previous = cipher;
    }
    // The last whole block holds the last plaintext block xored with the
    // stolen one; the tail of its decryption completes the stolen block.
    let tail = data.len() - (blocks - 1) * BLOCK;
    let mut last = block_at(blocks - 2);
    aes.decrypt(&mut last);
    let mut stolen = last;
    stolen[..tail].copy_from_slice(&data[(blocks - 1) * BLOCK..]);
    let mut final_plain = last;
    xor(&mut final_plain, &stolen);
    let mut before_last = stolen;
    aes.decrypt(&mut before_last);
    xor(&mut before_last, &previous);
    out.extend_from_slice(&before_last);
    out.extend_from_slice(&final_plain[..tail]);
    out
}

/// RFC 3961's n-fold: `input` stretched or folded to `bytes` bytes, by
/// adding copies of it, each rotated 13 bits more, with end-around
/// carry. This is MIT's way of computing it.
pub fn nfold(input: &[u8], bytes: usize) -> Vec<u8> {
    let in_len = input.len();
    let gcd = {
        let (mut a, mut b) = (bytes, in_len);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a
    };
    let lcm = bytes * in_len / gcd;
    let in_bits = in_len * 8;
    let mut out = vec![0u8; bytes];
    let mut carry = 0usize;
    for i in (0..lcm).rev() {
        // The input bit that lands at the top of this byte.
        let msbit = ((in_bits - 1) + (in_bits + 13) * (i / in_len) + ((in_len - i % in_len) << 3))
            % in_bits;
        let high = input[((in_len - 1) - (msbit >> 3)) % in_len] as usize;
        let low = input[(in_len - (msbit >> 3)) % in_len] as usize;
        carry += ((high << 8 | low) >> ((msbit & 7) + 1)) & 0xff;
        carry += out[i % bytes] as usize;
        out[i % bytes] = carry as u8;
        carry >>= 8;
    }
    // What carried out of the top goes back in at the bottom.
    if carry != 0 {
        for byte in out.iter_mut().rev() {
            carry += *byte as usize;
            *byte = carry as u8;
            carry >>= 8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        let text: String = text.split_whitespace().collect();
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn nfold_matches_rfc_3961() {
        assert_eq!(nfold(b"012345", 8), hex("be072631276b1955"));
        assert_eq!(nfold(b"password", 7), hex("78a07b6caf85fa"));
        assert_eq!(nfold(b"Rough Consensus, and Running Code", 8), hex("bb6ed30870b7f0e0"));
        assert_eq!(nfold(b"password", 21), hex("59e4a8ca7c0385c3c37b3f6d2000247cb6e6bd5b3e"));
        assert_eq!(
            nfold(b"MASSACHVSETTS INSTITVTE OF TECHNOLOGY", 24),
            hex("db3b0d8f0b061e603282b308a50841229ad798fab9540c1b")
        );
        assert_eq!(nfold(b"Q", 21), hex("518a54a215a8452a518a54a215a8452a518a54a215"));
        assert_eq!(nfold(b"ba", 21), hex("fb25d531ae8974499f52fd92ea9857c4ba24cf297e"));
        assert_eq!(nfold(b"kerberos", 8), hex("6b65726265726f73"));
        assert_eq!(nfold(b"kerberos", 16), hex("6b65726265726f737b9b5b2b93132b93"));
        assert_eq!(
            nfold(b"kerberos", 32),
            hex("6b65726265726f737b9b5b2b93132b935c9bdcdad95c9899c4cae4dee6d6cae4")
        );
    }

    #[test]
    fn passwords_make_rfc_3962s_keys() {
        let salt = b"ATHENA.MIT.EDUraeburn";
        let key = |enctype, rounds: u32| {
            Key::from_password(enctype, b"password", salt, Some(&rounds.to_be_bytes())).unwrap()
        };
        assert_eq!(key(Enctype::Aes128Sha1, 1).bytes(), hex("42263c6e89f4fc28b8df68ee09799f15"));
        assert_eq!(
            key(Enctype::Aes256Sha1, 1).bytes(),
            hex("fe697b52bc0d3ce14432ba036a92e65bbb52280990a2fa27883998d72af30161")
        );
        assert_eq!(key(Enctype::Aes128Sha1, 1200).bytes(), hex("4c01cd46d632d01e6dbe230a01ed642a"));
        assert_eq!(
            key(Enctype::Aes256Sha1, 1200).bytes(),
            hex("55a6ac740ad17b4846941051e1e8b0a7548d93b0ab30a8bc3ff16280382b8c2a")
        );
    }

    #[test]
    fn ciphertext_stealing_reads_back_at_every_length() {
        let aes = Key::aes(&hex("636869636b656e207465726979616b69"));
        for len in 16..80 {
            let data: Vec<u8> = (0..len as u8).collect();
            let sealed = cts_encrypt(&aes, &data);
            assert_eq!(sealed.len(), len);
            assert_eq!(cts_decrypt(&aes, &sealed), data, "length {len}");
        }
    }

    #[test]
    fn ciphertext_stealing_matches_rfc_3962() {
        // RFC 3962 appendix B: AES-128 under "chicken teriyaki".
        let aes = Key::aes(&hex("636869636b656e207465726979616b69"));
        let input = b"I would like the General Gau's Chicken, please, and wonton soup.";
        assert_eq!(cts_encrypt(&aes, &input[..17]), hex("c6353568f2bf8cb4d8a580362da7ff7f97"));
        assert_eq!(
            cts_encrypt(&aes, &input[..31]),
            hex("fc00783e0efdb2c1d445d4c8eff7ed2297687268d6ecccc0c07b25e25ecfe5")
        );
        assert_eq!(
            cts_encrypt(&aes, &input[..32]),
            hex("39312523a78662d5be7fcbcc98ebf5a897687268d6ecccc0c07b25e25ecfe584")
        );
    }

    #[test]
    fn rfc_8009s_keys_derive_as_it_says() {
        let base =
            Key::new(Enctype::Aes128Sha256, &hex("3705D96080C17728A0E800EAB6E0D23C")).unwrap();
        assert_eq!(base.derived(2, 0x99), hex("B31A018A48F54776F403E9A396325DC3"));
        assert_eq!(base.derived(2, 0xaa), hex("9B197DD1E8C5609D6E67C3E37C62C72E"));
        assert_eq!(base.derived(2, 0x55), hex("9FDA0E56AB2D85E1569A688696C26A6C"));
        let base = Key::new(
            Enctype::Aes256Sha384,
            &hex("6D404D37FAF79F9DF0D33568D320669800EB4836472EA8A026D16B7182460C52"),
        )
        .unwrap();
        assert_eq!(base.derived(2, 0x99), hex("EF5718BE86CC84963D8BBB5031E9F5C4BA41F28FAF69E73D"));
        assert_eq!(
            base.derived(2, 0xaa),
            hex("56AB22BEE63D82D7BC5227F6773F8EA7A5EB1C825160C38312980C442E5C7E49")
        );
        assert_eq!(base.derived(2, 0x55), hex("69B16514E3CD8E56B82010D5C73012B622C4D00FFC23ED1F"));
    }

    #[test]
    fn rfc_8009s_encryption_matches_its_vectors() {
        // RFC 8009 appendix A: an empty plaintext under aes128-sha2,
        // key usage 2.
        let key =
            Key::new(Enctype::Aes128Sha256, &hex("3705D96080C17728A0E800EAB6E0D23C")).unwrap();
        let sealed = key.encrypt_with(2, &hex("7E5895EAF2672435BAD817F545A37148"), b"");
        assert_eq!(sealed, hex("EF85FB890BB8472F4DAB20394DCA781DAD877EDA39D50C870C0D5A0A8E48C718"));
        assert!(key.decrypt(2, &sealed).unwrap().is_empty());
    }

    #[test]
    fn every_type_reads_back_its_own_and_refuses_a_tampered_one() {
        for enctype in ALL {
            let key = Key::random(enctype).unwrap();
            for len in [0, 1, 15, 16, 17, 100] {
                let plain: Vec<u8> = (0..len as u8).collect();
                let mut sealed = key.encrypt(7, &plain).unwrap();
                assert_eq!(sealed.len(), len + enctype.overhead());
                assert_eq!(key.decrypt(7, &sealed).unwrap(), plain);
                assert_eq!(key.decrypt(8, &sealed), Err(Error::Integrity));
                let last = sealed.len() - 1;
                sealed[last] ^= 1;
                assert_eq!(key.decrypt(7, &sealed), Err(Error::Integrity));
            }
            let mac = key.checksum(6, b"body");
            assert_eq!(mac.len(), enctype.mac_len());
            assert!(key.verify(6, b"body", &mac));
            assert!(!key.verify(6, b"bodY", &mac));
        }
    }
}
