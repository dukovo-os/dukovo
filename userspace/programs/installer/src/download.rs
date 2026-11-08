// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Setup files from the web (ADR 0319): an answers file fetched from an
//! `https://` address, over TLS only, and checked against a SHA-256 when
//! one is given, before a word of it is read.
//!
//! Nothing signs a setup file, so TLS is what keeps it from being changed
//! on the way: an `http://` address is refused, and so is a redirect to
//! one ([`tuf::http::get_https`]). The SHA-256, which the person who wrote
//! the file gives with its address, also catches a server that serves
//! another file than they meant. A file that does not match is not used.

use std::fmt::Write as _;
use std::io::Read as _;

use ring::digest;
use tuf::http;
use tuf::url::Url;

use crate::messages::{self, Arg};

/// The largest setup file taken: an answers file is a few kilobytes, so
/// more is the wrong file.
pub const MAX_BYTES: usize = 1 << 20;

/// A setup file fetched from the web.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Downloaded {
    /// The file.
    pub bytes: Vec<u8>,
    /// Its SHA-256, in hexadecimal.
    pub sha256: String,
    /// Whether it was checked against a SHA-256 given with its address.
    pub checked: bool,
}

impl Downloaded {
    /// Where it came from, for the install log: the address, the file's
    /// SHA-256 and whether it was checked.
    #[must_use]
    pub fn source(&self, address: &str) -> String {
        let check = if self.checked { "checked" } else { "not checked: none was given" };
        format!("{address} ({} bytes, SHA-256 {}, {check})", self.bytes.len(), self.sha256)
    }
}

/// Whether `text` is an address a setup file may come from: an
/// `https://` one. Gives it parsed, or the reason it is not one, as the
/// screens say it.
///
/// # Errors
///
/// A message: not https, or not an address.
pub fn address(text: &str) -> Result<Url, String> {
    let text = text.trim();
    // A scheme is any case, as Url's parse takes it.
    let https = text.get(..8).is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"));
    if !https {
        return Err(messages::text("setup-web-not-https"));
    }
    Url::parse(text).map_err(|_| messages::text("setup-web-address"))
}

/// The SHA-256 `text` gives: none when it is empty, or 64 hexadecimal
/// digits, in either case, as `sha256sum` prints them.
///
/// # Errors
///
/// A message when `text` is something else.
pub fn sha256(text: &str) -> Result<Option<[u8; 32]>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let bad = || messages::text("setup-web-sha");
    if text.len() != 64 {
        return Err(bad());
    }
    let mut digest = [0u8; 32];
    for (byte, pair) in digest.iter_mut().zip(text.as_bytes().chunks(2)) {
        let pair = std::str::from_utf8(pair).map_err(|_| bad())?;
        *byte = u8::from_str_radix(pair, 16).map_err(|_| bad())?;
    }
    Ok(Some(digest))
}

/// Fetches the setup file at `url`, over TLS only, and checks it against
/// `expected`, if given.
///
/// # Errors
///
/// A message: the fetch failed, the file is larger than [`MAX_BYTES`], or
/// it does not match `expected`.
pub fn fetch(url: &Url, expected: Option<[u8; 32]>) -> Result<Downloaded, String> {
    let failed = |why: &dyn std::fmt::Display| {
        messages::with("setup-web-failed", &[("why", Arg::Text(&why.to_string()))])
    };
    let body = http::get_https(url).map_err(|err| failed(&err))?;
    let mut bytes = Vec::new();
    // One byte more than the most taken tells a file that is too large.
    body.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|err| failed(&err))?;
    checked(bytes, expected)
}

/// `bytes`, if no larger than [`MAX_BYTES`] and, when `expected` is
/// given, of that SHA-256.
///
/// # Errors
///
/// A message: too large, or another SHA-256.
pub fn checked(bytes: Vec<u8>, expected: Option<[u8; 32]>) -> Result<Downloaded, String> {
    if bytes.len() > MAX_BYTES {
        return Err(messages::text("setup-web-too-large"));
    }
    let digest = digest::digest(&digest::SHA256, &bytes);
    if let Some(expected) = expected
        && digest.as_ref() != expected
    {
        return Err(messages::text("setup-web-mismatch"));
    }
    Ok(Downloaded { sha256: hex(digest.as_ref()), checked: expected.is_some(), bytes })
}

/// `bytes` in hexadecimal, small letters.
fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // Writing to a String cannot fail.
        let _ = write!(text, "{byte:02x}");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SHA-256 of `{}`, as `printf '{}' | sha256sum` prints it.
    const EMPTY_OBJECT: &str = "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a";

    #[test]
    fn only_https_addresses_are_taken() {
        let url = address(" https://setup.example/web01/install.json ").unwrap();
        assert_eq!(url.host(), "setup.example");
        assert_eq!(address("HTTPS://setup.example/x").unwrap().scheme(), "https");
        assert_eq!(
            address("http://setup.example/install.json"),
            Err(messages::text("setup-web-not-https"))
        );
        assert_eq!(address("ftp://x/y"), Err(messages::text("setup-web-not-https")));
        assert_eq!(address("https://"), Err(messages::text("setup-web-address")));
    }

    #[test]
    fn a_sha256_is_none_or_64_hexadecimal_digits() {
        assert_eq!(sha256("  "), Ok(None));
        let digest = sha256(EMPTY_OBJECT).unwrap().unwrap();
        assert_eq!(hex(&digest), EMPTY_OBJECT);
        assert_eq!(sha256(&EMPTY_OBJECT.to_uppercase()), Ok(Some(digest)));
        assert!(sha256(&EMPTY_OBJECT[1..]).is_err());
        assert!(sha256(&EMPTY_OBJECT.replace('4', "g")).is_err());
        // Two-byte characters are not digits, and do not split.
        assert!(sha256(&format!("é{}", &EMPTY_OBJECT[2..])).is_err());
    }

    #[test]
    fn a_file_is_used_only_as_its_sha256_says() {
        let digest = sha256(EMPTY_OBJECT).unwrap();
        let file = checked(b"{}".to_vec(), digest).unwrap();
        assert!(file.checked);
        assert_eq!(file.sha256, EMPTY_OBJECT);
        assert_eq!(checked(b"{ }".to_vec(), digest), Err(messages::text("setup-web-mismatch")));
        let unchecked = checked(b"{ }".to_vec(), None).unwrap();
        assert!(!unchecked.checked);
        assert!(unchecked.source("https://a/b").ends_with("not checked: none was given)"));
        assert!(file.source("https://a/b").starts_with("https://a/b (2 bytes, SHA-256 4413"));
    }

    #[test]
    fn a_file_larger_than_any_setup_file_is_refused() {
        assert!(checked(vec![b' '; MAX_BYTES], None).is_ok());
        assert_eq!(
            checked(vec![b' '; MAX_BYTES + 1], None),
            Err(messages::text("setup-web-too-large"))
        );
    }
}
