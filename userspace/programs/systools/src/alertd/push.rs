// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Alerts to a browser by Web Push (ADR 0566): the machine's own key, the
//! browsers' subscriptions, and each message encrypted for its browser.
//!
//! - **The key.** A VAPID key pair (RFC 8292), ECDSA on P-256, that alertd
//!   makes when it starts and finds none, kept in `vapid.pem` (0600). A
//!   browser subscribes with its public half; a new key would end every
//!   subscription, so one that is there but does not read is never
//!   replaced: push is off until it is mended.
//! - **The subscriptions.** `push.json` (0600): each browser's endpoint,
//!   its keys, a name the person gave it, who added it and when, and
//!   which alerts it is told; at most [`SUBSCRIPTIONS_MAX`]. They are
//!   secrets: whoever holds an endpoint and its keys can send to that
//!   browser, so none is ever in a log line, an error or `Debug`.
//! - **The endpoint.** https, at most [`ENDPOINT_MAX`] bytes, and a
//!   public host: an endpoint on a private, loopback or link-local
//!   address would let a page make alertd post into the local network.
//!   The rule is checked when a subscription is added and again at each
//!   send: alertd resolves the name, refuses when any address is not
//!   public (unless alerts.toml's `[push] private` lists it), and connects
//!   to an address it checked, with no second lookup. A redirect is not
//!   followed, and a push service's answer is read to [`ANSWER_MAX`]
//!   bytes at most.
//! - **A message** is one JSON object of at most [`PAYLOAD_MAX`] bytes,
//!   the first change of the gathered message and how many there are,
//!   encrypted as RFC 8291 says (`aes128gcm`, RFC 8188): a new ephemeral
//!   key and salt each time, one record. It is posted with a `TTL` of a
//!   day, `Urgency` high for a critical alert, a `Topic` made from the
//!   alert's source and key, so that a newer push of the same alert
//!   replaces one not yet delivered, and an ES256 token for the
//!   endpoint's origin. 201 or 202 is delivered; 404 or 410 means the
//!   browser ended the subscription, which alertd then removes (ALR-104);
//!   413 is sent once more with a shorter detail; anything else is a
//!   failure that is tried again (ALR-105).

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::{Mutex, OnceLock, PoisonError};

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use ring::{aead, agreement, digest, hkdf};
use serde::{Deserialize, Serialize};
use tuf::http;
use tuf::url::Url;

use super::settings::{Channel, Filter, Target};
use crate::alerts::protocol::{Browser, Change, Severity, State, Subscribe};

/// The source of the event alertd records when a browser ends its
/// subscription.
pub const SOURCE: &str = "alertd";

/// The machine's key, in alertd's state folder.
pub const KEY_FILE: &str = "vapid.pem";

/// The subscriptions, in alertd's state folder.
pub const STORE_FILE: &str = "push.json";

/// The most subscriptions kept.
pub const SUBSCRIPTIONS_MAX: usize = 32;

/// The longest an endpoint may be, in bytes.
pub const ENDPOINT_MAX: usize = 1024;

/// The longest a subscription's name may be, in characters.
pub const NAME_MAX: usize = 64;

/// The most bytes of a message before it is encrypted.
pub const PAYLOAD_MAX: usize = 3000;

/// The most characters of a detail in a message, and in one sent again
/// after a 413.
const DETAIL_MAX: usize = 600;
const DETAIL_SHORT: usize = 200;

/// The most bytes of a push service's answer that are read.
pub const ANSWER_MAX: u64 = 4096;

/// The size of the one record of a message (RFC 8188): larger than any
/// message, so that one record holds it.
const RECORD_SIZE: u32 = 4096;

/// How long a push service keeps a message for a browser that is away.
const TTL_SECONDS: &str = "86400";

/// Who to ask about this machine's pushes, as RFC 8292's `sub` says.
const SUBJECT: &str = "https://dukovo.org";

/// How long a token stays good, in seconds.
const TOKEN_SECONDS: u64 = 12 * 60 * 60;

/// The bytes of a browser's public key (an uncompressed P-256 point) and
/// of its authentication secret.
const P256DH_BYTES: usize = 65;
const AUTH_BYTES: usize = 16;

/// The machine's key: what signs each token, and the public half a
/// browser subscribes with.
pub struct Vapid {
    pair: EcdsaKeyPair,
}

impl fmt::Debug for Vapid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Vapid(..)")
    }
}

/// The key once alertd has it, for the threads that send, or why there
/// is none.
static KEY: OnceLock<Result<Vapid, String>> = OnceLock::new();

/// The IDs of the subscriptions whose browsers ended them since alertd
/// last asked, which alertd removes.
static ENDED: Mutex<Vec<String>> = Mutex::new(Vec::new());

