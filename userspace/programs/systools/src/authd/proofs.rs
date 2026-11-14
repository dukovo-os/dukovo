// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The proofs a person gives the web console beside a password (ADR
//! 0514): codes from an authenticator app (RFC 6238), recovery codes,
//! and passkeys (WebAuthn Level 3, section 7).
//!
//! Nothing here reads a file, a clock or the random source: authd passes
//! in the time, the secrets and the bytes the browser sent, and keeps
//! what a check returns. So every rule is a unit test on the host.

pub mod cbor;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::{digest, hmac, signature};
use serde::Deserialize;

use cbor::Item;

/// The seconds of each step of an app's codes (RFC 6238's default).
pub const STEP_SECONDS: u64 = 30;

/// The digits of an app's code.
pub const DIGITS: usize = 6;

/// The bytes of an app's secret: the HMAC-SHA-1 key length RFC 4226
/// recommends.
pub const SECRET_BYTES: usize = 20;

/// The app's code for `step`, RFC 4226's HOTP of the step with
/// HMAC-SHA-1 and dynamic truncation, as six digits.
pub fn app_code(secret: &[u8], step: u64) -> u32 {
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let tag = hmac::sign(&key, &step.to_be_bytes());
    let mac = tag.as_ref();
    let offset = usize::from(mac[mac.len() - 1] & 0x0f);
    let word = u32::from_be_bytes([mac[offset], mac[offset + 1], mac[offset + 2], mac[offset + 3]]);
    (word & 0x7fff_ffff) % 1_000_000
}

/// Why an app's code is not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeRefusal {
    /// It is not six digits.
    NotACode,
    /// It is not the code of the step before now, now or the step after.
    Wrong,
    /// It is the code of a step that a code was taken for already.
    Used,
}

/// Checks `typed`, an app's code, at `now` (Unix seconds). The steps
/// before and after now are taken too, for a clock that is a little
/// off; a step at or before `last`, the last one a code was taken for,
/// is not, so a code is taken once even within its 90 seconds. Returns
/// the step taken, for authd to keep as the new `last`.
pub fn check_app_code(
    secret: &[u8],
    typed: &str,
    now: u64,
    last: Option<u64>,
) -> Result<u64, CodeRefusal> {
    let typed = typed.trim();
    if typed.len() != DIGITS || !typed.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(CodeRefusal::NotACode);
    }
    let step = now / STEP_SECONDS;
    let mut matched = None;
    // Every step is tried, so the time taken says nothing of which one
    // matched.
    for candidate in [step.saturating_sub(1), step, step.saturating_add(1)] {
        let code = format!("{:06}", app_code(secret, candidate));
        if same(code.as_bytes(), typed.as_bytes()) && matched.is_none() {
            matched = Some(candidate);
        }
    }
    match (matched, last) {
        (None, _) => Err(CodeRefusal::Wrong),
        (Some(taken), Some(last)) if taken <= last => Err(CodeRefusal::Used),
        (Some(taken), _) => Ok(taken),
    }
}

/// Whether `a` and `b` are the same, in a time that depends on their
/// lengths alone.
pub fn same(a: &[u8], b: &[u8]) -> bool {
    let differ = a.iter().zip(b).fold(0u8, |differ, (x, y)| differ | (x ^ y));
    a.len() == b.len() && std::hint::black_box(differ) == 0
}

/// RFC 4648's base32 alphabet, as apps read secrets.
const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// `bytes` in base32 without padding, as `otpauth://` URIs carry
/// secrets.
pub fn base32(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            text.push(char::from(BASE32[((buffer >> bits) & 0x1f) as usize]));
        }
    }
    if bits > 0 {
        text.push(char::from(BASE32[((buffer << (5 - bits)) & 0x1f) as usize]));
    }
    text
}

