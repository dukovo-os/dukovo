// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Who wrote a backup (ADR 0464). age encrypts a file to a public key, so
//! anyone who has the key's public half, which a destination's
//! `backup.toml` names, can write a file that decrypts. Each file is
//! therefore tagged: an HMAC-SHA256 of its path from the machine's folder
//! and its bytes, with a key derived by HKDF-SHA256 from the destination's
//! own age key. Only the machine and whoever holds the recovery key can
//! make a tag, and a file moved to another place, chain or time no longer
//! matches its own.
//!
//! The tag is written beside the file as `<time>.tag`: one line of 64
//! hex digits.

use ring::{hkdf, hmac};

use age::secrecy::ExposeSecret;

/// What the tags of this format begin with, before the path.
const CONTEXT: &[u8] = b"dukovo-backup tag 1\0";

/// The HKDF salt and info that make the tag key from an age key.
const SALT: &[u8] = b"dukovo-backup";
const INFO: &[u8] = b"tag 1";

/// The ending of a tag's file.
pub const ENDING: &str = ".tag";

/// The key tags are made with.
pub struct TagKey(hmac::Key);

impl TagKey {
    /// The tag key of the age key `identity`.
    pub fn of(identity: &age::x25519::Identity) -> TagKey {
        let secret = identity.to_string();
        TagKey::from_secret(secret.expose_secret().as_bytes())
    }

    fn from_secret(secret: &[u8]) -> TagKey {
        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, SALT).extract(secret);
        // The length is HMAC-SHA256's own, which expand always gives.
        let okm = prk.expand(&[INFO], hmac::HMAC_SHA256).unwrap_or_else(|_| unreachable!());
        TagKey(okm.into())
    }

    /// Starts the tag of the file whose path from the machine's folder,
    /// without its ending, is `path`, as `users/CHAIN/TIME`.
    pub fn start(&self, path: &str) -> Tagging {
        let mut context = hmac::Context::with_key(&self.0);
        context.update(CONTEXT);
        context.update(path.as_bytes());
        context.update(b"\0");
        Tagging { context, key: self.0.clone() }
    }
}

/// A tag being made, over a file's bytes as they are written or read.
pub struct Tagging {
    context: hmac::Context,
    key: hmac::Key,
}

impl Tagging {
    pub fn update(&mut self, data: &[u8]) {
        self.context.update(data);
    }

    /// The tag, as its file holds it: 64 hex digits and a newline.
    pub fn text(self) -> String {
        let mut text = hex(self.context.sign().as_ref());
        text.push('\n');
        text
    }

    /// Whether the tag is the one in `text`, a tag's file. The two are
    /// compared through an HMAC of each, which ring compares in constant
    /// time.
    pub fn matches(self, text: &str) -> bool {
        let Some(expected) = unhex(text.trim()) else { return false };
        let made = self.context.sign();
        let expected_tag = hmac::sign(&self.key, &expected);
        hmac::verify(&self.key, made.as_ref(), expected_tag.as_ref()).is_ok()
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        text.push(char::from(DIGITS[usize::from(b >> 4)]));
        text.push(char::from(DIGITS[usize::from(b & 15)]));
    }
    text
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(key: &TagKey, path: &str, data: &[u8]) -> String {
        let mut tagging = key.start(path);
        tagging.update(data);
        tagging.text()
    }

    #[test]
    fn a_tag_matches_only_its_own_key_path_and_bytes() {
        let key = TagKey::from_secret(b"AGE-SECRET-KEY-1ONE");
        let other = TagKey::from_secret(b"AGE-SECRET-KEY-1TWO");
        let text = tag(&key, "users/2026-11-11-030000/2026-11-11-030000", b"stream");
        assert_eq!(text.len(), 65);
        assert!(text.ends_with('\n'));
        let check = |key: &TagKey, path: &str, data: &[u8]| {
            let mut tagging = key.start(path);
            tagging.update(data);
            tagging.matches(&text)
        };
        assert!(check(&key, "users/2026-11-11-030000/2026-11-11-030000", b"stream"));
        // Fed in pieces, the same.
        let mut pieces = key.start("users/2026-11-11-030000/2026-11-11-030000");
        pieces.update(b"str");
        pieces.update(b"eam");
        assert!(pieces.matches(&text));
        assert!(!check(&other, "users/2026-11-11-030000/2026-11-11-030000", b"stream"));
        assert!(!check(&key, "shared/2026-11-11-030000/2026-11-11-030000", b"stream"));
        assert!(!check(&key, "users/2026-11-11-030000/2026-11-12-030000", b"stream"));
        assert!(!check(&key, "users/2026-11-11-030000/2026-11-11-030000", b"streaM"));
        // A path cannot borrow the bytes' first byte.
        assert!(!check(&key, "users/2026-11-11-030000/2026-11-11-030000s", b"tream"));
    }

    #[test]
    fn a_tag_file_that_is_not_one_matches_nothing() {
        let key = TagKey::from_secret(b"AGE-SECRET-KEY-1ONE");
        for text in ["", "zz", &"0".repeat(63), &"g".repeat(64), &"0".repeat(66)] {
            let mut tagging = key.start("machine/2026-11-11-030000");
            tagging.update(b"record");
            assert!(!tagging.matches(text), "{text:?}");
        }
    }
}
