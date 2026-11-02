// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! NEGOTIATE (MS-SMB2 2.2.3, 2.2.4 and 3.3.5.4): which dialect, cipher
//! and signature a connection uses.
//!
//! smbd speaks SMB 3.0, 3.0.2 and 3.1.1, which encrypt, and requires
//! signing; a client that offers only SMB 2, or no cipher smbd has, is
//! refused. SMB 3.1.1's preauthentication integrity (SHA-512) is always
//! on with it. A client that opens with SMB 1's NEGOTIATE asking for SMB 2,
//! as older Windows does, is told to send SMB 2's.

use smb2::crypto::encryption::Cipher;
use smb2::crypto::kdf::PreauthHasher;

use crate::auth;
use crate::crypto::{self, Signing, dialect};
use crate::wire::{self, Header, Out, Status, status};

/// The capabilities smbd has (MS-SMB2 2.2.4): reads and writes over 64
/// KiB at once.
const LARGE_MTU: u32 = 0x0000_0004;
/// Encryption, which SMB 3.0 and 3.0.2 announce as a capability.
const ENCRYPTION: u32 = 0x0000_0040;
/// Signing enabled and required.
pub const SECURITY_MODE: u16 = 0x0003;

/// The negotiate contexts (MS-SMB2 2.2.3.1).
const PREAUTH_INTEGRITY: u16 = 1;
const ENCRYPTION_CAPABILITIES: u16 = 2;
const SIGNING_CAPABILITIES: u16 = 8;
/// SHA-512, the one preauthentication hash.
const SHA_512: u16 = 1;

/// The ciphers smbd offers, best first: GCM is faster than CCM.
const CIPHERS: [Cipher; 4] =
    [Cipher::Aes128Gcm, Cipher::Aes256Gcm, Cipher::Aes128Ccm, Cipher::Aes256Ccm];

/// What a connection's NEGOTIATE settled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Negotiated {
    /// The dialect.
    pub dialect: u16,
    /// The cipher.
    pub cipher: Cipher,
    /// How messages are signed.
    pub signing: Signing,
    /// What the client said of itself, which FSCTL_VALIDATE_NEGOTIATE_INFO
    /// checks again: its GUID, security mode, capabilities and dialects.
    pub client_guid: [u8; 16],
    /// The client's security mode.
    pub client_security_mode: u16,
    /// The client's capabilities.
    pub client_capabilities: u32,
    /// The dialects the client offered.
    pub client_dialects: Vec<u16>,
    /// The server's GUID.
    pub server_guid: [u8; 16],
    /// The server's capabilities, as the response said them.
    pub server_capabilities: u32,
    /// SMB 3.1.1's preauthentication hash, after the NEGOTIATE request
    /// and response.
    pub preauth: [u8; 64],
}

/// What answers a NEGOTIATE request.
pub enum Outcome {
    /// The connection goes on with these, after the response.
    Settled(Box<Negotiated>, Vec<u8>),
    /// The response refuses the connection, which then ends.
    Refused(Vec<u8>),
}

/// Answers SMB 1's NEGOTIATE (MS-SMB2 3.3.5.3.1): SMB 2's response with
/// dialect 0x02FF when it asks for "SMB 2.???", nothing otherwise.
pub fn smb1(request: &[u8], server_guid: [u8; 16]) -> Option<Vec<u8>> {
    // The header is 32 bytes; then WordCount, which is 0, ByteCount and
    // the dialects, each 0x02 and a string ended by a NUL.
    if request.len() < 35 || request[..4] != wire::PROTOCOL_1 || request[4] != 0x72 {
        return None;
    }
    let count = wire::u16_at(request, 33).ok()? as usize;
    let dialects = request.get(35..35 + count)?;
    let asks = dialects.split(|&b| b == 0x02).any(|d| d.strip_suffix(&[0]) == Some(b"SMB 2.???"));
    if !asks {
        return None;
    }
    let header = Header { flags: wire::flags::SERVER_TO_REDIR, credits: 1, ..Header::default() };
    Some(wire::message(&header, &response_body(dialect::WILDCARD, server_guid, 0, &[])))
}