/// The bytes of base32 `text`, upper or lower case, without padding;
/// `None` for any other character, or bits left over that are not 0.
pub fn from_base32(text: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(text.len() * 5 / 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for character in text.bytes() {
        let value = BASE32.iter().position(|&c| c == character.to_ascii_uppercase())?;
        buffer = (buffer << 5) | value as u32;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            bytes.push((buffer >> bits) as u8);
        }
    }
    (buffer & ((1 << bits) - 1) == 0).then_some(bytes)
}

/// Percent-encodes `text` for a URI's path or query, keeping RFC 3986's
/// unreserved characters.
fn percent(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// The `otpauth://` URI an app adds an account from, for `name` on the
/// machine `host`, as Google Authenticator's key URI format writes it.
pub fn app_uri(host: &str, name: &str, secret: &[u8]) -> String {
    let issuer = percent(&format!("Dukovo ({host})"));
    format!(
        "otpauth://totp/{issuer}:{}?secret={}&issuer={issuer}&algorithm=SHA1&digits=6&period=30",
        percent(name),
        base32(secret)
    )
}

/// The characters of a recovery code, without its dashes.
pub const RECOVERY_LENGTH: usize = 16;

/// A recovery code from 10 random bytes: their 16 base32 characters in
/// groups of four, `ABCD-EFGH-IJKL-MNOP`.
pub fn recovery_code(random: &[u8; 10]) -> String {
    let plain = base32(random);
    let groups: Vec<&str> = (0..4).map(|i| &plain[i * 4..i * 4 + 4]).collect();
    groups.join("-")
}

/// What authd keeps of a recovery code, and compares a typed one by:
/// SHA-256 of its 16 characters in upper case. A typed code may have
/// spaces and dashes anywhere, and be in lower case. `None` when it is
/// not 16 base32 characters. 80 random bits need no slow hash.
pub fn recovery_hash(typed: &str) -> Option<[u8; 32]> {
    let plain: String =
        typed.chars().filter(|c| !matches!(c, ' ' | '-')).map(|c| c.to_ascii_uppercase()).collect();
    if plain.len() != RECOVERY_LENGTH || !plain.bytes().all(|b| BASE32.contains(&b)) {
        return None;
    }
    let hash = digest::digest(&digest::SHA256, plain.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(hash.as_ref());
    Some(out)
}

/// Which of WebAuthn's two ceremonies the browser ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ceremony {
    /// `navigator.credentials.create`: a passkey added.
    Create,
    /// `navigator.credentials.get`: a passkey used.
    Get,
}

impl Ceremony {
    fn client_type(self) -> &'static str {
        match self {
            Ceremony::Create => "webauthn.create",
            Ceremony::Get => "webauthn.get",
        }
    }
}

/// What a ceremony has to have been, from what authd gave out and what
/// admind saw: the challenge, the page's origin, the relying party, and
/// whether the authenticator must have verified the person (a PIN, a
/// fingerprint) rather than only seen them touch it.
#[derive(Clone, Copy, Debug)]
pub struct Expected<'a> {
    /// The ceremony.
    pub ceremony: Ceremony,
    /// The challenge authd gave out for it.
    pub challenge: &'a [u8],
    /// The console's origin, as `https://duke-nas.local:9090`.
    pub origin: &'a str,
    /// The relying party's ID: the origin's host or a name it ends in.
    pub relying_party: &'a str,
    /// Whether the person must have been verified.
    pub verified: bool,
}

/// The client data the browser wrote and signed over (WebAuthn
/// `CollectedClientData`): what was asked, by whom.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientData {
    #[serde(rename = "type")]
    kind: String,
    challenge: String,
    origin: String,
    cross_origin: Option<bool>,
    top_origin: Option<String>,
}

/// The longest client data taken: a browser's is a few hundred bytes.
pub const CLIENT_DATA_MAX: usize = 4096;

/// The longest credential ID (WebAuthn's limit).
pub const CREDENTIAL_ID_MAX: usize = 1023;

