// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! NTLMv2, as a server checks it (MS-NLMP), for the sharing passwords
//! of ADR 0228.
//!
//! A Windows PC signs in to a share with NTLM: smbd(8) sends a challenge,
//! and the PC answers with a proof made from it and the NT hash of the
//! password, the MD4 of its UTF-16 ([`nt_hash`]). So a server has to keep
//! that hash, which is worth the password itself to anyone who reads it,
//! and which no salt or slow hash can protect. That is why a sharing
//! password is one of its own, set with `share password`, never the
//! password that signs in to Dukovo: the NT hash of the sharing password
//! is all [`SHARING`](crate::SHARING) holds, and only root reads it.
//!
//! [`check`] takes the three messages of a sign-in (NEGOTIATE, CHALLENGE
//! and AUTHENTICATE), as they were sent, and the NT hash of the account
//! the AUTHENTICATE message names, and answers with the session key both
//! sides now share. NTLMv1, and NTLMv2 without the server's challenge in
//! its proof (LMv2 alone), are refused, as Windows refuses them by
//! default since Windows Vista.

use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use md4::{Digest, Md4};

type HmacMd5 = Hmac<md5::Md5>;

/// A key's length, and an NT hash's: 16 bytes.
pub const KEY: usize = 16;

/// NTLMSSP's signature, at the start of every message.
pub const SIGNATURE: &[u8; 8] = b"NTLMSSP\0";

/// The flags of MS-NLMP 2.2.2.5 a server looks at.
pub mod flags {
    /// The strings are UTF-16.
    pub const UNICODE: u32 = 0x0000_0001;
    /// The OEM character set; here, ASCII.
    pub const OEM: u32 = 0x0000_0002;
    /// The server is to send its name.
    pub const REQUEST_TARGET: u32 = 0x0000_0004;
    /// Messages after sign-in are signed.
    pub const SIGN: u32 = 0x0000_0010;
    /// And sealed.
    pub const SEAL: u32 = 0x0000_0020;
    /// NTLM, not LM.
    pub const NTLM: u32 = 0x0000_0200;
    /// Signing even with no session key: never offered.
    pub const ALWAYS_SIGN: u32 = 0x0000_8000;
    /// The target is a server.
    pub const TARGET_TYPE_SERVER: u32 = 0x0002_0000;
    /// NTLMv2's session security, which the keys for signing need.
    pub const EXTENDED_SESSION_SECURITY: u32 = 0x0008_0000;
    /// The CHALLENGE message carries the target's information.
    pub const TARGET_INFO: u32 = 0x0080_0000;
    /// The message carries a version.
    pub const VERSION: u32 = 0x0200_0000;
    /// 128-bit keys.
    pub const KEY_128: u32 = 0x2000_0000;
    /// The client picks the session key and sends it, encrypted.
    pub const KEY_EXCH: u32 = 0x4000_0000;
    /// 56-bit keys.
    pub const KEY_56: u32 = 0x8000_0000;
}

/// The NT hash of `password`: the MD4 of its UTF-16LE (MS-NLMP 3.3.1,
/// NTOWFv1). A password that is not UTF-8 has none.
pub fn nt_hash(password: &[u8]) -> Option<[u8; KEY]> {
    let text = std::str::from_utf8(password).ok()?;
    let mut units = utf16(text);
    let mut md4 = Md4::new();
    md4.update(&units);
    crate::wipe(&mut units);
    Some(array(&md4.finalize()))
}

