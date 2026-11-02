// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! SMB 3's signatures, keys and encryption, as a server makes and checks
//! them (MS-SMB2 3.1.4).
//!
//! The keys come from the session key NTLM gave, through SP800-108's
//! key derivation, with the preauthentication hash of the sign-in as
//! its context in SMB 3.1.1. A message is signed with AES-CMAC, or with
//! AES-GMAC where 3.1.1 agreed on it; the server's signatures set the
//! role bit of GMAC's nonce, so that a client's signature is never one
//! of the server's. Encryption is AES-CCM or AES-GCM, smb2's, with the
//! client's key for what comes in and the server's for what goes out.

use aes::Aes128;
use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes128Gcm, KeyInit, Nonce};
use cmac::{Cmac, Mac};
use smb2::crypto::encryption::{self, Cipher};
use smb2::crypto::kdf::sp800_108_kdf;

use crate::wire::{self, Status, status};

/// The dialects smbd speaks: SMB 3.0, 3.0.2 and 3.1.1, each of which
/// encrypts. SMB 2's are refused, as they cannot.
pub mod dialect {
    pub const SMB_3_0: u16 = 0x0300;
    pub const SMB_3_0_2: u16 = 0x0302;
    pub const SMB_3_1_1: u16 = 0x0311;
    /// What answers SMB 1's NEGOTIATE that asks for SMB 2: "send SMB 2's".
    pub const WILDCARD: u16 = 0x02ff;
}

/// How messages are signed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signing {
    /// AES-128-CMAC: SMB 3.0 and 3.0.2, and 3.1.1 by default.
    Cmac,
    /// AES-128-GMAC, which SMB 3.1.1 may agree on.
    Gmac,
}

impl Signing {
    /// Its number in SMB2_SIGNING_CAPABILITIES.
    pub fn id(self) -> u16 {
        match self {
            Signing::Cmac => 1,
            Signing::Gmac => 2,
        }
    }

    /// The algorithm numbered `id`.
    pub fn of(id: u16) -> Option<Signing> {
        match id {
            1 => Some(Signing::Cmac),
            2 => Some(Signing::Gmac),
            _ => None,
        }
    }
}

/// A cipher's number in SMB2_ENCRYPTION_CAPABILITIES.
pub fn cipher_id(cipher: Cipher) -> u16 {
    match cipher {
        Cipher::Aes128Ccm => 1,
        Cipher::Aes128Gcm => 2,
        Cipher::Aes256Ccm => 3,
        Cipher::Aes256Gcm => 4,
    }
}

/// The cipher numbered `id`.
pub fn cipher_of(id: u16) -> Option<Cipher> {
    match id {
        1 => Some(Cipher::Aes128Ccm),
        2 => Some(Cipher::Aes128Gcm),
        3 => Some(Cipher::Aes256Ccm),
        4 => Some(Cipher::Aes256Gcm),
        _ => None,
    }
}

/// The signature of `message`, whose signature field is zero.
fn signature(
    message: &[u8],
    key: &[u8; 16],
    signing: Signing,
    message_id: u64,
    cancel: bool,
    server: bool,
) -> [u8; 16] {
    match signing {
        Signing::Cmac => {
            let mut mac = <Cmac<Aes128> as KeyInit>::new_from_slice(key).expect("a 16-byte key");
            mac.update(message);
            let mut out = [0u8; 16];
            out.copy_from_slice(&mac.finalize().into_bytes());
            out
        }
        Signing::Gmac => {
            // MS-SMB2 3.1.4.1: the message's number, then the role bit
            // and the CANCEL bit.
            let mut nonce = [0u8; 12];
            nonce[..8].copy_from_slice(&message_id.to_le_bytes());
            nonce[8] = u8::from(server) | (u8::from(cancel) << 1);
            let cipher = Aes128Gcm::new(key.into());
            let tag = cipher
                .encrypt(&Nonce::from(nonce), Payload { msg: &[], aad: message })
                .expect("GMAC of any message");
            let mut out = [0u8; 16];
            out.copy_from_slice(&tag);
            out
        }
    }
}

/// Signs `message`, a response, in place: sets the flag, then the
/// signature over the whole of it.
pub fn sign(message: &mut [u8], key: &[u8; 16], signing: Signing) {
    let flags = u32::from_le_bytes([message[16], message[17], message[18], message[19]]);
    message[16..20].copy_from_slice(&(flags | wire::flags::SIGNED).to_le_bytes());
    message[48..64].fill(0);
    let id = u64::from_le_bytes(message[24..32].try_into().expect("8 bytes"));
    let command = u16::from_le_bytes([message[12], message[13]]);
    let cancel = command == wire::command::CANCEL;
    let made = signature(message, key, signing, id, cancel, true);
    message[48..64].copy_from_slice(&made);
}

