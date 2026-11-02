// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What wg(8) and wgd(8) share (ADR 0340): WireGuard's keys, its
//! configuration files, and the text protocol wg speaks to wgd, the one
//! every userspace WireGuard speaks (wireguard.com/xplatform).
//!
//! A key is 32 bytes, written in base64 in files and on wg's command
//! line, and in hexadecimal in the protocol. A configuration file is
//! wg-quick's: an `[Interface]` section and a `[Peer]` section for each
//! peer, with `Key = value` lines and comments after `#`. wg(8)'s own
//! files are the same without wg-quick's settings (`Address`, `DNS`,
//! `MTU` and the rest), which wg passes over.
//!
//! The protocol is lines of `key=value`, a request or an answer each
//! ended by an empty line: `get=1` asks for an interface's settings and
//! its peers', which come back with `errno=0`; `set=1` and the settings
//! to change, in order, get `errno=0`, or the error number of the first
//! that failed.

use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use boringtun::x25519;

/// The configuration files wgd brings up, NAME.conf for interface NAME,
/// which only the system reads.
pub const STATE: &str = "/system/state/wireguard";

/// The folder of wgd's sockets, NAME.sock for interface NAME.
pub const LIVE: &str = "/system/live/wireguard";

/// A tunnel's MTU: the kernel's, which leaves room in a 1500-byte frame
/// for WireGuard's header, UDP's and IPv6's (ADR 0339).
pub const MTU: u32 = 1420;

/// The longest interface name, as Linux's IFNAMSIZ less its 0.
pub const NAME_MAX: usize = 15;

/// The socket of interface `name`.
pub fn socket_path(name: &str) -> PathBuf {
    PathBuf::from(LIVE).join(format!("{name}.sock"))
}

/// Whether `name` can be an interface's: 1 to 15 letters, digits, `_`,
/// `=`, `+`, `.` and `-`, as wg-quick allows.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= NAME_MAX
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"_=+.-".contains(&b))
}

/// A WireGuard key: a private key, a public one, or a preshared one.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key(pub [u8; 32]);

impl std::fmt::Debug for Key {
    // Never a private key in a message.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(..)")
    }
}

impl Key {
    /// A key in base64, as wg writes them: 44 characters, the last `=`.
    pub fn from_base64(text: &str) -> Option<Key> {
        let text = text.trim();
        if text.len() != 44 || !text.ends_with('=') {
            return None;
        }
        let bytes = STANDARD.decode(text).ok()?;
        Some(Key(bytes.try_into().ok()?))
    }

    pub fn base64(&self) -> String {
        STANDARD.encode(self.0)
    }

    /// A key in hexadecimal, as the protocol writes them.
    pub fn from_hex(text: &str) -> Option<Key> {
        let text = text.as_bytes();
        if text.len() != 64 {
            return None;
        }
        let mut key = [0u8; 32];
        for (at, byte) in key.iter_mut().enumerate() {
            let pair = std::str::from_utf8(&text[at * 2..at * 2 + 2]).ok()?;
            *byte = u8::from_str_radix(pair, 16).ok()?;
        }
        Some(Key(key))
    }

    pub fn hex(&self) -> String {
        self.0.iter().fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
    }

    /// Whether it is all zeros: no key, in the protocol.
    pub fn is_zero(&self) -> bool {
        self.0 == [0; 32]
    }

    /// The public key of this private key.
    pub fn public(&self) -> Key {
        Key(x25519::PublicKey::from(&x25519::StaticSecret::from(self.0)).to_bytes())
    }

    /// 32 random bytes, from the kernel's generator.
    ///
    /// # Errors
    ///
    /// If the generator fails.
    pub fn random() -> io::Result<Key> {
        let mut key = [0u8; 32];
        ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut key)
            .map_err(|_| io::Error::other("no random numbers"))?;
        Ok(Key(key))
    }

    /// A new private key, clamped as Curve25519's are (RFC 7748), as
    /// `wg genkey` makes them.
    ///
    /// # Errors
    ///
    /// If the generator fails.
    pub fn generate() -> io::Result<Key> {
        let mut key = Key::random()?;
        key.0[0] &= 0b1111_1000;
        key.0[31] &= 0b0111_1111;
        key.0[31] |= 0b0100_0000;
        Ok(key)
    }
}

