// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A database role's password, and the SCRAM-SHA-256 verifier that
//! PostgreSQL keeps in its place (RFC 5802, RFC 7677).
//!
//! `database` makes the password and hands PostgreSQL only the verifier,
//! as `ALTER ROLE ... PASSWORD 'SCRAM-SHA-256$...'`, so the password
//! never reaches PostgreSQL, its log or a statement that fails: the
//! verifier is what PostgreSQL would have made of the password itself,
//! and lets a client prove it holds the password without sending it.
//!
//! The password is 24 random bytes written as 32 characters of base64url
//! without padding: ASCII, so SASLprep leaves it as it is. It lives in a
//! buffer that is overwritten with zeros when it goes.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::rand::SecureRandom;
use ring::{digest, hmac, pbkdf2};

/// The iterations of PBKDF2, as PostgreSQL makes a verifier by default
/// (`scram_iterations`).
pub const ITERATIONS: u32 = 4096;

/// The random bytes of a password: 192 bits.
const PASSWORD_BYTES: usize = 24;

/// The bytes of a verifier's salt, as PostgreSQL makes its own.
pub const SALT_BYTES: usize = 16;

/// A password made for a database's role: text of 32 characters, shown
/// once and kept nowhere, which reads as `(secret)` in a log and is
/// overwritten with zeros when it goes.
pub struct Password(Vec<u8>);

impl Password {
    /// A new password, from `random`.
    ///
    /// # Errors
    ///
    /// When the system gives no random bytes.
    pub fn new(random: &dyn SecureRandom) -> Result<Password, String> {
        let mut bytes = [0u8; PASSWORD_BYTES];
        random.fill(&mut bytes).map_err(|_| "the system gave no random bytes".to_string())?;
        let password = Password::of(&bytes);
        bytes.fill(0);
        password
    }

    /// The password written from `bytes`.
    fn of(bytes: &[u8; PASSWORD_BYTES]) -> Result<Password, String> {
        // The room is taken once, so that no copy is left behind by a
        // growing buffer: 24 bytes are 32 characters.
        let mut text = vec![0u8; 32];
        match URL_SAFE_NO_PAD.encode_slice(bytes, &mut text) {
            Ok(32) => Ok(Password(text)),
            _ => Err("the password did not come out as 32 characters".into()),
        }
    }

    /// The password's characters: what the person is shown once.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // Base64url is ASCII.
        std::str::from_utf8(&self.0).unwrap_or_default()
    }

    /// The password's bytes, for its verifier.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for Password {
    fn drop(&mut self) {
        accounts::wipe(&mut self.0);
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("(secret)")
    }
}

/// A salt for a verifier, from `random`.
///
/// # Errors
///
/// When the system gives no random bytes.
pub fn salt(random: &dyn SecureRandom) -> Result<[u8; SALT_BYTES], String> {
    let mut salt = [0u8; SALT_BYTES];
    random.fill(&mut salt).map_err(|_| "the system gave no random bytes".to_string())?;
    Ok(salt)
}

/// The two keys a verifier holds: what the server checks a client's
/// proof with, and what it signs its own answer with.
#[derive(Debug, PartialEq, Eq)]
struct Keys {
    /// The SHA-256 of the client's key.
    stored: [u8; 32],
    /// The server's key.
    server: [u8; 32],
}

/// The keys of `password` with `salt` and `iterations`: the salted
/// password is PBKDF2 with HMAC-SHA-256, the client's key and the
/// server's its HMACs of "Client Key" and "Server Key", and the stored
/// key the SHA-256 of the client's (RFC 5802 section 3).
fn keys(password: &[u8], salt: &[u8], iterations: u32) -> Keys {
    let mut salted = [0u8; 32];
    let rounds = std::num::NonZeroU32::new(iterations.max(1)).unwrap_or(std::num::NonZeroU32::MIN);
    pbkdf2::derive(pbkdf2::PBKDF2_HMAC_SHA256, rounds, salt, password, &mut salted);
    let key = hmac::Key::new(hmac::HMAC_SHA256, &salted);
    salted.fill(0);
    let client = hmac::sign(&key, b"Client Key");
    let server = hmac::sign(&key, b"Server Key");
    let mut keys = Keys { stored: [0; 32], server: [0; 32] };
    keys.stored.copy_from_slice(digest::digest(&digest::SHA256, client.as_ref()).as_ref());
    keys.server.copy_from_slice(server.as_ref());
    keys
}

/// The verifier PostgreSQL keeps for `password`:
/// `SCRAM-SHA-256$ITERATIONS:SALT$STOREDKEY:SERVERKEY`, each part in
/// base64, as PostgreSQL writes it in `pg_authid`.
#[must_use]
pub fn verifier(password: &[u8], salt: &[u8; SALT_BYTES], iterations: u32) -> String {
    let keys = keys(password, salt, iterations);
    format!(
        "SCRAM-SHA-256${iterations}:{}${}:{}",
        STANDARD.encode(salt),
        STANDARD.encode(keys.stored),
        STANDARD.encode(keys.server)
    )
}

