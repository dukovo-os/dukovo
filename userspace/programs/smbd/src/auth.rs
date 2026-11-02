// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The server's side of a sign-in: SPNEGO (RFC 4178, MS-SPNG) around
//! NTLM's messages (MS-NLMP), as SESSION_SETUP carries them.
//!
//! smbd offers NTLM alone. It reads the client's NEGOTIATE message, makes
//! the CHALLENGE, and passes all three messages to authd, which checks
//! the AUTHENTICATE message against the person's sharing password; smbd
//! never sees a password or its hash. With the session key authd answers
//! with, it checks the client's mechListMIC, which says that no one
//! changed the list of mechanisms on the way, and makes its own.
//!
//! The DER here is read with every length checked against what holds
//! it, and nothing is read that SPNEGO does not need.

use accounts::ntlm::{self, Rc4, av, flags};

use crate::wire::to_utf16;

/// SPNEGO's object identifier, 1.3.6.1.5.5.2.
const OID_SPNEGO: &[u8] = &[0x06, 0x06, 0x2b, 0x06, 0x01, 0x05, 0x05, 0x02];
/// NTLMSSP's, 1.3.6.1.4.1.311.2.2.10.
pub const OID_NTLMSSP: &[u8] =
    &[0x06, 0x0a, 0x2b, 0x06, 0x01, 0x04, 0x01, 0x82, 0x37, 0x02, 0x02, 0x0a];

/// A DER element: its tag, its contents, and all its bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Element<'a> {
    tag: u8,
    contents: &'a [u8],
    whole: &'a [u8],
}

/// The DER element at the start of `bytes`, and what follows it.
fn element(bytes: &[u8]) -> Option<(Element<'_>, &[u8])> {
    let tag = *bytes.first()?;
    let first = *bytes.get(1)?;
    let (length, head) = if first < 0x80 {
        (first as usize, 2)
    } else {
        let count = (first & 0x7f) as usize;
        if count == 0 || count > 4 {
            return None;
        }
        let mut length = 0usize;
        for &b in bytes.get(2..2 + count)? {
            length = (length << 8) | b as usize;
        }
        (length, 2 + count)
    };
    let end = head.checked_add(length)?;
    let contents = bytes.get(head..end)?;
    Some((Element { tag, contents, whole: &bytes[..end] }, &bytes[end..]))
}

/// The elements of `bytes`, one after another, all of them.
fn elements(mut bytes: &[u8]) -> Option<Vec<Element<'_>>> {
    let mut all = Vec::new();
    while !bytes.is_empty() {
        let (one, rest) = element(bytes)?;
        all.push(one);
        bytes = rest;
    }
    Some(all)
}

/// A DER element of `tag` holding `contents`.
fn der(tag: u8, contents: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let length = contents.len();
    if length < 0x80 {
        out.push(length as u8);
    } else {
        let bytes = length.to_be_bytes();
        let skip = bytes.iter().take_while(|&&b| b == 0).count();
        out.push(0x80 | (bytes.len() - skip) as u8);
        out.extend_from_slice(&bytes[skip..]);
    }
    out.extend_from_slice(contents);
    out
}

/// What the NEGOTIATE response offers: SPNEGO's NegTokenInit with NTLM
/// as the one mechanism (RFC 4178 4.2.1, MS-SPNG 2.2.1).
pub fn offer() -> Vec<u8> {
    let mechs = der(0xa0, &der(0x30, OID_NTLMSSP));
    let init = der(0xa0, &der(0x30, &mechs));
    let mut token = OID_SPNEGO.to_vec();
    token.extend_from_slice(&init);
    der(0x60, &token)
}

/// What a client's SESSION_SETUP brings.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Token {
    /// The NTLM message, if it holds one.
    pub ntlm: Option<Vec<u8>>,
    /// The DER of the client's list of mechanisms, from its first token:
    /// what the mechListMIC covers.
    pub mech_types: Option<Vec<u8>>,
    /// Whether NTLM is the client's first choice, or among them at all.
    pub ntlm_first: bool,
    /// Whether NTLM is among them.
    pub ntlm_offered: bool,
    /// The client's mechListMIC.
    pub mic: Option<Vec<u8>>,
}