/// The body of a NEGOTIATE response, with `contexts` after the security
/// buffer.
fn response_body(
    dialect: u16,
    server_guid: [u8; 16],
    capabilities: u32,
    contexts: &[Vec<u8>],
) -> Vec<u8> {
    let offer = auth::offer();
    let mut out = Out::new();
    out.u16(65)
        .u16(SECURITY_MODE)
        .u16(dialect)
        .u16(contexts.len() as u16)
        .bytes(&server_guid)
        .u32(capabilities)
        .u32(wire::MAX_IO)
        .u32(wire::MAX_IO)
        .u32(wire::MAX_IO)
        .u64(wire::now())
        .u64(0)
        .u16((wire::HEADER + 64) as u16)
        .u16(offer.len() as u16);
    let contexts_at = out.len();
    out.u32(0);
    out.bytes(&offer);
    if !contexts.is_empty() {
        // Each context starts on 8 bytes, counted from the header.
        out.align(8);
        let at = wire::HEADER + out.len();
        out.set_u32(contexts_at, at as u32);
        for (i, context) in contexts.iter().enumerate() {
            if i > 0 {
                out.align(8);
            }
            out.bytes(context);
        }
    }
    out.0
}

/// A negotiate context of `kind` holding `data`.
fn context(kind: u16, data: &[u8]) -> Vec<u8> {
    let mut out = Out::new();
    out.u16(kind).u16(data.len() as u16).u32(0).bytes(data);
    out.0
}

/// The `u16`s of a list of `count` from `at`.
fn ids(bytes: &[u8], at: usize, count: usize) -> Result<Vec<u16>, Status> {
    (0..count).map(|i| wire::u16_at(bytes, at + 2 * i)).collect()
}

/// Answers SMB 2's NEGOTIATE request `request`, whose header is
/// `header`, as the server `server_guid`, with `salt` for the
/// preauthentication context.
pub fn negotiate(
    request: &[u8],
    header: &Header,
    server_guid: [u8; 16],
    salt: [u8; 32],
) -> Outcome {
    match settle(request, server_guid, salt) {
        Ok((mut negotiated, body)) => {
            let mut out = header.response(status::SUCCESS);
            out.credits = header.credits.max(1);
            out.async_id = 0;
            let response = wire::message(&out, &body);
            if negotiated.dialect == dialect::SMB_3_1_1 {
                let mut hasher = PreauthHasher::new();
                hasher.update(request);
                hasher.update(&response);
                negotiated.preauth = *hasher.value();
            }
            Outcome::Settled(Box::new(negotiated), response)
        }
        Err(code) => {
            let mut out = header.response(code);
            out.credits = 1;
            out.async_id = 0;
            Outcome::Refused(wire::message(&out, &wire::error_body()))
        }
    }
}