impl Vapid {
    /// The key kept in `dir`, made and kept there when there is none.
    ///
    /// # Errors
    ///
    /// Why there is none: a file that is there and does not read is never
    /// replaced, as a new key would end every subscription.
    pub fn load_or_make(dir: &Path) -> Result<Vapid, String> {
        let path = dir.join(KEY_FILE);
        match fs::read_to_string(&path) {
            Ok(text) => Vapid::from_pem(&text).map_err(|why| {
                format!("{}: {why}; push is off until it is mended", path.display())
            }),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                let rng = SystemRandom::new();
                let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
                    .map_err(|_| "cannot make a key".to_string())?;
                let pem = pem(pkcs8.as_ref());
                write_secret(dir, KEY_FILE, pem.as_bytes())
                    .map_err(|err| format!("cannot keep {}: {err}", path.display()))?;
                Vapid::from_pem(&pem)
            }
            Err(err) => Err(format!("cannot read {}: {err}", path.display())),
        }
    }

    /// The key in a PEM `PRIVATE KEY` (PKCS #8).
    fn from_pem(text: &str) -> Result<Vapid, String> {
        let body: String = text
            .lines()
            .map(str::trim)
            .skip_while(|line| *line != "-----BEGIN PRIVATE KEY-----")
            .skip(1)
            .take_while(|line| *line != "-----END PRIVATE KEY-----")
            .collect();
        let der = STANDARD.decode(body).map_err(|_| "it is not a PEM private key".to_string())?;
        Vapid::from_pkcs8(&der)
    }

    /// The key in PKCS #8 `der`.
    fn from_pkcs8(der: &[u8]) -> Result<Vapid, String> {
        EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, der, &SystemRandom::new())
            .map(|pair| Vapid { pair })
            .map_err(|_| "it is not an ECDSA P-256 key".to_string())
    }

    /// The public half as a browser takes it (`applicationServerKey`):
    /// the uncompressed point, in base64url.
    #[must_use]
    pub fn public_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.pair.public_key().as_ref())
    }

    /// The `Authorization` header for a push to `endpoint`'s origin, good
    /// for 12 hours from `now`, seconds since 1970 (RFC 8292).
    fn authorization(&self, endpoint: &Url, now: u64) -> Result<String, String> {
        let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = serde_json::json!({
            "aud": origin(endpoint),
            "exp": now + TOKEN_SECONDS,
            "sub": SUBJECT,
        });
        let claims = URL_SAFE_NO_PAD.encode(claims.to_string());
        let signed = format!("{header}.{claims}");
        let signature = self
            .pair
            .sign(&SystemRandom::new(), signed.as_bytes())
            .map_err(|_| "cannot sign the token".to_string())?;
        let token = format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature.as_ref()));
        Ok(format!("vapid t={token}, k={}", self.public_key()))
    }
}

/// Takes the key kept in `dir`, or makes it, for the threads that send;
/// why there is none, if there is none. The first key taken stays.
pub fn start(dir: &Path) -> Option<String> {
    KEY.get_or_init(|| Vapid::load_or_make(dir)).as_ref().err().cloned()
}

/// The key, or why there is none.
///
/// # Errors
///
/// Why alertd has no key: the file does not read, or it was never
/// taken.
pub fn key() -> Result<&'static Vapid, String> {
    match KEY.get() {
        Some(Ok(key)) => Ok(key),
        Some(Err(why)) => Err(why.clone()),
        None => Err("alertd has not read its push key".into()),
    }
}

/// `der` as a PEM `PRIVATE KEY`.
fn pem(der: &[u8]) -> String {
    let body = STANDARD.encode(der);
    let mut text = String::from("-----BEGIN PRIVATE KEY-----\n");
    for line in body.as_bytes().chunks(64) {
        text.push_str(&String::from_utf8_lossy(line));
        text.push('\n');
    }
    text.push_str("-----END PRIVATE KEY-----\n");
    text
}

/// Writes `bytes` as `name` in `dir`, whole or not at all, readable by its
/// owner alone: a new file of mode 0600, made durable, then renamed over
/// the old one.
fn write_secret(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let new = dir.join(format!(".{name}.new"));
    let _ = fs::remove_file(&new);
    let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&new)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&new, dir.join(name))?;
    File::open(dir)?.sync_all()
}

/// A browser's subscription, as `push.json` keeps it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    /// 8 random bytes, in hexadecimal: what the page keeps and removes
    /// it by.
    pub id: String,
    /// What the person called it: "Firefox on a phone".
    pub name: String,
    /// Where its messages go: the browser maker's push service.
    pub endpoint: String,
    /// The browser's public key and its authentication secret, base64url.
    pub p256dh: String,
    pub auth: String,
    /// The account that added it.
    pub by: u32,
    /// When, in milliseconds since 1970.
    pub added_ms: u64,
    /// The least severe alert it is told, and whether it is told of
    /// clears.
    pub severity: Severity,
    pub cleared: bool,
    /// The kinds of alert it is told, or none for every kind.
    #[serde(default)]
    pub about: Option<Vec<String>>,
}

impl fmt::Debug for Subscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Subscription")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("by", &self.by)
            .field("severity", &self.severity)
            .field("cleared", &self.cleared)
            .finish_non_exhaustive()
    }
}

impl Subscription {
    /// The host of its endpoint: what push-list and the log name.
    #[must_use]
    pub fn host(&self) -> String {
        Url::parse(&self.endpoint).map_or_default(|url| url.host().to_string())
    }

