// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! netd(8): sets the network cards up (ADR 0191).
//!
//! Each card gets its address, router and DNS servers by DHCP (the
//! client is [`crate::dhcp`]), unless `/system/settings/network`
//! (network(5)) gives it addresses of its own or says to leave it alone.
//! netd sets a card up through radix-network(2)'s `configure`, as ip(8)
//! does, and changes only the addresses it gave: those an administrator
//! added stay. It keeps each lease in `/system/state/netd`, so that a
//! card asks for the same address after a reboot and netd, started
//! again, goes on with the lease the card has; and it writes the DNS
//! servers into `/system/live/netd/resolv.conf`, which resolved(8) asks
//! when its settings name none (ADR 0193).
//!
//! A DHCP card has a socket of its own on port 68, bound to it, which the
//! kernel lets the sockets of other cards share. netd looks at the cards
//! every few seconds, for cards that come and links that come back, and
//! otherwise sleeps until a client has something to send. SIGHUP reads
//! the settings again; SIGTERM ends netd and leaves the cards as they
//! are, so that connections go on.
//!
//! It turns each card's IPv6 on (ADR 0331) unless the settings say no:
//! the kernel makes its addresses from the prefixes routers advertise,
//! stable ones from the machine's secret, which netd makes once and
//! keeps in `/system/state/netd/ipv6-secret`, and temporary ones to send
//! from unless the settings say no. When its routers say that DHCPv6
//! gives addresses or other settings, netd asks for them too (the client
//! is [`crate::dhcp6`], ADR 0335), on a socket of its own on port 546,
//! and gives the card the address it gets as ip(8) would. The DNS servers
//! DHCPv6 gives and routers advertise go into resolv.conf after DHCP's.
//!
//! Bridges and VLANs that ip(8) makes (ADR 0333) are set up as cards are,
//! by their names; a bridge's port is not, while it is one: its bridge
//! has the addresses.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6, UdpSocket};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use radix_abi::network::{Ipv6Flags, Ipv6Settings};

use crate::args::Args;
use crate::dhcp::{self, Action, Client, Lease};
use crate::dhcp6;
use crate::ip::{self, Cidr, Cidr6, Interface, Network};
use crate::util::{self, Fail};

/// The settings file, the leases' directory and the resolver's file.
pub const SETTINGS: &str = "/system/settings/network";
const LEASES: &str = "/system/state/netd";
const RESOLV: &str = "/system/live/netd/resolv.conf";
/// The machine's IPv6 secret, which only netd reads.
const SECRET: &str = "/system/state/netd/ipv6-secret";

/// How often netd looks at the cards.
const SCAN: Duration = Duration::from_secs(5);

/// The most addresses and DNS servers settings give a card, and the most
/// DNS servers resolv.conf takes.
const ADDRESSES_MAX: usize = 4;
const DNS_MAX: usize = 3;

/// The ports of DHCP.
const CLIENT_PORT: u16 = 68;
const SERVER_PORT: u16 = 67;

/// How the settings file has netd set a card up.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Setup {
    /// Whether it asks DHCP: unless it says no, or gives addresses of
    /// its own and does not say yes.
    pub dhcp: Option<bool>,
    pub addresses: Vec<Cidr>,
    pub router: Option<Ipv4Addr>,
    pub dns: Vec<IpAddr>,
    /// Whether it has IPv6, and makes temporary addresses: unless they
    /// say no.
    pub ipv6: Option<bool>,
    pub temporary: Option<bool>,
    /// Its IPv6 addresses and router of its own, besides those it makes.
    pub addresses6: Vec<Cidr6>,
    pub router6: Option<Ipv6Addr>,
    /// When it asks DHCPv6: as its routers say, unless this says.
    pub dhcpv6: Option<Dhcpv6>,
}

/// When a card asks DHCPv6, the `dhcpv6` setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dhcpv6 {
    /// As its routers say: for an address and the settings when they say
    /// it gives addresses (M), for the settings when they say it gives
    /// those (O).
    #[default]
    Auto,
    /// Always, for the settings only.
    Settings,
    /// Always, for an address and the settings.
    Address,
    /// Never.
    No,
}

impl Dhcpv6 {
    /// Whether the card asks, and whether for an address, when its routers
    /// say `managed` and `other`.
    pub fn asks(self, managed: bool, other: bool) -> Option<bool> {
        match self {
            Dhcpv6::Auto => (managed || other).then_some(managed),
            Dhcpv6::Settings => Some(false),
            Dhcpv6::Address => Some(true),
            Dhcpv6::No => None,
        }
    }
}

impl Setup {
    fn dhcp(&self) -> bool {
        self.dhcp.unwrap_or(self.addresses.is_empty())
    }

    /// Whether netd leaves the card alone: no DHCP, and nothing of its
    /// own, of either version.
    fn alone(&self) -> bool {
        !self.dhcp()
            && self.addresses.is_empty()
            && self.addresses6.is_empty()
            && self.ipv6 != Some(true)
    }

    /// How its IPv6 is set: on, with temporary addresses, its own
    /// addresses and router.
    fn ipv6(&self) -> Ipv6Setup {
        Ipv6Setup {
            on: self.ipv6 != Some(false),
            temporary: self.temporary != Some(false),
            addresses: self.addresses6.clone(),
            router: self.router6,
            dhcpv6: self.dhcpv6.unwrap_or_default(),
        }
    }
}

/// How netd set a card's IPv6.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Ipv6Setup {
    on: bool,
    temporary: bool,
    addresses: Vec<Cidr6>,
    router: Option<Ipv6Addr>,
    dhcpv6: Dhcpv6,
}

/// Whether `ip` is an IPv6 address a host may have: not unspecified,
/// loopback, multicast or IPv4-mapped.
pub fn unicast6(ip: Ipv6Addr) -> bool {
    !ip.is_unspecified() && !ip.is_loopback() && !ip.is_multicast() && ip.to_ipv4_mapped().is_none()
}

