// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! timed's time service (ADR 0487): the time this machine keeps, served to
//! its networks over Network Time Security (RFC 8915), and over plain NTP
//! too when timed(5) says so, for printers, cameras and switches that
//! cannot do NTS.
//!
//! A client first runs key establishment on TCP port 4460: TLS 1.3 alone,
//! with the protocol `ntske/1` and the certificate timed(5)'s
//! `serve-certificate` names. The keys the TLS session exports, one for
//! each direction, go into eight cookies, each sealed with a master key of
//! the server's that only it holds, so that it keeps nothing per client. A
//! request on UDP port 123 then carries a cookie and is authenticated with
//! the client's key in it; the answer is sealed with the server's key in
//! it, and carries a new cookie for each one used and asked for, so that
//! no two requests can be linked by their cookies. A new master key is
//! made each day and the newest three are kept, so a client's cookies
//! work for two days at least. The keys live in memory alone: after a
//! restart a cookie no longer opens, the client is told so (a kiss of
//! death, `NTSN`), and it runs key establishment again, as RFC 8915 has
//! it do.
//!
//! What it serves is what timed last set the clock by, its [`Upstream`]:
//! one stratum further from the reference clock, with its delay and
//! dispersion added. Three hours after the last sync, or before the first,
//! it says it is not synchronized and clients ignore it, unless
//! `serve-unsynced` has it serve its own clock at stratum 10 for a
//! network with no other time.
//!
//! Each client address has a bucket of eight answers, one coming back
//! every two seconds; past it NTS requests get none, and plain ones the
//! RATE kiss, at most one a second. Only the networks of the machine's
//! own cards are answered, unless `serve-allow` says otherwise.

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use md5::{Digest, Md5};
use ring::rand::{SecureRandom, SystemRandom};
use rustls::crypto::ring as provider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use socket2::{Domain, Protocol, Socket, Type};

use netproto::ntp::{self, KEY_LEN, KeRecord};

/// Where the certificate and key served with must be, as for logforward's
/// (ADR 0482): so that a settings file cannot point timed at another
/// file as its key.
pub const SSL: &str = "/system/settings/ssl/";

/// How many clients may run key establishment at once, how long one may
/// take in all, and how much it may send.
const KE_AT_ONCE: usize = 64;
const KE_TIME: Duration = Duration::from_secs(10);
const KE_REQUEST_MAX: usize = 4096;
/// The cookies a key establishment gives, and the most an answer
/// carries.
const COOKIES: usize = 8;
/// How often a new master key is made, and how many are kept.
const KEY_EVERY: Duration = Duration::from_hours(24);
const KEYS_KEPT: usize = 3;
/// How long after its last sync the clock is served as synchronized.
const SYNC_LASTS: Duration = Duration::from_hours(3);
/// How fast a clock is taken to drift from its last sync, in
/// nanoseconds a second: NTP's PHI, 15 ppm.
const DRIFT_NS_PER_S: i128 = 15_000;
/// The rate limit: a bucket of BURST answers for each client address, one
/// coming back every REFILL, for the CLIENTS_MAX addresses heard from
/// last; and at most one RATE kiss each KISS_EVERY.
const BURST: f64 = 8.0;
const REFILL: Duration = Duration::from_secs(2);
const CLIENTS_MAX: usize = 4096;
const KISS_EVERY: Duration = Duration::from_secs(1);
/// How long a thread waits on its socket before it looks at whether to
/// stop.
const WAKE: Duration = Duration::from_secs(1);
/// How often the machine's own networks are read again.
const OWN_EVERY: Duration = Duration::from_mins(1);
/// The stratum and reference ID of a clock served with no sync, as
/// chrony's `local stratum 10`.
const LOCAL_STRATUM: u8 = 10;
const LOCAL_REFERENCE: [u8; 4] = *b"LOCL";
/// The precision answers give: 2^-20 s, about a microsecond.
const PRECISION: i8 = -20;
/// The shortest Unique Identifier RFC 8915 takes.
const UNIQUE_MIN: usize = 32;

/// A network: an address and how many of its first bits are the
/// network's.
type Network = (IpAddr, u8);

/// The keys a TLS session exports for NTS: client to server, then server
/// to client.
type SessionKeys = ([u8; KEY_LEN], [u8; KEY_LEN]);

/// What timed serves, from timed(5)'s `serve` setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Nothing: the default.
    #[default]
    Off,
    /// NTS alone.
    Nts,
    /// NTS, and plain NTP to those that cannot do NTS.
    NtsAndNtp,
}

/// Which clients are answered, from `serve-allow`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Allow {
    /// Those on the networks of the machine's own cards: the default.
    #[default]
    Own,
    /// Any.
    Any,
    /// Those on these networks.
    Networks(Vec<Network>),
}

/// timed(5)'s `serve` settings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServeSettings {
    pub mode: Mode,
    /// The PEM files of the certificate chain and its key.
    pub certificate: String,
    pub key: String,
    pub allow: Allow,
    /// Whether the clock is served at stratum 10 while it has no sync.
    pub unsynced: bool,
}

impl ServeSettings {
    /// Whether these settings answer plain NTP from `client`, an address on
    /// one of the machine's own networks: what a DHCP client that netd(8)
    /// gives this machine as its time server (option 42) speaks.
    pub fn answer_plain_ntp_from(&self, client: IpAddr) -> bool {
        self.mode == Mode::NtsAndNtp
            && match &self.allow {
                Allow::Own | Allow::Any => true,
                Allow::Networks(networks) => networks.iter().any(|&network| on(client, network)),
            }
    }
}

/// Takes the timed(5) setting `key` = `value` if it is one of serving's:
/// `None` if it is not, else whether `value` is wrong.
pub fn setting(settings: &mut ServeSettings, key: &str, value: &str) -> Option<Result<(), String>> {
    let value = value.trim();
    let result = match key {
        "serve" => match value {
            "off" => Ok(Mode::Off),
            "nts" => Ok(Mode::Nts),
            "nts+ntp" => Ok(Mode::NtsAndNtp),
            _ => Err(format!("serve is off, nts or nts+ntp, not '{value}'")),
        }
        .map(|mode| settings.mode = mode),
        "serve-certificate" => in_ssl(key, value).map(|path| settings.certificate = path),
        "serve-key" => in_ssl(key, value).map(|path| settings.key = path),
        "serve-allow" => allow(value).map(|allow| settings.allow = allow),
        "serve-unsynced" => match value {
            "yes" => Ok(true),
            "no" => Ok(false),
            _ => Err(format!("serve-unsynced is yes or no, not '{value}'")),
        }
        .map(|unsynced| settings.unsynced = unsynced),
        _ => return None,
    };
    Some(result)
}