/// Whether `message`, a client's request, bears a right signature: the
/// tests', as smbd takes only encrypted requests.
#[cfg(test)]
pub fn verify(message: &[u8], key: &[u8; 16], signing: Signing) -> bool {
    if message.len() < wire::HEADER {
        return false;
    }
    let mut copy = message.to_vec();
    copy[48..64].fill(0);
    let id = u64::from_le_bytes(message[24..32].try_into().expect("8 bytes"));
    let cancel = u16::from_le_bytes([message[12], message[13]]) == wire::command::CANCEL;
    let made = signature(&copy, key, signing, id, cancel, false);
    accounts::ntlm::constant_eq(&made, &message[48..64])
}

/// A session's keys (MS-SMB2 3.3.5.5.3).
#[derive(Clone)]
pub struct Keys {
    /// Signs and checks messages.
    pub signing: [u8; 16],
    /// Decrypts what the client sends: its ServerIn key.
    pub decrypt: Vec<u8>,
    /// Encrypts what the server sends: its ServerOut key.
    pub encrypt: Vec<u8>,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Keys { .. }")
    }
}

impl Drop for Keys {
    fn drop(&mut self) {
        self.signing.fill(0);
        self.decrypt.fill(0);
        self.encrypt.fill(0);
    }
}

/// SMB 3.1.1's preauthentication hash after `message`, from `previous`
/// (MS-SMB2 3.3.5.4): SHA-512 of the two.
pub fn preauth(previous: &[u8; 64], message: &[u8]) -> [u8; 64] {
    use sha2::{Digest, Sha512};
    let mut hasher = Sha512::new();
    hasher.update(previous);
    hasher.update(message);
    let mut out = [0u8; 64];
    out.copy_from_slice(&hasher.finalize());
    out
}

/// The keys of a session whose sign-in gave `session_key`, in `dialect`,
/// with `preauth` the hash of SMB 3.1.1's sign-in and `cipher` its
/// cipher.
pub fn keys(session_key: &[u8; 16], dialect: u16, preauth: &[u8; 64], cipher: Cipher) -> Keys {
    let long = matches!(cipher, Cipher::Aes256Ccm | Cipher::Aes256Gcm);
    let bits = if long { 256 } else { 128 };
    let (signing, decrypt, encrypt) = if dialect == dialect::SMB_3_1_1 {
        (
            sp800_108_kdf(session_key, b"SMBSigningKey\0", preauth, 128),
            sp800_108_kdf(session_key, b"SMBC2SCipherKey\0", preauth, bits),
            sp800_108_kdf(session_key, b"SMBS2CCipherKey\0", preauth, bits),
        )
    } else {
        (
            sp800_108_kdf(session_key, b"SMB2AESCMAC\0", b"SmbSign\0", 128),
            sp800_108_kdf(session_key, b"SMB2AESCCM\0", b"ServerIn \0", 128),
            sp800_108_kdf(session_key, b"SMB2AESCCM\0", b"ServerOut\0", 128),
        )
    };
    let mut key = [0u8; 16];
    key.copy_from_slice(&signing);
    Keys { signing: key, decrypt, encrypt }
}

/// What encrypts a session's messages: its cipher, its key, and the
/// count its nonces come from, never used twice with the key.
pub struct Sealer {
    /// The cipher.
    pub cipher: Cipher,
    /// The next nonce's count.
    pub count: u64,
}

impl Sealer {
    /// Encrypts `message`, one or a compound of several, for session
    /// `session_id`: a TRANSFORM_HEADER and the ciphertext.
    pub fn seal(&mut self, keys: &Keys, session_id: u64, message: &[u8]) -> Vec<u8> {
        let mut nonce = [0u8; 16];
        nonce[..8].copy_from_slice(&self.count.to_le_bytes());
        self.count += 1;
        let (mut header, body) =
            encryption::encrypt_message(message, &keys.encrypt, self.cipher, &nonce, session_id)
                .expect("a key of the cipher's length");
        header.extend_from_slice(&body);
        header
    }
}

/// The session a TRANSFORM_HEADER names.
pub fn sealed_session(frame: &[u8]) -> Result<u64, Status> {
    if frame.len() < 52 || frame[..4] != wire::PROTOCOL_ENCRYPTED {
        return Err(status::INVALID_PARAMETER);
    }
    wire::u64_at(frame, 44)
}