/// Reads a client's security buffer: SPNEGO's first token (NegTokenInit
/// in GSS-API's wrapping), a later one (NegTokenResp), or NTLM's message
/// bare. `None` for anything else.
pub fn read_token(buffer: &[u8]) -> Option<Token> {
    if buffer.starts_with(ntlm::SIGNATURE) {
        return Some(Token {
            ntlm: Some(buffer.to_vec()),
            ntlm_first: true,
            ntlm_offered: true,
            ..Token::default()
        });
    }
    let (outer, rest) = element(buffer)?;
    if !rest.is_empty() {
        return None;
    }
    match outer.tag {
        // [APPLICATION 0]: SPNEGO's OID, then NegTokenInit, [0].
        0x60 => {
            let (oid, rest) = element(outer.contents)?;
            if oid.whole != OID_SPNEGO {
                return None;
            }
            let (init, rest) = element(rest)?;
            if init.tag != 0xa0 || !rest.is_empty() {
                return None;
            }
            read_init(init.contents)
        }
        0xa0 => read_init(outer.contents),
        // NegTokenResp, [1].
        0xa1 => {
            let (sequence, rest) = element(outer.contents)?;
            if sequence.tag != 0x30 || !rest.is_empty() {
                return None;
            }
            let mut token = Token::default();
            for field in elements(sequence.contents)? {
                let (inner, rest) = element(field.contents)?;
                if !rest.is_empty() {
                    return None;
                }
                match field.tag {
                    // negState and supportedMech say nothing a server needs.
                    0xa0 | 0xa1 => {}
                    0xa2 if inner.tag == 0x04 => token.ntlm = Some(inner.contents.to_vec()),
                    0xa3 if inner.tag == 0x04 => token.mic = Some(inner.contents.to_vec()),
                    _ => return None,
                }
            }
            Some(token)
        }
        _ => None,
    }
}

/// Reads NegTokenInit's SEQUENCE.
fn read_init(contents: &[u8]) -> Option<Token> {
    let (sequence, rest) = element(contents)?;
    if sequence.tag != 0x30 || !rest.is_empty() {
        return None;
    }
    let mut token = Token::default();
    for field in elements(sequence.contents)? {
        let (inner, rest) = element(field.contents)?;
        if !rest.is_empty() {
            return None;
        }
        match field.tag {
            0xa0 if inner.tag == 0x30 => {
                let mechs = elements(inner.contents)?;
                token.ntlm_first = mechs.first().is_some_and(|m| m.whole == OID_NTLMSSP);
                token.ntlm_offered = mechs.iter().any(|m| m.whole == OID_NTLMSSP);
                token.mech_types = Some(inner.whole.to_vec());
            }
            // reqFlags.
            0xa1 => {}
            0xa2 if inner.tag == 0x04 => token.ntlm = Some(inner.contents.to_vec()),
            0xa3 if inner.tag == 0x04 => token.mic = Some(inner.contents.to_vec()),
            _ => return None,
        }
    }
    Some(token)
}

/// SPNEGO's negState.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// The sign-in is done.
    Completed = 0,
    /// It needs another round.
    Incomplete = 1,
    /// It is refused.
    Rejected = 2,
}

/// The server's NegTokenResp: `state`, NTLM as the mechanism when
/// `mech`, then the NTLM message and the mechListMIC, if any.
pub fn answer(state: State, mech: bool, ntlm: Option<&[u8]>, mic: Option<&[u8]>) -> Vec<u8> {
    let mut fields = der(0xa0, &der(0x0a, &[state as u8]));
    if mech {
        fields.extend(der(0xa1, OID_NTLMSSP));
    }
    if let Some(ntlm) = ntlm {
        fields.extend(der(0xa2, &der(0x04, ntlm)));
    }
    if let Some(mic) = mic {
        fields.extend(der(0xa3, &der(0x04, mic)));
    }
    der(0xa1, &der(0x30, &fields))
}

/// The server's names, as a CHALLENGE message gives them.
pub struct Names {
    /// The NetBIOS name: the host's name, in capitals, at most 15
    /// characters.
    pub netbios: String,
    /// The DNS name.
    pub dns: String,
}

impl Names {
    /// The names of a host called `hostname`.
    pub fn of(hostname: &str) -> Names {
        let short = hostname.split('.').next().unwrap_or_default();
        let netbios: String = short.to_uppercase().chars().take(15).collect();
        let netbios = if netbios.is_empty() { "DUKOVO".to_string() } else { netbios };
        Names { netbios, dns: hostname.to_lowercase() }
    }
}

/// The flags of the CHALLENGE answering a NEGOTIATE message with
/// `asked`: what both sides do, with NTLMv2's session security, 128-bit
/// keys, and the target's information always.
pub fn challenge_flags(asked: u32) -> u32 {
    let echoed = flags::UNICODE
        | flags::SIGN
        | flags::SEAL
        | flags::ALWAYS_SIGN
        | flags::KEY_EXCH
        | flags::KEY_56
        | flags::KEY_128
        | flags::VERSION;
    let mut out = (asked & echoed)
        | flags::REQUEST_TARGET
        | flags::NTLM
        | flags::TARGET_TYPE_SERVER
        | flags::EXTENDED_SESSION_SECURITY
        | flags::TARGET_INFO;
    if out & flags::UNICODE == 0 {
        out |= flags::OEM;
    }
    out
}