/// What is wrong with `settings` as a whole once every line is read: NTS
/// needs a certificate and its key. Serving is turned off when it is.
pub fn check(settings: &mut ServeSettings) -> Option<String> {
    if settings.mode == Mode::Off || !(settings.certificate.is_empty() || settings.key.is_empty()) {
        return None;
    }
    settings.mode = Mode::Off;
    Some("serve needs serve-certificate and serve-key; no time is served".into())
}

/// `value` as the path of a file in [`SSL`].
fn in_ssl(key: &str, value: &str) -> Result<String, String> {
    let inside = value
        .strip_prefix(SSL)
        .is_some_and(|name| !name.is_empty() && !name.contains('/') && name != "." && name != "..");
    if inside { Ok(value.into()) } else { Err(format!("{key} must be a file in {SSL}")) }
}

/// `serve-allow`'s value: `any`, or networks as `address/prefix`, with
/// commas or spaces between them.
fn allow(value: &str) -> Result<Allow, String> {
    if value == "any" {
        return Ok(Allow::Any);
    }
    let networks: Result<Vec<_>, _> =
        value.split([',', ' ']).filter(|word| !word.is_empty()).map(network).collect();
    match networks? {
        networks if networks.is_empty() => Err("serve-allow names no network".into()),
        networks => Ok(Allow::Networks(networks)),
    }
}

/// A network as `address/prefix`, or one address alone.
fn network(text: &str) -> Result<Network, String> {
    let wrong = || format!("serve-allow: '{text}' is not a network such as 192.168.1.0/24");
    let (address, prefix) = text.split_once('/').unwrap_or((text, ""));
    let address: IpAddr = address.parse().map_err(|_| wrong())?;
    let longest = if address.is_ipv4() { 32 } else { 128 };
    let prefix = if prefix.is_empty() { longest } else { prefix.parse().map_err(|_| wrong())? };
    if prefix > longest {
        return Err(wrong());
    }
    Ok((address, prefix))
}

/// Whether `address` is on the network `(base, prefix)`.
fn on(address: IpAddr, (base, prefix): Network) -> bool {
    let (address, base, bits) = match (address, base) {
        (IpAddr::V4(a), IpAddr::V4(b)) => (u128::from(a.to_bits()), u128::from(b.to_bits()), 32),
        (IpAddr::V6(a), IpAddr::V6(b)) => (a.to_bits(), b.to_bits(), 128),
        // An IPv4 client over an IPv6 socket.
        (IpAddr::V6(a), IpAddr::V4(_)) => match a.to_ipv4_mapped() {
            Some(a) => return on(IpAddr::V4(a), (base, prefix)),
            None => return false,
        },
        (IpAddr::V4(_), IpAddr::V6(_)) => return false,
    };
    let host_bits = bits - u32::from(prefix);
    host_bits >= bits || (address >> host_bits) == (base >> host_bits)
}

/// What timed last set the clock by, from which the answers are made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Upstream {
    /// When, by the monotonic clock, and as an NTP timestamp.
    pub at: Instant,
    pub timestamp: u64,
    /// Its server's stratum, and the reference ID this server gives for
    /// it ([`reference_id`]).
    pub stratum: u8,
    pub reference: [u8; 4],
    /// The round trip to the reference clock, and how far off it may be,
    /// both in nanoseconds: the server's root delay plus the sample's
    /// delay, and its root dispersion plus half that delay.
    pub root_delay_ns: i128,
    pub root_dispersion_ns: i128,
}

/// The reference ID a server synced from `address` gives (RFC 5905
/// §7.3): the IPv4 address, or the first four bytes of the MD5 of the
/// IPv6 one.
pub fn reference_id(address: IpAddr) -> [u8; 4] {
    match address {
        IpAddr::V4(v4) => v4.octets(),
        IpAddr::V6(v6) => {
            let hash = Md5::digest(v6.octets());
            [hash[0], hash[1], hash[2], hash[3]]
        }
    }
}

/// The header fields an answer gives for the clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Served {
    leap: u8,
    stratum: u8,
    reference: [u8; 4],
    root_delay: u32,
    root_dispersion: u32,
    reference_time: u64,
}

/// What to serve, `now`, from `upstream`: synchronized for three hours
/// after a sync; else the local clock at stratum 10 with `unsynced`, or
/// not synchronized at all (leap indicator 3, stratum 16).
fn served(upstream: Option<Upstream>, unsynced: bool, now: Instant, receive: u64) -> Served {
    match upstream {
        Some(up) if now.saturating_duration_since(up.at) < SYNC_LASTS => {
            let since = i128::from(now.saturating_duration_since(up.at).as_secs());
            Served {
                leap: 0,
                stratum: up.stratum.saturating_add(1).min(15),
                reference: up.reference,
                root_delay: short(up.root_delay_ns),
                root_dispersion: short(up.root_dispersion_ns + since * DRIFT_NS_PER_S),
                reference_time: up.timestamp,
            }
        }
        _ if unsynced => Served {
            leap: 0,
            stratum: LOCAL_STRATUM,
            reference: LOCAL_REFERENCE,
            root_delay: 0,
            root_dispersion: 0,
            reference_time: receive,
        },
        _ => Served {
            leap: 3,
            stratum: 16,
            reference: [0; 4],
            root_delay: 0,
            root_dispersion: 0,
            reference_time: 0,
        },
    }
}

/// Nanoseconds in NTP's short format, 16.16 seconds, held at its largest
/// and at 0.
fn short(ns: i128) -> u32 {
    u32::try_from((ns.max(0) << 16) / 1_000_000_000).unwrap_or(u32::MAX)
}

/// The 48-byte header of an answer to `request`: its version, mode 4,
/// `served`, the request's poll and transmit time as its origin,
/// `receive` and `transmit`.
fn header(request: &[u8], served: Served, receive: u64, transmit: u64) -> Vec<u8> {
    let mut packet = vec![0; ntp::HEADER];
    let version = (request[0] >> 3) & 7;
    packet[0] = (served.leap << 6) | (version << 3) | 4;
    packet[1] = served.stratum;
    packet[2] = request[2];
    packet[3] = PRECISION.to_be_bytes()[0];
    packet[4..8].copy_from_slice(&served.root_delay.to_be_bytes());
    packet[8..12].copy_from_slice(&served.root_dispersion.to_be_bytes());
    packet[12..16].copy_from_slice(&served.reference);
    packet[16..24].copy_from_slice(&served.reference_time.to_be_bytes());
    packet[24..32].copy_from_slice(&request[40..48]);
    packet[32..40].copy_from_slice(&receive.to_be_bytes());
    packet[40..48].copy_from_slice(&transmit.to_be_bytes());
    packet
}