    /// As push-list shows it: without the endpoint and the keys.
    #[must_use]
    pub fn browser(&self) -> Browser {
        Browser {
            id: self.id.clone(),
            name: self.name.clone(),
            host: self.host(),
            by: self.by,
            added_ms: self.added_ms,
            severity: self.severity,
            cleared: self.cleared,
            about: self.about.clone(),
        }
    }

    /// The channel that sends to it, an endpoint on one of `private`
    /// let through: "push to Firefox on a phone".
    #[must_use]
    pub fn channel(&self, private: &[Network]) -> Channel {
        Channel {
            name: format!("push to {}", self.name),
            filter: Filter { severity: self.severity, cleared: self.cleared },
            target: Target::Push(Box::new(Push {
                subscription: self.clone(),
                private: private.to_vec(),
            })),
        }
    }
}

/// Where a push channel sends: a browser's subscription, and the
/// networks alerts.toml lets its endpoint be on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Push {
    pub subscription: Subscription,
    pub private: Vec<Network>,
}

/// `push.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct StoreFile {
    format: u32,
    subscriptions: Vec<Subscription>,
}

/// The subscriptions kept in `dir`, and what was wrong with the file.
pub fn load(dir: &Path) -> (Vec<Subscription>, Option<String>) {
    let path = dir.join(STORE_FILE);
    match fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<StoreFile>(&text) {
            Ok(file) if file.format == 1 => (file.subscriptions, None),
            Ok(file) => {
                (Vec::new(), Some(format!("{}: format {} is not 1", path.display(), file.format)))
            }
            Err(err) => (Vec::new(), Some(format!("{}: {err}", path.display()))),
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => (Vec::new(), None),
        Err(err) => (Vec::new(), Some(format!("cannot read {}: {err}", path.display()))),
    }
}

/// Keeps `subscriptions` in `dir`, whole or not at all, mode 0600.
///
/// # Errors
///
/// If the file cannot be written.
pub fn save(dir: &Path, subscriptions: &[Subscription]) -> io::Result<()> {
    let file = StoreFile { format: 1, subscriptions: subscriptions.to_vec() };
    let text = serde_json::to_string(&file).map_err(io::Error::other)?;
    write_secret(dir, STORE_FILE, format!("{text}\n").as_bytes())
}

/// Adds the subscription `asked` at `now_ms` to `subscriptions`, or,
/// when one has its endpoint, makes that one what `asked` says and keeps
/// its ID; gives the ID. `private` is the networks alerts.toml lets an
/// endpoint be on.
///
/// # Errors
///
/// Why it is refused: a name, endpoint or key that is not right, or
/// [`SUBSCRIPTIONS_MAX`] already kept.
pub fn add(
    subscriptions: &mut Vec<Subscription>,
    asked: Subscribe,
    now_ms: u64,
    private: &[Network],
) -> Result<String, String> {
    let name = asked.name.trim();
    if name.is_empty() || name.chars().count() > NAME_MAX || name.chars().any(char::is_control) {
        return Err(format!("a name is 1 to {NAME_MAX} characters on one line"));
    }
    let url = endpoint_url(&asked.endpoint)?;
    if let Ok(address) = http::bare_host(&url).parse::<IpAddr>()
        && !allowed(address, private)
    {
        return Err(format!(
            "the endpoint is at {address}, which is not a public address; alerts.toml's [push] \
             private lets one through"
        ));
    }
    let p256dh = decode_key(&asked.p256dh, P256DH_BYTES, "p256dh")?;
    if p256dh[0] != 4 {
        return Err("p256dh is not an uncompressed P-256 key".into());
    }
    decode_key(&asked.auth, AUTH_BYTES, "auth")?;
    let about = asked.about.filter(|words| !words.is_empty());
    let made = |id: String| Subscription {
        id,
        name: name.to_string(),
        endpoint: asked.endpoint.clone(),
        p256dh: asked.p256dh.clone(),
        auth: asked.auth.clone(),
        by: asked.by,
        added_ms: now_ms,
        severity: asked.severity.unwrap_or(Severity::Warning),
        cleared: asked.cleared.unwrap_or(true),
        about: about.clone(),
    };
    if let Some(kept) = subscriptions.iter_mut().find(|kept| kept.endpoint == asked.endpoint) {
        *kept = made(kept.id.clone());
        return Ok(kept.id.clone());
    }
    if subscriptions.len() >= SUBSCRIPTIONS_MAX {
        return Err(format!(
            "{SUBSCRIPTIONS_MAX} browsers get alerts already; remove one that no longer needs to"
        ));
    }
    let mut bytes = [0u8; 8];
    SystemRandom::new().fill(&mut bytes).map_err(|_| "cannot make an ID".to_string())?;
    let id = crate::cert::inspect::hex(&bytes);
    subscriptions.push(made(id.clone()));
    Ok(id)
}

/// Takes the subscription `id` out of `subscriptions`; whether there was
/// one.
pub fn remove(subscriptions: &mut Vec<Subscription>, id: &str) -> bool {
    let before = subscriptions.len();
    subscriptions.retain(|kept| kept.id != id);
    subscriptions.len() != before
}