/// The settings file: how each card it names is set up, by its name or
/// its MAC address, and what is wrong in it, a line each.
pub fn parse_settings(text: &str) -> (Vec<(String, Setup)>, Vec<String>) {
    let mut cards: Vec<(String, Setup)> = Vec::new();
    let mut wrong = Vec::new();
    for (at, line) in text.lines().enumerate() {
        let at = at + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            let name = name.trim().to_ascii_lowercase();
            if name.is_empty() || cards.iter().any(|(card, _)| *card == name) {
                wrong.push(format!(
                    "{SETTINGS}:{at}: '[{name}]' names no card, or one named before"
                ));
                // What follows, up to the next card, goes nowhere.
                cards.push((String::new(), Setup::default()));
            } else {
                cards.push((name, Setup::default()));
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            wrong.push(format!("{SETTINGS}:{at}: not `setting = value` or `[card]`"));
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        let Some((_, setup)) = cards.last_mut() else {
            wrong.push(format!("{SETTINGS}:{at}: {key} before any `[card]`"));
            continue;
        };
        let address = || value.parse::<Ipv4Addr>().ok().filter(|&ip| dhcp::unicast(ip));
        let address6 = || value.parse::<Ipv6Addr>().ok().filter(|&ip| unicast6(ip));
        let v6 = value.contains(':');
        let yes_no = |flag: &mut Option<bool>| match value {
            "yes" | "no" if flag.is_none() => {
                *flag = Some(value == "yes");
                true
            }
            _ => false,
        };
        let ok = match key {
            "dhcp" => yes_no(&mut setup.dhcp),
            "ipv6" => yes_no(&mut setup.ipv6),
            "temporary-addresses" => yes_no(&mut setup.temporary),
            "dhcpv6" => {
                let mode = match value {
                    "auto" => Some(Dhcpv6::Auto),
                    "settings" => Some(Dhcpv6::Settings),
                    "address" => Some(Dhcpv6::Address),
                    "no" => Some(Dhcpv6::No),
                    _ => None,
                };
                match mode {
                    Some(mode) if setup.dhcpv6.is_none() => {
                        setup.dhcpv6 = Some(mode);
                        true
                    }
                    _ => false,
                }
            }
            "address" if v6 => match Cidr6::parse(value) {
                Some(cidr)
                    if unicast6(cidr.ip)
                        && !ip::link_local(cidr.ip)
                        && cidr.prefix > 0
                        && setup.addresses6.len() < ADDRESSES_MAX =>
                {
                    setup.addresses6.push(cidr);
                    true
                }
                _ => false,
            },
            "address" => match Cidr::parse(value) {
                Some(cidr) if dhcp::unicast(cidr.ip) && setup.addresses.len() < ADDRESSES_MAX => {
                    setup.addresses.push(cidr);
                    true
                }
                _ => false,
            },
            "router" if v6 => match address6() {
                Some(router) if setup.router6.is_none() => {
                    setup.router6 = Some(router);
                    true
                }
                _ => false,
            },
            "router" => match address() {
                Some(router) if setup.router.is_none() => {
                    setup.router = Some(router);
                    true
                }
                _ => false,
            },
            "dns" => {
                match if v6 { address6().map(IpAddr::V6) } else { address().map(IpAddr::V4) } {
                    Some(dns) if setup.dns.len() < DNS_MAX => {
                        setup.dns.push(dns);
                        true
                    }
                    _ => false,
                }
            }
            _ => {
                wrong.push(format!("{SETTINGS}:{at}: no setting is called {key}"));
                continue;
            }
        };
        if !ok {
            wrong.push(format!("{SETTINGS}:{at}: '{value}' is not a {key} here"));
        }
    }
    for (name, setup) in &cards {
        if let Some(router) = setup.router
            && !setup.addresses.is_empty()
            && !setup.addresses.iter().any(|cidr| cidr.contains(router))
        {
            wrong
                .push(format!("{SETTINGS}: [{name}]'s router {router} is on none of its networks"));
        }
        // A link-local router is on every link; another must be on one of
        // the card's networks, if it names any.
        if let Some(router) = setup.router6
            && !ip::link_local(router)
            && !setup.addresses6.is_empty()
            && !setup.addresses6.iter().any(|cidr| cidr.contains(router))
        {
            wrong
                .push(format!("{SETTINGS}: [{name}]'s router {router} is on none of its networks"));
        }
    }
    cards.retain(|(name, _)| !name.is_empty());
    (cards, wrong)
}

/// A MAC address, `52:54:00:12:34:56`.
fn mac_of(text: &str) -> Option<[u8; 6]> {
    let mut mac = [0; 6];
    let mut parts = text.split(':');
    for byte in &mut mac {
        *byte = u8::from_str_radix(parts.next()?, 16).ok()?;
    }
    parts.next().is_none().then_some(mac)
}

/// A lease as netd keeps it, given at `obtained`, in seconds since 1970.
pub fn lease_text(lease: &Lease, card: &str, obtained: u64) -> String {
    let mut text = format!("# netd(8)'s lease for {card}.\naddress = {}\n", lease.address);
    if let Some(router) = lease.router {
        text += &format!("router = {router}\n");
    }
    for dns in &lease.dns {
        text += &format!("dns = {dns}\n");
    }
    if let Some(domain) = &lease.domain {
        text += &format!("domain = {domain}\n");
    }
    text += &format!(
        "server = {}\nobtained = {obtained}\nseconds = {}\nrenew = {}\nrebind = {}\n",
        lease.server, lease.seconds, lease.renew, lease.rebind
    );
    if lease.metered {
        text += "metered = yes\n";
    }
    text
}

/// The lease a lease file holds, and when it was given, if it is whole.
pub fn parse_lease(text: &str) -> Option<(Lease, u64)> {
    let mut fields: HashMap<&str, Vec<&str>> = HashMap::new();
    for line in text.lines().filter(|line| !line.starts_with('#')) {
        let (key, value) = line.split_once('=')?;
        fields.entry(key.trim()).or_default().push(value.trim());
    }
    let one = |key: &str| fields.get(key).and_then(|values| values.first().copied());
    let number = |key: &str| one(key)?.parse::<u32>().ok();
    let address = Cidr::parse(one("address")?)?;
    let dns = fields.get("dns").into_iter().flatten().filter_map(|dns| dns.parse().ok());
    let lease = Lease {
        address,
        router: one("router").and_then(|router| router.parse().ok()),
        dns: dns.collect(),
        domain: one("domain").map(str::to_string),
        server: one("server")?.parse().ok()?,
        seconds: number("seconds")?,
        renew: number("renew")?,
        rebind: number("rebind")?,
        metered: one("metered") == Some("yes"),
    };
    Some((lease, one("obtained")?.parse().ok()?))
}

/// The resolv.conf of `servers` and the domains `search`, at most
/// [`DNS_MAX`] servers.
pub fn resolv_text(servers: &[IpAddr], search: &[String]) -> String {
    let mut text = format!(
        "# Written by netd(8) from what DHCP and {SETTINGS} give.\n\
         # /etc/resolv.conf leads here; make it a file of your own for other servers.\n"
    );
    for server in servers.iter().take(DNS_MAX) {
        text += &format!("nameserver {server}\n");
    }
    if !search.is_empty() {
        text += &format!("search {}\n", search.join(" "));
    }
    text
}

/// The addresses a card has after netd gives it `ours` in place of
/// `before`, those it gave: ours first, then the others it has, at most
/// four.
pub fn merge(has: &[Cidr], before: &[Cidr], ours: &[Cidr]) -> Vec<Cidr> {
    let mut addresses = ours.to_vec();
    for cidr in has {
        if !before.contains(cidr) && !addresses.iter().any(|ours| ours.ip == cidr.ip) {
            addresses.push(*cidr);
        }
    }
    addresses.truncate(ADDRESSES_MAX);
    addresses
}

/// How netd sets a card up.
#[derive(Debug)]
enum How {
    /// By DHCP, through its own socket.
    Dhcp { socket: UdpSocket, client: Box<Client> },
    /// By DHCP once port 68 of the card is free: another socket held it.
    Waiting,
    /// With the settings' addresses.
    Fixed,
    /// Not at all.
    Alone,
}

/// A card netd knows.
#[derive(Debug)]
struct Card {
    name: String,
    index: u32,
    mac: [u8; 6],
    carrier: bool,
    how: How,
    /// What netd gave it: addresses, router, DNS servers and domain.
    given: Vec<Cidr>,
    router: Option<Ipv4Addr>,
    dns: Vec<IpAddr>,
    domain: Option<String>,
    /// How netd set its IPv6, if it did, and the DNS servers its routers
    /// advertise.
    ipv6: Option<Ipv6Setup>,
    dns6: Vec<Ipv6Addr>,
    /// Its DHCPv6 client, while its routers say to ask.
    dhcp6: Option<Dhcp6>,
    /// The last failure to send, so that each is said once.
    failure: Option<String>,
}

/// A card's DHCPv6 client, on its socket, and what it got.
#[derive(Debug)]
struct Dhcp6 {
    socket: UdpSocket,
    client: Box<dhcp6::Client>,
    /// The address netd gave the card from it, a /128.
    given: Option<Cidr6>,
    dns: Vec<Ipv6Addr>,
    domains: Vec<String>,
}

/// Set by SIGTERM: netd ends. Set by SIGHUP: it reads the settings
/// again.
static STOP: AtomicBool = AtomicBool::new(false);
static RELOAD: AtomicBool = AtomicBool::new(false);

extern "C" fn on_term(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

extern "C" fn on_hangup(_: libc::c_int) {
    RELOAD.store(true, Ordering::Relaxed);
}

/// The seconds since 1970.
fn wall() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

/// The secret a secret file holds: 32 hexadecimal digits, on a line
/// after the comments.
pub fn parse_secret(text: &str) -> Option<[u8; 16]> {
    let line =
        text.lines().map(str::trim).find(|line| !line.is_empty() && !line.starts_with('#'))?;
    if line.len() != 32 {
        return None;
    }
    let mut secret = [0u8; 16];
    for (at, byte) in secret.iter_mut().enumerate() {
        *byte = u8::from_str_radix(line.get(at * 2..at * 2 + 2)?, 16).ok()?;
    }
    Some(secret)
}

/// A secret file's text.
pub fn secret_text(secret: &[u8; 16]) -> String {
    let hex = secret.iter().fold(String::new(), |mut hex, byte| {
        let _ = std::fmt::Write::write_fmt(&mut hex, format_args!("{byte:02x}"));
        hex
    });
    format!(
        "# netd(8)'s IPv6 secret (RFC 7217): the stable addresses are made from it.\n\
         # Keep it private; a new one gives the machine new addresses.\n{hex}\n"
    )
}

/// The machine's IPv6 secret: the one netd keeps, or a new one, made at
/// random and kept, readable by netd alone.
fn load_secret() -> [u8; 16] {
    if let Some(secret) = fs::read_to_string(SECRET).ok().as_deref().and_then(parse_secret) {
        return secret;
    }
    let mut secret = [0u8; 16];
    // SAFETY: the buffer is valid for its length.
    let got = unsafe { libc::getrandom(secret.as_mut_ptr().cast(), secret.len(), 0) };
    if got != 16 {
        secret[..8].copy_from_slice(&seed().to_le_bytes());
        secret[8..].copy_from_slice(&seed().rotate_left(17).to_le_bytes());
    }
    let new = format!("{SECRET}.new");
    let written = fs::create_dir_all(LEASES)
        .and_then(|()| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&new)?;
            file.write_all(secret_text(&secret).as_bytes())?;
            file.sync_all()
        })
        .and_then(|()| fs::rename(&new, SECRET));
    if let Err(err) = written {
        eprintln!("netd: cannot keep the IPv6 secret in {SECRET}: {}", util::strip_os_error(&err));
    }
    secret
}