/// A kiss of death with the code `kiss` (RFC 5905 §7.4), in answer to
/// `request`: leap indicator 3, stratum 0.
fn kiss(request: &[u8], kiss: [u8; 4], receive: u64) -> Vec<u8> {
    let served = Served {
        leap: 3,
        stratum: 0,
        reference: kiss,
        root_delay: 0,
        root_dispersion: 0,
        reference_time: 0,
    };
    header(request, served, receive, receive)
}

/// The master keys cookies are sealed with: the newest first.
struct Keys {
    keys: VecDeque<(u32, [u8; KEY_LEN])>,
    made: Instant,
}

impl Keys {
    fn new(random: &SystemRandom) -> Keys {
        let mut keys = Keys { keys: VecDeque::new(), made: Instant::now() };
        keys.add(random, 0);
        keys
    }

    fn add(&mut self, random: &SystemRandom, number: u32) {
        let mut key = [0; KEY_LEN];
        // ring's system random source does not fail on a working system;
        // a key of zeros would only make cookies anyone could open, so a
        // failure keeps the old keys instead.
        if random.fill(&mut key).is_ok() {
            self.keys.push_front((number, key));
            self.keys.truncate(KEYS_KEPT);
            self.made = Instant::now();
        }
    }

    /// The key cookies are sealed with now, after making a new one if the
    /// newest is a day old.
    fn current(lock: &RwLock<Keys>, random: &SystemRandom) -> (u32, [u8; KEY_LEN]) {
        let fresh = |keys: &Keys| keys.made.elapsed() < KEY_EVERY;
        {
            let keys = lock.read().unwrap_or_else(PoisonError::into_inner);
            if fresh(&keys) {
                return keys.keys[0];
            }
        }
        let mut keys = lock.write().unwrap_or_else(PoisonError::into_inner);
        if !fresh(&keys) {
            let number = keys.keys[0].0.wrapping_add(1);
            keys.add(random, number);
        }
        keys.keys[0]
    }

    /// The key numbered `number`, if it is still kept.
    fn find(lock: &RwLock<Keys>, number: u32) -> Option<[u8; KEY_LEN]> {
        let keys = lock.read().unwrap_or_else(PoisonError::into_inner);
        keys.keys.iter().find(|(n, _)| *n == number).map(|(_, key)| *key)
    }
}

/// What the rate limit says to a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Answer,
    /// Send the RATE kiss, to a plain request.
    Kiss,
    Drop,
}

/// A client address's bucket.
struct Bucket {
    tokens: f64,
    at: Instant,
    kissed: Option<Instant>,
}

/// The rate limit's buckets, by client address.
#[derive(Default)]
struct Limiter {
    clients: HashMap<IpAddr, Bucket>,
}

impl Limiter {
    fn take(&mut self, address: IpAddr, now: Instant) -> Verdict {
        if !self.clients.contains_key(&address) && self.clients.len() >= CLIENTS_MAX {
            // The address heard from longest ago makes room.
            let oldest = self.clients.iter().min_by_key(|(_, b)| b.at).map(|(a, _)| *a);
            if let Some(oldest) = oldest {
                self.clients.remove(&oldest);
            }
        }
        let bucket =
            self.clients.entry(address).or_insert(Bucket { tokens: BURST, at: now, kissed: None });
        let back = now.saturating_duration_since(bucket.at).as_secs_f64() / REFILL.as_secs_f64();
        bucket.tokens = (bucket.tokens + back).min(BURST);
        bucket.at = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            return Verdict::Answer;
        }
        if bucket.kissed.is_none_or(|at| now.saturating_duration_since(at) >= KISS_EVERY) {
            bucket.kissed = Some(now);
            return Verdict::Kiss;
        }
        Verdict::Drop
    }
}

/// What the serving threads share.
struct Shared {
    settings: ServeSettings,
    upstream: Arc<Mutex<Option<Upstream>>>,
    tls: Arc<ServerConfig>,
    keys: RwLock<Keys>,
    limiter: Mutex<Limiter>,
    /// The networks of the machine's own cards, and when they were read.
    own: Mutex<OwnNetworks>,
    random: SystemRandom,
    stop: AtomicBool,
    answers: AtomicU64,
    established: AtomicU64,
    establishing: AtomicUsize,
}

impl Shared {
    fn random<const N: usize>(&self) -> [u8; N] {
        let mut bytes = [0; N];
        let _ = self.random.fill(&mut bytes);
        bytes
    }

    /// Whether `address` may be answered.
    fn allowed(&self, address: IpAddr) -> bool {
        match &self.settings.allow {
            Allow::Any => true,
            Allow::Networks(networks) => networks.iter().any(|&n| on(address, n)),
            Allow::Own => {
                let mut own = self.own.lock().unwrap_or_else(PoisonError::into_inner);
                if own.read.is_none_or(|at| at.elapsed() >= OWN_EVERY) {
                    *own = OwnNetworks { read: Some(Instant::now()), networks: own_networks() };
                }
                own.networks.iter().any(|&n| on(address, n))
            }
        }
    }
}

/// The networks of the machine's own cards, as last read.
#[derive(Default)]
struct OwnNetworks {
    read: Option<Instant>,
    networks: Vec<Network>,
}

/// The networks of the machine's own cards, loopback's among them.
fn own_networks() -> Vec<Network> {
    let interfaces = if_addrs::get_if_addrs().unwrap_or_default();
    interfaces
        .iter()
        .map(|interface| match &interface.addr {
            if_addrs::IfAddr::V4(v4) => (IpAddr::V4(v4.ip), v4.prefixlen),
            if_addrs::IfAddr::V6(v6) => (IpAddr::V6(v6.ip), v6.prefixlen),
        })
        .collect()
}