/// `endpoint` as an address that may be posted to: https, and not too
/// long.
fn endpoint_url(endpoint: &str) -> Result<Url, String> {
    if endpoint.len() > ENDPOINT_MAX {
        return Err(format!("the endpoint is longer than {ENDPOINT_MAX} bytes"));
    }
    let url = Url::parse(endpoint).map_err(|_| "the endpoint is not an address".to_string())?;
    if url.scheme() != "https" {
        return Err("the endpoint is not an https address".into());
    }
    Ok(url)
}

/// The `bytes` bytes of key `name` in base64url.
fn decode_key(text: &str, bytes: usize, name: &str) -> Result<Vec<u8>, String> {
    let key = URL_SAFE_NO_PAD
        .decode(text.trim_end_matches('='))
        .map_err(|_| format!("{name} is not base64url"))?;
    if key.len() == bytes {
        Ok(key)
    } else {
        Err(format!("{name} is {} bytes, not {bytes}", key.len()))
    }
}

/// A network alerts.toml's `[push] private` names: an address and the
/// bits of it that count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Network {
    address: IpAddr,
    bits: u8,
}

impl Network {
    /// `text`: an address, or an address and its prefix length
    /// (`10.0.2.2`, `192.168.0.0/16`, `fd00::/8`).
    ///
    /// # Errors
    ///
    /// If it is neither.
    pub fn parse(text: &str) -> Result<Network, String> {
        let wrong = || format!("{text} is not an address or a network such as 192.168.0.0/16");
        let (address, bits) = match text.split_once('/') {
            Some((address, bits)) => (address, Some(bits)),
            None => (text, None),
        };
        let address: IpAddr = address.parse().map_err(|_| wrong())?;
        let most = if address.is_ipv4() { 32 } else { 128 };
        let bits = match bits {
            Some(bits) => bits.parse::<u8>().ok().filter(|bits| *bits <= most).ok_or_else(wrong)?,
            None => most,
        };
        Ok(Network { address, bits })
    }

    /// Whether `address` is in it.
    fn holds(self, address: IpAddr) -> bool {
        let (ours, theirs, width) = match (self.address, address) {
            (IpAddr::V4(ours), IpAddr::V4(theirs)) => {
                (u128::from(ours.to_bits()), u128::from(theirs.to_bits()), 32)
            }
            (IpAddr::V6(ours), IpAddr::V6(theirs)) => (ours.to_bits(), theirs.to_bits(), 128),
            _ => return false,
        };
        let bits = u32::from(self.bits);
        if bits == 0 {
            return true;
        }
        let shift = width - bits;
        ours >> shift == theirs >> shift
    }
}

/// Whether an endpoint may be at `address`: a public one, or one that
/// `private` lets through.
fn allowed(address: IpAddr, private: &[Network]) -> bool {
    is_public(address) || private.iter().any(|network| network.holds(address))
}

/// Whether `address` is one on the internet: not unspecified, loopback,
/// private, shared, link-local, multicast, broadcast, reserved or kept
/// for documentation and benchmarks, and for IPv6 not unique local, nor
/// an IPv4 address inside one that is not public.
fn is_public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_v4(address),
        IpAddr::V6(address) => is_public_v6(address),
    }
}

fn is_public_v4(address: Ipv4Addr) -> bool {
    let [a, b, c, _] = address.octets();
    !(a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..128).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..32).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 192 && b == 168)
        || (a == 198 && (18..20).contains(&b))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224)
}

fn is_public_v6(address: Ipv6Addr) -> bool {
    if let Some(inside) = address.to_ipv4_mapped() {
        return is_public_v4(inside);
    }
    let segments = address.segments();
    // NAT64's well-known prefix carries an IPv4 address in its last bits.
    if segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        let bits = address.to_bits();
        #[allow(clippy::cast_possible_truncation)] // the last 32 bits
        return is_public_v4(Ipv4Addr::from_bits(bits as u32));
    }
    let first = segments[0];
    !(address.is_unspecified()
        || address.is_loopback()
        || first & 0xfe00 == 0xfc00
        || first & 0xffc0 == 0xfe80
        || first & 0xffc0 == 0xfec0
        || first & 0xff00 == 0xff00
        || (first == 0x2001 && segments[1] == 0x0db8)
        || (first == 0x0100 && segments[1..4] == [0, 0, 0])
        || segments[..6] == [0, 0, 0, 0, 0, 0])
}

/// The addresses of `url`'s host that a push may go to, resolved once.
///
/// # Errors
///
/// If the name has no address, or any of its addresses is not public and
/// not let through by `private`: a name that leads into the local network
/// at all is refused, not tried at another of its addresses.
fn addresses(url: &Url, private: &[Network]) -> Result<Vec<SocketAddr>, String> {
    let host = http::bare_host(url);
    let found: Vec<SocketAddr> = (host, http::port_of(url))
        .to_socket_addrs()
        .map_err(|err| format!("cannot find the push service {host}: {err}"))?
        .collect();
    if found.is_empty() {
        return Err(format!("the push service {host} has no address"));
    }
    if let Some(blocked) = found.iter().find(|found| !allowed(found.ip(), private)) {
        return Err(format!(
            "the push service {host} is at {}, which is not a public address; nothing was sent",
            blocked.ip()
        ));
    }
    Ok(found)
}