#[cfg(test)]
mod tests {
    use ring::rand::SystemRandom;

    use super::*;

    /// RFC 7677 section 3's exchange: the user "user" with the password
    /// "pencil".
    const NONCE: &str = "rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0";
    const SALT: &str = "W22ZaJ0SNY7soEsUEjb6gQ==";
    const CLIENT_FIRST_BARE: &str = "n=user,r=rOprNGfwEbeRWgbNEkqO";
    const PROOF: &str = "dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ=";
    const SIGNATURE: &str = "6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=";

    fn rfc_salt() -> [u8; SALT_BYTES] {
        STANDARD.decode(SALT).unwrap().try_into().unwrap()
    }

    /// What a client sends as its proof and what a server signs, from the
    /// password, and from the keys a verifier holds (RFC 5802 section 3).
    fn proof_and_signature(password: &[u8], keys: &Keys) -> (String, String) {
        let server_first = format!("r={NONCE},s={SALT},i=4096");
        let without_proof = format!("c=biws,r={NONCE}");
        let message = format!("{CLIENT_FIRST_BARE},{server_first},{without_proof}");
        // The client's key comes from the password; the server has only
        // its SHA-256.
        let mut salted = [0u8; 32];
        pbkdf2::derive(
            pbkdf2::PBKDF2_HMAC_SHA256,
            std::num::NonZeroU32::new(4096).unwrap(),
            &rfc_salt(),
            password,
            &mut salted,
        );
        let client_key = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, &salted), b"Client Key");
        assert_eq!(digest::digest(&digest::SHA256, client_key.as_ref()).as_ref(), keys.stored);
        let client_signature =
            hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, &keys.stored), message.as_bytes());
        let proof: Vec<u8> = client_key
            .as_ref()
            .iter()
            .zip(client_signature.as_ref())
            .map(|(key, signature)| key ^ signature)
            .collect();
        let signature =
            hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, &keys.server), message.as_bytes());
        (STANDARD.encode(proof), STANDARD.encode(signature))
    }

    #[test]
    fn the_rfc_s_password_gives_keys_whose_proof_and_signature_are_the_rfc_s() {
        let keys = keys(b"pencil", &rfc_salt(), 4096);
        assert_eq!(proof_and_signature(b"pencil", &keys), (PROOF.into(), SIGNATURE.into()));
        // Another password gives another proof.
        let other = super::keys(b"pencil!", &rfc_salt(), 4096);
        assert_ne!(proof_and_signature(b"pencil!", &other).0, PROOF);
    }

    #[test]
    fn the_verifier_is_written_as_postgresql_writes_it() {
        let keys = keys(b"pencil", &rfc_salt(), 4096);
        let written = verifier(b"pencil", &rfc_salt(), 4096);
        assert_eq!(
            written,
            format!(
                "SCRAM-SHA-256$4096:{SALT}${}:{}",
                STANDARD.encode(keys.stored),
                STANDARD.encode(keys.server)
            )
        );
        // The parts read back to what the keys are.
        let (head, rest) = written.split_once('$').unwrap();
        let (count_salt, stored_server) = rest.split_once('$').unwrap();
        let (count, salt) = count_salt.split_once(':').unwrap();
        let (stored, server) = stored_server.split_once(':').unwrap();
        assert_eq!((head, count, salt), ("SCRAM-SHA-256", "4096", SALT));
        assert_eq!(STANDARD.decode(stored).unwrap(), keys.stored);
        assert_eq!(STANDARD.decode(server).unwrap(), keys.server);
        assert!(!written.contains("pencil"));
    }

    #[test]
    fn a_password_is_32_characters_of_base64url_from_24_random_bytes_and_never_shown() {
        let password = Password::of(&[0xfb; 24]).unwrap();
        assert_eq!(password.as_str(), URL_SAFE_NO_PAD.encode([0xfb; 24]));
        assert_eq!(password.as_str().len(), 32);
        assert!(
            password.as_str().bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        assert_eq!(password.bytes(), password.as_str().as_bytes());
        assert_eq!(format!("{password:?}"), "(secret)");
        // From the system's random bytes, two are never the same.
        let random = SystemRandom::new();
        let (one, two) = (Password::new(&random).unwrap(), Password::new(&random).unwrap());
        assert_eq!(one.as_str().len(), 32);
        assert_ne!(one.as_str(), two.as_str());
    }

    #[test]
    fn a_salt_is_16_random_bytes() {
        let random = SystemRandom::new();
        let (one, two) = (salt(&random).unwrap(), salt(&random).unwrap());
        assert_eq!(one.len(), SALT_BYTES);
        assert_ne!(one, two);
    }
}