/// The time service while it runs; dropping it stops it.
pub struct Server {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl Server {
    /// Starts serving as `settings` say, from what `upstream` holds:
    /// listens on port 4460 and 123, over IPv4 and, where the machine has
    /// it, IPv6.
    ///
    /// # Errors
    ///
    /// When the certificate or key cannot be read, or a port cannot be
    /// listened on over IPv4, in words for the log.
    pub fn start(
        settings: ServeSettings,
        upstream: Arc<Mutex<Option<Upstream>>>,
    ) -> Result<Server, String> {
        let tls = tls_config(&settings.certificate, &settings.key)?;
        let random = SystemRandom::new();
        let shared = Arc::new(Shared {
            settings,
            upstream,
            tls,
            keys: RwLock::new(Keys::new(&random)),
            limiter: Mutex::new(Limiter::default()),
            own: Mutex::new(OwnNetworks::default()),
            random,
            stop: AtomicBool::new(false),
            answers: AtomicU64::new(0),
            established: AtomicU64::new(0),
            establishing: AtomicUsize::new(0),
        });
        let mut threads = Vec::new();
        for (v6, port) in [(false, ntp::NTS_KE_PORT), (true, ntp::NTS_KE_PORT)] {
            match listener(v6, port) {
                Ok(listener) => {
                    let shared = shared.clone();
                    threads.push(thread::spawn(move || accept(&shared, &listener)));
                }
                Err(err) if v6 => eprintln!("timed: not serving NTS over IPv6: {err}"),
                Err(err) => return Err(format!("cannot listen on port {port}: {err}")),
            }
        }
        for (v6, port) in [(false, ntp::NTP_PORT), (true, ntp::NTP_PORT)] {
            match udp(v6, port) {
                Ok(socket) => {
                    let shared = shared.clone();
                    threads.push(thread::spawn(move || answer_all(&shared, &socket)));
                }
                Err(err) if v6 => eprintln!("timed: not serving NTP over IPv6: {err}"),
                Err(err) => return Err(format!("cannot listen on port {port}: {err}")),
            }
        }
        Ok(Server { shared, threads })
    }