/// Whether `host`, the origin's, may use `relying_party`: the same name,
/// or one ending in `.` and it (WebAuthn's registrable-suffix rule,
/// which authd narrows to the machine's own names before this).
fn host_may_use(host: &str, relying_party: &str) -> bool {
    !relying_party.is_empty()
        && (host == relying_party
            || host.strip_suffix(relying_party).is_some_and(|rest| rest.ends_with('.')))
}

/// The host of an `https://` origin: what is between the scheme and the
/// port. An origin with a path, a user or another scheme is no origin.
fn origin_host(origin: &str) -> Option<&str> {
    let rest = origin.strip_prefix("https://")?;
    if rest.contains(['/', '@', '?', '#']) {
        return None;
    }
    let host = match rest.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => host,
        Some(_) => return None,
        None => rest,
    };
    (!host.is_empty()).then_some(host)
}

/// Checks the client data (WebAuthn §7.1 steps 5 to 12, §7.2 steps 10 to
/// 15) and returns its SHA-256, which the signature covers.
fn check_client_data(expected: &Expected<'_>, client_data: &[u8]) -> Result<[u8; 32], String> {
    if client_data.len() > CLIENT_DATA_MAX {
        return Err("the browser's client data is longer than 4 KiB".into());
    }
    let data: ClientData = serde_json::from_slice(client_data)
        .map_err(|err| format!("the browser's client data does not read: {err}"))?;
    if data.kind != expected.ceremony.client_type() {
        return Err(format!(
            "the browser's client data is for {}, not {}",
            data.kind,
            expected.ceremony.client_type()
        ));
    }
    let challenge = URL_SAFE_NO_PAD
        .decode(data.challenge.as_bytes())
        .map_err(|_| String::from("the browser's challenge is not base64url"))?;
    if challenge != expected.challenge {
        return Err("the passkey answered another challenge than the one given".into());
    }
    if data.origin != expected.origin {
        return Err(format!("the passkey was used on {}, not on {}", data.origin, expected.origin));
    }
    if data.cross_origin == Some(true) || data.top_origin.is_some() {
        return Err("the passkey was used from a page inside another site's page".into());
    }
    let host = origin_host(expected.origin)
        .ok_or_else(|| format!("{} is not an https origin", expected.origin))?;
    if !host_may_use(host, expected.relying_party) {
        return Err(format!("{host} cannot use passkeys for {}", expected.relying_party));
    }
    let hash = digest::digest(&digest::SHA256, client_data);
    let mut out = [0u8; 32];
    out.copy_from_slice(hash.as_ref());
    Ok(out)
}

/// The flags of authenticator data (WebAuthn §6.1).
const USER_PRESENT: u8 = 0x01;
const USER_VERIFIED: u8 = 0x04;
const ATTESTED: u8 = 0x40;
const EXTENSIONS: u8 = 0x80;

/// Authenticator data, read: its flags, its count, and on a passkey's
/// creation the credential's ID and key.
struct AuthenticatorData<'a> {
    flags: u8,
    count: u32,
    credential: Option<(&'a [u8], &'a [u8])>,
}