/// An address and its prefix length, as AllowedIPs and Address give them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Prefix {
    pub ip: IpAddr,
    pub len: u8,
}

impl Prefix {
    /// `10.0.0.0/24`, `fd00::/64`, or an address alone, its own /32 or
    /// /128.
    pub fn parse(text: &str) -> Option<Prefix> {
        let text = text.trim();
        let (ip, len) = match text.split_once('/') {
            Some((ip, len)) => (ip.parse::<IpAddr>().ok()?, Some(len.parse::<u8>().ok()?)),
            None => (text.parse().ok()?, None),
        };
        let max = if ip.is_ipv4() { 32 } else { 128 };
        let len = len.unwrap_or(max);
        (len <= max).then_some(Prefix { ip, len })
    }

    /// Its network: the address with the bits after the prefix cleared,
    /// as the kernel's allowed IPs keep them.
    pub fn network(self) -> Prefix {
        let ip = match self.ip {
            IpAddr::V4(ip) => {
                let mask = u32::MAX.checked_shl(32 - u32::from(self.len)).unwrap_or(0);
                IpAddr::V4(Ipv4Addr::from(u32::from(ip) & mask))
            }
            IpAddr::V6(ip) => {
                let mask = u128::MAX.checked_shl(128 - u32::from(self.len)).unwrap_or(0);
                IpAddr::V6(Ipv6Addr::from(u128::from(ip) & mask))
            }
        };
        Prefix { ip, len: self.len }
    }

    /// Whether `ip` is in it.
    pub fn contains(self, ip: IpAddr) -> bool {
        match (self.ip, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = u32::MAX.checked_shl(32 - u32::from(self.len)).unwrap_or(0);
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = u128::MAX.checked_shl(128 - u32::from(self.len)).unwrap_or(0);
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

impl std::fmt::Display for Prefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.ip, self.len)
    }
}

/// An interface's settings, as a configuration file gives them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub private_key: Option<Key>,
    pub listen_port: Option<u16>,
    pub fwmark: Option<u32>,
    /// wg-quick's `Address`es, which wgd gives the interface.
    pub addresses: Vec<Prefix>,
    pub peers: Vec<PeerConfig>,
}

/// A peer's settings, as a configuration file gives them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerConfig {
    pub public_key: Key,
    pub preshared_key: Option<Key>,
    pub allowed_ips: Vec<Prefix>,
    /// As written: a name is looked up when the settings are used.
    pub endpoint: Option<String>,
    pub keepalive: Option<u16>,
}

/// The settings wg-quick reads that wgd does not: what to say of each.
const UNUSED: &[(&str, &str)] = &[
    ("dns", "DNS is not supported yet: set the DNS servers in network(5)"),
    ("table", "Table is not supported: the kernel routes by the interface's subnet (ADR 0339)"),
    ("preup", "PreUp is not run"),
    ("postup", "PostUp is not run"),
    ("predown", "PreDown is not run"),
    ("postdown", "PostDown is not run"),
    ("saveconfig", "SaveConfig is not supported"),
];

