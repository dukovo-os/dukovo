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
//! A card with an address of its own and a `dhcp-server` range in network(5)
//! gives the clients on its network addresses by DHCP (the server is
//! [`crate::dhcp_server`], ADR 0486). It has a thread of its own on a
//! socket on port 67, bound to the card, which answers each request under
//! the server's lock and then keeps the leases in
//! `/system/state/netd/dhcp-server-<card>` and lists them in
//! `/system/live/netd/leases`, which `netd leases` shows. The log has a line
//! for each new lease, decline and release, and when the pool is full. The
//! settings read again give each server its new pool, with the leases that
//! still fit it.
//!
//! Bridges and VLANs that ip(8) makes (ADR 0333) are set up as cards are,
//! by their names; a bridge's port is not, while it is one: its bridge
//! has the addresses.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6, UdpSocket};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use radix_abi::network::{Ipv6Flags, Ipv6Settings};

use crate::args::Args;
use crate::dhcp::{self, Action, Client, Lease};
use crate::dhcp_server::{
    self, Event, Lease as Granted, Pool, Reply, Reservation, Server, mac_of, mac_text,
};
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

/// Whether the connection is metered, for updated(8): `yes` or `no` for
/// the card that carries the default route, then a line for each card.
const METERED: &str = "/system/live/netd/metered";

/// What `netd leases` reads: each serving card's leases, a line each.
const SERVER_LISTING: &str = "/system/live/netd/leases";

/// How long a lease of a card's server lasts unless the settings say.
const SERVER_LEASE_DEFAULT: Duration = Duration::from_hours(12);

/// The most reservations a card's server takes.
const RESERVATIONS_MAX: usize = 1024;

/// The shortest and longest leases a server gives.
const SERVER_LEASE_MIN: Duration = Duration::from_mins(1);
const SERVER_LEASE_MAX: Duration = Duration::from_hours(30 * 24);

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
    /// Whether its connection is metered: as `metered` says, or, with
    /// `auto` or nothing, as the server of its DHCP lease says.
    pub metered: Option<bool>,
    /// Its DHCP server, if `dhcp-server` names a range.
    pub server: ServerSetup,
}

/// A card's DHCP server, as the `dhcp-server` lines of the settings give
/// it, which is valid once [`parse_settings`] has left it on. What a line
/// leaves out is the default, which netd works out when it starts the
/// server.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerSetup {
    /// The addresses given to clients, `first-last`: setting it turns
    /// the server on.
    pub range: Option<(Ipv4Addr, Ipv4Addr)>,
    pub lease: Option<Duration>,
    /// `Some(None)` is `none`: no router, and no default one.
    pub router: Option<Option<Ipv4Addr>>,
    /// The DNS servers; empty for `none`.
    pub dns: Option<Vec<Ipv4Addr>>,
    pub domain: Option<String>,
    pub time: Option<Option<Ipv4Addr>>,
    pub metered: Option<bool>,
    /// The reservations, with the lines that gave them.
    pub reserved: Vec<(usize, Reservation)>,
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
    /// Whether the card asks DHCP.
    pub(crate) fn dhcp(&self) -> bool {
        self.dhcp.unwrap_or(self.addresses.is_empty())
    }

    /// The address its DHCP server answers from: the first whose network
    /// holds `first`, the range's first address, else the first one.
    fn server_address(&self, first: Ipv4Addr) -> Option<Cidr> {
        self.addresses.iter().find(|cidr| cidr.contains(first)).or(self.addresses.first()).copied()
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
            "metered" => match value {
                "yes" | "no" if setup.metered.is_none() => {
                    setup.metered = Some(value == "yes");
                    true
                }
                "auto" => true,
                _ => false,
            },
            _ => match server_line(&mut setup.server, key, value, at) {
                Some(ok) => ok,
                None => {
                    wrong.push(format!("{SETTINGS}:{at}: no setting is called {key}"));
                    continue;
                }
            },
        };
        if !ok {
            wrong.push(format!("{SETTINGS}:{at}: '{value}' is not a {key} here"));
        }
    }
    for (name, setup) in &mut cards {
        check_server(name, setup, &mut wrong);
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

/// A lease time, a number with `s`, `m`, `h` or `d`, from a minute to 30
/// days.
fn lease_time(text: &str) -> Option<Duration> {
    let (number, unit) = text.split_at(text.len().checked_sub(1)?);
    let unit = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => return None,
    };
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let time = Duration::from_secs(number.parse::<u64>().ok()?.checked_mul(unit)?);
    (SERVER_LEASE_MIN..=SERVER_LEASE_MAX).contains(&time).then_some(time)
}

/// A reservation, `mac address [name]`.
fn reservation(text: &str) -> Option<Reservation> {
    let mut words = text.split_whitespace();
    let mac = mac_of(words.next()?)?;
    let address = words.next()?.parse::<Ipv4Addr>().ok().filter(|&ip| dhcp::unicast(ip))?;
    let name = words.next();
    if words.next().is_some() || name.is_some_and(|name| !dhcp_server::valid_name(name)) {
        return None;
    }
    Some(Reservation { mac, address, name: name.map(str::to_string) })
}

/// Reads one of a card's DHCP server lines into `server`: whether its
/// value is right and not given before, or `None` if `key` is none of
/// theirs.
fn server_line(server: &mut ServerSetup, key: &str, value: &str, at: usize) -> Option<bool> {
    let address = |text: &str| text.parse::<Ipv4Addr>().ok().filter(|&ip| dhcp::unicast(ip));
    let or_none = |text: &str| if text == "none" { Some(None) } else { address(text).map(Some) };
    let ok = match key {
        "dhcp-server" => match value
            .split_once('-')
            .and_then(|(first, last)| Some((address(first.trim())?, address(last.trim())?)))
        {
            Some(range) if server.range.is_none() => {
                server.range = Some(range);
                true
            }
            _ => false,
        },
        "dhcp-server-lease" => match lease_time(value) {
            Some(lease) if server.lease.is_none() => {
                server.lease = Some(lease);
                true
            }
            _ => false,
        },
        "dhcp-server-router" => match or_none(value) {
            Some(router) if server.router.is_none() => {
                server.router = Some(router);
                true
            }
            _ => false,
        },
        "dhcp-server-time" => match or_none(value) {
            Some(time) if server.time.is_none() => {
                server.time = Some(time);
                true
            }
            _ => false,
        },
        "dhcp-server-dns" => {
            let list: Option<Vec<Ipv4Addr>> = if value == "none" {
                Some(Vec::new())
            } else {
                value.split_whitespace().map(address).collect()
            };
            // One line, or lines that add up to at most DNS_MAX; `none`
            // stands alone.
            match (server.dns.as_mut(), list) {
                (None, Some(list)) if list.len() <= DNS_MAX && !value.is_empty() => {
                    server.dns = Some(list);
                    true
                }
                (Some(have), Some(list))
                    if !have.is_empty()
                        && !list.is_empty()
                        && have.len() + list.len() <= DNS_MAX =>
                {
                    have.extend(list);
                    true
                }
                _ => false,
            }
        }
        "dhcp-server-domain" => match dhcp::domain(value.as_bytes()) {
            Some(domain) if server.domain.is_none() => {
                server.domain = Some(domain);
                true
            }
            _ => false,
        },
        "dhcp-server-metered" => match value {
            "yes" | "no" if server.metered.is_none() => {
                server.metered = Some(value == "yes");
                true
            }
            _ => false,
        },
        "reserve" => match reservation(value) {
            Some(new)
                if server.reserved.len() < RESERVATIONS_MAX
                    && !server
                        .reserved
                        .iter()
                        .any(|(_, old)| old.mac == new.mac || old.address == new.address) =>
            {
                server.reserved.push((at, new));
                true
            }
            _ => false,
        },
        _ => return None,
    };
    Some(ok)
}