/// The origin of `url`, which a token is for: its scheme and host, and
/// its port when it names one.
fn origin(url: &Url) -> String {
    format!("{}://{}", url.scheme(), http::host_header(url))
}

/// The message for `changes`, from machine `host`: the first change and
/// how many there are, with the first's detail cut to `detail_max`
/// characters, and shorter still if the whole would pass
/// [`PAYLOAD_MAX`] bytes.
fn payload(changes: &[Change], host: &str, detail_max: usize) -> Result<Vec<u8>, String> {
    let first = changes.first().ok_or("a message with no change")?;
    let state = match first.state {
        State::Raised => "raised",
        State::Cleared => "cleared",
        State::Event => "event",
        State::Test => "test",
    };
    let mut most = detail_max;
    loop {
        let detail: String = first.alert.detail.chars().take(most).collect();
        let message = serde_json::json!({
            "v": 1,
            "host": host,
            "count": changes.len(),
            "first": {
                "state": state,
                "severity": first.alert.severity.name(),
                "title": first.alert.title,
                "detail": detail,
                "source": first.source,
                "key": first.alert.key,
                "at_ms": first.at.saturating_mul(1000),
            },
        });
        let bytes = message.to_string().into_bytes();
        if bytes.len() <= PAYLOAD_MAX {
            return Ok(bytes);
        }
        if most == 0 {
            return Err("the message is too long to push".into());
        }
        most /= 2;
    }
}

/// The length of a key HKDF makes.
struct Len(usize);

impl hkdf::KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

/// The `bytes` bytes HKDF-SHA-256 expands from `prk` with `info`.
fn expand<const N: usize>(prk: &hkdf::Prk, info: &[&[u8]]) -> Result<[u8; N], String> {
    let mut out = [0u8; N];
    prk.expand(info, Len(N))
        .and_then(|okm| okm.fill(&mut out))
        .map_err(|_| "cannot derive the message's key".to_string())?;
    Ok(out)
}

/// `plain` encrypted for the browser whose public key is `p256dh` and
/// authentication secret `auth` (RFC 8291), with `salt` and an ephemeral
/// key made from `rng`: the body of the push, its header and one record.
fn encrypt(
    plain: &[u8],
    p256dh: &[u8],
    auth: &[u8],
    salt: [u8; 16],
    rng: &dyn SecureRandom,
) -> Result<Vec<u8>, String> {
    let ours = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, rng)
        .map_err(|_| "cannot make the message's key".to_string())?;
    let our_public =
        ours.compute_public_key().map_err(|_| "cannot make the message's key".to_string())?;
    let theirs = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, p256dh);
    let shared = agreement::agree_ephemeral(ours, &theirs, <[u8]>::to_vec)
        .map_err(|_| "the browser's key is not a P-256 key".to_string())?;
    // The key of the browser and the message's: RFC 8291, section 3.4.
    let prk_key = hkdf::Salt::new(hkdf::HKDF_SHA256, auth).extract(&shared);
    let ikm: [u8; 32] = expand(&prk_key, &[b"WebPush: info\0", p256dh, our_public.as_ref()])?;
    // The content's key and nonce: RFC 8188, section 2.2 and 2.3.
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, &salt).extract(&ikm);
    let cek: [u8; 16] = expand(&prk, &[b"Content-Encoding: aes128gcm\0"])?;
    let nonce: [u8; 12] = expand(&prk, &[b"Content-Encoding: nonce\0"])?;
    let key = aead::UnboundKey::new(&aead::AES_128_GCM, &cek)
        .map(aead::LessSafeKey::new)
        .map_err(|_| "cannot encrypt the message".to_string())?;
    // One record, the last: the text, then the delimiter 2.
    let mut record = plain.to_vec();
    record.push(2);
    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::empty(),
        &mut record,
    )
    .map_err(|_| "cannot encrypt the message".to_string())?;
    let public = our_public.as_ref();
    let mut body = Vec::with_capacity(16 + 4 + 1 + public.len() + record.len());
    body.extend_from_slice(&salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    #[allow(clippy::cast_possible_truncation)] // 65 bytes
    body.push(public.len() as u8);
    body.extend_from_slice(public);
    body.extend_from_slice(&record);
    Ok(body)
}

/// The `Topic` of a push of `change`: the same for each push of the same
/// alert, so that a push service replaces one it has not delivered yet.
fn topic(change: &Change) -> String {
    let named = format!("{}\n{}", change.source, change.alert.key);
    let hash = digest::digest(&digest::SHA256, named.as_bytes());
    URL_SAFE_NO_PAD.encode(hash.as_ref()).chars().take(32).collect()
}