/// Reads a configuration file's text: its settings, and through `warn`
/// what it passes over.
///
/// # Errors
///
/// What is wrong, and on which line.
pub fn parse_config(text: &str, warn: &mut dyn FnMut(String)) -> Result<Config, String> {
    let mut config = Config::default();
    let mut interface = false;
    // The peer being read, and the line of its [Peer].
    let mut peer: Option<(usize, PeerConfig)> = None;
    for (number, line) in text.lines().enumerate() {
        let number = number + 1;
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let at = |what: String| format!("line {number}: {what}");
        if line.starts_with('[') {
            finish(&mut peer, &mut config)?;
            match line.to_ascii_lowercase().as_str() {
                "[interface]" => interface = true,
                "[peer]" => {
                    interface = false;
                    peer = Some((number, PeerConfig::new(Key([0; 32]))));
                }
                _ => return Err(at(format!("{line} is not a section: [Interface] or [Peer]"))),
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(at(format!("'{line}' is not a 'Key = value' line")));
        };
        let (name, value) = (key.trim().to_ascii_lowercase(), value.trim());
        let bad = |what: &str| at(format!("{} is not {what}: '{value}'", key.trim()));
        if let Some((_, peer)) = &mut peer {
            match name.as_str() {
                "publickey" => {
                    if !peer.public_key.is_zero() {
                        return Err(at("a second PublicKey in one [Peer]".into()));
                    }
                    peer.public_key = Key::from_base64(value).ok_or(bad("a key"))?;
                }
                "presharedkey" => {
                    peer.preshared_key = Some(Key::from_base64(value).ok_or(bad("a key"))?);
                }
                "allowedips" => {
                    for prefix in value.split(',').filter(|p| !p.trim().is_empty()) {
                        let prefix = Prefix::parse(prefix).ok_or(bad("a list of addresses"))?;
                        peer.allowed_ips.push(prefix.network());
                    }
                }
                "endpoint" => {
                    split_endpoint(value).ok_or(bad("HOST:PORT"))?;
                    peer.endpoint = Some(value.to_string());
                }
                "persistentkeepalive" => {
                    peer.keepalive = match value {
                        "off" | "0" => None,
                        _ => Some(value.parse().map_err(|_| bad("a number of seconds"))?),
                    };
                }
                _ => return Err(at(format!("{} is not a peer's setting", key.trim()))),
            }
            continue;
        }
        if !interface {
            return Err(at("a setting before [Interface] or [Peer]".into()));
        }
        match name.as_str() {
            "privatekey" => config.private_key = Some(Key::from_base64(value).ok_or(bad("a key"))?),
            "listenport" => config.listen_port = Some(value.parse().map_err(|_| bad("a port"))?),
            "fwmark" => {
                config.fwmark = match value {
                    "off" | "0" => None,
                    _ => Some(parse_number(value).ok_or(bad("a number"))?),
                };
            }
            "address" => {
                for address in value.split(',') {
                    config.addresses.push(Prefix::parse(address).ok_or(bad("an address"))?);
                }
            }
            "mtu" => {
                let mtu: u32 = value.parse().map_err(|_| bad("a number"))?;
                if mtu != MTU {
                    warn(at(format!("the MTU is {MTU}, whatever MTU says")));
                }
            }
            _ => match UNUSED.iter().find(|(unused, _)| *unused == name) {
                Some((_, why)) => warn(at((*why).to_string())),
                None => return Err(at(format!("{} is not a setting", key.trim()))),
            },
        }
    }
    finish(&mut peer, &mut config)?;
    Ok(config)
}

/// Adds the peer read, if one was, to `config`.
fn finish(peer: &mut Option<(usize, PeerConfig)>, config: &mut Config) -> Result<(), String> {
    if let Some((line, peer)) = peer.take() {
        if peer.public_key.is_zero() {
            return Err(format!("line {line}: a [Peer] without its PublicKey"));
        }
        config.peers.push(peer);
    }
    Ok(())
}

impl PeerConfig {
    fn new(public_key: Key) -> PeerConfig {
        PeerConfig {
            public_key,
            preshared_key: None,
            allowed_ips: Vec::new(),
            endpoint: None,
            keepalive: None,
        }
    }
}

/// A number in decimal, or hexadecimal after `0x`, as wg reads fwmarks.
pub fn parse_number(text: &str) -> Option<u32> {
    match text.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
}

/// `HOST:PORT` or `[HOST]:PORT`: the host and the port.
pub fn split_endpoint(text: &str) -> Option<(&str, u16)> {
    let (host, port) = match text.strip_prefix('[') {
        Some(rest) => {
            let (host, port) = rest.split_once("]:")?;
            (host, port)
        }
        None => text.rsplit_once(':')?,
    };
    if host.is_empty() {
        return None;
    }
    Some((host, port.parse().ok()?))
}

/// The address of `endpoint`, looking its name up if it is one: the
/// first address it has.
///
/// # Errors
///
/// If it is not `HOST:PORT`, or the name has no address.
pub fn resolve_endpoint(endpoint: &str) -> Result<SocketAddr, String> {
    let (host, port) =
        split_endpoint(endpoint).ok_or_else(|| format!("{endpoint}: not HOST:PORT"))?;
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, port));
    }
    (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("{host}: {e}"))?
        .next()
        .ok_or_else(|| format!("{host}: no address"))
}

/// An endpoint as the protocol and wg write it: `1.2.3.4:51820`, or
/// `[fd00::1]:51820`, never an IPv4-mapped IPv6 address.
pub fn endpoint_text(endpoint: SocketAddr) -> String {
    SocketAddr::new(endpoint.ip().to_canonical(), endpoint.port()).to_string()
}