    /// The status file's line for it.
    pub fn status(&self) -> String {
        let settings = &self.shared.settings;
        let what = match settings.mode {
            Mode::NtsAndNtp => "NTS and plain NTP",
            _ => "NTS",
        };
        let to = match &settings.allow {
            Allow::Own => "this machine's networks".to_string(),
            Allow::Any => "any address".to_string(),
            Allow::Networks(networks) => networks.iter().fold(String::new(), |mut text, (a, p)| {
                let _ = write!(text, "{}{a}/{p}", if text.is_empty() { "" } else { ", " });
                text
            }),
        };
        let clients = self.shared.limiter.lock().unwrap_or_else(PoisonError::into_inner);
        format!(
            "Serving: time over {what} to {to}: {} answers and {} key establishments to {} \
             clients since timed started\n",
            self.shared.answers.load(Ordering::Relaxed),
            self.shared.established.load(Ordering::Relaxed),
            clients.clients.len(),
        )
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// The TLS settings key establishment is served with: TLS 1.3 alone, the
/// protocol `ntske/1`, the chain in `certificate` and its `key`.
fn tls_config(certificate: &str, key: &str) -> Result<Arc<ServerConfig>, String> {
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(certificate)
        .and_then(Iterator::collect)
        .map_err(|err| format!("cannot read the certificate {certificate}: {err}"))?;
    if chain.is_empty() {
        return Err(format!("no certificate in {certificate}"));
    }
    let key = PrivateKeyDer::from_pem_file(key)
        .map_err(|err| format!("cannot read the key {key}: {err}"))?;
    let mut config = ServerConfig::builder_with_provider(Arc::new(provider::default_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|err| err.to_string())?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(|err| format!("the certificate and key do not go together: {err}"))?;
    config.alpn_protocols = vec![ntp::NTS_ALPN.to_vec()];
    Ok(Arc::new(config))
}

/// A socket of `kind` on `port` of every address, of IPv6 alone when `v6`,
/// so that the IPv4 one beside it can be bound too.
fn bound(v6: bool, kind: Type, protocol: Protocol, port: u16) -> io::Result<Socket> {
    let (domain, address): (Domain, SocketAddr) = if v6 {
        (Domain::IPV6, SocketAddr::from(([0u16; 8], port)))
    } else {
        (Domain::IPV4, SocketAddr::from(([0u8; 4], port)))
    };
    let socket = Socket::new(domain, kind, Some(protocol))?;
    if v6 {
        socket.set_only_v6(true)?;
    }
    socket.set_reuse_address(true)?;
    socket.bind(&address.into())?;
    Ok(socket)
}

fn listener(v6: bool, port: u16) -> io::Result<TcpListener> {
    let socket = bound(v6, Type::STREAM, Protocol::TCP, port)?;
    socket.listen(64)?;
    Ok(socket.into())
}

fn udp(v6: bool, port: u16) -> io::Result<UdpSocket> {
    let socket: UdpSocket = bound(v6, Type::DGRAM, Protocol::UDP, port)?.into();
    socket.set_read_timeout(Some(WAKE))?;
    Ok(socket)
}

/// Whether `listener` has a connection waiting, after waiting up to
/// [`WAKE`] for one.
fn waiting(listener: &TcpListener) -> bool {
    let mut poll = libc::pollfd { fd: listener.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    let wake = libc::c_int::try_from(WAKE.as_millis()).unwrap_or(1000);
    // SAFETY: one pollfd, which lives across the call.
    unsafe { libc::poll(&raw mut poll, 1, wake) > 0 }
}

/// Takes key establishment connections on `listener` until told to stop,
/// each on a thread of its own, at most [`KE_AT_ONCE`] at once.
fn accept(shared: &Arc<Shared>, listener: &TcpListener) {
    while !shared.stop.load(Ordering::Relaxed) {
        if !waiting(listener) {
            continue;
        }
        let Ok((tcp, from)) = listener.accept() else { continue };
        if !shared.allowed(from.ip()) {
            continue;
        }
        if shared.establishing.fetch_add(1, Ordering::AcqRel) >= KE_AT_ONCE {
            shared.establishing.fetch_sub(1, Ordering::AcqRel);
            continue;
        }
        let shared = shared.clone();
        thread::spawn(move || {
            if establish(&shared, tcp).is_ok() {
                shared.established.fetch_add(1, Ordering::Relaxed);
            }
            shared.establishing.fetch_sub(1, Ordering::AcqRel);
        });
    }
}

/// Runs key establishment with a client over `tcp`.
fn establish(shared: &Shared, tcp: TcpStream) -> io::Result<()> {
    let deadline = Instant::now() + KE_TIME;
    tcp.set_write_timeout(Some(KE_TIME))?;
    let connection = ServerConnection::new(shared.tls.clone()).map_err(io::Error::other)?;
    let mut stream = StreamOwned::new(connection, tcp);
    let mut data = Vec::new();
    let mut buf = [0; 1024];
    let records = loop {
        if let Some((records, _)) = ntp::ke_records(&data) {
            break records;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || data.len() > KE_REQUEST_MAX {
            return Err(io::ErrorKind::InvalidData.into());
        }
        stream.sock.set_read_timeout(Some(left))?;
        let n = stream.read(&mut buf)?;
        if n == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        data.extend_from_slice(&buf[..n]);
    };
    // A client that did not offer `ntske/1` is not an NTS client.
    if stream.conn.alpn_protocol() != Some(ntp::NTS_ALPN) {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let conn = &stream.conn;
    let keys = || -> Option<SessionKeys> {
        let mut c2s = [0; KEY_LEN];
        let mut s2c = [0; KEY_LEN];
        let label = ntp::EXPORTER_LABEL;
        conn.export_keying_material(&mut c2s, label, Some(&ntp::exporter_context(false))).ok()?;
        conn.export_keying_material(&mut s2c, label, Some(&ntp::exporter_context(true))).ok()?;
        Some((c2s, s2c))
    };
    let answer = ke_answer(&records, keys, |c2s, s2c| cookie(shared, c2s, s2c));
    stream.write_all(&answer)?;
    stream.flush()?;
    stream.conn.send_close_notify();
    stream.flush()
}

/// A new cookie for the keys `c2s` and `s2c`.
fn cookie(shared: &Shared, c2s: &[u8; KEY_LEN], s2c: &[u8; KEY_LEN]) -> Option<Vec<u8>> {
    let (number, key) = Keys::current(&shared.keys, &shared.random);
    ntp::seal_cookie(&key, number, &shared.random(), c2s, s2c)
}

/// The answer to a key establishment request's `records` (RFC 8915
/// §4): NTPv4 and AES-SIV-CMAC-256 with eight cookies for the keys
/// `keys` gives; or what the client asked for that this server does not
/// do.
fn ke_answer(
    records: &[KeRecord],
    keys: impl FnOnce() -> Option<SessionKeys>,
    mut cookie: impl FnMut(&[u8; KEY_LEN], &[u8; KEY_LEN]) -> Option<Vec<u8>>,
) -> Vec<u8> {
    let end = ntp::record(ntp::END_OF_MESSAGE, true, &[]);
    let error =
        |code: u16| [ntp::record(ntp::ERROR, true, &code.to_be_bytes()), end.clone()].concat();
    const UNRECOGNIZED_CRITICAL: u16 = 0;
    const BAD_REQUEST: u16 = 1;
    const INTERNAL: u16 = 2;
    let known = [ntp::NEXT_PROTOCOL, ntp::AEAD_ALGORITHM, ntp::SERVER, ntp::PORT];
    if records.iter().any(|(kind, critical, _)| *critical && !known.contains(kind)) {
        return error(UNRECOGNIZED_CRITICAL);
    }
    let list = |kind: u16| -> Option<Option<Vec<u16>>> {
        let mut found = records.iter().filter(|(k, _, _)| *k == kind);
        let Some((_, _, body)) = found.next() else { return Some(None) };
        if found.next().is_some() || body.len() % 2 != 0 {
            return None;
        }
        Some(Some(body.as_chunks::<2>().0.iter().map(|pair| u16::from_be_bytes(*pair)).collect()))
    };
    let (Some(Some(protocols)), Some(algorithms)) =
        (list(ntp::NEXT_PROTOCOL), list(ntp::AEAD_ALGORITHM))
    else {
        return error(BAD_REQUEST);
    };
    if !protocols.contains(&ntp::PROTOCOL_NTPV4) {
        return [ntp::record(ntp::NEXT_PROTOCOL, true, &[]), end].concat();
    }
    let mut answer = ntp::record(ntp::NEXT_PROTOCOL, true, &ntp::PROTOCOL_NTPV4.to_be_bytes());
    if !algorithms.unwrap_or_default().contains(&ntp::AES_SIV_CMAC_256) {
        answer.extend(ntp::record(ntp::AEAD_ALGORITHM, true, &[]));
        answer.extend(end);
        return answer;
    }
    let Some((c2s, s2c)) = keys() else { return error(INTERNAL) };
    answer.extend(ntp::record(ntp::AEAD_ALGORITHM, true, &ntp::AES_SIV_CMAC_256.to_be_bytes()));
    for _ in 0..COOKIES {
        let Some(cookie) = cookie(&c2s, &s2c) else { return error(INTERNAL) };
        answer.extend(ntp::record(ntp::NEW_COOKIE, false, &cookie));
    }
    answer.extend(end);
    answer
}

/// Answers NTP requests on `socket` until told to stop.
fn answer_all(shared: &Shared, socket: &UdpSocket) {
    let mut buf = [0; 2048];
    while !shared.stop.load(Ordering::Relaxed) {
        let Ok((len, from)) = socket.recv_from(&mut buf) else { continue };
        let receive = ntp::timestamp(unix_ns());
        if !shared.allowed(from.ip()) {
            continue;
        }
        let verdict = shared
            .limiter
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take(from.ip(), Instant::now());
        let upstream = *shared.upstream.lock().unwrap_or_else(PoisonError::into_inner);
        let context = Context {
            mode: shared.settings.mode,
            served: served(upstream, shared.settings.unsynced, Instant::now(), receive),
            verdict,
        };
        let keys = |number| Keys::find(&shared.keys, number);
        let cookie = |c2s: &[u8; KEY_LEN], s2c: &[u8; KEY_LEN]| cookie(shared, c2s, s2c);
        let nonce = shared.random::<16>();
        let made = answer(&buf[..len], &context, receive, keys, cookie, &nonce, || {
            ntp::timestamp(unix_ns())
        });
        if let Some(answer) = made
            && socket.send_to(&answer, from).is_ok()
        {
            shared.answers.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The time now, in nanoseconds since 1970.
fn unix_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

/// What an answer to one request is made with, besides the request.
struct Context {
    mode: Mode,
    served: Served,
    verdict: Verdict,
}

/// The answer to the NTP `request`, received at `receive`, if it gets
/// one: plain NTP's, or NTS's sealed with the keys its cookie stands for.
/// `keys` gives the master key of a number, `cookie` makes new cookies,
/// `nonce` seals the answer, and `transmit` is read last, just before the
/// answer is sent.
fn answer(
    request: &[u8],
    context: &Context,
    receive: u64,
    keys: impl FnOnce(u32) -> Option<[u8; KEY_LEN]>,
    mut cookie: impl FnMut(&[u8; KEY_LEN], &[u8; KEY_LEN]) -> Option<Vec<u8>>,
    nonce: &[u8; 16],
    transmit: impl FnOnce() -> u64,
) -> Option<Vec<u8>> {
    let first = *request.first()?;
    let version = (first >> 3) & 7;
    if request.len() < ntp::HEADER || first & 7 != 3 || !(1..=4).contains(&version) {
        return None;
    }
    if request.len() == ntp::HEADER {
        if context.mode != Mode::NtsAndNtp {
            return None;
        }
        return match context.verdict {
            Verdict::Answer => Some(header(request, context.served, receive, transmit())),
            Verdict::Kiss => Some(kiss(request, *b"RATE", receive)),
            Verdict::Drop => None,
        };
    }
    // Extension fields that are not NTS's get no answer, nor does NTS past
    // the rate limit: a kiss of death that is not authenticated is one a
    // client must not trust (RFC 8915 §5.7).
    if context.verdict != Verdict::Answer {
        return None;
    }
    let fields = ntp::fields(request)?;
    let body = |kind: u16| {
        fields.iter().find(|(k, _, _)| *k == kind).map(|&(_, at, end)| &request[at + 4..end])
    };
    let unique = body(ntp::UNIQUE_IDENTIFIER).filter(|unique| unique.len() >= UNIQUE_MIN)?;
    let used = body(ntp::COOKIE)?;
    let &(_, at, end) = fields.iter().find(|(kind, _, _)| *kind == ntp::AUTHENTICATOR)?;
    let unique_field = ntp::field(ntp::UNIQUE_IDENTIFIER, unique);
    let Some((c2s, s2c)) = ntp::open_cookie(used, keys) else {
        return Some([kiss(request, *b"NTSN", receive), unique_field].concat());
    };
    if ntp::open(&c2s, request, at, end).is_none() {
        return Some([kiss(request, *b"NTSN", receive), unique_field].concat());
    }
    let placeholders = fields.iter().filter(|(kind, _, _)| *kind == ntp::COOKIE_PLACEHOLDER);
    let wanted = (1 + placeholders.count()).min(COOKIES);
    let mut cookies = Vec::with_capacity(wanted);
    for _ in 0..wanted {
        cookies.push(ntp::field(ntp::COOKIE, &cookie(&c2s, &s2c)?));
    }
    let mut packet = header(request, context.served, receive, transmit());
    packet.extend(unique_field);
    // An answer is never longer than its request, so that it cannot be
    // used to amplify an attack: a cookie less for each that does not fit.
    loop {
        let sealed = ntp::authenticator(&s2c, nonce, &packet, &cookies.concat())?;
        if packet.len() + sealed.len() <= request.len() || cookies.len() <= 1 {
            packet.extend(sealed);
            return Some(packet);
        }
        cookies.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECEIVE: u64 = 0xEA00_0000_8000_0000;
    const TRANSMIT: u64 = 0xEA00_0000_8000_1000;

    fn context(mode: Mode, verdict: Verdict) -> Context {
        let up = Upstream {
            at: Instant::now(),
            timestamp: 0xEA00_0000_0000_0000,
            stratum: 1,
            reference: [192, 0, 2, 1],
            root_delay_ns: 2_000_000,
            root_dispersion_ns: 500_000,
        };
        Context { mode, served: served(Some(up), false, Instant::now(), RECEIVE), verdict }
    }

    fn master(number: u32) -> Option<[u8; KEY_LEN]> {
        (number == 5).then_some([7; KEY_LEN])
    }

    fn new_cookie(c2s: &[u8; KEY_LEN], s2c: &[u8; KEY_LEN]) -> Option<Vec<u8>> {
        ntp::seal_cookie(&[7; KEY_LEN], 5, &[3; 16], c2s, s2c)
    }

    /// A client's NTS request with a cookie for the keys `([1; 32], [2;
    /// 32])` and `placeholders` placeholders.
    fn nts_request(placeholders: usize) -> (Vec<u8>, [u8; 32]) {
        let cookie = new_cookie(&[1; KEY_LEN], &[2; KEY_LEN]).unwrap();
        let unique = [9; 32];
        let request =
            ntp::nts_request(0x1234, &unique, &cookie, placeholders, &[1; KEY_LEN], &[4; 16])
                .unwrap();
        (request, unique)
    }

    fn serve(request: &[u8], context: &Context) -> Option<Vec<u8>> {
        answer(request, context, RECEIVE, master, new_cookie, &[6; 16], || TRANSMIT)
    }

    #[test]
    fn an_nts_request_is_answered_sealed_with_a_cookie_for_each_used_and_asked() {
        let (request, unique) = nts_request(3);
        let answer = serve(&request, &context(Mode::Nts, Verdict::Answer)).unwrap();
        assert!(answer.len() <= request.len());
        let cookies = ntp::nts_answer(&answer, &unique, &[2; KEY_LEN]).unwrap();
        assert_eq!(cookies.len(), 4);
        let opened = ntp::open_cookie(&cookies[0], master).unwrap();
        assert_eq!(opened, ([1; KEY_LEN], [2; KEY_LEN]));
        let header = ntp::answer(&answer).unwrap();
        assert_eq!((header.mode, header.stratum, header.leap), (4, 2, 0));
        assert_eq!(header.reference, [192, 0, 2, 1]);
        assert_eq!((header.origin, header.receive, header.transmit), (0x1234, RECEIVE, TRANSMIT));
        assert_eq!(header.root_delay, short(2_000_000));
    }

    #[test]
    fn a_cookie_that_does_not_open_gets_a_kiss_with_the_unique_identifier() {
        let (request, unique) = nts_request(0);
        let answer = answer(
            &request,
            &context(Mode::Nts, Verdict::Answer),
            RECEIVE,
            |_| None,
            new_cookie,
            &[6; 16],
            || TRANSMIT,
        )
        .unwrap();
        let header = ntp::answer(&answer).unwrap();
        assert_eq!((header.stratum, header.reference), (0, *b"NTSN"));
        let fields = ntp::fields(&answer).unwrap();
        assert!(fields.iter().all(|(kind, _, _)| *kind != ntp::AUTHENTICATOR));
        assert!(answer.windows(32).any(|window| window == unique));
    }

    #[test]
    fn a_request_whose_authenticator_does_not_open_gets_a_kiss() {
        let (mut request, _) = nts_request(0);
        // The transmit timestamp is inside what the Authenticator covers.
        request[47] ^= 1;
        let answer = serve(&request, &context(Mode::Nts, Verdict::Answer)).unwrap();
        assert_eq!(ntp::answer(&answer).unwrap().reference, *b"NTSN");
    }

    #[test]
    fn plain_ntp_is_answered_only_when_served() {
        let request = ntp::request(0x55);
        assert_eq!(serve(&request, &context(Mode::Nts, Verdict::Answer)), None);
        let answer = serve(&request, &context(Mode::NtsAndNtp, Verdict::Answer)).unwrap();
        assert_eq!(answer.len(), ntp::HEADER);
        let header = ntp::answer(&answer).unwrap();
        assert_eq!((header.mode, header.version, header.origin), (4, 4, 0x55));
        let kissed = serve(&request, &context(Mode::NtsAndNtp, Verdict::Kiss)).unwrap();
        assert_eq!(ntp::answer(&kissed).unwrap().reference, *b"RATE");
        assert_eq!(serve(&request, &context(Mode::NtsAndNtp, Verdict::Drop)), None);
    }

    #[test]
    fn nts_past_the_rate_limit_and_odd_packets_get_nothing() {
        let (request, _) = nts_request(0);
        assert_eq!(serve(&request, &context(Mode::Nts, Verdict::Kiss)), None);
        let mut server_mode = ntp::request(1);
        server_mode[0] = 0x24;
        assert_eq!(serve(&server_mode, &context(Mode::NtsAndNtp, Verdict::Answer)), None);
        assert_eq!(serve(&[0x23; 47], &context(Mode::NtsAndNtp, Verdict::Answer)), None);
        let mut other_field = ntp::request(1);
        other_field.extend(ntp::field(0x0999, &[0; 12]));
        assert_eq!(serve(&other_field, &context(Mode::NtsAndNtp, Verdict::Answer)), None);
    }

    #[test]
    fn the_clock_is_served_as_synced_local_or_not_at_all() {
        let now = Instant::now();
        let up = Upstream {
            at: now,
            timestamp: 1,
            stratum: 15,
            reference: [1, 2, 3, 4],
            root_delay_ns: 0,
            root_dispersion_ns: 0,
        };
        assert_eq!(served(Some(up), false, now, 9).stratum, 15, "never past 15");
        let later = now + SYNC_LASTS + Duration::from_secs(1);
        let stale = served(Some(up), false, later, 9);
        assert_eq!((stale.leap, stale.stratum), (3, 16));
        let local = served(None, true, now, 9);
        assert_eq!((local.stratum, local.reference, local.reference_time), (10, *b"LOCL", 9));
        let drifted = served(Some(up), false, now + Duration::from_secs(100), 9);
        assert_eq!(drifted.root_dispersion, short(100 * DRIFT_NS_PER_S));
    }

    #[test]
    fn key_establishment_gives_eight_cookies_for_the_exported_keys() {
        let protocols = (ntp::NEXT_PROTOCOL, true, ntp::PROTOCOL_NTPV4.to_be_bytes().to_vec());
        let aead = (ntp::AEAD_ALGORITHM, false, [0, 17, 0, 15].to_vec());
        let answer = ke_answer(&[protocols, aead], || Some(([1; 32], [2; 32])), new_cookie);
        let established = ntp::ke_answer(&ntp::ke_records(&answer).unwrap().0).unwrap();
        assert_eq!(established.cookies.len(), COOKIES);
        assert_eq!(ntp::open_cookie(&established.cookies[0], master), Some(([1; 32], [2; 32])));
    }

    #[test]
    fn key_establishment_says_what_it_does_not_do() {
        let protocols = (ntp::NEXT_PROTOCOL, true, ntp::PROTOCOL_NTPV4.to_be_bytes().to_vec());
        let aead = (ntp::AEAD_ALGORITHM, false, ntp::AES_SIV_CMAC_256.to_be_bytes().to_vec());
        let error = |code: u16| {
            [
                ntp::record(ntp::ERROR, true, &code.to_be_bytes()),
                ntp::record(ntp::END_OF_MESSAGE, true, &[]),
            ]
            .concat()
        };
        let unknown = (0x4000, true, Vec::new());
        assert_eq!(ke_answer(&[protocols.clone(), unknown], || None, new_cookie), error(0));
        assert_eq!(
            ke_answer(std::slice::from_ref(&aead), || None, new_cookie),
            error(1),
            "no protocols"
        );
        let odd = (ntp::NEXT_PROTOCOL, true, vec![0]);
        assert_eq!(ke_answer(&[odd, aead.clone()], || None, new_cookie), error(1));
        let other = (ntp::NEXT_PROTOCOL, true, 0x8001u16.to_be_bytes().to_vec());
        let none = ke_answer(&[other, aead], || None, new_cookie);
        assert_eq!(
            none,
            [
                ntp::record(ntp::NEXT_PROTOCOL, true, &[]),
                ntp::record(ntp::END_OF_MESSAGE, true, &[])
            ]
            .concat()
        );
        let gcm = (ntp::AEAD_ALGORITHM, false, 1u16.to_be_bytes().to_vec());
        let no_aead = ke_answer(&[protocols, gcm], || None, new_cookie);
        assert!(
            ntp::ke_records(&no_aead)
                .unwrap()
                .0
                .iter()
                .all(|(kind, _, _)| *kind != ntp::NEW_COOKIE)
        );
    }

    #[test]
    fn the_rate_limit_gives_eight_then_one_each_two_seconds_and_one_kiss_a_second() {
        let mut limiter = Limiter::default();
        let client: IpAddr = "192.0.2.7".parse().unwrap();
        let start = Instant::now();
        for _ in 0..8 {
            assert_eq!(limiter.take(client, start), Verdict::Answer);
        }
        assert_eq!(limiter.take(client, start), Verdict::Kiss);
        assert_eq!(limiter.take(client, start), Verdict::Drop);
        assert_eq!(limiter.take(client, start + Duration::from_secs(2)), Verdict::Answer);
        let other: IpAddr = "192.0.2.8".parse().unwrap();
        assert_eq!(limiter.take(other, start), Verdict::Answer, "each address has its own");
    }

    #[test]
    fn the_rate_limit_forgets_the_oldest_address_when_full() {
        let mut limiter = Limiter::default();
        let start = Instant::now();
        for n in 0..CLIENTS_MAX as u32 {
            let address = IpAddr::V4(std::net::Ipv4Addr::from_bits(0x0A00_0000 + n));
            limiter.take(address, start + Duration::from_micros(u64::from(n)));
        }
        limiter.take("192.0.2.1".parse().unwrap(), start + Duration::from_secs(1));
        assert_eq!(limiter.clients.len(), CLIENTS_MAX);
        assert!(!limiter.clients.contains_key(&"10.0.0.0".parse().unwrap()));
    }

    #[test]
    fn master_keys_turn_over_daily_and_three_are_kept() {
        let random = SystemRandom::new();
        let lock = RwLock::new(Keys::new(&random));
        let (first, key) = Keys::current(&lock, &random);
        assert_eq!(Keys::find(&lock, first), Some(key));
        for _ in 0..3 {
            lock.write().unwrap().made -= KEY_EVERY;
            Keys::current(&lock, &random);
        }
        assert_eq!(Keys::find(&lock, first), None, "the fourth key drops the first");
        assert_eq!(lock.read().unwrap().keys.len(), KEYS_KEPT);
    }

    #[test]
    fn settings_are_read_and_checked() {
        let mut settings = ServeSettings::default();
        assert_eq!(setting(&mut settings, "servers", "x"), None);
        assert_eq!(setting(&mut settings, "serve", "nts+ntp"), Some(Ok(())));
        assert!(matches!(setting(&mut settings, "serve", "yes"), Some(Err(_))));
        assert!(matches!(setting(&mut settings, "serve-key", "/etc/key.pem"), Some(Err(_))));
        assert!(matches!(
            setting(&mut settings, "serve-key", "/system/settings/ssl/../key"),
            Some(Err(_))
        ));
        assert_eq!(
            check(&mut settings),
            Some("serve needs serve-certificate and serve-key; no time is served".into())
        );
        assert_eq!(settings.mode, Mode::Off);
        settings.mode = Mode::Nts;
        assert_eq!(
            setting(&mut settings, "serve-certificate", "/system/settings/ssl/time.pem"),
            Some(Ok(()))
        );
        assert_eq!(
            setting(&mut settings, "serve-key", "/system/settings/ssl/time.key"),
            Some(Ok(()))
        );
        assert_eq!(check(&mut settings), None);
        assert_eq!(setting(&mut settings, "serve-allow", "10.9.0.0/24, fd00::/8"), Some(Ok(())));
        assert_eq!(
            settings.allow,
            Allow::Networks(vec![
                ("10.9.0.0".parse().unwrap(), 24),
                ("fd00::".parse().unwrap(), 8)
            ])
        );
        assert!(matches!(setting(&mut settings, "serve-allow", "10.9.0.0/33"), Some(Err(_))));
        assert_eq!(setting(&mut settings, "serve-allow", "any"), Some(Ok(())));
        assert_eq!(setting(&mut settings, "serve-unsynced", "yes"), Some(Ok(())));
        assert!(settings.unsynced);
    }

    #[test]
    fn networks_take_their_addresses() {
        let net = |text: &str| network(text).unwrap();
        let ip = |text: &str| text.parse::<IpAddr>().unwrap();
        assert!(on(ip("10.9.0.20"), net("10.9.0.0/24")));
        assert!(!on(ip("10.9.1.20"), net("10.9.0.0/24")));
        assert!(on(ip("::ffff:10.9.0.20"), net("10.9.0.0/24")), "IPv4 over an IPv6 socket");
        assert!(on(ip("fd00::1"), net("fd00::/8")));
        assert!(on(ip("203.0.113.9"), net("0.0.0.0/0")));
        assert!(on(ip("10.9.0.20"), net("10.9.0.20")));
        assert!(!on(ip("10.9.0.21"), net("10.9.0.20")));
    }

    #[test]
    fn an_ipv6_reference_is_the_start_of_its_md5() {
        assert_eq!(reference_id(ip4()), [192, 0, 2, 1]);
        let v6: IpAddr = "2001:db8::1".parse().unwrap();
        let hash = Md5::digest(match v6 {
            IpAddr::V6(v6) => v6.octets(),
            IpAddr::V4(_) => unreachable!(),
        });
        assert_eq!(reference_id(v6), [hash[0], hash[1], hash[2], hash[3]]);
    }

    fn ip4() -> IpAddr {
        "192.0.2.1".parse().unwrap()
    }

    fn testdata(name: &str) -> String {
        format!("{}/../tls-test/testdata/{name}", env!("CARGO_MANIFEST_DIR"))
    }

    /// What the serving threads share, serving NTS with tls-test's
    /// certificate for tls.test.
    fn shared() -> Arc<Shared> {
        let random = SystemRandom::new();
        Arc::new(Shared {
            settings: ServeSettings { mode: Mode::Nts, ..ServeSettings::default() },
            upstream: Arc::new(Mutex::new(None)),
            tls: tls_config(&testdata("rsa-2048.pem"), &testdata("rsa-2048.key")).unwrap(),
            keys: RwLock::new(Keys::new(&random)),
            limiter: Mutex::new(Limiter::default()),
            own: Mutex::new(OwnNetworks::default()),
            random,
            stop: AtomicBool::new(false),
            answers: AtomicU64::new(0),
            established: AtomicU64::new(0),
            establishing: AtomicUsize::new(0),
        })
    }

    /// Runs key establishment against a server thread of `shared`'s, as a
    /// client offering `alpn`: the TLS session's keys as the client exports
    /// them, the server's answer, and how establish ended.
    fn key_establishment(
        shared: &Arc<Shared>,
        alpn: &[&[u8]],
    ) -> (SessionKeys, Vec<u8>, io::Result<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = {
            let shared = shared.clone();
            thread::spawn(move || establish(&shared, listener.accept().unwrap().0))
        };
        let mut roots = rustls::RootCertStore::empty();
        let ca = CertificateDer::pem_file_iter(testdata("ca.pem")).unwrap();
        roots.add_parsable_certificates(ca.flatten());
        let config = crate::tls::verified(roots, alpn);
        let name = rustls::pki_types::ServerName::try_from("tls.test").unwrap();
        let mut stream =
            crate::tls::connect(address, name, &config, Duration::from_secs(5)).unwrap();
        let mut keys = ([0; KEY_LEN], [0; KEY_LEN]);
        for (key, to_client) in [(&mut keys.0, false), (&mut keys.1, true)] {
            let context = ntp::exporter_context(to_client);
            stream.conn.export_keying_material(key, ntp::EXPORTER_LABEL, Some(&context)).unwrap();
        }
        let _ = stream.write_all(&ntp::ke_request()).and_then(|()| stream.flush());
        let mut answer = Vec::new();
        let _ = stream.read_to_end(&mut answer);
        (keys, answer, server.join().unwrap())
    }

    #[test]
    fn key_establishment_over_tls_gives_cookies_that_authenticate_requests() {
        let shared = shared();
        let ((c2s, s2c), ke, ended) = key_establishment(&shared, &[ntp::NTS_ALPN]);
        ended.unwrap();
        let established = ntp::ke_answer(&ntp::ke_records(&ke).unwrap().0).unwrap();
        assert_eq!(established.cookies.len(), COOKIES);
        let unique = [5; 32];
        let request =
            ntp::nts_request(77, &unique, &established.cookies[0], 2, &c2s, &[4; 16]).unwrap();
        let context = Context {
            mode: Mode::Nts,
            served: served(None, true, Instant::now(), RECEIVE),
            verdict: Verdict::Answer,
        };
        let keys = |number| Keys::find(&shared.keys, number);
        let new = |c2s: &[u8; KEY_LEN], s2c: &[u8; KEY_LEN]| cookie(&shared, c2s, s2c);
        let answer = answer(&request, &context, RECEIVE, keys, new, &[6; 16], || TRANSMIT).unwrap();
        assert_eq!(ntp::nts_answer(&answer, &unique, &s2c).unwrap().len(), 3);
        assert!(answer.len() <= request.len());
    }

    #[test]
    fn a_client_that_does_not_offer_ntske_gets_nothing() {
        let shared = shared();
        let (_, answer, ended) = key_establishment(&shared, &[]);
        assert!(ended.is_err());
        assert!(answer.is_empty());
    }
}