/// A number from the kernel's random source, else from the clock.
fn seed() -> u64 {
    let mut bytes = [0u8; 8];
    // SAFETY: the buffer is valid for its length.
    let got = unsafe { libc::getrandom(bytes.as_mut_ptr().cast(), bytes.len(), 0) };
    if got == 8 { u64::from_ne_bytes(bytes) } else { util::now_ns() }
}

/// A UDP socket on port 68 of the card named `name` that may broadcast,
/// which does not block.
fn client_socket(name: &str) -> io::Result<UdpSocket> {
    let check = |result: libc::c_int| {
        if result < 0 { Err(io::Error::last_os_error()) } else { Ok(result) }
    };
    // SAFETY: socket takes numbers; the descriptor is owned below.
    let fd =
        check(unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) })?;
    // SAFETY: fd is a socket of netd's own, which the UdpSocket closes.
    let socket = unsafe { UdpSocket::from_raw_fd(fd) };
    let on: libc::c_int = 1;
    // SAFETY: the option's values are valid for the lengths given.
    unsafe {
        check(libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            name.as_ptr().cast(),
            name.len() as libc::socklen_t,
        ))?;
        check(libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_BROADCAST,
            (&raw const on).cast(),
            size_of::<libc::c_int>() as libc::socklen_t,
        ))?;
    }
    // SAFETY: all zeros is a valid sockaddr_in: 0.0.0.0, port 0.
    let mut address: libc::sockaddr_in = unsafe { util::zeroed() };
    address.sin_family = libc::AF_INET as libc::sa_family_t;
    address.sin_port = CLIENT_PORT.to_be();
    // SAFETY: the address is valid for its length.
    check(unsafe {
        libc::bind(
            fd,
            (&raw const address).cast(),
            size_of::<libc::sockaddr_in>() as libc::socklen_t,
        )
    })?;
    socket.set_nonblocking(true)?;
    Ok(socket)
}

/// A UDP socket on IPv6 port 546 of the card named `name`, for DHCPv6,
/// which does not block.
fn client_socket6(name: &str) -> io::Result<UdpSocket> {
    let check = |result: libc::c_int| {
        if result < 0 { Err(io::Error::last_os_error()) } else { Ok(result) }
    };
    // SAFETY: socket takes numbers; the descriptor is owned below.
    let fd =
        check(unsafe { libc::socket(libc::AF_INET6, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) })?;
    // SAFETY: fd is a socket of netd's own, which the UdpSocket closes.
    let socket = unsafe { UdpSocket::from_raw_fd(fd) };
    let on: libc::c_int = 1;
    // SAFETY: the option's values are valid for the lengths given.
    unsafe {
        check(libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            name.as_ptr().cast(),
            name.len() as libc::socklen_t,
        ))?;
        check(libc::setsockopt(
            fd,
            libc::IPPROTO_IPV6,
            libc::IPV6_V6ONLY,
            (&raw const on).cast(),
            size_of::<libc::c_int>() as libc::socklen_t,
        ))?;
    }
    // SAFETY: all zeros is a valid sockaddr_in6: ::, port 0.
    let mut address: libc::sockaddr_in6 = unsafe { util::zeroed() };
    address.sin6_family = libc::AF_INET6 as libc::sa_family_t;
    address.sin6_port = dhcp6::CLIENT_PORT.to_be();
    // SAFETY: the address is valid for its length.
    check(unsafe {
        libc::bind(
            fd,
            (&raw const address).cast(),
            size_of::<libc::sockaddr_in6>() as libc::socklen_t,
        )
    })?;
    socket.set_nonblocking(true)?;
    Ok(socket)
}

/// netd's state.
struct Netd {
    cards: Vec<Card>,
    settings: Vec<(String, Setup)>,
    /// When it last looked at the cards.
    scanned: Option<Instant>,
    network: &'static dyn Network,
    /// The machine's IPv6 secret.
    secret: [u8; 16],
    /// How the cards' IPv6 was set before the settings were read again,
    /// and their DHCPv6 clients, by their numbers, so that what did not
    /// change stays as it is.
    ipv6_before: Vec<(u32, Ipv6Setup)>,
    dhcp6_before: Vec<(u32, Dhcp6)>,
}

impl Netd {
    /// How the settings have the card named `name`, with the address
    /// `mac`, set up.
    fn setup(&self, name: &str, mac: &str) -> Setup {
        let mac = mac.to_ascii_lowercase();
        self.settings
            .iter()
            .find(|(card, _)| *card == name || *card == mac)
            .map_or_else(Setup::default, |(_, setup)| setup.clone())
    }