/// Pushes `changes` to `subscription`'s browser, telling it they are from
/// machine `host`, at `now`, seconds since 1970; `private` is the
/// networks alerts.toml lets an endpoint be on.
///
/// # Errors
///
/// Why it was not delivered, with the push service's words when it
/// answered (ALR-105); a subscription the browser ended (ALR-104) is
/// also noted for [`take_ended`].
pub fn send(
    subscription: &Subscription,
    changes: &[Change],
    host: &str,
    now: u64,
    private: &[Network],
) -> Result<(), String> {
    let key = key()?;
    let url = endpoint_url(&subscription.endpoint)?;
    let addresses = addresses(&url, private)?;
    let p256dh = decode_key(&subscription.p256dh, P256DH_BYTES, "p256dh")?;
    let auth = decode_key(&subscription.auth, AUTH_BYTES, "auth")?;
    let first = changes.first().ok_or("a message with no change")?;
    let urgency = if first.alert.severity == Severity::Critical { "high" } else { "normal" };
    let authorization = key.authorization(&url, now)?;
    let topic = topic(first);
    let mut detail_max = DETAIL_MAX;
    loop {
        let plain = payload(changes, host, detail_max)?;
        let rng = SystemRandom::new();
        let mut salt = [0u8; 16];
        rng.fill(&mut salt).map_err(|_| "cannot make the message's salt".to_string())?;
        let body = encrypt(&plain, &p256dh, &auth, salt, &rng)?;
        let headers = [
            ("Content-Type", "application/octet-stream"),
            ("Content-Encoding", "aes128gcm"),
            ("TTL", TTL_SECONDS),
            ("Urgency", urgency),
            ("Topic", topic.as_str()),
            ("Authorization", authorization.as_str()),
        ];
        let answer = http::exchange_to("POST", &url, &addresses, &headers, &body)
            .map_err(|err| format!("cannot reach the push service {}: {err}", url.host()))?;
        let mut said = Vec::new();
        let _ = answer.body.take(ANSWER_MAX).read_to_end(&mut said);
        match answer.status {
            201 | 202 => return Ok(()),
            404 | 410 => {
                ENDED.lock().unwrap_or_else(PoisonError::into_inner).push(subscription.id.clone());
                return Err(format!(
                    "the browser ended this subscription (the push service answered {}); it is \
                     removed (ALR-104)",
                    answer.status
                ));
            }
            413 if detail_max > DETAIL_SHORT => detail_max = DETAIL_SHORT,
            status => return Err(refusal(status, &said)),
        }
    }
}

/// The words of a push service's refusal: its status and the first of
/// what it said.
fn refusal(status: u16, said: &[u8]) -> String {
    let words: String =
        String::from_utf8_lossy(said).split_whitespace().collect::<Vec<_>>().join(" ");
    let words: String = words.chars().take(200).collect();
    let redirect =
        if (300..400).contains(&status) { " (a redirect, which is not followed)" } else { "" };
    if words.is_empty() {
        format!("the push service answered {status}{redirect} (ALR-105)")
    } else {
        format!("the push service answered {status}{redirect}: {words} (ALR-105)")
    }
}