/// Decrypts `frame`, which starts with a TRANSFORM_HEADER: the message,
/// or messages, it holds. An error for one that does not decrypt with
/// the session's key, or whose size is not the one its header says.
pub fn unseal(frame: &[u8], keys: &Keys, cipher: Cipher) -> Result<Vec<u8>, Status> {
    sealed_session(frame)?;
    let size = wire::u32_at(frame, 36)? as usize;
    let flags = wire::u16_at(frame, 42)?;
    if flags != 1 || size != frame.len() - 52 {
        return Err(status::INVALID_PARAMETER);
    }
    encryption::decrypt_message(&frame[..52], &frame[52..], &keys.decrypt, cipher)
        .map_err(|_| status::ACCESS_DENIED)
}

#[cfg(test)]
mod tests {
    use super::*;
    use smb2::crypto::signing::{self, SigningAlgorithm};

    fn request(id: u64) -> Vec<u8> {
        let header = wire::Header {
            command: wire::command::READ,
            message_id: id,
            session_id: 7,
            ..Default::default()
        };
        wire::message(&header, &[0x31; 49])
    }

    #[test]
    fn a_clients_signature_is_checked_as_smb2_makes_it() {
        let key = [9u8; 16];
        for (ours, theirs) in
            [(Signing::Cmac, SigningAlgorithm::AesCmac), (Signing::Gmac, SigningAlgorithm::AesGmac)]
        {
            let mut message = request(5);
            signing::sign_message(&mut message, &key, theirs, 5, false).unwrap();
            assert!(verify(&message, &key, ours));
            message[70] ^= 1;
            assert!(!verify(&message, &key, ours));
        }
    }

    #[test]
    fn a_servers_signature_is_checked_by_smb2() {
        let key = [4u8; 16];
        for (ours, theirs) in
            [(Signing::Cmac, SigningAlgorithm::AesCmac), (Signing::Gmac, SigningAlgorithm::AesGmac)]
        {
            let mut message = request(11);
            sign(&mut message, &key, ours);
            signing::verify_signature(&message, &key, theirs, 11, false).unwrap();
            // A GMAC signature of the client's role is not the server's.
            if ours == Signing::Gmac {
                assert!(!verify(&message, &key, ours));
            }
        }
    }

    #[test]
    fn what_is_sealed_unseals_with_the_clients_keys() {
        let session = [0x42u8; 16];
        let preauth = [7u8; 64];
        for cipher in [Cipher::Aes128Ccm, Cipher::Aes128Gcm, Cipher::Aes256Ccm, Cipher::Aes256Gcm] {
            let server = keys(&session, dialect::SMB_3_1_1, &preauth, cipher);
            // The client's keys are the other way around.
            let client = Keys {
                signing: server.signing,
                decrypt: server.encrypt.clone(),
                encrypt: server.decrypt.clone(),
            };
            let mut sealer = Sealer { cipher, count: 0 };
            let sealed = sealer.seal(&server, 99, b"a message");
            assert_eq!(sealed_session(&sealed), Ok(99));
            assert_eq!(unseal(&sealed, &client, cipher).as_deref(), Ok(&b"a message"[..]));
            assert!(unseal(&sealed, &server, cipher).is_err());
            let mut changed = sealed.clone();
            *changed.last_mut().unwrap() ^= 1;
            assert_eq!(unseal(&changed, &client, cipher), Err(status::ACCESS_DENIED));
        }
    }

    #[test]
    fn keys_are_smb2s() {
        // The client's derivation, for the same session: its encryption
        // key is the one the server decrypts with.
        let session = [0x11u8; 16];
        let preauth = [0x22u8; 64];
        let ours = keys(&session, dialect::SMB_3_1_1, &preauth, Cipher::Aes128Gcm);
        let theirs = smb2::crypto::kdf::derive_session_keys(
            &session,
            smb2::types::Dialect::Smb3_1_1,
            Some(&preauth),
            128,
        );
        assert_eq!(ours.signing.to_vec(), theirs.signing_key);
        assert_eq!(ours.decrypt, theirs.encryption_key);
        assert_eq!(ours.encrypt, theirs.decryption_key);
        let ours = keys(&session, dialect::SMB_3_0, &preauth, Cipher::Aes128Ccm);
        let theirs = smb2::crypto::kdf::derive_session_keys(
            &session,
            smb2::types::Dialect::Smb3_0,
            None,
            128,
        );
        assert_eq!(ours.decrypt, theirs.encryption_key);
        assert_eq!(ours.encrypt, theirs.decryption_key);
    }
}