    fn read_settings(&mut self) {
        let text = fs::read_to_string(SETTINGS).unwrap_or_default();
        let (settings, wrong) = parse_settings(&text);
        for line in wrong {
            eprintln!("netd: {line}");
        }
        self.settings = settings;
    }

    /// Looks at the cards: sets up those that came, forgets those that
    /// went, and has those whose link came back ask again.
    fn scan(&mut self, now: Instant) {
        let first = self.scanned.is_none();
        self.scanned = Some(now);
        let Ok(interfaces) = ip::read_interfaces(Path::new(ip::INTERFACES), Path::new(ip::ROUTES))
        else {
            return;
        };
        let known = self.cards.len();
        self.cards.retain(|card| {
            let found = interfaces.iter().find(|i| i.index == card.index && i.name == card.name);
            match found.map(|interface| &interface.master) {
                None => eprintln!("netd: {} went", card.name),
                // A bridge's port has no addresses: the bridge has them.
                Some(Some(bridge)) => eprintln!("netd: {} is a port of {bridge} now", card.name),
                Some(None) => return true,
            }
            false
        });
        let mut changed = self.cards.len() != known;
        // A tunnel's addresses are its program's to give, as wgd(8) gives
        // a WireGuard interface's (ADR 0340).
        let set_up = |interface: &&Interface| {
            !interface.loopback && interface.master.is_none() && interface.kind != ip::Kind::Tunnel
        };
        for interface in interfaces.iter().filter(set_up) {
            if let Some(card) = self.cards.iter_mut().find(|card| card.index == interface.index) {
                // The DNS servers its routers advertise now.
                let advertised: Vec<Ipv6Addr> =
                    interface.ipv6.dns.iter().map(|(dns, _)| *dns).collect();
                if card.ipv6.as_ref().is_some_and(|ipv6| ipv6.on) && card.dns6 != advertised {
                    card.dns6 = advertised;
                    changed = true;
                }
                changed |= Netd::follow_dhcp6(self.network, card, interface, now);
                let back = interface.carrier && !card.carrier;
                card.carrier = interface.carrier;
                if back && let How::Dhcp { client, .. } = &mut card.how {
                    eprintln!("netd: {}'s link is up again; asking for its address", card.name);
                    client.link_up(now);
                }
                if back && let Some(dhcp6) = &mut card.dhcp6 {
                    dhcp6.client.link_up(now);
                }
                if matches!(card.how, How::Waiting) {
                    Netd::ask(card, interface, now, false);
                }
                continue;
            }
            if let Some(mut card) = self.card(interface, now, first) {
                self.keep_dhcp6(&mut card);
                self.cards.push(card);
                changed = true;
            }
        }
        if changed {
            self.write_resolv();
        }
    }

    /// Sets a card that came up as the settings say: at netd's `first`
    /// look, one set up already is left as it is.
    fn card(&self, interface: &Interface, now: Instant, first: bool) -> Option<Card> {
        let mac = mac_of(&interface.mac)?;
        let setup = self.setup(&interface.name, &interface.mac);
        let mut card = Card {
            name: interface.name.clone(),
            index: interface.index,
            mac,
            carrier: interface.carrier,
            how: How::Alone,
            given: Vec::new(),
            router: None,
            dns: setup.dns.clone(),
            domain: None,
            ipv6: None,
            dns6: Vec::new(),
            dhcp6: None,
            failure: None,
        };
        if setup.alone() {
            eprintln!("netd: {} is left as it is, as {SETTINGS} says", card.name);
            return Some(card);
        }
        self.set_ipv6(&mut card, &setup);
        if !setup.addresses.is_empty() {
            self.give(&mut card, interface, &setup.addresses, setup.router);
            card.how = How::Fixed;
        }
        if !setup.dhcp() {
            return Some(card);
        }
        if first
            && setup.addresses.is_empty()
            && !interface.addresses.is_empty()
            && read_lease(card.mac).is_none()
        {
            // Set up already: by the kernel's ip= flag, or by hand.
            let addresses: Vec<String> = interface.addresses.iter().map(Cidr::to_string).collect();
            eprintln!(
                "netd: {} has {} already; it is left as it is",
                card.name,
                addresses.join(", ")
            );
            return Some(card);
        }
        Netd::ask(&mut card, interface, now, true);
        Some(card)
    }

    /// Gives `card` a DHCP client on a socket of its own on port 68, going
    /// on with the lease it keeps; while another socket holds the port,
    /// the card waits for it, which is said once, when `say`.
    fn ask(card: &mut Card, interface: &Interface, now: Instant, say: bool) {
        let socket = match client_socket(&card.name) {
            Ok(socket) => socket,
            Err(err) => {
                if say {
                    eprintln!(
                        "netd: {}: no socket on port {CLIENT_PORT}, for now: {err}",
                        card.name
                    );
                }
                card.how = How::Waiting;
                return;
            }
        };
        let client = match read_lease(card.mac) {
            Some((lease, obtained)) => {
                let ago = Duration::from_secs(wall().saturating_sub(obtained));
                let given = now.checked_sub(ago).unwrap_or(now);
                let applied = interface.addresses.contains(&lease.address);
                if applied {
                    card.given = vec![lease.address];
                    card.router = lease.router;
                    card.dns.extend(lease.dns.iter().map(|&dns| IpAddr::V4(dns)));
                    card.domain.clone_from(&lease.domain);
                }
                let client = Client::with_lease(card.mac, seed(), now, lease, given, applied);
                match (client.lease(), client.phase()) {
                    (Some(lease), _) => eprintln!(
                        "netd: {} goes on with its lease of {} from {}",
                        card.name, lease.address, lease.server
                    ),
                    (None, dhcp::Phase::Rebooting) => {
                        eprintln!("netd: {}: asking for the address it had", card.name);
                    }
                    (None, _) => forget_lease(card.mac),
                }
                client
            }
            None => {
                eprintln!("netd: {}: asking for an address by DHCP", card.name);
                Client::new(card.mac, seed(), now)
            }
        };
        card.how = How::Dhcp { socket, client: Box::new(client) };
    }

    /// Gives `card` the addresses `ours` and `router` in place of those
    /// netd gave it, keeping others' addresses: true if the kernel took
    /// them.
    fn give(
        &self,
        card: &mut Card,
        interface: &Interface,
        ours: &[Cidr],
        router: Option<Ipv4Addr>,
    ) -> bool {
        let addresses = merge(&interface.addresses, &card.given, ours);
        // Another's router stays while it is on one of the networks.
        let kept = interface.router.filter(|&router| {
            Some(router) != card.router && addresses.iter().any(|cidr| cidr.contains(router))
        });
        match self.network.configure(card.index, &addresses, router.or(kept)) {
            Ok(()) => {
                card.given = ours.to_vec();
                card.router = router;
                true
            }
            Err(status) => {
                eprintln!(
                    "netd: {}: the kernel refused its addresses: {}",
                    card.name,
                    crate::sys::status_name(status.0)
                );
                false
            }
        }
    }