/// The CHALLENGE message (MS-NLMP 2.2.1.2) answering `negotiate`, with
/// the random `challenge` and the time `now`, from a server of `names`.
/// Its target information has the time, so that the client sends a MIC.
pub fn challenge(negotiate: &[u8], challenge: [u8; 8], now: u64, names: &Names) -> Vec<u8> {
    let asked = negotiate.get(12..16).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let flags = challenge_flags(asked);
    let target = to_utf16(&names.netbios);
    let mut info = Vec::new();
    let mut pair = |id: u16, value: &[u8]| {
        info.extend_from_slice(&id.to_le_bytes());
        info.extend_from_slice(&(value.len() as u16).to_le_bytes());
        info.extend_from_slice(value);
    };
    pair(av::NB_DOMAIN_NAME, &target);
    pair(av::NB_COMPUTER_NAME, &target);
    pair(av::DNS_DOMAIN_NAME, &to_utf16(&names.dns));
    pair(av::DNS_COMPUTER_NAME, &to_utf16(&names.dns));
    pair(av::TIMESTAMP, &now.to_le_bytes());
    pair(av::EOL, &[]);
    let mut out = ntlm::SIGNATURE.to_vec();
    out.extend_from_slice(&2u32.to_le_bytes());
    let at = 56u32;
    out.extend_from_slice(&(target.len() as u16).to_le_bytes());
    out.extend_from_slice(&(target.len() as u16).to_le_bytes());
    out.extend_from_slice(&at.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&challenge);
    out.extend_from_slice(&[0; 8]);
    out.extend_from_slice(&(info.len() as u16).to_le_bytes());
    out.extend_from_slice(&(info.len() as u16).to_le_bytes());
    out.extend_from_slice(&(at + target.len() as u32).to_le_bytes());
    // The version: 10.0, NTLM's revision 15.
    out.extend_from_slice(&[10, 0, 0, 0, 0, 0, 0, 15]);
    out.extend_from_slice(&target);
    out.extend_from_slice(&info);
    out
}