/// The protocol's `set=1` request that gives an interface `config`, a
/// file's: with `replace`, its peers become the file's alone, as `wg
/// setconf` makes them; without, the file's are added or changed, as
/// `wg addconf` does.
///
/// # Errors
///
/// If an endpoint's name has no address.
pub fn set_request(config: &Config, replace: bool) -> Result<String, String> {
    let mut text = String::from("set=1\n");
    if let Some(key) = config.private_key {
        let _ = writeln!(text, "private_key={}", key.hex());
    }
    if let Some(port) = config.listen_port {
        let _ = writeln!(text, "listen_port={port}");
    }
    if let Some(mark) = config.fwmark {
        let _ = writeln!(text, "fwmark={mark}");
    }
    if replace {
        text.push_str("replace_peers=true\n");
    }
    for peer in &config.peers {
        peer_lines(&mut text, peer)?;
    }
    text.push('\n');
    Ok(text)
}

/// The lines of a `set=1` request that make a peer `peer`.
///
/// # Errors
///
/// If its endpoint's name has no address.
pub fn peer_lines(text: &mut String, peer: &PeerConfig) -> Result<(), String> {
    let _ = writeln!(text, "public_key={}", peer.public_key.hex());
    let preshared = peer.preshared_key.unwrap_or(Key([0; 32]));
    let _ = writeln!(text, "preshared_key={}", preshared.hex());
    if let Some(endpoint) = &peer.endpoint {
        let _ = writeln!(text, "endpoint={}", endpoint_text(resolve_endpoint(endpoint)?));
    }
    let _ = writeln!(text, "persistent_keepalive_interval={}", peer.keepalive.unwrap_or(0));
    text.push_str("replace_allowed_ips=true\n");
    for prefix in &peer.allowed_ips {
        let _ = writeln!(text, "allowed_ip={prefix}");
    }
    Ok(())
}

/// An interface as wgd says it is (`get=1`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Device {
    pub private_key: Option<Key>,
    pub listen_port: u16,
    pub fwmark: u32,
    pub peers: Vec<Peer>,
}

/// A peer as wgd says it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Peer {
    pub public_key: Key,
    pub preshared_key: Option<Key>,
    pub endpoint: Option<SocketAddr>,
    pub allowed_ips: Vec<Prefix>,
    /// Seconds and nanoseconds since 1970 of its latest handshake, or 0.
    pub last_handshake: (u64, u32),
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub keepalive: u16,
}

/// Reads the answer to `get=1`.
///
/// # Errors
///
/// The error number it ended with, or what in it could not be read.
pub fn parse_device(text: &str) -> Result<Device, String> {
    let mut device = Device::default();
    for line in text.lines() {
        if line.is_empty() {
            break;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(format!("wgd said '{line}'"));
        };
        let bad = || format!("wgd said '{line}'");
        let number = || value.parse::<u64>().map_err(|_| bad());
        let peer = device.peers.last_mut();
        match (key, peer) {
            ("private_key", _) => {
                device.private_key = Key::from_hex(value).filter(|k| !k.is_zero());
            }
            ("listen_port", _) => device.listen_port = value.parse().map_err(|_| bad())?,
            ("fwmark", _) => device.fwmark = value.parse().map_err(|_| bad())?,
            ("public_key", _) => device.peers.push(Peer {
                public_key: Key::from_hex(value).ok_or_else(bad)?,
                preshared_key: None,
                endpoint: None,
                allowed_ips: Vec::new(),
                last_handshake: (0, 0),
                rx_bytes: 0,
                tx_bytes: 0,
                keepalive: 0,
            }),
            ("preshared_key", Some(peer)) => {
                peer.preshared_key = Key::from_hex(value).filter(|k| !k.is_zero());
            }
            ("endpoint", Some(peer)) => peer.endpoint = Some(value.parse().map_err(|_| bad())?),
            ("allowed_ip", Some(peer)) => {
                peer.allowed_ips.push(Prefix::parse(value).ok_or_else(bad)?);
            }
            ("last_handshake_time_sec", Some(peer)) => peer.last_handshake.0 = number()?,
            ("last_handshake_time_nsec", Some(peer)) => {
                peer.last_handshake.1 = value.parse().map_err(|_| bad())?;
            }
            ("rx_bytes", Some(peer)) => peer.rx_bytes = number()?,
            ("tx_bytes", Some(peer)) => peer.tx_bytes = number()?,
            ("persistent_keepalive_interval", Some(peer)) => {
                peer.keepalive = value.parse().map_err(|_| bad())?;
            }
            ("protocol_version", Some(_)) => {}
            ("errno", _) => {
                if value != "0" {
                    return Err(errno_text(value));
                }
            }
            _ => return Err(bad()),
        }
    }
    Ok(device)
}