    /// Sets `card`'s IPv6 as `setup` says, with the machine's secret,
    /// unless it was set so before the settings were read again: the
    /// kernel makes its addresses afresh.
    fn set_ipv6(&self, card: &mut Card, setup: &Setup) {
        let want = setup.ipv6();
        if let Some((_, before)) = self.ipv6_before.iter().find(|(index, _)| *index == card.index)
            && *before == want
        {
            card.ipv6 = Some(want);
            return;
        }
        let mut flags = Ipv6Flags::NONE.bits();
        if want.on {
            flags |= Ipv6Flags::ENABLED.bits() | Ipv6Flags::AUTOCONF.bits();
            if want.temporary {
                flags |= Ipv6Flags::TEMPORARY.bits();
            }
        }
        let settings = Ipv6Settings { flags, reserved: 0, secret: self.secret };
        let (addresses, router) =
            if want.on { (&want.addresses[..], want.router) } else { (&[][..], None) };
        match self.network.configure_ipv6(card.index, Some(&settings), addresses, router) {
            Ok(()) => card.ipv6 = Some(want),
            Err(status) => eprintln!(
                "netd: {}: the kernel refused its IPv6 settings: {}",
                card.name,
                crate::sys::status_name(status.0)
            ),
        }
    }

    /// Gives a card that came back after the settings were read again the
    /// DHCPv6 client it had, if its IPv6 stayed as it was: the address
    /// it gave stays.
    fn keep_dhcp6(&mut self, card: &mut Card) {
        let Some(at) = self.dhcp6_before.iter().position(|(index, _)| *index == card.index) else {
            return;
        };
        let (_, dhcp6) = self.dhcp6_before.swap_remove(at);
        let same = self
            .ipv6_before
            .iter()
            .any(|(index, before)| *index == card.index && card.ipv6.as_ref() == Some(before));
        if same {
            card.dhcp6 = Some(dhcp6);
        }
    }

    /// Starts or stops `card`'s DHCPv6 client as its routers and its
    /// `dhcpv6` setting say, its IPv6 being on: one that asks for an
    /// address and the settings, or one that asks for the settings. True
    /// if what goes into resolv.conf changed.
    fn follow_dhcp6(
        network: &dyn Network,
        card: &mut Card,
        interface: &Interface,
        now: Instant,
    ) -> bool {
        let asks = card
            .ipv6
            .as_ref()
            .filter(|ipv6| ipv6.on)
            .and_then(|ipv6| ipv6.dhcpv6.asks(interface.ipv6.managed, interface.ipv6.other));
        if card.dhcp6.as_ref().is_some_and(|dhcp6| asks == Some(dhcp6.client.managed())) {
            return false;
        }
        let mut changed = false;
        if let Some(old) = card.dhcp6.take() {
            if old.given.is_some() {
                give6(network, &card.name, card.index, interface, old.given, None);
            }
            changed = !(old.dns.is_empty() && old.domains.is_empty());
        }
        let Some(managed) = asks else { return changed };
        match client_socket6(&card.name) {
            Ok(socket) => {
                let what = if managed { "an address and its settings" } else { "its settings" };
                eprintln!("netd: {}: asking DHCPv6 for {what}", card.name);
                card.dhcp6 = Some(Dhcp6 {
                    socket,
                    client: Box::new(dhcp6::Client::new(card.mac, managed, seed(), now)),
                    given: None,
                    dns: Vec::new(),
                    domains: Vec::new(),
                });
            }
            Err(err) => eprintln!(
                "netd: {}: no socket on port {}: {}",
                card.name,
                dhcp6::CLIENT_PORT,
                util::strip_os_error(&err)
            ),
        }
        changed
    }

    /// Does what card `at`'s DHCPv6 client asks.
    fn act6(&mut self, at: usize, actions: Vec<dhcp6::Action>) {
        for action in actions {
            let card = &mut self.cards[at];
            let Some(dhcp6) = &mut card.dhcp6 else { return };
            match action {
                dhcp6::Action::Send(message) => {
                    let to = SocketAddrV6::new(dhcp6::SERVERS, dhcp6::SERVER_PORT, 0, card.index);
                    let failure = dhcp6
                        .socket
                        .send_to(&message, to)
                        .err()
                        .map(|err| util::strip_os_error(&err));
                    if failure.is_some() && failure != card.failure {
                        eprintln!(
                            "netd: {}: cannot send to {}: {}",
                            card.name,
                            dhcp6::SERVERS,
                            failure.as_deref().unwrap_or("")
                        );
                    }
                    card.failure = failure;
                }
                dhcp6::Action::Bind(lease) => {
                    let ours = Cidr6 { ip: lease.address.ip, prefix: 128 };
                    let Some(interface) = Netd::interface(card.index) else { continue };
                    let had = interface.ipv6.given().contains(&ours);
                    if !(had && dhcp6.given == Some(ours))
                        && give6(
                            self.network,
                            &card.name,
                            card.index,
                            &interface,
                            dhcp6.given,
                            Some(ours),
                        )
                    {
                        let verb = if dhcp6.given == Some(ours) { "keeps" } else { "has" };
                        dhcp6.given = Some(ours);
                        eprintln!(
                            "netd: {} {verb} {} by DHCPv6 for {}",
                            card.name,
                            ours.ip,
                            lasting(lease.address.valid)
                        );
                    }
                }
                dhcp6::Action::Unbind => {
                    let Some(lost) = dhcp6.given.take() else { continue };
                    if let Some(interface) = Netd::interface(card.index) {
                        give6(self.network, &card.name, card.index, &interface, Some(lost), None);
                    }
                    eprintln!("netd: {} gave up {}; asking for another", card.name, lost.ip);
                }
                dhcp6::Action::Settings { dns, domains } => {
                    if dhcp6.dns == dns && dhcp6.domains == domains {
                        continue;
                    }
                    let servers: Vec<String> = dns.iter().map(Ipv6Addr::to_string).collect();
                    let servers =
                        if servers.is_empty() { "none".to_owned() } else { servers.join(" ") };
                    eprintln!("netd: {} has DNS {servers} by DHCPv6", card.name);
                    dhcp6.dns = dns;
                    dhcp6.domains = domains;
                    self.write_resolv();
                }
            }
        }
    }

    /// The card numbered `index`'s interface now.
    fn interface(index: u32) -> Option<Interface> {
        let interfaces =
            ip::read_interfaces(Path::new(ip::INTERFACES), Path::new(ip::ROUTES)).ok()?;
        interfaces.into_iter().find(|interface| interface.index == index)
    }