/// Checks the server `setup` of the card `name` as a whole, where the
/// addresses and the lines are all in: a server that cannot run is left
/// off, and why is said; a reservation that is wrong is left out.
fn check_server(name: &str, setup: &mut Setup, wrong: &mut Vec<String>) {
    let server = std::mem::take(&mut setup.server);
    let Some((first, last)) = server.range else {
        if server != ServerSetup::default() {
            wrong.push(format!("{name}: its dhcp-server settings do nothing without dhcp-server"));
        }
        return;
    };
    let range = format!("{name}: dhcp-server {first}-{last}");
    if setup.dhcp == Some(true) {
        wrong.push(format!("{name}: dhcp-server cannot go with dhcp = yes"));
        return;
    }
    let Some(card) = setup.server_address(first) else {
        wrong.push(format!("{name}: dhcp-server needs an address of the card's own"));
        return;
    };
    if let Err(why) = dhcp_server::check_range(card, first, last) {
        wrong.push(format!("{range} {why}"));
        return;
    }
    let network = format!("{}/{}", card.network(), card.prefix);
    let on_network =
        |ip: Ipv4Addr| card.contains(ip) && ip != card.network() && Some(ip) != card.broadcast();
    if let Some(Some(router)) = server.router
        && !on_network(router)
    {
        wrong.push(format!(
            "{name}: dhcp-server-router {router} is not on the card's network, {network}"
        ));
        return;
    }
    let mut reserved = Vec::new();
    for (at, reservation) in server.reserved {
        let address = reservation.address;
        if on_network(address) && address != card.ip {
            reserved.push((at, reservation));
        } else {
            wrong.push(format!(
                "{SETTINGS}:{at}: reserve {address} is not a client's address on {name}'s network, {network}"
            ));
        }
    }
    setup.server = ServerSetup { range: Some((first, last)), reserved, ..server };
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
    /// Its DHCP server, while the settings give it one.
    serving: Option<Serving>,
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

/// What each serving card's server lists for `netd leases`, by card: the
/// lines of [`SERVER_LISTING`].
type Listing = Arc<Mutex<BTreeMap<String, String>>>;

/// A card's DHCP server at work: its pool and leases behind a lock, and
/// the thread that answers the clients on its socket, which ends when the
/// server is dropped.
#[derive(Debug)]
struct Serving {
    card: String,
    server: Arc<Mutex<Server>>,
    stop: Arc<AtomicBool>,
    listing: Listing,
    thread: Option<thread::JoinHandle<()>>,
}

impl Serving {
    /// Starts a server for `pool` with the `leases` it had, answering on
    /// `socket`.
    fn start(
        pool: Pool,
        leases: Vec<Granted>,
        socket: UdpSocket,
        listing: &Listing,
    ) -> io::Result<Serving> {
        let card = pool.card.clone();
        let server = Arc::new(Mutex::new(Server::new(pool, leases)));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = thread::Builder::new().name(format!("dhcp-server {card}")).spawn({
            let (card, server, stop, listing) =
                (card.clone(), server.clone(), stop.clone(), listing.clone());
            move || serve(&socket, &card, &server, &stop, &listing)
        })?;
        let serving =
            Serving { card, server, stop, listing: listing.clone(), thread: Some(thread) };
        serving.keep();
        Ok(serving)
    }

    /// Gives the server `pool`, which keeps the leases that fit it.
    fn set_pool(&self, pool: Pool) {
        let mut server = self.server.lock().unwrap_or_else(PoisonError::into_inner);
        if *server.pool() == pool {
            return;
        }
        server.set_pool(pool);
        drop(server);
        self.keep();
    }

    /// Writes the server's leases to the card's file and lists them.
    fn keep(&self) {
        let server = self.server.lock().unwrap_or_else(PoisonError::into_inner);
        let (leases, rows) = (server.leases().to_vec(), server.listing(wall()));
        drop(server);
        keep_leases(&self.card, &self.listing, &leases, rows);
    }
}

impl Drop for Serving {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            // The thread sees the flag within a poll's timeout; it
            // holds the socket until then, which a new server on the
            // same card needs free.
            let _ = thread.join();
        }
        publish(&self.listing, &self.card, String::new());
    }
}

/// How long a server's thread waits for a request before it looks at its
/// stop flag.
const SERVE_POLL_MS: libc::c_int = 250;