/// `text` in UTF-16LE.
pub fn utf16(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

/// The HMAC-MD5 of `parts`, one after another, with `key`.
///
/// # Panics
///
/// Never: HMAC takes a key of any length.
pub fn hmac_md5(key: &[u8], parts: &[&[u8]]) -> [u8; KEY] {
    let mut mac = <HmacMd5 as KeyInit>::new_from_slice(key).expect("HMAC takes any key");
    for part in parts {
        mac.update(part);
    }
    array(&mac.finalize().into_bytes())
}

/// The MD5 of `parts`, one after another.
pub fn md5(parts: &[&[u8]]) -> [u8; KEY] {
    let mut md5 = md5::Md5::new();
    for part in parts {
        md5.update(part);
    }
    array(&md5.finalize())
}

/// A digest's 16 bytes as an array.
fn array(bytes: &[u8]) -> [u8; KEY] {
    let mut key = [0u8; KEY];
    key.copy_from_slice(bytes);
    key
}

/// RC4, the stream cipher NTLM encrypts its session key and its
/// signatures' checksums with: a stream whose state goes on from one
/// message to the next.
pub struct Rc4 {
    s: [u8; 256],
    i: u8,
    j: u8,
}

impl Rc4 {
    /// The stream for `key`.
    pub fn new(key: &[u8]) -> Rc4 {
        let mut s = [0u8; 256];
        for (k, byte) in s.iter_mut().enumerate() {
            *byte = k as u8;
        }
        let mut j = 0u8;
        for k in 0..256 {
            j = j.wrapping_add(s[k]).wrapping_add(key[k % key.len()]);
            s.swap(k, j as usize);
        }
        Rc4 { s, i: 0, j: 0 }
    }

    /// Encrypts, or decrypts, `data` in place.
    pub fn apply(&mut self, data: &mut [u8]) {
        for byte in data {
            self.i = self.i.wrapping_add(1);
            self.j = self.j.wrapping_add(self.s[self.i as usize]);
            self.s.swap(self.i as usize, self.j as usize);
            let k = self.s[self.s[self.i as usize].wrapping_add(self.s[self.j as usize]) as usize];
            *byte ^= k;
        }
    }
}

/// An AUTHENTICATE message's fields (MS-NLMP 2.2.1.3).
#[derive(Debug)]
pub struct Authenticate<'a> {
    /// The NT response: for NTLMv2, the proof and the client's blob.
    pub nt_response: &'a [u8],
    /// The domain the client says, which NTLMv2's key is made with.
    pub domain: String,
    /// The user's name, as typed at the client.
    pub user: String,
    /// The client's name.
    pub workstation: String,
    /// The session key, encrypted with the one NTLMv2 makes, when the
    /// client chose it ([`flags::KEY_EXCH`]).
    pub encrypted_key: &'a [u8],
    /// The flags the client and server agreed on.
    pub flags: u32,
}

/// Why a sign-in is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A message that does not read as NTLM's.
    Malformed(&'static str),
    /// NTLMv1, or LMv2 alone, which are not accepted.
    Old,
    /// The proof is not of this password: the password is wrong.
    Wrong,
    /// The message integrity code is wrong: a message was changed on the
    /// way.
    Tampered,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Malformed(what) => write!(f, "a malformed NTLM message: {what}"),
            Refusal::Old => write!(f, "NTLMv1 is not accepted: the client needs NTLMv2"),
            Refusal::Wrong => write!(f, "the name or the password is wrong"),
            Refusal::Tampered => write!(f, "the sign-in's messages were changed on the way"),
        }
    }
}

/// A message's field: its length and where it is, as its 8 bytes at
/// `at` say.
fn field(message: &[u8], at: usize) -> Result<&[u8], Refusal> {
    let bytes = message.get(at..at + 8).ok_or(Refusal::Malformed("it is too short"))?;
    let length = u16::from_le_bytes([bytes[0], bytes[1]]) as usize;
    let offset = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    if length == 0 {
        return Ok(&[]);
    }
    let end = offset.checked_add(length).ok_or(Refusal::Malformed("a field past its end"))?;
    message.get(offset..end).ok_or(Refusal::Malformed("a field past its end"))
}

/// A string field, in UTF-16LE or in ASCII as the flags say.
fn text(bytes: &[u8], unicode: bool) -> Result<String, Refusal> {
    if !unicode {
        return Ok(bytes.iter().map(|&b| if b.is_ascii() { b as char } else { '?' }).collect());
    }
    if !bytes.len().is_multiple_of(2) {
        return Err(Refusal::Malformed("a UTF-16 string of an odd length"));
    }
    let units: Vec<u16> =
        bytes.as_chunks::<2>().0.iter().map(|&pair| u16::from_le_bytes(pair)).collect();
    String::from_utf16(&units).map_err(|_| Refusal::Malformed("a string that is not UTF-16"))
}

/// Whether `message` is an NTLM message of `kind` (1, 2 or 3).
pub fn is_kind(message: &[u8], kind: u32) -> bool {
    message.len() >= 12
        && message[..8] == *SIGNATURE
        && u32::from_le_bytes([message[8], message[9], message[10], message[11]]) == kind
}