    /// Does what card `at`'s client asks.
    fn act(&mut self, at: usize, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Send { to, message } => self.send(at, to, &message),
                Action::Bind(lease) => self.bind(at, &lease),
                Action::Unbind => self.unbind(at),
            }
        }
    }

    fn send(&mut self, at: usize, to: Ipv4Addr, message: &[u8]) {
        let card = &mut self.cards[at];
        let How::Dhcp { socket, .. } = &card.how else { return };
        let failure = socket
            .send_to(message, SocketAddrV4::new(to, SERVER_PORT))
            .err()
            .map(|err| util::strip_os_error(&err));
        if failure.is_some() && failure != card.failure {
            eprintln!(
                "netd: {}: cannot send to {to}: {}",
                card.name,
                failure.as_deref().unwrap_or("")
            );
        }
        card.failure = failure;
    }

    fn bind(&mut self, at: usize, lease: &Lease) {
        let Some(interface) = Netd::interface(self.cards[at].index) else { return };
        let mut card = std::mem::replace(&mut self.cards[at], placeholder());
        let setup = self.setup(&card.name, &interface.mac);
        let changed = card.given.first() != Some(&lease.address)
            || card.router != lease.router
            || !interface.addresses.contains(&lease.address);
        let mut ours = vec![lease.address];
        ours.extend(setup.addresses.iter().filter(|cidr| cidr.ip != lease.address.ip));
        let name = format!("{} ({})", card.name, interface.mac);
        if let Err(err) = write_lease(card.mac, &lease_text(lease, &name, wall())) {
            eprintln!("netd: {}: cannot keep its lease: {err}", card.name);
        }
        if !changed || self.give(&mut card, &interface, &ours, lease.router) {
            let mut dns = setup.dns.clone();
            dns.extend(
                lease.dns.iter().map(|&dns| IpAddr::V4(dns)).filter(|dns| !setup.dns.contains(dns)),
            );
            card.dns = dns;
            card.domain.clone_from(&lease.domain);
            let router = lease.router.map_or(String::new(), |router| format!(", router {router}"));
            let dns: Vec<String> = lease.dns.iter().map(Ipv4Addr::to_string).collect();
            let dns =
                if dns.is_empty() { String::new() } else { format!(", DNS {}", dns.join(" ")) };
            let verb = if changed { "has" } else { "keeps" };
            eprintln!(
                "netd: {} {verb} {} from {} for {}{router}{dns}",
                card.name,
                lease.address,
                lease.server,
                lasting(lease.seconds),
            );
        }
        self.cards[at] = card;
        self.write_resolv();
    }

    fn unbind(&mut self, at: usize) {
        let mut card = std::mem::replace(&mut self.cards[at], placeholder());
        forget_lease(card.mac);
        if let Some(interface) = Netd::interface(card.index) {
            let setup = self.setup(&card.name, &interface.mac);
            let lost: Vec<String> = card.given.iter().map(Cidr::to_string).collect();
            if self.give(&mut card, &interface, &setup.addresses, setup.router) {
                eprintln!("netd: {} gave up {}; asking for another", card.name, lost.join(", "));
            }
            card.dns = setup.dns;
            card.domain = None;
        }
        self.cards[at] = card;
        self.write_resolv();
    }

    /// Writes the cards' DNS servers into resolv.conf, or takes it away
    /// when there are none.
    fn write_resolv(&self) {
        let mut servers: Vec<IpAddr> = Vec::new();
        let mut search: Vec<String> = Vec::new();
        // The settings' and DHCP's first, then DHCPv6's and those routers
        // advertise.
        let advertised = self.cards.iter().flat_map(|card| {
            let dhcp6 = card.dhcp6.iter().flat_map(|dhcp6| &dhcp6.dns);
            dhcp6.chain(&card.dns6).map(|&dns| IpAddr::V6(dns))
        });
        for dns in self.cards.iter().flat_map(|card| card.dns.iter().copied()).chain(advertised) {
            if !servers.contains(&dns) {
                servers.push(dns);
            }
        }
        for card in &self.cards {
            let dhcp6 = card.dhcp6.iter().flat_map(|dhcp6| &dhcp6.domains);
            for domain in card.domain.iter().chain(dhcp6) {
                if !search.contains(domain) {
                    search.push(domain.clone());
                }
            }
        }
        if servers.is_empty() && search.is_empty() {
            let _ = fs::remove_file(RESOLV);
            return;
        }
        let text = resolv_text(&servers, &search);
        if fs::read_to_string(RESOLV).is_ok_and(|old| old == text) {
            return;
        }
        let new = format!("{RESOLV}.new");
        let written = Path::new(RESOLV)
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&new, &text))
            .and_then(|()| fs::rename(&new, RESOLV));
        if let Err(err) = written {
            eprintln!("netd: cannot write {RESOLV}: {}", util::strip_os_error(&err));
        }
    }

    fn run(&mut self) -> io::Result<()> {
        loop {
            if STOP.load(Ordering::Relaxed) {
                return Ok(());
            }
            if RELOAD.swap(false, Ordering::Relaxed) {
                self.reload();
            }
            let now = Instant::now();
            if self.scanned.is_none_or(|scanned| now >= scanned + SCAN) {
                self.scan(now);
            }
            for at in 0..self.cards.len() {
                if let Some(dhcp6) = &mut self.cards[at].dhcp6 {
                    let actions = dhcp6.client.tick(now);
                    self.act6(at, actions);
                }
                let actions = match &mut self.cards[at].how {
                    How::Dhcp { client, .. } => client.tick(now),
                    _ => continue,
                };
                self.act(at, actions);
            }
            let mut fds: Vec<libc::pollfd> = Vec::new();
            let mut deadline = self.scanned.map_or(now, |scanned| scanned + SCAN);
            for card in &self.cards {
                if let How::Dhcp { socket, client } = &card.how {
                    fds.push(libc::pollfd {
                        fd: socket.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    });
                    deadline = deadline.min(client.next());
                }
                if let Some(dhcp6) = &card.dhcp6 {
                    fds.push(libc::pollfd {
                        fd: dhcp6.socket.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    });
                    deadline = deadline.min(dhcp6.client.next());
                }
            }
            let timeout = deadline.saturating_duration_since(now).as_millis().min(60_000);
            // SAFETY: `fds` holds valid pollfds.
            let ready = unsafe {
                libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout as libc::c_int + 1)
            };
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
                continue;
            }
            let now = Instant::now();
            let mut buffer = [0u8; dhcp::MESSAGE_MAX as usize];
            for at in 0..self.cards.len() {
                while let How::Dhcp { socket, client } = &mut self.cards[at].how
                    && let Ok((len, from)) = socket.recv_from(&mut buffer)
                {
                    if from.port() != SERVER_PORT {
                        continue;
                    }
                    let actions = client.receive(&buffer[..len], now);
                    self.act(at, actions);
                }
                let mut buffer = [0u8; dhcp6::MESSAGE_MAX];
                while let Some(dhcp6) = &mut self.cards[at].dhcp6
                    && let Ok(len) = dhcp6.socket.recv(&mut buffer)
                {
                    // From any port, as other clients take them: QEMU's
                    // server answers from 8962, 547 with its bytes swapped.
                    // The transaction and the client's DUID say whose an
                    // answer is (RFC 8415, 16).
                    let actions = dhcp6.client.receive(&buffer[..len], now);
                    self.act6(at, actions);
                }
            }
        }
    }

    /// Reads the settings again and sets each card up afresh as they say.
    fn reload(&mut self) {
        eprintln!("netd: reading {SETTINGS} again");
        self.read_settings();
        let now = Instant::now();
        let cards = std::mem::take(&mut self.cards);
        self.ipv6_before =
            cards.iter().filter_map(|card| Some((card.index, card.ipv6.clone()?))).collect();
        for mut card in cards {
            // Its addresses from the old settings go; a lease's stay with
            // the lease, which the new client goes on with.
            if let Some(interface) = Netd::interface(card.index) {
                let leased: Vec<Cidr> = match &card.how {
                    How::Dhcp { client, .. } => {
                        client.lease().map(|l| l.address).into_iter().collect()
                    }
                    _ => Vec::new(),
                };
                let setup = self.setup(&card.name, &interface.mac);
                if !setup.dhcp() && !leased.is_empty() {
                    forget_lease(card.mac);
                }
                let keep = if setup.dhcp() { leased } else { Vec::new() };
                let router = card.router.filter(|_| !keep.is_empty());
                if card.given != keep && !matches!(card.how, How::Alone) {
                    self.give(&mut card, &interface, &keep, router);
                }
            }
            if let Some(dhcp6) = card.dhcp6.take() {
                self.dhcp6_before.push((card.index, dhcp6));
            }
            drop(card);
        }
        self.scan(now);
        // A card the settings now leave alone keeps its IPv6 as it was,
        // and the address DHCPv6 gave it; one whose IPv6 was set afresh
        // lost that.
        self.ipv6_before.clear();
        self.dhcp6_before.clear();
        self.write_resolv();
    }
}