/// What an `errno=` answer means, in the C library's words.
pub fn errno_text(value: &str) -> String {
    match value.parse::<i32>() {
        Ok(errno) => strip(&io::Error::from_raw_os_error(errno)),
        Err(_) => format!("wgd said errno={value}"),
    }
}

fn strip(error: &io::Error) -> String {
    crate::util::strip_os_error(error)
}

/// How long wg waits for wgd's answer.
const ANSWER_MAX: Duration = Duration::from_secs(10);

/// Asks wgd, on interface `name`'s socket, `request`: its answer, up to
/// the empty line that ends it.
///
/// # Errors
///
/// If there is no such interface, or wgd does not answer.
pub fn ask(name: &str, request: &str) -> io::Result<String> {
    let path = socket_path(name);
    let mut stream = UnixStream::connect(&path).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => io::Error::new(
            e.kind(),
            format!("Unable to access interface {name}: No such device (is wgd running?)"),
        ),
        _ => e,
    })?;
    stream.write_all(request.as_bytes())?;
    let mut answer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if !readable(&stream, ANSWER_MAX)? {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "wgd did not answer"));
        }
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        answer.extend_from_slice(&chunk[..read]);
        if answer.ends_with(b"\n\n") {
            break;
        }
    }
    String::from_utf8(answer).map_err(|_| io::Error::other("wgd's answer is not text"))
}