/// The flags an AUTHENTICATE message settled on.
pub fn authenticate_flags(authenticate: &[u8]) -> u32 {
    authenticate.get(60..64).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// NTLM's keys for signing (MS-NLMP 3.4.5.2 and 3.4.5.3), one side's.
fn side_keys(session: &[u8; 16], flags: u32, client: bool) -> ([u8; 16], Rc4) {
    let (sign, seal): (&[u8], &[u8]) = if client {
        (
            b"session key to client-to-server signing key magic constant\0",
            b"session key to client-to-server sealing key magic constant\0",
        )
    } else {
        (
            b"session key to server-to-client signing key magic constant\0",
            b"session key to server-to-client sealing key magic constant\0",
        )
    };
    let seal_key: &[u8] = if flags & flags::KEY_128 != 0 {
        session
    } else if flags & flags::KEY_56 != 0 {
        &session[..7]
    } else {
        &session[..5]
    };
    (ntlm::md5(&[session, sign]), Rc4::new(&ntlm::md5(&[seal_key, seal])))
}

/// NTLM's signature of `message`, the first a side makes (MS-NLMP
/// 3.4.4.2): its version, the checksum, sealed when the keys were
/// exchanged, and the sequence number 0.
fn mac(session: &[u8; 16], flags: u32, client: bool, message: &[u8]) -> [u8; 16] {
    let (sign, mut seal) = side_keys(session, flags, client);
    let sequence = 0u32.to_le_bytes();
    let mut checksum = [0u8; 8];
    checksum.copy_from_slice(&ntlm::hmac_md5(&sign, &[&sequence, message])[..8]);
    if flags & flags::KEY_EXCH != 0 {
        seal.apply(&mut checksum);
    }
    let mut out = [0u8; 16];
    out[..4].copy_from_slice(&1u32.to_le_bytes());
    out[4..12].copy_from_slice(&checksum);
    out[12..].copy_from_slice(&sequence);
    out
}

/// Whether the client's mechListMIC is right for `mech_types`.
pub fn mic_is_right(session: &[u8; 16], flags: u32, mech_types: &[u8], mic: &[u8]) -> bool {
    ntlm::constant_eq(&mac(session, flags, true, mech_types), mic)
}

/// The server's mechListMIC of `mech_types`.
pub fn server_mic(session: &[u8; 16], flags: u32, mech_types: &[u8]) -> [u8; 16] {
    mac(session, flags, false, mech_types)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn der_lengths_go_both_ways() {
        for length in [0, 1, 127, 128, 255, 256, 70_000] {
            let bytes = der(0x04, &vec![7; length]);
            let (one, rest) = element(&bytes).unwrap();
            assert_eq!(one.contents.len(), length);
            assert!(rest.is_empty());
        }
        assert_eq!(element(&[0x04, 0x85, 0, 0, 0, 0, 1]), None);
        assert_eq!(element(&[0x04, 0x82, 0xff, 0xff, 1]), None);
        assert_eq!(element(&[0x04]), None);
    }

    #[test]
    fn smb2s_tokens_are_read() {
        let negotiate = b"NTLMSSP\0\x01\0\0\0\x07\x82\x08\xa2".to_vec();
        let init = smb2::auth::spnego::wrap_neg_token_init(&[OID_NTLMSSP], &negotiate);
        let token = read_token(&init).unwrap();
        assert_eq!(token.ntlm.as_deref(), Some(&negotiate[..]));
        assert!(token.ntlm_first && token.ntlm_offered);
        assert_eq!(token.mech_types.as_deref(), Some(&der(0x30, OID_NTLMSSP)[..]));
        let resp = smb2::auth::spnego::wrap_neg_token_resp(b"NTLMSSP\0\x03");
        assert_eq!(read_token(&resp).unwrap().ntlm.as_deref(), Some(&b"NTLMSSP\0\x03"[..]));
        assert_eq!(read_token(&negotiate).unwrap().ntlm, Some(negotiate.clone()));
        assert_eq!(read_token(b"\x60\x02\x06\x00"), None);
        // Every cut of a token is refused, never read past.
        for end in 0..init.len() {
            let _ = read_token(&init[..end]);
        }
    }

    #[test]
    fn answers_are_read_by_smb2() {
        let made = answer(State::Incomplete, true, Some(b"NTLMSSP\0\x02"), None);
        let read = smb2::auth::spnego::parse_neg_token_resp(&made).unwrap();
        assert_eq!(read.response_token.as_deref(), Some(&b"NTLMSSP\0\x02"[..]));
        let made = answer(State::Completed, false, None, Some(&[1; 16]));
        let read = smb2::auth::spnego::parse_neg_token_resp(&made).unwrap();
        assert_eq!(read.response_token, None);
        assert!(read_token(&offer()).is_some());
    }

    #[test]
    fn names_are_netbioss() {
        let names = Names::of("my-long-computer-name.local");
        assert_eq!(names.netbios, "MY-LONG-COMPUTE");
        assert_eq!(names.dns, "my-long-computer-name.local");
        assert_eq!(Names::of("").netbios, "DUKOVO");
    }

    #[test]
    fn smb2s_client_signs_in_against_the_challenge() {
        use smb2::auth::ntlm::{NtlmAuthenticator, NtlmCredentials};
        let credentials = NtlmCredentials {
            username: "Alice".into(),
            password: "for windows".into(),
            domain: String::new(),
        };
        let mut client = NtlmAuthenticator::new(credentials);
        let negotiate = client.negotiate();
        let made = challenge(
            &negotiate,
            [1, 2, 3, 4, 5, 6, 7, 8],
            133_000_000_000_000_000,
            &Names::of("box"),
        );
        let authenticate = client.authenticate(&made).unwrap();
        let hash = ntlm::nt_hash(b"for windows").unwrap();
        let key = ntlm::check(&hash, &negotiate, &made, &authenticate).unwrap();
        assert_eq!(Some(&key[..]), client.session_key());
        assert_eq!(ntlm::user_of(&authenticate).as_deref(), Some("Alice"));
        let wrong = ntlm::nt_hash(b"for linux").unwrap();
        assert_eq!(
            ntlm::check(&wrong, &negotiate, &made, &authenticate),
            Err(ntlm::Refusal::Wrong)
        );
        // A challenge changed on the way breaks the MIC, or the proof.
        let mut changed = made.clone();
        changed[24] ^= 1;
        assert!(ntlm::check(&hash, &negotiate, &changed, &authenticate).is_err());
    }

    #[test]
    fn the_client_and_server_mics_differ() {
        let session = [5u8; 16];
        let f = flags::KEY_EXCH | flags::KEY_128 | flags::EXTENDED_SESSION_SECURITY;
        let types = der(0x30, OID_NTLMSSP);
        let client = mac(&session, f, true, &types);
        assert!(mic_is_right(&session, f, &types, &client));
        assert_ne!(server_mic(&session, f, &types), client);
        assert!(!mic_is_right(&session, f, &types, &server_mic(&session, f, &types)));
    }
}