/// Answers the requests that come to `socket` for the card `card`, until
/// `stop` is set: each under the server's lock, then, with the lock
/// released, the lease files written, the reply sent and the event logged.
fn serve(
    socket: &UdpSocket,
    card: &str,
    server: &Mutex<Server>,
    stop: &AtomicBool,
    listing: &Listing,
) {
    let mut buffer = [0u8; 2048];
    let mut failure: Option<String> = None;
    while !stop.load(Ordering::Relaxed) {
        let mut poll = libc::pollfd { fd: socket.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: `poll` is one valid pollfd.
        let ready = unsafe { libc::poll(&raw mut poll, 1, SERVE_POLL_MS) };
        if ready < 0 {
            // Not a signal's interruption: wait before trying again, as
            // the loop would spin.
            thread::sleep(Duration::from_millis(100));
            continue;
        }
        // A datagram longer than the buffer is cut, and the server takes
        // none that long.
        while !stop.load(Ordering::Relaxed)
            && let Ok((len, from)) = socket.recv_from(&mut buffer)
        {
            // Clients send from port 68 (RFC 2131).
            if from.port() != CLIENT_PORT {
                continue;
            }
            let now = wall();
            let mut guard = server.lock().unwrap_or_else(PoisonError::into_inner);
            let (reply, event) = guard.handle(&buffer[..len], Instant::now(), now);
            let writes = matches!(
                event,
                Some(
                    Event::Leased(_) | Event::Renewed | Event::Released(_) | Event::Declined { .. }
                )
            );
            let kept = writes.then(|| (guard.leases().to_vec(), guard.listing(now)));
            drop(guard);
            // The lease is kept before the client is told it has it.
            if let Some((leases, rows)) = kept {
                keep_leases(card, listing, &leases, rows);
            }
            if let Reply::Send { to, message } = reply {
                let sent = socket.send_to(&message, to).err().map(|err| util::strip_os_error(&err));
                if sent.is_some() && sent != failure {
                    eprintln!(
                        "netd: {card}: cannot send to {to}: {}",
                        sent.as_deref().unwrap_or("")
                    );
                }
                failure = sent;
            }
            if let Some(event) = event {
                say(card, &event, now);
            }
        }
    }
}

/// `52:54:00:12:34:56`, and the name after it if there is one.
fn client_text(mac: [u8; 6], name: Option<&str>) -> String {
    match name {
        Some(name) => format!("{} ({name})", mac_text(mac)),
        None => mac_text(mac),
    }
}

/// Logs what happened to the card `card`'s server at `wall`, if it is
/// worth a line: not the renewal of a lease.
fn say(card: &str, event: &Event, wall: u64) {
    match event {
        Event::Leased(lease) => {
            let left = u32::try_from(lease.ends.saturating_sub(wall)).unwrap_or(u32::MAX);
            let client = client_text(lease.mac, lease.name.as_deref());
            eprintln!("netd: {card}: gave {} to {client} for {}", lease.address, lasting(left));
        }
        Event::Released(lease) => {
            let client = client_text(lease.mac, lease.name.as_deref());
            eprintln!("netd: {card}: {client} released {}", lease.address);
        }
        Event::Declined { address, mac, name } => {
            let client = client_text(*mac, name.as_deref());
            eprintln!(
                "netd: {card}: {client} says {address} is in use by another machine; it is left \
                 alone for 10 minutes"
            );
        }
        Event::Full { mac } => eprintln!(
            "netd: {card}: the pool is full: {} gets no address until a lease ends",
            mac_text(*mac)
        ),
        Event::Relayed => {
            eprintln!(
                "netd: {card}: a request came through a relay agent; relayed requests are ignored"
            );
        }
        Event::Renewed => {}
    }
}

/// Keeps the card `card`'s leases in its file in [`LEASES`], and lists
/// `rows` of them for `netd leases`.
fn keep_leases(card: &str, listing: &Listing, leases: &[Granted], rows: String) {
    let path = format!("{LEASES}/dhcp-server-{card}");
    if let Err(err) = write_synced(&path, &dhcp_server::leases_text(leases), 0o600) {
        eprintln!(
            "netd: {card}: cannot keep its DHCP leases in {path}: {}",
            util::strip_os_error(&err)
        );
    }
    publish(listing, card, rows);
}

/// Puts the card `card`'s `rows` in the list and writes it, which anyone
/// may read. The list is written under its lock, so that two cards'
/// writes do not cross.
fn publish(listing: &Listing, card: &str, rows: String) {
    let mut listing = listing.lock().unwrap_or_else(PoisonError::into_inner);
    if rows.is_empty() {
        listing.remove(card);
    } else {
        listing.insert(card.to_string(), rows);
    }
    let text: String = listing.values().map(String::as_str).collect();
    if let Err(err) = write_synced(SERVER_LISTING, &text, 0o644) {
        eprintln!("netd: cannot write {SERVER_LISTING}: {}", util::strip_os_error(&err));
    }
}

/// Writes `text` to `path` with `mode`, whole or not at all: to a file
/// beside it, which is synced, then renamed.
fn write_synced(path: &str, text: &str, mode: u32) -> io::Result<()> {
    if let Some(dir) = Path::new(path).parent() {
        fs::create_dir_all(dir)?;
    }
    let new = format!("{path}.new");
    let mut file =
        fs::OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(&new)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    fs::rename(new, path)
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

/// A UDP socket on `port`, 68 for a client and 67 for a server, of the
/// card named `name` that may broadcast, which does not block.
fn dhcp_socket(name: &str, port: u16) -> io::Result<UdpSocket> {
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
    address.sin_port = port.to_be();
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
    /// The cards' DHCP servers while the settings are read again, which
    /// the cards that serve still take over, by their numbers.
    serving_before: Vec<(u32, Serving)>,
    /// The leases each card's server lists for `netd leases`.
    listing: Listing,
    /// The text of the metered file as it was written last.
    metered: Option<String>,
}

/// How `settings` have the card named `name`, with the address `mac`, set
/// up: the first section that names it, else the defaults.
pub(crate) fn setup_of(settings: &[(String, Setup)], name: &str, mac: &str) -> Setup {
    let mac = mac.to_ascii_lowercase();
    settings
        .iter()
        .find(|(card, _)| *card == name || *card == mac)
        .map_or_else(Setup::default, |(_, setup)| setup.clone())
}

impl Netd {
    /// How the settings have the card named `name`, with the address
    /// `mac`, set up.
    fn setup(&self, name: &str, mac: &str) -> Setup {
        setup_of(&self.settings, name, mac)
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
                self.start_serving(&mut card);
                self.cards.push(card);
                changed = true;
            }
        }
        if changed {
            self.write_resolv();
        }
        self.update_metered(&interfaces);
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
            serving: None,
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
        let socket = match dhcp_socket(&card.name, CLIENT_PORT) {
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

    /// The IPv4 DNS servers resolv.conf names that are not loopback
    /// addresses: those a card's server gives its clients unless the
    /// settings name others.
    fn default_dns(&self) -> Vec<Ipv4Addr> {
        let mut servers: Vec<Ipv4Addr> = Vec::new();
        for dns in self.cards.iter().flat_map(|card| &card.dns) {
            if let IpAddr::V4(dns) = dns
                && !dns.is_loopback()
                && !servers.contains(dns)
            {
                servers.push(*dns);
            }
        }
        servers.truncate(DNS_MAX);
        servers
    }

    /// The pool `setup` gives the server of the card `name`, with the
    /// defaults of what it leaves out worked out now; `timed` is timed(5)'s
    /// settings, which say whether the card's address is the time server.
    fn pool_of(&self, name: &str, setup: &Setup, timed: &str) -> Option<Pool> {
        let server = &setup.server;
        let (first, last) = server.range?;
        let card = setup.server_address(first)?;
        let router = match server.router {
            Some(router) => router,
            None => setup.router.filter(|&router| card.contains(router)),
        };
        // DHCP clients ask their time server in plain NTP.
        let time = server.time.unwrap_or_else(|| {
            crate::timed::answers_plain_ntp_from(timed, IpAddr::V4(first)).then_some(card.ip)
        });
        Some(Pool {
            card: name.to_string(),
            address: card.ip,
            prefix: card.prefix,
            first,
            last,
            lease: server.lease.unwrap_or(SERVER_LEASE_DEFAULT),
            router,
            dns: server.dns.clone().unwrap_or_else(|| self.default_dns()),
            domain: server.domain.clone(),
            time,
            metered: server.metered.unwrap_or(false),
            reserved: server.reserved.iter().map(|(_, reservation)| reservation.clone()).collect(),
        })
    }

    /// Starts the DHCP server of `card` if its settings give it one, on a
    /// socket of its own on port 67, with the leases it kept; or goes on
    /// with the server it had before the settings were read again, now
    /// with the pool they give.
    fn start_serving(&mut self, card: &mut Card) {
        let setup = self.setup(&card.name, &mac_text(card.mac));
        let Some(pool) = self.pool_of(&card.name, &setup, &timed_settings()) else { return };
        // It answers from the card's address, which the kernel must have
        // taken.
        if !card.given.iter().any(|cidr| cidr.ip == pool.address) {
            eprintln!(
                "netd: {}: dhcp-server is off: the card does not have {}",
                card.name, pool.address
            );
            return;
        }
        if let Some(at) = self.serving_before.iter().position(|(index, _)| *index == card.index) {
            let (_, serving) = self.serving_before.swap_remove(at);
            serving.set_pool(pool);
            card.serving = Some(serving);
            return;
        }
        let socket = match dhcp_socket(&card.name, SERVER_PORT) {
            Ok(socket) => socket,
            Err(err) => {
                eprintln!(
                    "netd: {}: no socket on port {SERVER_PORT} for its DHCP server: {}",
                    card.name,
                    util::strip_os_error(&err)
                );
                return;
            }
        };
        let kept = fs::read_to_string(format!("{LEASES}/dhcp-server-{}", card.name));
        let leases = kept.map_or_else(|_| Vec::new(), |text| dhcp_server::parse_leases(&text));
        let (kept, range) = (leases.len(), format!("{}-{}", pool.first, pool.last));
        let lease = lasting(u32::try_from(pool.lease.as_secs()).unwrap_or(u32::MAX));
        match Serving::start(pool, leases, socket, &self.listing) {
            Ok(serving) => {
                eprintln!(
                    "netd: {}: giving addresses {range} by DHCP, for {lease} each; {kept} leases kept",
                    card.name
                );
                card.serving = Some(serving);
            }
            Err(err) => eprintln!(
                "netd: {}: cannot start its DHCP server: {}",
                card.name,
                util::strip_os_error(&err)
            ),
        }
    }

    /// Whether `card`'s connection is metered: as its `metered` setting
    /// says, else as the server of its DHCP lease says.
    fn is_metered(&self, card: &Card) -> bool {
        let setup = self.setup(&card.name, &mac_text(card.mac));
        setup.metered.unwrap_or_else(|| match &card.how {
            How::Dhcp { client, .. } => client.lease().is_some_and(|lease| lease.metered),
            _ => false,
        })
    }

    /// Writes the metered file again if what it says changed, and logs
    /// when the connection that carries the default route became metered
    /// or stopped being.
    fn update_metered(&mut self, interfaces: &[Interface]) {
        let cards: Vec<(&str, bool)> =
            self.cards.iter().map(|card| (card.name.as_str(), self.is_metered(card))).collect();
        let default = default_route_card(interfaces);
        let text = metered_text(&cards, default);
        if self.metered.as_deref() == Some(&text) {
            return;
        }
        let was = self.metered.as_deref().is_some_and(|text| text.starts_with("yes"));
        let is = text.starts_with("yes");
        if is {
            eprintln!(
                "netd: {} is metered: updates wait for another connection",
                default.unwrap_or("the connection")
            );
        } else if was {
            eprintln!("netd: the default connection is not metered now: updates go ahead");
        }
        if let Err(err) = write_synced(METERED, &text, 0o644) {
            eprintln!("netd: cannot write {METERED}: {}", util::strip_os_error(&err));
        }
        self.metered = Some(text);
    }

    /// [`Netd::update_metered`] with the interfaces as they are now.
    fn refresh_metered(&mut self) {
        let read = ip::read_interfaces(Path::new(ip::INTERFACES), Path::new(ip::ROUTES));
        if let Ok(interfaces) = read {
            self.update_metered(&interfaces);
        }
    }

    /// Gives the servers that use the default DNS servers or time
    /// server the current ones.
    fn refresh_pools(&self) {
        for card in &self.cards {
            let Some(serving) = &card.serving else { continue };
            let setup = self.setup(&card.name, &mac_text(card.mac));
            if let Some(pool) = self.pool_of(&card.name, &setup, &timed_settings()) {
                serving.set_pool(pool);
            }
        }
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
        self.refresh_metered();
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
        self.refresh_metered();
    }

    /// Writes the cards' DNS servers into resolv.conf, or takes it away
    /// when there are none, and gives the DHCP servers that default to
    /// them the same.
    fn write_resolv(&self) {
        self.refresh_pools();
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
            // the lease, which the new client goes on with, and those the
            // new settings give it too stay, so that a reload that leaves
            // them as they were leaves their connections alone.
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
                let unchanged = !setup.dhcp() && setup.addresses == card.given;
                let (keep, router) = if unchanged {
                    (card.given.clone(), setup.router)
                } else {
                    let keep = if setup.dhcp() { leased } else { Vec::new() };
                    let router = card.router.filter(|_| !keep.is_empty());
                    (keep, router)
                };
                if (card.given != keep || card.router != router) && !matches!(card.how, How::Alone)
                {
                    self.give(&mut card, &interface, &keep, router);
                }
            }
            if let Some(dhcp6) = card.dhcp6.take() {
                self.dhcp6_before.push((card.index, dhcp6));
            }
            if let Some(serving) = card.serving.take() {
                self.serving_before.push((card.index, serving));
            }
            drop(card);
        }
        self.scan(now);
        // A card the settings now leave alone keeps its IPv6 as it was,
        // and the address DHCPv6 gave it; one whose IPv6 was set afresh
        // lost that.
        self.ipv6_before.clear();
        self.dhcp6_before.clear();
        // A server whose card no longer serves ends here.
        self.serving_before.clear();
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
        serving: None,
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

/// timed(5)'s settings as they are now.
fn timed_settings() -> String {
    fs::read_to_string(crate::timed::SETTINGS).unwrap_or_default()
}

/// The card that carries the default route, by its interface's routes:
/// the first with an IPv4 default route, else the first with an IPv6 one.
fn default_route_card(interfaces: &[Interface]) -> Option<&str> {
    let by = |has: fn(&Interface) -> bool| interfaces.iter().find(|interface| has(interface));
    let carrying = by(|interface| interface.router.is_some())
        .or_else(|| by(|interface| interface.ipv6.router().is_some()));
    carrying.map(|interface| interface.name.as_str())
}

/// The text of the metered file for `cards`, each with whether it is
/// metered, and the card `default` that carries the default route: the
/// first line says whether that one is, and `no` when there is none.
fn metered_text(cards: &[(&str, bool)], default: Option<&str>) -> String {
    let word = |metered: bool| if metered { "yes" } else { "no" };
    let on_default = cards.iter().any(|(name, metered)| Some(*name) == default && *metered);
    let mut text = format!("{}\n", word(on_default));
    for (name, metered) in cards {
        text += &format!("{name} {}\n", word(*metered));
    }
    text
}

/// The table `netd leases` shows for the lines of [`SERVER_LISTING`] at
/// `wall`: each lease with the time left of it, and each reservation no
/// client has. A line that is not one, or a lease that has ended, is left
/// out.
fn leases_table(text: &str, wall: u64) -> String {
    let mut rows: Vec<[String; 5]> = Vec::new();
    for line in text.lines() {
        let [card, address, mac, name, ends] = line.split_whitespace().collect::<Vec<_>>()[..]
        else {
            continue;
        };
        let left = match ends.parse::<u64>() {
            Ok(ends) if ends > wall => util::elapsed(ends - wall),
            Ok(_) => continue,
            Err(_) if ends == "reserved" => "reserved".to_string(),
            Err(_) => continue,
        };
        let name = if name == "-" { "" } else { name };
        rows.push([card, address, mac, name, &left].map(str::to_string));
    }
    if rows.is_empty() {
        return String::new();
    }
    let header = ["CARD", "ADDRESS", "MAC", "NAME", "LEFT"].map(str::to_string);
    rows.insert(0, header);
    let widths: Vec<usize> =
        (0..5).map(|at| rows.iter().map(|row| row[at].len()).max().unwrap_or(0)).collect();
    let mut table = String::new();
    for row in &rows {
        let cells: Vec<String> =
            row.iter().zip(&widths).map(|(cell, width)| format!("{cell:width$}")).collect();
        table += cells.join("  ").trim_end();
        table += "\n";
    }
    table
}

/// `netd leases`: the leases the cards' DHCP servers have given.
fn leases(args: &mut Args) -> Result<i32, Fail> {
    if let Some(arg) = args.next()? {
        return Err(args.unknown(&arg));
    }
    match fs::read_to_string(SERVER_LISTING) {
        Ok(text) => {
            print!("{}", leases_table(&text, wall()));
            Ok(0)
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Err(util::fail(
            "no card gives addresses by DHCP: network(5)'s dhcp-server turns it on".into(),
        )),
        Err(err) => Err(util::failed(&SERVER_LISTING, err)),
    }
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
    if let Some(command) = args.subcommand() {
        return match command.to_str() {
            Some("leases") => leases(args),
            _ => Err(util::usage("usage: netd [leases]".into())),
        };
    }
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
        serving_before: Vec::new(),
        listing: Listing::default(),
        metered: None,
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

    const EXAMPLE: &str = "[net1]\n\
         address = 192.168.10.1/24\n\
         router = 192.168.10.254\n\
         dhcp-server = 192.168.10.100-192.168.10.199\n\
         dhcp-server-lease = 12h\n\
         dhcp-server-router = 192.168.10.254\n\
         dhcp-server-dns = 192.168.10.1\n\
         dhcp-server-domain = lab.example\n\
         dhcp-server-time = 192.168.10.1\n\
         dhcp-server-metered = no\n\
         reserve = 52:54:00:12:34:56 192.168.10.20 printer\n\
         metered = auto\n";

    fn ip(text: &str) -> Ipv4Addr {
        text.parse().unwrap()
    }

    #[test]
    fn a_card_serves_dhcp_as_its_settings_say() {
        let (cards, wrong) = parse_settings(EXAMPLE);
        assert_eq!(wrong, Vec::<String>::new());
        let (name, net1) = &cards[0];
        assert_eq!(name, "net1");
        let mac = [0x52, 0x54, 0, 0x12, 0x34, 0x56];
        assert_eq!(
            net1.server,
            ServerSetup {
                range: Some((ip("192.168.10.100"), ip("192.168.10.199"))),
                lease: Some(Duration::from_hours(12)),
                router: Some(Some(ip("192.168.10.254"))),
                dns: Some(vec![ip("192.168.10.1")]),
                domain: Some("lab.example".into()),
                time: Some(Some(ip("192.168.10.1"))),
                metered: Some(false),
                reserved: vec![(
                    11,
                    Reservation { mac, address: ip("192.168.10.20"), name: Some("printer".into()) }
                )],
            }
        );
        assert_eq!(net1.metered, None);
        // The card keeps its own address, and asks no one for another.
        assert!(!net1.dhcp() && !net1.alone());
        // What is left out is the default, to be worked out; each of the
        // values `none` is its own.
        let (cards, wrong) = parse_settings(
            "[net1]\naddress = 10.1.0.1/16\ndhcp-server = 10.1.1.1-10.1.1.9\n\
             dhcp-server-router = none\ndhcp-server-dns = none\ndhcp-server-time = none\n\
             dhcp-server-dns = 10.1.0.1\nmetered = yes\n",
        );
        assert_eq!(wrong, [format!("{SETTINGS}:7: '10.1.0.1' is not a dhcp-server-dns here")]);
        let server = &cards[0].1.server;
        assert_eq!(
            (server.router, &server.dns, server.time),
            (Some(None), &Some(vec![]), Some(None))
        );
        assert_eq!((server.lease, &server.domain, server.metered), (None, &None, None));
        assert_eq!(cards[0].1.metered, Some(true));
        // Up to three DNS servers, on a line or on lines.
        let (cards, wrong) = parse_settings(
            "[net1]\naddress = 10.1.0.1/16\ndhcp-server = 10.1.1.1-10.1.1.9\n\
             dhcp-server-dns = 1.1.1.1 9.9.9.9\ndhcp-server-dns = 8.8.8.8\n\
             dhcp-server-dns = 8.8.4.4\n",
        );
        assert_eq!(wrong, [format!("{SETTINGS}:6: '8.8.4.4' is not a dhcp-server-dns here")]);
        assert_eq!(cards[0].1.server.dns.as_ref().map(Vec::len), Some(3));
    }

    #[test]
    fn a_server_that_cannot_run_is_left_off_and_why_is_said() {
        // The server's range, and what is said of it, on a card of
        // `lines`.
        let card = |lines: &str| {
            let (cards, wrong) = parse_settings(&format!("[net1]\n{lines}"));
            (cards[0].1.server.range.is_some(), wrong)
        };
        let on = (true, Vec::<String>::new());
        let off = |why: &str| (false, vec![format!("net1: {why}")]);
        let range = "dhcp-server = 192.168.10.100-192.168.10.199\n";
        let with = |more: &str| card(&format!("address = 192.168.10.1/24\n{range}{more}"));
        assert_eq!(with(""), on);
        assert_eq!(card(range), off("dhcp-server needs an address of the card's own"));
        assert_eq!(with("dhcp = yes\n"), off("dhcp-server cannot go with dhcp = yes"));
        assert_eq!(
            card("dhcp = no\naddress = fd00::1/64\ndhcp-server = 10.0.0.5-10.0.0.9\n"),
            off("dhcp-server needs an address of the card's own")
        );
        // The range lies inside the network, and holds neither its own
        // address, the broadcast address, nor the card's.
        let range_of = |address: &str, range: &str| {
            card(&format!("address = {address}\ndhcp-server = {range}\n"))
        };
        let net = "192.168.10.1/24";
        assert_eq!(
            range_of(net, "192.168.10.150-192.168.10.100"),
            off("dhcp-server 192.168.10.150-192.168.10.100 ends before it begins")
        );
        assert_eq!(
            range_of(net, "192.168.10.100-192.168.11.20"),
            off("dhcp-server 192.168.10.100-192.168.11.20 is not inside the card's network, \
                 192.168.10.0/24")
        );
        for range in [
            "192.168.10.0-192.168.10.9",
            "192.168.10.200-192.168.10.255",
            "192.168.10.70-192.168.10.80",
        ] {
            let said = format!(
                "dhcp-server {range} holds the network's own address, its broadcast address or \
                 the card's own"
            );
            assert_eq!(range_of("192.168.10.77/24", range), off(&said), "{range}");
        }
        assert_eq!(range_of("192.168.10.77/24", "192.168.10.78-192.168.10.254"), on);
        // At most 65536 addresses.
        assert_eq!(
            range_of("10.0.0.1/8", "10.0.1.0-10.1.1.0"),
            off("dhcp-server 10.0.1.0-10.1.1.0 holds more than 65536 addresses")
        );
        assert_eq!(range_of("10.0.0.1/8", "10.0.1.0-10.1.0.255"), on);
        // The address whose network holds the range is the one the server
        // is on.
        let two =
            "address = 192.168.1.1/24\naddress = 10.1.0.1/16\ndhcp-server = 10.1.5.1-10.1.5.9\n";
        assert_eq!(card(two), on);
        // A router on the network, or `none`, and one that is not.
        let router = |router: &str| with(&format!("dhcp-server-router = {router}\n"));
        assert_eq!(router("192.168.10.1"), on);
        assert_eq!(router("none"), on);
        assert_eq!(
            router("192.168.11.1"),
            off("dhcp-server-router 192.168.11.1 is not on the card's network, 192.168.10.0/24")
        );
        assert!(!router("192.168.10.255").0);
        // Settings of the server without its range do nothing, and are
        // said to.
        assert_eq!(
            card("address = 10.0.0.1/24\ndhcp-server-lease = 1h\n"),
            off("its dhcp-server settings do nothing without dhcp-server")
        );
    }

    #[test]
    fn what_is_wrong_in_the_servers_lines_is_said_by_line() {
        let (cards, wrong) = parse_settings(
            "[net1]\n\
             address = 192.168.10.1/24\n\
             dhcp-server = 192.168.10.100\n\
             dhcp-server = 192.168.10.100-192.168.10.199\n\
             dhcp-server = 192.168.10.50-192.168.10.60\n\
             dhcp-server-lease = 30s\n\
             dhcp-server-lease = 31d\n\
             dhcp-server-lease = twelve\n\
             dhcp-server-lease = h\n\
             dhcp-server-lease = 90m\n\
             dhcp-server-router = 192.168.10.254 192.168.10.253\n\
             dhcp-server-dns = 1.1.1.1 9.9.9.9 8.8.8.8 8.8.4.4\n\
             dhcp-server-dns = none 1.1.1.1\n\
             dhcp-server-dns =\n\
             dhcp-server-domain = bad domain\n\
             dhcp-server-time = soon\n\
             dhcp-server-metered = maybe\n\
             metered = sometimes\n",
        );
        let server = &cards[0].1.server;
        assert_eq!(server.range, Some((ip("192.168.10.100"), ip("192.168.10.199"))));
        assert_eq!(server.lease, Some(Duration::from_mins(90)));
        assert_eq!(wrong.len(), 14, "{wrong:#?}");
        let said = |at: usize, key: &str, value: &str| {
            format!("{SETTINGS}:{at}: '{value}' is not a {key} here")
        };
        assert_eq!(wrong[0], said(3, "dhcp-server", "192.168.10.100"));
        assert_eq!(wrong[1], said(5, "dhcp-server", "192.168.10.50-192.168.10.60"));
        assert_eq!(wrong[2], said(6, "dhcp-server-lease", "30s"));
        assert_eq!(wrong[3], said(7, "dhcp-server-lease", "31d"));
        assert_eq!(wrong[4], said(8, "dhcp-server-lease", "twelve"));
        assert_eq!(wrong[5], said(9, "dhcp-server-lease", "h"));
        assert_eq!(wrong[6], said(11, "dhcp-server-router", "192.168.10.254 192.168.10.253"));
        assert_eq!(wrong[7], said(12, "dhcp-server-dns", "1.1.1.1 9.9.9.9 8.8.8.8 8.8.4.4"));
        assert_eq!(wrong[8], said(13, "dhcp-server-dns", "none 1.1.1.1"));
        assert_eq!(wrong[9], said(14, "dhcp-server-dns", ""));
        assert_eq!(wrong[10], said(15, "dhcp-server-domain", "bad domain"));
        assert_eq!(wrong[11], said(16, "dhcp-server-time", "soon"));
        assert_eq!(wrong[12], said(17, "dhcp-server-metered", "maybe"));
        assert_eq!(wrong[13], said(18, "metered", "sometimes"));
        // The line left out leaves the default; the others stand.
        assert_eq!((server.router, &server.dns, server.time), (None, &None, None));
        assert_eq!(cards[0].1.metered, None);
    }

    #[test]
    fn a_reservation_is_a_mac_address_an_address_and_a_name() {
        let (cards, wrong) = parse_settings(
            "[net1]\n\
             address = 192.168.10.1/24\n\
             dhcp-server = 192.168.10.100-192.168.10.199\n\
             reserve = 52:54:00:12:34:56 192.168.10.20 printer\n\
             reserve = 52:54:00:12:34:57 192.168.10.150\n\
             reserve = 52:54:00:12:34:58 192.168.11.20 elsewhere\n\
             reserve = 52:54:00:12:34:59 192.168.10.1 itself\n\
             reserve = 52:54:00:12:34:5a 192.168.10.255\n\
             reserve = 52:54:00:12:34:56 192.168.10.21 same-mac\n\
             reserve = 52:54:00:12:34:5b 192.168.10.20 same-address\n\
             reserve = 52:54:00:12:34 192.168.10.30\n\
             reserve = 52:54:00:12:34:5c 192.168.10 short\n\
             reserve = 52:54:00:12:34:5d 192.168.10.31 bad/name\n\
             reserve = 52:54:00:12:34:5e 192.168.10.32 two names\n\
             reserve = 52:54:00:12:34:5f\n",
        );
        let reserved: Vec<(usize, String)> = cards[0]
            .1
            .server
            .reserved
            .iter()
            .map(|(at, reservation)| (*at, reservation.address.to_string()))
            .collect();
        assert_eq!(reserved, [(4, "192.168.10.20".to_string()), (5, "192.168.10.150".to_string())]);
        assert_eq!(cards[0].1.server.reserved[1].1.name, None);
        let lines: Vec<&str> = wrong.iter().map(String::as_str).collect();
        assert_eq!(
            lines,
            [
                format!("{SETTINGS}:9: '52:54:00:12:34:56 192.168.10.21 same-mac' is not a reserve here").as_str(),
                format!("{SETTINGS}:10: '52:54:00:12:34:5b 192.168.10.20 same-address' is not a reserve here").as_str(),
                format!("{SETTINGS}:11: '52:54:00:12:34 192.168.10.30' is not a reserve here").as_str(),
                format!("{SETTINGS}:12: '52:54:00:12:34:5c 192.168.10 short' is not a reserve here").as_str(),
                format!("{SETTINGS}:13: '52:54:00:12:34:5d 192.168.10.31 bad/name' is not a reserve here").as_str(),
                format!("{SETTINGS}:14: '52:54:00:12:34:5e 192.168.10.32 two names' is not a reserve here").as_str(),
                format!("{SETTINGS}:15: '52:54:00:12:34:5f' is not a reserve here").as_str(),
                format!("{SETTINGS}:6: reserve 192.168.11.20 is not a client's address on net1's network, 192.168.10.0/24").as_str(),
                format!("{SETTINGS}:7: reserve 192.168.10.1 is not a client's address on net1's network, 192.168.10.0/24").as_str(),
                format!("{SETTINGS}:8: reserve 192.168.10.255 is not a client's address on net1's network, 192.168.10.0/24").as_str(),
            ]
        );
        // At most 1024 of them.
        let mut text =
            String::from("[net1]\naddress = 10.0.0.1/8\ndhcp-server = 10.1.0.1-10.1.0.9\n");
        for n in 0..1026_u32 {
            text += &format!(
                "reserve = 52:54:00:00:{:02x}:{:02x} 10.2.{}.{}\n",
                n / 256,
                n % 256,
                n / 256,
                n % 256
            );
        }
        let (cards, wrong) = parse_settings(&text);
        assert_eq!(cards[0].1.server.reserved.len(), RESERVATIONS_MAX);
        assert_eq!(wrong.len(), 2);
    }

    #[test]
    fn lease_times_are_a_number_and_a_unit() {
        let secs = |text| lease_time(text).map(|time| time.as_secs());
        assert_eq!(secs("60s"), Some(60));
        assert_eq!(secs("1m"), Some(60));
        assert_eq!(secs("12h"), Some(43_200));
        assert_eq!(secs("30d"), Some(2_592_000));
        for bad in [
            "59s",
            "31d",
            "721h",
            "0m",
            "",
            "m",
            "1",
            "1w",
            "+5m",
            "-5m",
            "5 m",
            "1.5h",
            "99999999999999999999d",
        ] {
            assert_eq!(secs(bad), None, "{bad}");
        }
    }

    fn netd() -> Netd {
        Netd {
            cards: Vec::new(),
            settings: Vec::new(),
            scanned: None,
            network: &ip::Slots,
            secret: [0; 16],
            ipv6_before: Vec::new(),
            dhcp6_before: Vec::new(),
            serving_before: Vec::new(),
            listing: Listing::default(),
            metered: None,
        }
    }

    #[test]
    fn a_server_pool_has_the_defaults_the_settings_leave_out() {
        let mut netd = netd();
        // The DNS servers resolv.conf names: not loopback, not IPv6, once
        // each, at most three.
        let dns =
            ["127.0.0.53", "10.0.2.3", "fec0::3", "10.0.2.3", "1.1.1.1", "8.8.8.8", "9.9.9.9"];
        netd.cards
            .push(Card { dns: dns.map(|dns| dns.parse().unwrap()).to_vec(), ..placeholder() });
        let pool = |text: &str, timed: &str| {
            let (cards, wrong) = parse_settings(text);
            assert_eq!(wrong, Vec::<String>::new());
            netd.pool_of("net1", &cards[0].1, timed).unwrap()
        };
        let bare = "[net1]\naddress = 192.168.10.1/24\nrouter = 192.168.10.254\n\
                    dhcp-server = 192.168.10.100-192.168.10.199\n";
        let serving = "serve = nts+ntp\nserve-certificate = /system/settings/ssl/a.pem\n\
                       serve-key = /system/settings/ssl/a.key\n";
        let pool_bare = pool(bare, serving);
        assert_eq!(
            (pool_bare.address, pool_bare.prefix, pool_bare.lease, pool_bare.router),
            (ip("192.168.10.1"), 24, Duration::from_hours(12), Some(ip("192.168.10.254")))
        );
        assert_eq!(pool_bare.dns, [ip("10.0.2.3"), ip("1.1.1.1"), ip("8.8.8.8")]);
        // The card's address is the time server while timed answers plain
        // NTP from the pool, which DHCP clients speak: not NTS alone, nor
        // to other networks, nor without a certificate.
        assert_eq!(pool_bare.time, Some(ip("192.168.10.1")));
        assert_eq!(pool(bare, "").time, None);
        assert_eq!(pool(bare, &serving.replace("nts+ntp", "nts")).time, None);
        let elsewhere = format!("{serving}serve-allow = 10.0.0.0/8\n");
        assert_eq!(pool(bare, &elsewhere).time, None);
        let here = format!("{serving}serve-allow = 10.0.0.0/8, 192.168.10.0/24\n");
        assert_eq!(pool(bare, &here).time, Some(ip("192.168.10.1")));
        assert_eq!(pool(bare, "serve = nts+ntp\n").time, None);
        assert_eq!(
            (pool_bare.domain, pool_bare.metered, pool_bare.reserved.len()),
            (None, false, 0)
        );
        // What the settings say is used, and `none` is none.
        let set = format!(
            "{bare}dhcp-server-router = none\ndhcp-server-dns = 192.168.10.1\n\
             dhcp-server-time = none\ndhcp-server-lease = 1m\ndhcp-server-metered = yes\n\
             dhcp-server-domain = lab.example\n"
        );
        let pool_set = pool(&set, serving);
        assert_eq!((pool_set.router, pool_set.time), (None, None));
        assert_eq!(pool_set.dns, [ip("192.168.10.1")]);
        assert_eq!((pool_set.lease, pool_set.metered), (Duration::from_mins(1), true));
        assert_eq!(pool_set.domain.as_deref(), Some("lab.example"));
        // A section's router that is not on the network is not given.
        let (cards, _) = parse_settings(
            "[net1]\naddress = 192.168.10.1/24\naddress = 10.0.0.1/8\nrouter = 10.0.0.254\n\
             dhcp-server = 192.168.10.100-192.168.10.199\n",
        );
        assert_eq!(netd.pool_of("net1", &cards[0].1, "").unwrap().router, None);
        // No range, no pool.
        let (cards, _) = parse_settings("[net1]\naddress = 192.168.10.1/24\n");
        assert_eq!(netd.pool_of("net1", &cards[0].1, ""), None);
    }

    /// A card with the routes given: an IPv4 default router, an IPv6 one.
    fn routed(name: &str, v4: Option<Ipv4Addr>, v6: Option<Ipv6Addr>) -> Interface {
        Interface {
            name: name.to_string(),
            index: 2,
            mtu: 1500,
            mac: "52:54:00:12:34:56".to_string(),
            loopback: false,
            carrier: true,
            operstate: "up".to_string(),
            addresses: Vec::new(),
            router: v4,
            ipv6: ip::Ipv6 { given_router: v6, ..ip::Ipv6::default() },
            kind: ip::Kind::Card,
            master: None,
            received: [0; 4],
            sent: [0; 4],
        }
    }

    #[test]
    fn the_default_route_is_carried_by_the_card_that_has_one() {
        let v4 = Some(Ipv4Addr::new(192, 168, 1, 1));
        let v6 = Some("fe80::1".parse().unwrap());
        assert_eq!(default_route_card(&[]), None);
        assert_eq!(default_route_card(&[routed("net0", None, None)]), None);
        let cards = [routed("net0", None, None), routed("net1", v4, None)];
        assert_eq!(default_route_card(&cards), Some("net1"));
        // An IPv4 route is preferred; one that is only IPv6 still counts.
        let cards = [routed("net0", None, v6), routed("net1", v4, None)];
        assert_eq!(default_route_card(&cards), Some("net1"));
        let cards = [routed("net0", None, None), routed("net1", None, v6)];
        assert_eq!(default_route_card(&cards), Some("net1"));
    }

    #[test]
    fn the_metered_file_says_whether_the_default_connection_is() {
        // The card with the default route decides the first line.
        assert_eq!(
            metered_text(&[("net0", false), ("net1", true)], Some("net1")),
            "yes\nnet0 no\nnet1 yes\n"
        );
        // A metered card that is not the way out does not matter.
        assert_eq!(
            metered_text(&[("net0", false), ("net1", true)], Some("net0")),
            "no\nnet0 no\nnet1 yes\n"
        );
        // Without a default route nothing is metered.
        assert_eq!(metered_text(&[("net1", true)], None), "no\nnet1 yes\n");
        assert_eq!(metered_text(&[], None), "no\n");
    }

    #[test]
    fn netd_leases_shows_each_lease_with_its_time_left() {
        let text = "net1 192.168.10.20 52:54:00:12:34:58 printer reserved\n\
                    net1 192.168.10.100 52:54:00:12:34:56 laptop 1800043200\n\
                    net1 192.168.10.101 52:54:00:12:34:57 - 1800000100\n\
                    net1 192.168.10.102 52:54:00:12:34:59 gone 1799999999\n\
                    not a lease\n\
                    net1 192.168.10.103 52:54:00:12:34:5a - soon\n\
                    net2 10.0.0.5 52:54:00:12:34:5b phone 1800003600\n";
        assert_eq!(
            leases_table(text, 1_800_000_000),
            "CARD  ADDRESS         MAC                NAME     LEFT\n\
             net1  192.168.10.20   52:54:00:12:34:58  printer  reserved\n\
             net1  192.168.10.100  52:54:00:12:34:56  laptop   12:00:00\n\
             net1  192.168.10.101  52:54:00:12:34:57           01:40\n\
             net2  10.0.0.5        52:54:00:12:34:5b  phone    01:00:00\n"
        );
        assert_eq!(leases_table("", 1_800_000_000), "");
        assert_eq!(
            leases_table("net1 192.168.10.102 52:54:00:12:34:59 gone 5\n", 1_800_000_000),
            ""
        );
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
        let metered_text = lease_text(&metered, "net0", 5);
        assert!(metered_text.ends_with("rebind = 75600\nmetered = yes\n"));
        assert_eq!(parse_lease(&metered_text), Some((metered, 5)));
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