/// Waits up to `limit` for `stream` to be readable: whether it is. Radix's
/// Unix sockets have no SO_RCVTIMEO.
pub fn readable(stream: &impl std::os::fd::AsRawFd, limit: Duration) -> io::Result<bool> {
    let mut fd = libc::pollfd { fd: stream.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    let ms = i32::try_from(limit.as_millis()).unwrap_or(i32::MAX);
    loop {
        // SAFETY: one pollfd, of a descriptor `stream` owns.
        let ready = unsafe { libc::poll(&raw mut fd, 1, ms) };
        if ready >= 0 {
            return Ok(ready > 0);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// The interfaces wgd runs: those with a socket in [`LIVE`], by name.
pub fn interfaces() -> Vec<String> {
    let Ok(entries) = fs::read_dir(LIVE) else { return Vec::new() };
    let mut names: Vec<String> = entries
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            let name = name.strip_suffix(".sock")?.to_string();
            valid_name(&name).then_some(name)
        })
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7748's Alice: her private key and her public one.
    const ALICE: &str = "77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a";
    const ALICE_PUBLIC: &str = "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a";

    #[test]
    fn a_public_key_is_curve25519s_of_the_private_one() {
        let private = Key::from_hex(ALICE).unwrap();
        assert_eq!(private.public().hex(), ALICE_PUBLIC);
        let text = private.base64();
        assert_eq!(text.len(), 44);
        assert_eq!(Key::from_base64(&text), Some(private));
        assert_eq!(Key::from_base64("short="), None);
        assert_eq!(Key::from_hex(&ALICE[2..]), None);
    }

    #[test]
    fn a_generated_key_is_clamped() {
        let key = Key::generate().unwrap();
        assert_eq!(key.0[0] & 7, 0);
        assert_eq!(key.0[31] & 0xc0, 0x40);
    }

    #[test]
    fn prefixes_are_read_and_matched() {
        let net = Prefix::parse("10.9.0.7/24").unwrap();
        assert_eq!(net.network().to_string(), "10.9.0.0/24");
        assert!(net.contains("10.9.0.200".parse().unwrap()));
        assert!(!net.contains("10.9.1.1".parse().unwrap()));
        assert!(!net.contains("::1".parse().unwrap()));
        assert_eq!(Prefix::parse("fd09::2").unwrap().to_string(), "fd09::2/128");
        assert_eq!(Prefix::parse("0.0.0.0/0").unwrap().network().to_string(), "0.0.0.0/0");
        assert!(Prefix::parse("::/0").unwrap().contains("2001:db8::1".parse().unwrap()));
        assert_eq!(Prefix::parse("10.0.0.1/33"), None);
    }

    #[test]
    fn a_wg_quick_file_is_read_with_what_it_passes_over() {
        let private = Key::from_hex(ALICE).unwrap();
        let public = private.public();
        let text = format!(
            "# wg0\n[Interface]\nPrivateKey = {}\nListenPort = 51820\nAddress = 10.9.0.1/24, \
             fd09::1/64\nDNS = 1.1.1.1\nMTU = 1380\n\n[Peer]\nPublicKey = {}\nAllowedIPs = \
             10.9.0.2/32, fd09::2/128\nEndpoint = [fd00::2]:51821\nPersistentKeepalive = 25\n",
            private.base64(),
            public.base64()
        );
        let mut warnings = Vec::new();
        let config = parse_config(&text, &mut |w| warnings.push(w)).unwrap();
        assert_eq!(config.private_key, Some(private));
        assert_eq!(config.listen_port, Some(51820));
        assert_eq!(config.addresses.len(), 2);
        assert_eq!(config.peers.len(), 1);
        let peer = &config.peers[0];
        assert_eq!(peer.public_key, public);
        assert_eq!(peer.allowed_ips.len(), 2);
        assert_eq!(peer.endpoint.as_deref(), Some("[fd00::2]:51821"));
        assert_eq!(peer.keepalive, Some(25));
        assert_eq!(warnings.len(), 2, "DNS and MTU: {warnings:?}");
        let request = set_request(&config, true).unwrap();
        assert!(request.starts_with("set=1\nprivate_key="));
        assert!(request.contains("replace_peers=true\n"));
        assert!(request.contains("endpoint=[fd00::2]:51821\n"));
        assert!(request.contains("allowed_ip=fd09::2/128\n"));
        assert!(request.ends_with("\n\n"));
    }

    #[test]
    fn a_file_with_a_mistake_says_where() {
        let mut ignore = |_| {};
        let error = parse_config("[Interface]\nListenPort = x\n", &mut ignore).unwrap_err();
        assert!(error.starts_with("line 2:"), "{error}");
        let error = parse_config("[Peer]\nAllowedIPs = 10.0.0.0/8\n", &mut ignore).unwrap_err();
        assert!(error.contains("without its PublicKey"), "{error}");
        let error = parse_config("[Interface]\nColour = red\n", &mut ignore).unwrap_err();
        assert!(error.contains("not a setting"), "{error}");
    }

    #[test]
    fn endpoints_are_split_and_written() {
        assert_eq!(split_endpoint("vpn.example:51820"), Some(("vpn.example", 51820)));
        assert_eq!(split_endpoint("[fd00::1]:7"), Some(("fd00::1", 7)));
        assert_eq!(split_endpoint("10.0.0.1"), None);
        let mapped: SocketAddr = "[::ffff:10.0.0.1]:51820".parse().unwrap();
        assert_eq!(endpoint_text(mapped), "10.0.0.1:51820");
    }

    #[test]
    fn a_get_answer_is_read() {
        let key = Key::from_hex(ALICE).unwrap();
        let text = format!(
            "private_key={}\nlisten_port=51820\npublic_key={}\npreshared_key={}\n\
             protocol_version=1\nendpoint=10.0.0.2:51820\nlast_handshake_time_sec=5\n\
             last_handshake_time_nsec=6\ntx_bytes=7\nrx_bytes=8\n\
             persistent_keepalive_interval=25\nallowed_ip=10.9.0.2/32\nerrno=0\n\n",
            key.hex(),
            key.public().hex(),
            Key([0; 32]).hex()
        );
        let device = parse_device(&text).unwrap();
        assert_eq!(device.private_key, Some(key));
        assert_eq!(device.listen_port, 51820);
        let peer = &device.peers[0];
        assert_eq!(peer.preshared_key, None);
        assert_eq!(peer.last_handshake, (5, 6));
        assert_eq!((peer.tx_bytes, peer.rx_bytes, peer.keepalive), (7, 8, 25));
        assert_eq!(peer.allowed_ips[0].to_string(), "10.9.0.2/32");
        assert!(parse_device("errno=22\n\n").unwrap_err().contains("Invalid argument"));
    }
}