impl<'a> Authenticate<'a> {
    /// Reads an AUTHENTICATE message.
    pub fn parse(message: &'a [u8]) -> Result<Authenticate<'a>, Refusal> {
        if !is_kind(message, 3) || message.len() < 64 {
            return Err(Refusal::Malformed("not an AUTHENTICATE message"));
        }
        let flags = u32::from_le_bytes([message[60], message[61], message[62], message[63]]);
        let unicode = flags & flags::UNICODE != 0;
        Ok(Authenticate {
            nt_response: field(message, 20)?,
            domain: text(field(message, 28)?, unicode)?,
            user: text(field(message, 36)?, unicode)?,
            workstation: text(field(message, 44)?, unicode)?,
            encrypted_key: field(message, 52)?,
            flags,
        })
    }
}

/// MS-NLMP 2.2.2.1's AV_PAIR identifiers.
pub mod av {
    /// The end of the list.
    pub const EOL: u16 = 0;
    /// The server's NetBIOS name.
    pub const NB_COMPUTER_NAME: u16 = 1;
    /// The server's NetBIOS domain: its workgroup.
    pub const NB_DOMAIN_NAME: u16 = 2;
    /// The server's DNS name.
    pub const DNS_COMPUTER_NAME: u16 = 3;
    /// Its DNS domain.
    pub const DNS_DOMAIN_NAME: u16 = 4;
    /// Flags: 0x2, the AUTHENTICATE message has a MIC.
    pub const FLAGS: u16 = 6;
    /// The server's time, which has the client send a MIC.
    pub const TIMESTAMP: u16 = 7;
}

/// The value of AV_PAIR `id` in `list`, if it is there.
pub fn av_pair(list: &[u8], id: u16) -> Option<&[u8]> {
    let mut at = 0;
    while let Some(head) = list.get(at..at + 4) {
        let kind = u16::from_le_bytes([head[0], head[1]]);
        let length = u16::from_le_bytes([head[2], head[3]]) as usize;
        if kind == av::EOL {
            return None;
        }
        let value = list.get(at + 4..at + 4 + length)?;
        if kind == id {
            return Some(value);
        }
        at += 4 + length;
    }
    None
}

/// The MIC's place in an AUTHENTICATE message that has one, after the
/// fields and the version.
const MIC_AT: usize = 72;

/// Checks a sign-in's AUTHENTICATE message against `nt_hash`, the NT hash
/// of the password of the account it names, with the NEGOTIATE and
/// CHALLENGE messages before it, as sent. Returns the session key both
/// sides now hold, from which SMB makes its keys for signing and
/// encryption (MS-NLMP 3.2.5.1.2 and 3.3.5.1.2).
pub fn check(
    nt_hash: &[u8; KEY],
    negotiate: &[u8],
    challenge: &[u8],
    authenticate: &[u8],
) -> Result<[u8; KEY], Refusal> {
    let auth = Authenticate::parse(authenticate)?;
    if !is_kind(challenge, 2) || challenge.len() < 32 {
        return Err(Refusal::Malformed("not a CHALLENGE message"));
    }
    let server_challenge = &challenge[24..32];
    // NTLMv2's response is the 16-byte proof and a blob of at least 28
    // bytes and the AV pairs' end; NTLMv1's is 24 bytes.
    if auth.nt_response.len() < 16 + 32 {
        return Err(Refusal::Old);
    }
    let (proof, blob) = auth.nt_response.split_at(16);
    if blob[0] != 1 || blob[1] != 1 {
        return Err(Refusal::Malformed("an NTLMv2 blob of an unknown version"));
    }
    // The key is made with the domain the client sent; some send none.
    // Each is a check of the same password.
    let user = utf16(&auth.user.to_uppercase());
    let domains = [auth.domain.as_str(), ""];
    let mut matched = None;
    for domain in domains {
        let key = hmac_md5(nt_hash, &[&user, &utf16(domain)]);
        let made = hmac_md5(&key, &[server_challenge, blob]);
        if constant_eq(&made, proof) {
            matched = Some(key);
            break;
        }
    }
    let key = matched.ok_or(Refusal::Wrong)?;
    let base = hmac_md5(&key, &[proof]);
    let exported = if auth.flags & flags::KEY_EXCH != 0 {
        let mut key: [u8; KEY] = auth
            .encrypted_key
            .try_into()
            .map_err(|_| Refusal::Malformed("an encrypted session key not 16 bytes long"))?;
        Rc4::new(&base).apply(&mut key);
        key
    } else {
        base
    };
    // The MIC covers the three messages, so that no flag is changed on
    // the way: the client says in its blob that it sent one.
    let pairs = &blob[28..];
    let has_mic = av_pair(pairs, av::FLAGS)
        .and_then(|v| v.try_into().ok())
        .is_some_and(|v: [u8; 4]| u32::from_le_bytes(v) & 0x2 != 0);
    if has_mic {
        let mic = authenticate
            .get(MIC_AT..MIC_AT + 16)
            .ok_or(Refusal::Malformed("a MIC past the message's end"))?;
        let mut zeroed = authenticate.to_vec();
        zeroed[MIC_AT..MIC_AT + 16].fill(0);
        let made = hmac_md5(&exported, &[negotiate, challenge, &zeroed]);
        if !constant_eq(&made, mic) {
            return Err(Refusal::Tampered);
        }
    }
    Ok(exported)
}

/// Whether `a` and `b` are the same, in a time that does not say where
/// they differ.
pub fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// The user's name an AUTHENTICATE message gives, for slowing down the
/// wrong passwords given for it before it is checked.
pub fn user_of(authenticate: &[u8]) -> Option<String> {
    Authenticate::parse(authenticate).ok().map(|a| a.user)
}

pub use crate::request::{hex, unhex};

#[cfg(test)]
mod tests {
    use super::*;