/// Reads authenticator data and checks what both ceremonies check of it
/// (WebAuthn §7.1 steps 13 to 16, §7.2 steps 15 to 18): the relying
/// party's hash, the person's presence, and their verification when it
/// was asked for.
fn read_authenticator_data<'a>(
    expected: &Expected<'_>,
    data: &'a [u8],
) -> Result<AuthenticatorData<'a>, String> {
    let short = || String::from("the passkey's authenticator data is cut short");
    if data.len() < 37 {
        return Err(short());
    }
    let relying_party = digest::digest(&digest::SHA256, expected.relying_party.as_bytes());
    if data[..32] != *relying_party.as_ref() {
        return Err(format!("the passkey is not one for {}", expected.relying_party));
    }
    let flags = data[32];
    if flags & USER_PRESENT == 0 {
        return Err("the passkey says no one touched it".into());
    }
    if expected.verified && flags & USER_VERIFIED == 0 {
        return Err("the passkey did not check that it is you: it needs its PIN, \
                    fingerprint or face"
            .into());
    }
    let count = u32::from_be_bytes([data[33], data[34], data[35], data[36]]);
    let mut rest = &data[37..];
    let attested = flags & ATTESTED != 0;
    let credential = match (expected.ceremony, attested) {
        (Ceremony::Create, false) => {
            return Err("the new passkey's authenticator data holds no key".into());
        }
        (Ceremony::Get, true) => {
            return Err("the passkey's authenticator data holds a key where none belongs".into());
        }
        (Ceremony::Get, false) => None,
        (Ceremony::Create, true) => {
            // AAGUID, then the ID's length and the ID, then its key.
            if rest.len() < 18 {
                return Err(short());
            }
            let length = usize::from(u16::from_be_bytes([rest[16], rest[17]]));
            if length == 0 || length > CREDENTIAL_ID_MAX {
                return Err(format!("the new passkey's ID is {length} bytes long"));
            }
            rest = &rest[18..];
            if rest.len() < length {
                return Err(short());
            }
            let (id, after) = rest.split_at(length);
            let (_, used) = cbor::decode(after)
                .map_err(|err| format!("the new passkey's key does not read: {}", err.words()))?;
            let (key, after) = after.split_at(used);
            rest = after;
            Some((id, key))
        }
    };
    if flags & EXTENSIONS != 0 {
        let (_, used) = cbor::decode(rest)
            .map_err(|err| format!("the passkey's extensions do not read: {}", err.words()))?;
        rest = &rest[used..];
    }
    if !rest.is_empty() {
        return Err("the passkey's authenticator data has bytes after its end".into());
    }
    Ok(AuthenticatorData { flags, count, credential })
}

/// The kinds of key Dukovo checks passkeys' signatures with, by their
/// COSE algorithm numbers.
pub const ALGORITHMS: [i64; 3] = [ES256, EDDSA, RS256];

/// ECDSA with P-256 and SHA-256.
pub const ES256: i64 = -7;
/// Ed25519.
pub const EDDSA: i64 = -8;
/// RSASSA-PKCS1-v1_5 with SHA-256.
pub const RS256: i64 = -257;

/// A passkey's public key, from its COSE_Key (RFC 9053).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Key {
    /// A P-256 point, as `04 ‖ x ‖ y`.
    P256(Vec<u8>),
    /// An Ed25519 key.
    Ed25519(Vec<u8>),
    /// An RSA key's modulus and exponent, without leading zeros.
    Rsa { n: Vec<u8>, e: Vec<u8> },
}

/// The value at integer label `label` of a COSE_Key's pairs, refusing a
/// label written twice.
fn label<'b, 'a>(
    pairs: &'b [(Item<'a>, Item<'a>)],
    label: i64,
) -> Result<Option<&'b Item<'a>>, String> {
    let mut found = None;
    for (key, value) in pairs {
        if key.integer() == Some(label) {
            if found.is_some() {
                return Err(format!("the passkey's key names label {label} twice"));
            }
            found = Some(value);
        }
    }
    Ok(found)
}

/// The bytes at `at`, which must be there.
fn bytes_at<'a>(pairs: &[(Item<'a>, Item<'a>)], at: i64, what: &str) -> Result<&'a [u8], String> {
    match label(pairs, at)? {
        Some(Item::Bytes(bytes)) => Ok(bytes),
        _ => Err(format!("the passkey's key has no {what}")),
    }
}

/// The integer at `at`, which must be there.
fn integer_at(pairs: &[(Item<'_>, Item<'_>)], at: i64, what: &str) -> Result<i64, String> {
    label(pairs, at)?
        .and_then(Item::integer)
        .ok_or_else(|| format!("the passkey's key has no {what}"))
}