/// A card for the moment one is out of its place.
fn placeholder() -> Card {
    Card {
        name: String::new(),
        index: 0,
        mac: [0; 6],
        carrier: false,
        how: How::Alone,
        given: Vec::new(),
        router: None,
        dns: Vec::new(),
        domain: None,
        ipv6: None,
        dns6: Vec::new(),
        dhcp6: None,
        failure: None,
    }
}

/// Gives the card named `name`, numbered `index`, the IPv6 address `new`
/// (or none) in place of `old`, the one DHCPv6 gave it before, keeping
/// the others it was given and its router. False if the kernel refused.
fn give6(
    network: &dyn Network,
    name: &str,
    index: u32,
    interface: &Interface,
    old: Option<Cidr6>,
    new: Option<Cidr6>,
) -> bool {
    let mut given: Vec<Cidr6> = interface.ipv6.given();
    given.retain(|cidr| Some(*cidr) != old && Some(*cidr) != new);
    given.truncate(ip::ADDRESSES_IPV6_MAX - usize::from(new.is_some()));
    given.extend(new);
    match network.configure_ipv6(index, None, &given, interface.ipv6.given_router) {
        Ok(()) => true,
        Err(status) => {
            eprintln!(
                "netd: {name}: the kernel refused its DHCPv6 address: {}",
                crate::sys::status_name(status.0)
            );
            false
        }
    }
}

/// `3600 s` as `1 hour`, and the rest so.
fn lasting(seconds: u32) -> String {
    match seconds {
        u32::MAX => "ever".into(),
        s if s % 86_400 == 0 => plural(s / 86_400, "day"),
        s if s % 3600 == 0 => plural(s / 3600, "hour"),
        s if s % 60 == 0 => plural(s / 60, "minute"),
        s => plural(s, "second"),
    }
}

fn plural(count: u32, unit: &str) -> String {
    if count == 1 { format!("1 {unit}") } else { format!("{count} {unit}s") }
}

/// The file of the lease of the card with the address `mac`.
fn lease_path(mac: [u8; 6]) -> String {
    let name: Vec<String> = mac.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{LEASES}/{}.lease", name.join("-"))
}

fn read_lease(mac: [u8; 6]) -> Option<(Lease, u64)> {
    parse_lease(&fs::read_to_string(lease_path(mac)).ok()?)
}

fn write_lease(mac: [u8; 6], text: &str) -> io::Result<()> {
    fs::create_dir_all(LEASES)?;
    let path = lease_path(mac);
    let new = format!("{path}.new");
    let mut file = fs::File::create(&new)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    fs::rename(new, path)
}

fn forget_lease(mac: [u8; 6]) {
    let _ = fs::remove_file(lease_path(mac));
}