    // MS-NLMP 4.2.4's example: user "User", domain "Domain", password
    // "Password", the server's challenge 0123456789abcdef, the client's
    // aaaaaaaaaaaaaaaa, at time 0.
    const SERVER_CHALLENGE: [u8; 8] = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];

    fn unhexed(text: &str) -> Vec<u8> {
        unhex(&text.replace(' ', "")).unwrap()
    }

    fn blob() -> Vec<u8> {
        let mut blob = vec![1, 1, 0, 0, 0, 0, 0, 0];
        blob.extend([0; 8]);
        blob.extend([0xaa; 8]);
        blob.extend([0; 4]);
        blob.extend(unhexed("02 00 0c 00 44 00 6f 00 6d 00 61 00 69 00 6e 00"));
        blob.extend(unhexed("01 00 0c 00 53 00 65 00 72 00 76 00 65 00 72 00"));
        blob.extend([0; 4]);
        blob.extend([0; 4]);
        blob
    }

    /// An AUTHENTICATE message with `nt_response`, `user` and `domain`,
    /// with no MIC.
    fn authenticate(nt_response: &[u8], user: &str, domain: &str, key: &[u8], f: u32) -> Vec<u8> {
        let (user, domain) = (utf16(user), utf16(domain));
        let mut message = SIGNATURE.to_vec();
        message.extend(3u32.to_le_bytes());
        let mut payload = Vec::new();
        let mut put = |message: &mut Vec<u8>, bytes: &[u8]| {
            let offset = 88 + payload.len();
            message.extend((bytes.len() as u16).to_le_bytes());
            message.extend((bytes.len() as u16).to_le_bytes());
            message.extend((offset as u32).to_le_bytes());
            payload.extend_from_slice(bytes);
        };
        put(&mut message, &[]);
        put(&mut message, nt_response);
        put(&mut message, &domain);
        put(&mut message, &user);
        put(&mut message, &[]);
        put(&mut message, key);
        message.extend(f.to_le_bytes());
        message.extend([0; 8]);
        message.extend([0; 16]);
        message.extend(payload);
        message
    }

    fn challenge() -> Vec<u8> {
        let mut message = SIGNATURE.to_vec();
        message.extend(2u32.to_le_bytes());
        message.extend([0; 12]);
        message.extend(SERVER_CHALLENGE);
        message.extend([0; 24]);
        message
    }

    fn password_hash() -> [u8; KEY] {
        nt_hash(b"Password").unwrap()
    }

    #[test]
    fn the_nt_hash_is_ms_nlmps() {
        assert_eq!(hex(&password_hash()), "a4f49c406510bdcab6824ee7c30fd852");
    }

    #[test]
    fn ntlmv2_is_checked_as_ms_nlmp_makes_it() {
        let key = hmac_md5(&password_hash(), &[&utf16("USER"), &utf16("Domain")]);
        assert_eq!(hex(&key), "0c868a403bfd7a93a3001ef22ef02e3f");
        let proof = hmac_md5(&key, &[&SERVER_CHALLENGE, &blob()]);
        assert_eq!(hex(&proof), "68cd0ab851e51c96aabc927bebef6a1c");
        let mut response = proof.to_vec();
        response.extend(blob());
        let message = authenticate(&response, "User", "Domain", &[], flags::UNICODE);
        let key = check(&password_hash(), &[], &challenge(), &message).unwrap();
        assert_eq!(hex(&key), "8de40ccadbc14a82f15cb0ad0de95ca3");
        // The user's name in any case, as Windows takes it.
        let message = authenticate(&response, "user", "Domain", &[], flags::UNICODE);
        assert!(check(&password_hash(), &[], &challenge(), &message).is_ok());
    }