/// The IDs of the subscriptions whose browsers ended them since the last
/// call.
pub fn take_ended() -> Vec<String> {
    std::mem::take(&mut *ENDED.lock().unwrap_or_else(PoisonError::into_inner))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alerts::protocol::Alert;

    fn b64(text: &str) -> Vec<u8> {
        URL_SAFE_NO_PAD.decode(text).unwrap()
    }

    /// RFC 8291's Appendix A: its keys, salt, secret and text give its
    /// message.
    #[test]
    #[allow(deprecated)] // ring's fixed random source, for a known key
    fn the_rfc_s_example_is_encrypted_as_the_rfc_says() {
        let plain = b64("V2hlbiBJIGdyb3cgdXAsIEkgd2FudCB0byBiZSBhIHdhdGVybWVsb24");
        let as_private = b64("yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw");
        let ua_public = b64(
            "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4",
        );
        let salt: [u8; 16] = b64("DGv6ra1nlYgDCS1FRnbzlw").try_into().unwrap();
        let auth = b64("BTBZMqHH6r4Tts7J_aSIgg");
        let rng = ring::test::rand::FixedSliceRandom { bytes: &as_private };
        let body = encrypt(&plain, &ua_public, &auth, salt, &rng).unwrap();
        assert_eq!(
            URL_SAFE_NO_PAD.encode(body),
            "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocIn\
             mYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGN\
             WQexSgSxsj_Qulcy4a-fN"
        );
    }

    #[test]
    fn a_key_is_made_once_kept_private_and_read_back_the_same() {
        let dir = std::env::temp_dir().join(format!("vapid-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let made = Vapid::load_or_make(&dir).unwrap();
        let mode = fs::metadata(dir.join(KEY_FILE)).unwrap().permissions();
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777, 0o600);
        let read = Vapid::load_or_make(&dir).unwrap();
        assert_eq!(made.public_key(), read.public_key());
        assert_eq!(b64(&made.public_key()).len(), 65);
        // A file that does not read is not replaced: a new key would end
        // every subscription.
        fs::write(dir.join(KEY_FILE), "not a key\n").unwrap();
        let refused = Vapid::load_or_make(&dir).unwrap_err();
        assert!(refused.contains("push is off until it is mended"), "{refused}");
        assert_eq!(fs::read_to_string(dir.join(KEY_FILE)).unwrap(), "not a key\n");
        fs::remove_dir_all(&dir).unwrap();
    }

    /// RFC 8292's token: ES256 over the header and claims, for the
    /// endpoint's origin, 12 hours ahead, which the public key checks.
    #[test]
    fn the_token_is_for_the_endpoint_s_origin_and_signed_by_the_key() {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let key = Vapid::from_pkcs8(pkcs8.as_ref()).unwrap();
        let url = Url::parse("https://push.example.net:8443/wpush/v2/abc?x=1").unwrap();
        let header = key.authorization(&url, 1_794_000_000).unwrap();
        let (token, public) =
            header.strip_prefix("vapid t=").and_then(|rest| rest.split_once(", k=")).unwrap();
        assert_eq!(public, key.public_key());
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
        let head: serde_json::Value = serde_json::from_slice(&b64(parts[0])).unwrap();
        assert_eq!(head, serde_json::json!({"typ": "JWT", "alg": "ES256"}));
        let claims: serde_json::Value = serde_json::from_slice(&b64(parts[1])).unwrap();
        assert_eq!(
            claims,
            serde_json::json!({
                "aud": "https://push.example.net:8443", "exp": 1_794_043_200_u64,
                "sub": "https://dukovo.org"
            })
        );
        let signed = format!("{}.{}", parts[0], parts[1]);
        let check = ring::signature::UnparsedPublicKey::new(
            &ring::signature::ECDSA_P256_SHA256_FIXED,
            b64(public),
        );
        assert!(check.verify(signed.as_bytes(), &b64(parts[2])).is_ok());
    }

    fn asked(endpoint: &str) -> Subscribe {
        Subscribe {
            name: "Firefox on a phone".into(),
            endpoint: endpoint.into(),
            p256dh: "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4"
                .into(),
            auth: "BTBZMqHH6r4Tts7J_aSIgg".into(),
            by: 1000,
            ..Subscribe::default()
        }
    }

    #[test]
    fn a_subscription_is_added_once_by_its_endpoint_and_removed_by_its_id() {
        let mut kept = Vec::new();
        let endpoint = "https://updates.push.services.mozilla.com/wpush/v2/gAAAA";
        let id = add(&mut kept, asked(endpoint), 5, &[]).unwrap();
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(
            (kept[0].severity, kept[0].cleared, kept[0].by),
            (Severity::Warning, true, 1000)
        );
        // The same browser again keeps its ID, with what it says now.
        let again = Subscribe {
            name: "Phone".into(),
            severity: Some(Severity::Critical),
            ..asked(endpoint)
        };
        assert_eq!(add(&mut kept, again, 9, &[]).unwrap(), id);
        assert_eq!(
            (kept.len(), kept[0].name.as_str(), kept[0].severity),
            (1, "Phone", Severity::Critical)
        );
        assert!(remove(&mut kept, &id));
        assert!(!remove(&mut kept, &id));
        assert!(kept.is_empty());
    }

    #[test]
    fn a_subscription_that_is_not_right_is_refused() {
        let mut kept = Vec::new();
        let refused =
            |kept: &mut Vec<Subscription>, asked: Subscribe| add(kept, asked, 0, &[]).unwrap_err();
        assert!(refused(&mut kept, asked("http://push.example.net/x")).contains("not an https"));
        let long = format!("https://push.example.net/{}", "a".repeat(ENDPOINT_MAX));
        assert!(refused(&mut kept, asked(&long)).contains("longer than 1024"));
        for private in [
            "https://192.168.1.5/x",
            "https://[fe80::1]/x",
            "https://127.0.0.1/x",
            "https://10.0.2.2:8443/x",
        ] {
            assert!(
                refused(&mut kept, asked(private)).contains("not a public address"),
                "{private}"
            );
        }
        let lab = [Network::parse("10.0.2.0/24").unwrap()];
        assert!(add(&mut kept, asked("https://10.0.2.2:8443/x"), 0, &lab).is_ok());
        let short = Subscribe { auth: "AAAA".into(), ..asked("https://push.example.net/y") };
        assert!(refused(&mut kept, short).contains("auth is 3 bytes, not 16"));
        let unnamed = Subscribe { name: " ".into(), ..asked("https://push.example.net/y") };
        assert!(refused(&mut kept, unnamed).contains("a name is 1 to 64"));
        for at in 1..SUBSCRIPTIONS_MAX {
            add(&mut kept, asked(&format!("https://push.example.net/{at}")), 0, &[]).unwrap();
        }
        assert!(refused(&mut kept, asked("https://push.example.net/more")).contains("32 browsers"));
    }

    #[test]
    fn the_store_is_kept_private_and_read_back() {
        let dir = std::env::temp_dir().join(format!("push-store-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(load(&dir), (Vec::new(), None));
        let mut kept = Vec::new();
        add(&mut kept, asked("https://fcm.googleapis.com/fcm/send/abc"), 7, &[]).unwrap();
        save(&dir, &kept).unwrap();
        let mode = fs::metadata(dir.join(STORE_FILE)).unwrap().permissions();
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777, 0o600);
        assert_eq!(load(&dir), (kept.clone(), None));
        // Debug names no endpoint or key.
        let debug = format!("{:?}", kept[0]);
        assert!(!debug.contains("fcm") && !debug.contains("BCVx"), "{debug}");
        fs::write(dir.join(STORE_FILE), "{\"format\":2,\"subscriptions\":[]}").unwrap();
        assert!(load(&dir).1.unwrap().contains("format 2 is not 1"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_a_public_address_or_one_the_settings_name_is_pushed_to() {
        let public = ["1.1.1.1", "8.8.8.8", "2606:4700::1111", "2a00:1450:4001::200e"];
        for address in public {
            assert!(is_public(address.parse().unwrap()), "{address}");
        }
        let not = [
            "0.0.0.0",
            "10.1.2.3",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.2.1",
            "192.168.0.1",
            "198.18.0.1",
            "198.51.100.7",
            "203.0.113.9",
            "224.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "ff02::1",
            "2001:db8::1",
            "::ffff:192.168.1.1",
            "64:ff9b::a00:1",
            "100::1",
            "::127.0.0.1",
        ];
        for address in not {
            assert!(!is_public(address.parse().unwrap()), "{address}");
        }
        assert!(is_public("::ffff:8.8.8.8".parse().unwrap()));
        let lab = Network::parse("10.0.2.0/24").unwrap();
        assert!(lab.holds("10.0.2.2".parse().unwrap()));
        assert!(!lab.holds("10.0.3.2".parse().unwrap()));
        assert!(!lab.holds("fd00::1".parse().unwrap()));
        assert!(Network::parse("fd00::/8").unwrap().holds("fd12::1".parse().unwrap()));
        assert!(Network::parse("10.0.2.2").unwrap().holds("10.0.2.2".parse().unwrap()));
        assert!(Network::parse("0.0.0.0/0").unwrap().holds("10.9.9.9".parse().unwrap()));
        assert!(Network::parse("10.0.2.0/33").is_err());
        assert!(Network::parse("ten").is_err());
        // A name is resolved once, and refused if it leads anywhere private.
        let url = Url::parse("https://127.0.0.1:9/x").unwrap();
        assert!(
            addresses(&url, &[]).unwrap_err().contains("not a public address; nothing was sent")
        );
        let loopback = [Network::parse("127.0.0.0/8").unwrap()];
        assert_eq!(addresses(&url, &loopback).unwrap(), vec!["127.0.0.1:9".parse().unwrap()]);
    }

    fn change(severity: Severity, detail: &str) -> Change {
        Change {
            at: 1_794_000_000,
            source: "diskhealth".into(),
            state: State::Raised,
            alert: Alert::new("smart:wd-1".into(), severity, "The disk WD-1 is failing", detail),
        }
    }

    #[test]
    fn the_message_is_the_first_change_and_how_many_and_fits() {
        let changes =
            [change(Severity::Critical, "Copy its data off."), change(Severity::Warning, "")];
        let text = payload(&changes, "duke-nas", DETAIL_MAX).unwrap();
        let message: serde_json::Value = serde_json::from_slice(&text).unwrap();
        assert_eq!(
            message,
            serde_json::json!({
                "v": 1, "host": "duke-nas", "count": 2,
                "first": {
                    "state": "raised", "severity": "critical", "title": "The disk WD-1 is failing",
                    "detail": "Copy its data off.", "source": "diskhealth", "key": "smart:wd-1",
                    "at_ms": 1_794_000_000_000_u64
                }
            })
        );
        // A long detail is cut to 600 characters, and further while the
        // whole would pass 3,000 bytes.
        let long = change(Severity::Warning, &"é".repeat(2000));
        let cut: serde_json::Value =
            serde_json::from_slice(&payload(std::slice::from_ref(&long), "h", DETAIL_MAX).unwrap())
                .unwrap();
        assert_eq!(cut["first"]["detail"].as_str().unwrap().chars().count(), 600);
        let wide = change(Severity::Warning, &"\u{10348}".repeat(2000));
        let text = payload(&[wide], "h", DETAIL_MAX).unwrap();
        assert!(text.len() <= PAYLOAD_MAX);
        let short: serde_json::Value =
            serde_json::from_slice(&payload(&[long], "h", DETAIL_SHORT).unwrap()).unwrap();
        assert_eq!(short["first"]["detail"].as_str().unwrap().chars().count(), 200);
    }

    #[test]
    fn the_topic_is_the_same_for_the_same_alert_and_short() {
        let one = topic(&change(Severity::Warning, "a"));
        assert_eq!(one, topic(&change(Severity::Critical, "b")));
        assert_eq!(one.len(), 32);
        assert!(one.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        let mut other = change(Severity::Warning, "a");
        other.alert.key = "smart:wd-2".into();
        assert_ne!(one, topic(&other));
    }

    #[test]
    fn a_refusal_says_the_status_and_the_service_s_words() {
        assert_eq!(
            refusal(400, b"  UnauthorizedRegistration \n more "),
            "the push service answered 400: UnauthorizedRegistration more (ALR-105)"
        );
        assert_eq!(
            refusal(301, b""),
            "the push service answered 301 (a redirect, which is not followed) (ALR-105)"
        );
    }
}