/// Reads a COSE_Key: one of the three kinds in [`ALGORITHMS`], with the
/// curve and sizes that kind has, and returns its algorithm and key.
fn read_key(cose: &[u8]) -> Result<(i64, Key), String> {
    let item = cbor::decode_whole(cose)
        .map_err(|err| format!("the passkey's key does not read: {}", err.words()))?;
    let Item::Map(pairs) = item else {
        return Err("the passkey's key is not a COSE key".into());
    };
    for (key, _) in &pairs {
        if key.integer().is_none() && !matches!(key, Item::Text(_)) {
            return Err("the passkey's key has a label that is neither a number nor text".into());
        }
    }
    let kind = integer_at(&pairs, 1, "kind (kty)")?;
    let algorithm = integer_at(&pairs, 3, "algorithm (alg)")?;
    let refused = || {
        format!(
            "this passkey uses a kind of key Dukovo does not check (COSE kty {kind}, alg \
             {algorithm}): Dukovo checks ES256, EdDSA and RS256 keys"
        )
    };
    let key = match (kind, algorithm) {
        (2, ES256) => {
            if integer_at(&pairs, -1, "curve")? != 1 {
                return Err(refused());
            }
            let (x, y) = (bytes_at(&pairs, -2, "x")?, bytes_at(&pairs, -3, "y")?);
            if x.len() != 32 || y.len() != 32 {
                return Err("the passkey's P-256 key is not 32 bytes in each coordinate".into());
            }
            let mut point = Vec::with_capacity(65);
            point.push(0x04);
            point.extend_from_slice(x);
            point.extend_from_slice(y);
            Key::P256(point)
        }
        (1, EDDSA) => {
            if integer_at(&pairs, -1, "curve")? != 6 {
                return Err(refused());
            }
            let x = bytes_at(&pairs, -2, "x")?;
            if x.len() != 32 {
                return Err("the passkey's Ed25519 key is not 32 bytes".into());
            }
            Key::Ed25519(x.to_vec())
        }
        (3, RS256) => {
            let strip = |bytes: &[u8]| {
                let first = bytes.iter().position(|&b| b != 0).unwrap_or(bytes.len());
                bytes[first..].to_vec()
            };
            let n = strip(bytes_at(&pairs, -1, "modulus")?);
            let e = strip(bytes_at(&pairs, -2, "exponent")?);
            if !(256..=512).contains(&n.len()) {
                return Err(format!(
                    "the passkey's RSA key is {} bits: Dukovo takes 2048 to 4096",
                    n.len() * 8
                ));
            }
            if e.is_empty() || e.len() > 3 {
                return Err("the passkey's RSA exponent is not one RSA keys use".into());
            }
            Key::Rsa { n, e }
        }
        _ => return Err(refused()),
    };
    Ok((algorithm, key))
}

/// Whether `signature` is `key`'s over `message`.
fn signed_by(key: &Key, message: &[u8], signature_bytes: &[u8]) -> bool {
    match key {
        Key::P256(point) => {
            signature::UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_ASN1, point)
                .verify(message, signature_bytes)
                .is_ok()
        }
        Key::Ed25519(x) => signature::UnparsedPublicKey::new(&signature::ED25519, x)
            .verify(message, signature_bytes)
            .is_ok(),
        Key::Rsa { n, e } => signature::RsaPublicKeyComponents { n, e }
            .verify(&signature::RSA_PKCS1_2048_8192_SHA256, message, signature_bytes)
            .is_ok(),
    }
}

/// A passkey being added, once its registration is checked: what authd
/// keeps of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewPasskey {
    /// Its credential ID.
    pub id: Vec<u8>,
    /// Its COSE_Key, as the authenticator wrote it.
    pub key: Vec<u8>,
    /// Its COSE algorithm, one of [`ALGORITHMS`].
    pub algorithm: i64,
    /// Its signature count.
    pub count: u32,
}