    #[test]
    fn the_session_key_the_client_chose_is_decrypted() {
        let key = hmac_md5(&password_hash(), &[&utf16("USER"), &utf16("Domain")]);
        let proof = hmac_md5(&key, &[&SERVER_CHALLENGE, &blob()]);
        let mut response = proof.to_vec();
        response.extend(blob());
        // MS-NLMP 4.2.4.2.3: RC4 of the random session key, 0x55 x 16.
        let encrypted = unhexed("c5dad2544fc9799094ce1ce90bc9d03e");
        let f = flags::UNICODE | flags::KEY_EXCH;
        let message = authenticate(&response, "User", "Domain", &encrypted, f);
        let key = check(&password_hash(), &[], &challenge(), &message).unwrap();
        assert_eq!(key, [0x55; 16]);
    }

    #[test]
    fn a_wrong_password_and_ntlmv1_are_refused() {
        let key = hmac_md5(&password_hash(), &[&utf16("USER"), &utf16("Domain")]);
        let proof = hmac_md5(&key, &[&SERVER_CHALLENGE, &blob()]);
        let mut response = proof.to_vec();
        response.extend(blob());
        let message = authenticate(&response, "User", "Domain", &[], flags::UNICODE);
        let other = nt_hash(b"password").unwrap();
        assert_eq!(check(&other, &[], &challenge(), &message), Err(Refusal::Wrong));
        // Another user's name is another key.
        let message = authenticate(&response, "Someone", "Domain", &[], flags::UNICODE);
        assert_eq!(check(&password_hash(), &[], &challenge(), &message), Err(Refusal::Wrong));
        let message = authenticate(&[0; 24], "User", "Domain", &[], flags::UNICODE);
        assert_eq!(check(&password_hash(), &[], &challenge(), &message), Err(Refusal::Old));
    }

    #[test]
    fn a_mic_is_checked_when_the_client_says_it_sent_one() {
        let mut blob = blob();
        // MsvAvFlags 0x2 before the end of the list.
        let end = blob.len() - 8;
        blob.splice(end..end, unhexed("06 00 04 00 02 00 00 00"));
        let key = hmac_md5(&password_hash(), &[&utf16("USER"), &utf16("Domain")]);
        let proof = hmac_md5(&key, &[&SERVER_CHALLENGE, &blob]);
        let base = hmac_md5(&key, &[&proof]);
        let mut response = proof.to_vec();
        response.extend(&blob);
        let mut message = authenticate(&response, "User", "Domain", &[], flags::UNICODE);
        let negotiate = b"NTLMSSP\0\x01\0\0\0\x07\x82\x08\xa2".to_vec();
        let mic = hmac_md5(&base, &[&negotiate, &challenge(), &message]);
        assert_eq!(
            check(&password_hash(), &negotiate, &challenge(), &message),
            Err(Refusal::Tampered)
        );
        message[MIC_AT..MIC_AT + 16].copy_from_slice(&mic);
        assert_eq!(check(&password_hash(), &negotiate, &challenge(), &message), Ok(base));
        // A changed flag in the NEGOTIATE message shows.
        let mut changed = negotiate.clone();
        changed[12] ^= 0x10;
        assert_eq!(
            check(&password_hash(), &changed, &challenge(), &message),
            Err(Refusal::Tampered)
        );
    }

    #[test]
    fn malformed_messages_are_refused_not_read_past() {
        let message = authenticate(&[0; 60], "User", "Domain", &[], flags::UNICODE);
        for end in 0..message.len() {
            let _ = check(&password_hash(), &[], &challenge(), &message[..end]);
        }
        let mut wild = message.clone();
        wild[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            check(&password_hash(), &[], &challenge(), &wild),
            Err(Refusal::Malformed(_))
        ));
        assert_eq!(user_of(b"NTLMSSP\0"), None);
    }

    #[test]
    fn rc4_is_rfc_6229s() {
        // RFC 6229's first test vector: key 0102030405, offset 0.
        let mut stream = [0u8; 16];
        Rc4::new(&[1, 2, 3, 4, 5]).apply(&mut stream);
        assert_eq!(hex(&stream), "b2396305f03dc027ccc3524a0a1118a8");
    }

    #[test]
    fn hex_goes_both_ways() {
        assert_eq!(unhex(&hex(&[0, 1, 0xfe])), Some(vec![0, 1, 0xfe]));
        assert_eq!(unhex("0"), None);
        assert_eq!(unhex("zz"), None);
    }
}