/// netd(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    if let Some(arg) = args.next()? {
        return Err(args.unknown(&arg));
    }
    for (signal, handler) in
        [(libc::SIGTERM, on_term as extern "C" fn(libc::c_int)), (libc::SIGHUP, on_hangup)]
    {
        // SAFETY: all zeros is a valid sigaction: numbers, a signal set
        // and a function address.
        let mut action: libc::sigaction = unsafe { util::zeroed() };
        action.sa_sigaction = handler as *const () as usize;
        // SAFETY: the handlers only store to an atomic.
        unsafe { libc::sigaction(signal, &raw const action, std::ptr::null_mut()) };
    }
    let mut netd = Netd {
        cards: Vec::new(),
        settings: Vec::new(),
        scanned: None,
        network: &ip::Slots,
        secret: load_secret(),
        ipv6_before: Vec::new(),
        dhcp6_before: Vec::new(),
    };
    netd.read_settings();
    netd.run().map_err(|e| util::failed(&"netd", e))?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cidr(text: &str) -> Cidr {
        Cidr::parse(text).unwrap()
    }

    #[test]
    fn the_settings_name_cards_and_what_they_have() {
        let (cards, wrong) = parse_settings(
            "# a comment\n\
             [net0]\n\
             address = 192.168.1.10/24\n\
             address = 192.168.1.11/24\n\
             router = 192.168.1.1\n\
             dns = 192.168.1.1\n\
             dns = 9.9.9.9\n\
             \n\
             [52:54:00:AB:CD:EF]\n\
             dhcp = no\n\
             [net2]\n\
             dhcp = yes\n\
             address = 10.1.0.1/16\n",
        );
        assert_eq!(wrong, Vec::<String>::new());
        let net0 = Setup {
            dhcp: None,
            addresses: vec![cidr("192.168.1.10/24"), cidr("192.168.1.11/24")],
            router: Some(Ipv4Addr::new(192, 168, 1, 1)),
            dns: vec![
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
                IpAddr::V4(Ipv4Addr::new(9, 9, 9, 9)),
            ],
            ..Setup::default()
        };
        assert_eq!(cards[0], ("net0".to_string(), net0.clone()));
        assert!(!net0.dhcp() && !net0.alone());
        assert_eq!(cards[1].0, "52:54:00:ab:cd:ef");
        assert!(cards[1].1.alone());
        assert!(cards[2].1.dhcp() && !cards[2].1.alone());
        assert!(Setup::default().dhcp());
    }

    #[test]
    fn the_settings_set_ipv6_up() {
        let (cards, wrong) = parse_settings(
            "[net0]\n\
             address = 2001:db8::10/64\n\
             router = fe80::1\n\
             dns = 2001:db8::53\n\
             temporary-addresses = no\n\
             [net1]\n\
             ipv6 = no\n\
             [net2]\n\
             dhcp = no\n\
             ipv6 = yes\n\
             [net3]\n\
             address = fe80::5/64\n\
             address = ff02::1/64\n\
             router = 2001:db9::1\n\
             address = 2001:db8::11/64\n\
             ipv6 = maybe\n",
        );
        assert_eq!(
            wrong,
            [
                "/system/settings/network:12: 'fe80::5/64' is not a address here",
                "/system/settings/network:13: 'ff02::1/64' is not a address here",
                "/system/settings/network:16: 'maybe' is not a ipv6 here",
                "/system/settings/network: [net3]'s router 2001:db9::1 is on none of its networks",
            ]
        );
        let net0 = &cards[0].1;
        assert_eq!(net0.addresses6, [Cidr6::parse("2001:db8::10/64").unwrap()]);
        assert_eq!(net0.router6, Some("fe80::1".parse().unwrap()));
        assert_eq!(net0.dns, [IpAddr::V6("2001:db8::53".parse().unwrap())]);
        let ipv6 = net0.ipv6();
        assert!(ipv6.on && !ipv6.temporary);
        // IPv4 by DHCP still, with addresses of IPv6 alone.
        assert!(net0.dhcp() && !net0.alone());
        assert!(!cards[1].1.ipv6().on);
        // Without DHCP, IPv6 alone is not being left alone.
        assert!(!cards[2].1.dhcp() && !cards[2].1.alone());
        assert!(Setup::default().ipv6().on && Setup::default().ipv6().temporary);
    }

    #[test]
    fn dhcpv6_is_asked_as_routers_and_the_settings_say() {
        let (cards, wrong) = parse_settings(
            "[net0]\ndhcpv6 = settings\n[net1]\ndhcpv6 = address\n[net2]\ndhcpv6 = no\n\
             [net3]\ndhcpv6 = sometimes\n",
        );
        assert_eq!(wrong, [format!("{SETTINGS}:8: 'sometimes' is not a dhcpv6 here")]);
        let mode = |at: usize| cards[at].1.ipv6().dhcpv6;
        assert_eq!(
            [mode(0), mode(1), mode(2), mode(3)],
            [Dhcpv6::Settings, Dhcpv6::Address, Dhcpv6::No, Dhcpv6::Auto]
        );
        // As routers say: M asks for an address, O for the settings.
        assert_eq!(Dhcpv6::Auto.asks(false, false), None);
        assert_eq!(Dhcpv6::Auto.asks(false, true), Some(false));
        assert_eq!(Dhcpv6::Auto.asks(true, true), Some(true));
        assert_eq!(Dhcpv6::Settings.asks(true, false), Some(false));
        assert_eq!(Dhcpv6::Address.asks(false, false), Some(true));
        assert_eq!(Dhcpv6::No.asks(true, true), None);
    }

    #[test]
    fn the_secret_reads_back_as_it_was_written() {
        let secret = *b"0123456789abcdef";
        let text = secret_text(&secret);
        assert!(text.ends_with("\n30313233343536373839616263646566\n"));
        assert_eq!(parse_secret(&text), Some(secret));
        assert_eq!(parse_secret("# nothing\n"), None);
        assert_eq!(parse_secret("0123\n"), None);
        assert_eq!(parse_secret(&"zz".repeat(16)), None);
    }

    #[test]
    fn what_is_wrong_in_the_settings_is_said_by_line() {
        let (cards, wrong) = parse_settings(
            "dns = 1.1.1.1\n\
             [net0]\n\
             address = 300.1.1.1/24\n\
             address = 127.0.0.1/8\n\
             router = 10.0.0.1\n\
             router = 10.0.0.2\n\
             dhcp = maybe\n\
             colour = blue\n\
             just words\n\
             address = 192.168.1.10/24\n\
             [net0]\n\
             dns = 2.2.2.2\n\
             []\n",
        );
        assert_eq!(
            wrong,
            [
                "/system/settings/network:1: dns before any `[card]`",
                "/system/settings/network:3: '300.1.1.1/24' is not a address here",
                "/system/settings/network:4: '127.0.0.1/8' is not a address here",
                "/system/settings/network:6: '10.0.0.2' is not a router here",
                "/system/settings/network:7: 'maybe' is not a dhcp here",
                "/system/settings/network:8: no setting is called colour",
                "/system/settings/network:9: not `setting = value` or `[card]`",
                "/system/settings/network:11: '[net0]' names no card, or one named before",
                "/system/settings/network:13: '[]' names no card, or one named before",
                "/system/settings/network: [net0]'s router 10.0.0.1 is on none of its networks",
            ]
        );
        // The second [net0]'s lines went nowhere.
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].1.dns, Vec::<IpAddr>::new());
        assert_eq!(cards[0].1.addresses, [cidr("192.168.1.10/24")]);
    }

    #[test]
    fn a_lease_file_reads_back_as_it_was_written() {
        let lease = Lease {
            address: cidr("10.0.2.15/24"),
            router: Some(Ipv4Addr::new(10, 0, 2, 2)),
            dns: vec![Ipv4Addr::new(10, 0, 2, 3), Ipv4Addr::new(1, 1, 1, 1)],
            domain: Some("example.org".into()),
            server: Ipv4Addr::new(10, 0, 2, 2),
            seconds: 86_400,
            renew: 43_200,
            rebind: 75_600,
            metered: false,
        };
        let text = lease_text(&lease, "net0 (52:54:00:12:34:56)", 1_761_900_000);
        assert_eq!(
            text,
            "# netd(8)'s lease for net0 (52:54:00:12:34:56).\n\
             address = 10.0.2.15/24\n\
             router = 10.0.2.2\n\
             dns = 10.0.2.3\n\
             dns = 1.1.1.1\n\
             domain = example.org\n\
             server = 10.0.2.2\n\
             obtained = 1761900000\n\
             seconds = 86400\n\
             renew = 43200\n\
             rebind = 75600\n"
        );
        assert_eq!(parse_lease(&text), Some((lease.clone(), 1_761_900_000)));
        let bare = Lease { router: None, dns: Vec::new(), domain: None, ..lease };
        assert_eq!(parse_lease(&lease_text(&bare, "net0", 5)), Some((bare.clone(), 5)));
        // A lease file written before leases carried it reads as not
        // metered, and the line is only written when it is.
        assert!(!text.contains("metered"));
        let metered = Lease { metered: true, ..bare };
        let text = lease_text(&metered, "net0", 5);
        assert!(text.ends_with("rebind = 75600\nmetered = yes\n"));
        assert_eq!(parse_lease(&text), Some((metered, 5)));
        // A file cut short is no lease.
        assert_eq!(parse_lease(&text[..text.len() - 14]), None);
        assert_eq!(parse_lease("address = 10.0.2.15/24\n"), None);
    }

    #[test]
    fn resolv_conf_has_three_servers_and_the_domains() {
        let servers = [[10, 0, 2, 3], [1, 1, 1, 1], [9, 9, 9, 9], [8, 8, 8, 8]]
            .map(|ip| IpAddr::V4(Ipv4Addr::from(ip)));
        assert_eq!(
            resolv_text(&servers, &["example.org".into(), "lan".into()]),
            "# Written by netd(8) from what DHCP and /system/settings/network give.\n\
             # /etc/resolv.conf leads here; make it a file of your own for other servers.\n\
             nameserver 10.0.2.3\n\
             nameserver 1.1.1.1\n\
             nameserver 9.9.9.9\n\
             search example.org lan\n"
        );
        assert!(resolv_text(&servers[..1], &[]).ends_with("\nnameserver 10.0.2.3\n"));
        assert!(!resolv_text(&servers[..1], &[]).contains("search"));
        let six = [IpAddr::V6("fec0::3".parse().unwrap())];
        assert!(resolv_text(&six, &[]).ends_with("\nnameserver fec0::3\n"));
    }

    #[test]
    fn netd_changes_only_the_addresses_it_gave() {
        let (a, b, c, d, e) = (
            cidr("10.0.2.15/24"),
            cidr("10.0.2.16/24"),
            cidr("192.168.5.1/24"),
            cidr("172.16.0.1/12"),
            cidr("172.17.0.1/12"),
        );
        // An administrator's address stays when the lease's changes.
        assert_eq!(merge(&[a, c], &[a], &[b]), [b, c]);
        // Given up.
        assert_eq!(merge(&[a, c], &[a], &[]), [c]);
        // Given again, with the same address only once.
        assert_eq!(merge(&[c, a], &[], &[a]), [a, c]);
        // At most four.
        assert_eq!(merge(&[c, d, e], &[], &[a, b]), [a, b, c, d]);
    }

    #[test]
    fn mac_addresses_and_lease_files_are_named_alike() {
        assert_eq!(mac_of("52:54:00:12:34:56"), Some([0x52, 0x54, 0, 0x12, 0x34, 0x56]));
        assert_eq!(mac_of("52:54:00:12:34"), None);
        assert_eq!(mac_of("52:54:00:12:34:56:78"), None);
        assert_eq!(mac_of("zz:54:00:12:34:56"), None);
        assert_eq!(
            lease_path([0x52, 0x54, 0, 0x12, 0x34, 0x56]),
            "/system/state/netd/52-54-00-12-34-56.lease"
        );
        assert_eq!(lasting(86_400), "1 day");
        assert_eq!(lasting(7200), "2 hours");
        assert_eq!(lasting(90), "90 seconds");
        assert_eq!(lasting(u32::MAX), "ever");
    }
}