/// Checks a passkey's registration (WebAuthn §7.1): the client data,
/// then the attestation object's authenticator data and the key in it.
/// The attestation statement is not checked: Dukovo asks for none, and
/// takes the key of a person who is signed in and proved it again
/// (ADR 0514).
pub fn check_registration(
    expected: &Expected<'_>,
    client_data: &[u8],
    attestation: &[u8],
) -> Result<NewPasskey, String> {
    if expected.ceremony != Ceremony::Create {
        return Err("a passkey is added by a create ceremony".into());
    }
    check_client_data(expected, client_data)?;
    let item = cbor::decode_whole(attestation)
        .map_err(|err| format!("the new passkey's attestation does not read: {}", err.words()))?;
    let Item::Map(pairs) = item else {
        return Err("the new passkey's attestation is not a map".into());
    };
    let mut authenticator_data = None;
    let mut seen = [false; 3];
    for (key, value) in &pairs {
        let slot = match key {
            Item::Text("fmt") => 0,
            Item::Text("attStmt") => 1,
            Item::Text("authData") => 2,
            _ => return Err("the new passkey's attestation has a field WebAuthn does not".into()),
        };
        if std::mem::replace(&mut seen[slot], true) {
            return Err("the new passkey's attestation has a field twice".into());
        }
        match (slot, value) {
            (0, Item::Text(_)) | (1, Item::Map(_)) => {}
            (2, Item::Bytes(bytes)) => authenticator_data = Some(*bytes),
            _ => return Err("the new passkey's attestation has a field of the wrong kind".into()),
        }
    }
    if seen != [true; 3] {
        return Err("the new passkey's attestation lacks a field".into());
    }
    let data = read_authenticator_data(expected, authenticator_data.unwrap_or_default())?;
    let (id, key) = data.credential.ok_or_else(|| String::from("the new passkey holds no key"))?;
    let (algorithm, _) = read_key(key)?;
    Ok(NewPasskey { id: id.to_vec(), key: key.to_vec(), algorithm, count: data.count })
}

/// A passkey's use, once checked: its new count, for authd to keep, and
/// whether the person was verified.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Used {
    /// The signature count the authenticator gave.
    pub count: u32,
    /// Whether it verified the person.
    pub verified: bool,
}

/// Checks a passkey's use (WebAuthn §7.2) against `key`, the COSE_Key
/// kept when it was added, and `count`, the count kept since: the
/// client data, the authenticator data, the signature over the
/// authenticator data and the client data's hash, and the count, which
/// must rise unless both are 0 (authenticators that keep no count, as
/// synced passkeys, give 0 each time). A count that does not rise may be
/// a copied key: the use is refused, and authd keeps the old count.
pub fn check_assertion(
    expected: &Expected<'_>,
    client_data: &[u8],
    authenticator_data: &[u8],
    signature_bytes: &[u8],
    key: &[u8],
    count: u32,
) -> Result<Used, String> {
    if expected.ceremony != Ceremony::Get {
        return Err("a passkey is used by a get ceremony".into());
    }
    let client_hash = check_client_data(expected, client_data)?;
    let data = read_authenticator_data(expected, authenticator_data)?;
    let (_, key) = read_key(key)?;
    let mut message = Vec::with_capacity(authenticator_data.len() + 32);
    message.extend_from_slice(authenticator_data);
    message.extend_from_slice(&client_hash);
    if !signed_by(&key, &message, signature_bytes) {
        return Err("the passkey's signature is not its key's".into());
    }
    if (data.count != 0 || count != 0) && data.count <= count {
        return Err(format!(
            "the passkey's count went from {count} to {}: it may have been copied, so it is \
             not taken",
            data.count
        ));
    }
    Ok(Used { count: data.count, verified: data.flags & USER_VERIFIED != 0 })
}

#[cfg(test)]
pub(super) mod tests;