/// What a NEGOTIATE request settles, and the response's body.
fn settle(
    request: &[u8],
    server_guid: [u8; 16],
    salt: [u8; 32],
) -> Result<(Negotiated, Vec<u8>), Status> {
    let body = wire::HEADER;
    if wire::u16_at(request, body)? != 36 {
        return Err(status::INVALID_PARAMETER);
    }
    let count = wire::u16_at(request, body + 2)? as usize;
    let client_security_mode = wire::u16_at(request, body + 4)?;
    let client_capabilities = wire::u32_at(request, body + 8)?;
    let mut client_guid = [0u8; 16];
    client_guid.copy_from_slice(wire::slice_at(request, body + 12, 16)?);
    let client_dialects = ids(request, body + 36, count)?;
    let dialect = [dialect::SMB_3_1_1, dialect::SMB_3_0_2, dialect::SMB_3_0]
        .into_iter()
        .find(|d| client_dialects.contains(d))
        // SMB 2 cannot encrypt.
        .ok_or(status::NOT_SUPPORTED)?;
    let mut negotiated = Negotiated {
        dialect,
        cipher: Cipher::Aes128Ccm,
        signing: Signing::Cmac,
        client_guid,
        client_security_mode,
        client_capabilities,
        client_dialects,
        server_guid,
        server_capabilities: LARGE_MTU,
        preauth: [0; 64],
    };
    if dialect != dialect::SMB_3_1_1 {
        if client_capabilities & ENCRYPTION == 0 {
            return Err(status::NOT_SUPPORTED);
        }
        negotiated.server_capabilities |= ENCRYPTION;
        let body = response_body(dialect, server_guid, negotiated.server_capabilities, &[]);
        return Ok((negotiated, body));
    }
    let mut at = wire::u32_at(request, body + 28)? as usize;
    let contexts = wire::u16_at(request, body + 32)?;
    let (mut hashed, mut ciphered, mut signed) = (false, false, false);
    for i in 0..contexts {
        if i > 0 {
            at = at.next_multiple_of(8);
        }
        let kind = wire::u16_at(request, at)?;
        let length = wire::u16_at(request, at + 2)? as usize;
        let data = wire::slice_at(request, at + 8, length)?;
        match kind {
            PREAUTH_INTEGRITY => {
                let count = wire::u16_at(data, 0)? as usize;
                hashed = ids(data, 4, count)?.contains(&SHA_512);
            }
            ENCRYPTION_CAPABILITIES => {
                let count = wire::u16_at(data, 0)? as usize;
                let offered = ids(data, 2, count)?;
                if let Some(&cipher) =
                    CIPHERS.iter().find(|&&c| offered.contains(&crypto::cipher_id(c)))
                {
                    negotiated.cipher = cipher;
                    ciphered = true;
                }
            }
            SIGNING_CAPABILITIES => {
                let count = wire::u16_at(data, 0)? as usize;
                let offered = ids(data, 2, count)?;
                signed = true;
                negotiated.signing = [Signing::Gmac, Signing::Cmac]
                    .into_iter()
                    .find(|s| offered.contains(&s.id()))
                    .unwrap_or(Signing::Cmac);
            }
            // Compression, the server's name, transports and RDMA: none
            // of them, so no answer.
            _ => {}
        }
        at += 8 + length;
    }
    if !hashed {
        return Err(status::INVALID_PARAMETER);
    }
    if !ciphered {
        return Err(status::NOT_SUPPORTED);
    }
    let mut preauth = Out::new();
    preauth.u16(1).u16(salt.len() as u16).u16(SHA_512).bytes(&salt);
    let mut cipher = Out::new();
    cipher.u16(1).u16(crypto::cipher_id(negotiated.cipher));
    let mut contexts =
        vec![context(PREAUTH_INTEGRITY, &preauth.0), context(ENCRYPTION_CAPABILITIES, &cipher.0)];
    if signed {
        let mut signing = Out::new();
        signing.u16(1).u16(negotiated.signing.id());
        contexts.push(context(SIGNING_CAPABILITIES, &signing.0));
    }
    let body = response_body(dialect, server_guid, negotiated.server_capabilities, &contexts);
    Ok((negotiated, body))
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A NEGOTIATE request offering `dialects`, with 3.1.1's contexts
    /// offering `ciphers` and `signings`.
    pub fn request(
        dialects: &[u16],
        ciphers: &[u16],
        signings: &[u16],
        capabilities: u32,
    ) -> Vec<u8> {
        let header = Header { command: wire::command::NEGOTIATE, credits: 31, ..Header::default() };
        let mut body = Out::new();
        body.u16(36).u16(dialects.len() as u16).u16(1).u16(0).u32(capabilities).bytes(&[9; 16]);
        let contexts_at = body.len();
        body.u32(0).u16(0).u16(0);
        for &d in dialects {
            body.u16(d);
        }
        if dialects.contains(&dialect::SMB_3_1_1) {
            body.align(8);
            body.set_u32(contexts_at, (wire::HEADER + body.len()) as u32);
            let mut preauth = Out::new();
            preauth.u16(1).u16(4).u16(SHA_512).u32(0xdead_beef);
            let mut cipher = Out::new();
            cipher.u16(ciphers.len() as u16);
            for &c in ciphers {
                cipher.u16(c);
            }
            let mut signing = Out::new();
            signing.u16(signings.len() as u16);
            for &s in signings {
                signing.u16(s);
            }
            let all = [
                context(PREAUTH_INTEGRITY, &preauth.0),
                context(ENCRYPTION_CAPABILITIES, &cipher.0),
                context(SIGNING_CAPABILITIES, &signing.0),
            ];
            body.set_u16(contexts_at + 4, all.len() as u16);
            for (i, c) in all.iter().enumerate() {
                if i > 0 {
                    body.align(8);
                }
                body.bytes(c);
            }
        }
        wire::message(&header, &body.0)
    }

    fn answer(request: &[u8]) -> Outcome {
        let header = Header::parse(request).unwrap();
        negotiate(request, &header, [3; 16], [4; 32])
    }

    #[test]
    fn smb_3_1_1_settles_on_the_best_cipher_and_gmac() {
        let request = request(&[0x0202, 0x0210, 0x0300, 0x0302, 0x0311], &[1, 2, 4], &[1, 2], 0x7f);
        let Outcome::Settled(settled, response) = answer(&request) else { panic!("refused") };
        assert_eq!(settled.dialect, dialect::SMB_3_1_1);
        assert_eq!(settled.cipher, Cipher::Aes128Gcm);
        assert_eq!(settled.signing, Signing::Gmac);
        assert_ne!(settled.preauth, [0; 64]);
        let header = Header::parse(&response).unwrap();
        assert_eq!((header.status, header.credits), (status::SUCCESS, 31));
        assert_eq!(wire::u16_at(&response, 68).unwrap(), dialect::SMB_3_1_1);
        assert_eq!(wire::u16_at(&response, 70).unwrap(), 3);
        // The contexts start on 8 bytes, after the security buffer.
        let at = wire::u32_at(&response, 124).unwrap() as usize;
        assert_eq!(at % 8, 0);
        assert_eq!(wire::u16_at(&response, at).unwrap(), PREAUTH_INTEGRITY);
    }

    #[test]
    fn smb_3_0_needs_the_encryption_capability() {
        let Outcome::Settled(settled, _) = answer(&request(&[0x0300], &[], &[], 0x40)) else {
            panic!("refused")
        };
        assert_eq!((settled.dialect, settled.cipher), (dialect::SMB_3_0, Cipher::Aes128Ccm));
        assert!(matches!(answer(&request(&[0x0300], &[], &[], 0)), Outcome::Refused(_)));
    }

    #[test]
    fn smb_2_and_no_cipher_are_refused() {
        assert!(matches!(answer(&request(&[0x0202, 0x0210], &[], &[], 0x7f)), Outcome::Refused(_)));
        assert!(matches!(answer(&request(&[0x0311], &[], &[1], 0x7f)), Outcome::Refused(_)));
        let request = request(&[0x0311], &[2], &[1], 0x7f);
        for end in 64..request.len() {
            let mut cut = request[..end].to_vec();
            // Every cut is answered, never read past.
            let header = Header::parse(&cut).unwrap();
            let _ = negotiate(&cut, &header, [0; 16], [0; 32]);
            cut.clear();
        }
    }

    #[test]
    fn smb_1_asking_for_smb_2_is_told_to_send_it() {
        let mut request = wire::PROTOCOL_1.to_vec();
        request.push(0x72);
        request.extend([0; 27]);
        let dialects = b"\x02NT LM 0.12\0\x02SMB 2.002\0\x02SMB 2.???\0";
        request.push(0);
        request.extend((dialects.len() as u16).to_le_bytes());
        request.extend(dialects);
        let response = smb1(&request, [1; 16]).unwrap();
        assert_eq!(wire::u16_at(&response, 68).unwrap(), dialect::WILDCARD);
        let old = request.len() - 11;
        assert_eq!(smb1(&request[..old], [1; 16]), None);
    }
}
