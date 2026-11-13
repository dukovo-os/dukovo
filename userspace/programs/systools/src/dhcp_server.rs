// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The DHCPv4 server of netd(8) (RFC 2131, ADR 0486): what a card says to
//! the clients on its network, with no I/O of its own, so that the tests
//! can drive it with time of their own, as they do [`crate::dhcp`]'s
//! client.
//!
//! A card with an address of its own gives the clients on its network
//! addresses from a pool. A client has the address a reservation gives its
//! MAC address, else the one it had last (kept by its client identifier,
//! else its MAC address), else the one it asks for if that is free, else
//! the lowest free address. An address whose lease ended goes to another
//! client only when no free one is left. A lease lasts as long as the
//! pool says; the client asks for more at half of it. A client that finds
//! its address in use declines it, and it is left alone for ten minutes.
//!
//! Everything a client sends is read through bounds checks and checked
//! before it is used: a packet that is not a well-formed request from an
//! Ethernet client on this network is ignored, and never answered. Relayed
//! requests (`giaddr` set) are ignored too; netd says so once.
//!
//! The server keeps its leases in memory and in a file netd writes after
//! each change ([`leases_text`], [`parse_leases`]); netd's log has a line
//! for each new lease, decline and release. When a client gets no
//! address, look for `the pool ... is full` in the log first, then for
//! the leases with `netd leases`.

use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::{Duration, Instant};

use smoltcp::wire::{
    DHCP_MAX_DNS_SERVER_COUNT, DhcpFlags, DhcpMessageType, DhcpOpCode, DhcpOption, DhcpPacket,
    DhcpRepr, EthernetAddress,
};

use crate::dhcp::{self, ANDROID_METERED};
use crate::ip::Cidr;

/// The port clients listen on.
const CLIENT_PORT: u16 = 68;

/// The sizes of a request the server takes: a message with its fixed
/// header and cookie, and the largest a client may send (RFC 2131).
const REQUEST_MIN: usize = 240;
const REQUEST_MAX: usize = dhcp::MESSAGE_MAX as usize;

/// The longest client identifier taken: a DUID of at most 130 bytes
/// behind a type byte and an IAID (RFC 4361). A longer one is no client's.
const CLIENT_ID_MAX: usize = 135;

/// The longest host name kept with a lease.
const NAME_MAX: usize = 63;

/// How long an offer is held for the client it was made to.
const OFFER_HOLD: Duration = Duration::from_secs(60);

/// How long an address a client declined is left alone.
const DECLINE_WAIT: Duration = Duration::from_mins(10);

/// The most addresses a range may hold.
pub const RANGE_MAX: u32 = 65_536;

const OPTION_HOST_NAME: u8 = 12;
const OPTION_DOMAIN: u8 = 15;
const OPTION_NTP_SERVERS: u8 = 42;
const OPTION_VENDOR: u8 = 43;
const OPTION_REQUESTED_ADDRESS: u8 = 50;
const OPTION_MESSAGE_TYPE: u8 = 53;
const OPTION_SERVER_IDENTIFIER: u8 = 54;
const OPTION_RENEWAL_TIME: u8 = 58;
const OPTION_REBINDING_TIME: u8 = 59;
const OPTION_CLIENT_IDENTIFIER: u8 = 61;

/// The cookie that follows the fixed header (RFC 2131, 3).
const MAGIC_COOKIE: u32 = 0x6382_5363;

/// What one card's server gives, and to whom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pool {
    /// The card, for the log.
    pub card: String,
    /// The card's address and the prefix of its network: the server
    /// identifier, and the network the clients are on.
    pub address: Ipv4Addr,
    pub prefix: u8,
    /// The addresses given to clients without a reservation, `first` to
    /// `last`.
    pub first: Ipv4Addr,
    pub last: Ipv4Addr,
    pub lease: Duration,
    pub router: Option<Ipv4Addr>,
    /// The DNS servers, at most [`DHCP_MAX_DNS_SERVER_COUNT`] of them
    /// are sent.
    pub dns: Vec<Ipv4Addr>,
    pub domain: Option<String>,
    /// The NTP server (option 42).
    pub time: Option<Ipv4Addr>,
    /// Whether the clients are told the connection is metered (option 43,
    /// `ANDROID_METERED`).
    pub metered: bool,
    pub reserved: Vec<Reservation>,
}

/// The address a client with a MAC address always gets, and never anyone
/// else, even when it is outside the range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reservation {
    pub mac: [u8; 6],
    pub address: Ipv4Addr,
    pub name: Option<String>,
}

/// An address given to a client, until `ends`, in seconds since 1970.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lease {
    /// The client's identifier: its option 61, else `01` and its MAC
    /// address.
    pub client: Vec<u8>,
    pub mac: [u8; 6],
    pub address: Ipv4Addr,
    pub ends: u64,
    /// The reservation's name, else the host name the client gave.
    pub name: Option<String>,
}

/// What the server answers: nothing, or a message to send.
#[derive(Debug, PartialEq, Eq)]
pub enum Reply {
    None,
    Send { to: SocketAddrV4, message: Vec<u8> },
}

/// What happened that netd logs or keeps in its files.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    /// A client has an address it had no lease of: logged, and kept.
    Leased(Lease),
    /// A client's lease was extended: kept, not logged.
    Renewed,
    /// A client gave its address up. The lease has ended.
    Released(Lease),
    /// A client found its address in use by another machine.
    Declined { address: Ipv4Addr, mac: [u8; 6], name: Option<String> },
    /// A client asked, and no address of the pool is free. Said once
    /// until an address is.
    Full { mac: [u8; 6] },
    /// A request that came through a relay was ignored. Said once.
    Relayed,
}

/// A card's server.
#[derive(Debug)]
pub struct Server {
    pool: Pool,
    leases: Vec<Lease>,
    /// The offers made, and when: held for [`OFFER_HOLD`].
    offers: Vec<(Lease, Instant)>,
    /// The addresses declined, and when.
    declined: Vec<(Ipv4Addr, Instant)>,
    /// Whether the log said the pool is full, and that requests came
    /// through a relay.
    full: bool,
    relayed: bool,
}

/// What a client asked, read from its packet.
struct Request {
    kind: DhcpMessageType,
    xid: u32,
    broadcast: bool,
    ciaddr: Ipv4Addr,
    relayed: bool,
    mac: [u8; 6],
    client: Vec<u8>,
    requested: Option<Ipv4Addr>,
    server: Option<Ipv4Addr>,
    name: Option<String>,
}

/// What the server answers a request with.
enum Answer {
    Offer(Ipv4Addr),
    Ack(Ipv4Addr),
    /// An acknowledgement of an INFORM: the options, no address, no lease.
    Inform,
    Nak,
}

impl Request {
    /// The request `bytes` hold, if they are a well-formed message of a
    /// client on Ethernet that a server answers.
    fn parse(bytes: &[u8]) -> Option<Request> {
        if !(REQUEST_MIN..=REQUEST_MAX).contains(&bytes.len()) {
            return None;
        }
        let packet = DhcpPacket::new_checked(bytes).ok()?;
        let ethernet = u16::from(packet.hardware_type()) == 1 && packet.hardware_len() == 6;
        if packet.opcode() != DhcpOpCode::Request
            || !ethernet
            || packet.magic_number() != MAGIC_COOKIE
        {
            return None;
        }
        let mac = packet.client_hardware_address().0;
        // A multicast address cannot be a client's, and all zeros is none.
        if mac[0] & 1 == 1 || mac == [0; 6] {
            return None;
        }
        let (mut kind, mut requested, mut server, mut client, mut name) =
            (None, None, None, None, None);
        for option in packet.options() {
            match (option.kind, option.data) {
                (OPTION_MESSAGE_TYPE, &[byte]) => {
                    kind = kind.or(Some(DhcpMessageType::from(byte)));
                }
                (OPTION_REQUESTED_ADDRESS, &[a, b, c, d]) => {
                    requested = requested.or(Some(Ipv4Addr::new(a, b, c, d)));
                }
                (OPTION_SERVER_IDENTIFIER, &[a, b, c, d]) => {
                    server = server.or(Some(Ipv4Addr::new(a, b, c, d)));
                }
                (OPTION_CLIENT_IDENTIFIER, id) if id.len() > CLIENT_ID_MAX => return None,
                (OPTION_CLIENT_IDENTIFIER, id) if id.len() >= 2 => {
                    client = client.or(Some(id.to_vec()));
                }
                (OPTION_HOST_NAME, text) => name = name.or_else(|| host_name(text)),
                _ => {}
            }
        }
        let kind = kind.filter(|kind| {
            matches!(
                kind,
                DhcpMessageType::Discover
                    | DhcpMessageType::Request
                    | DhcpMessageType::Decline
                    | DhcpMessageType::Release
                    | DhcpMessageType::Inform
            )
        })?;
        Some(Request {
            kind,
            xid: packet.transaction_id(),
            broadcast: packet.flags().contains(DhcpFlags::BROADCAST),
            ciaddr: packet.client_ip(),
            relayed: !packet.relay_agent_ip().is_unspecified(),
            mac,
            client: client.unwrap_or_else(|| [&[1][..], &mac[..]].concat()),
            requested,
            server,
            name,
        })
    }
}

/// A host name a client gave, if it is one that can be shown and kept:
/// letters, digits, `-`, `_` and `.`, and at most [`NAME_MAX`] of them.
fn host_name(text: &[u8]) -> Option<String> {
    // Some clients end it with a NUL, which is no part of it.
    let end = text.iter().rposition(|&byte| byte != 0)? + 1;
    let name = std::str::from_utf8(&text[..end]).ok()?;
    valid_name(name).then(|| name.to_string())
}

/// Whether `name` is a name a lease or a reservation may have.
pub fn valid_name(name: &str) -> bool {
    (1..=NAME_MAX).contains(&name.len())
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

/// `52:54:00:12:34:56`.
pub fn mac_text(mac: [u8; 6]) -> String {
    let parts: Vec<String> = mac.iter().map(|byte| format!("{byte:02x}")).collect();
    parts.join(":")
}

/// A MAC address, `52:54:00:12:34:56`.
pub fn mac_of(text: &str) -> Option<[u8; 6]> {
    let mut mac = [0; 6];
    let mut parts = text.split(':');
    for byte in &mut mac {
        *byte = u8::from_str_radix(parts.next()?, 16).ok()?;
    }
    parts.next().is_none().then_some(mac)
}

/// Why `first` to `last` cannot be the range of a server on `network`, if
/// it cannot: the range is checked where the settings are read, so that
/// the server is never built with one that holds an address no client may
/// have.
///
/// # Errors
///
/// A sentence that finishes "the range ...": what is wrong with it.
pub fn check_range(network: Cidr, first: Ipv4Addr, last: Ipv4Addr) -> Result<(), String> {
    let (low, high) = (u32::from(first), u32::from(last));
    if high < low {
        return Err("ends before it begins".into());
    }
    if !network.contains(first) || !network.contains(last) {
        let inside = format!("{}/{}", network.network(), network.prefix);
        return Err(format!("is not inside the card's network, {inside}"));
    }
    if high - low >= RANGE_MAX {
        return Err(format!("holds more than {RANGE_MAX} addresses"));
    }
    let held = |ip: Ipv4Addr| (low..=high).contains(&u32::from(ip));
    if held(network.network()) || network.broadcast().is_some_and(held) || held(network.ip) {
        return Err(
            "holds the network's own address, its broadcast address or the card's own".into()
        );
    }
    Ok(())
}

impl Pool {
    fn network(&self) -> Cidr {
        Cidr { ip: self.address, prefix: self.prefix }
    }

    /// Whether `address` is one of the range's.
    fn in_range(&self, address: Ipv4Addr) -> bool {
        (u32::from(self.first)..=u32::from(self.last)).contains(&u32::from(address))
    }

    fn reservation_of(&self, mac: [u8; 6]) -> Option<&Reservation> {
        self.reserved.iter().find(|reservation| reservation.mac == mac)
    }

    /// Whether `address` is reserved for a MAC address other than `mac`.
    fn reserved_to_other(&self, address: Ipv4Addr, mac: [u8; 6]) -> bool {
        self.reserved
            .iter()
            .any(|reservation| reservation.address == address && reservation.mac != mac)
    }

    /// Whether a client of `mac` may keep `lease` after the pool changed:
    /// its address is one the range or its reservation gives it.
    fn allows(&self, lease: &Lease) -> bool {
        let reserved = self.reservation_of(lease.mac).is_some_and(|r| r.address == lease.address);
        (self.in_range(lease.address) || reserved)
            && lease.address != self.address
            && !self.reserved_to_other(lease.address, lease.mac)
    }
}

impl Server {
    /// A server for `pool` that goes on with the `leases` it had, those
    /// that still fit the pool: one for each client and each address.
    pub fn new(pool: Pool, leases: Vec<Lease>) -> Server {
        let mut server = Server {
            pool,
            leases,
            offers: Vec::new(),
            declined: Vec::new(),
            full: false,
            relayed: false,
        };
        server.keep_fitting();
        server
    }

    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    /// Changes the pool, keeping the leases and the offers that still fit
    /// it, and the addresses declined.
    pub fn set_pool(&mut self, pool: Pool) {
        self.pool = pool;
        let pool = &self.pool;
        self.offers.retain(|(offer, _)| pool.allows(offer));
        self.keep_fitting();
    }

    /// Every lease, those that have ended too: a client that comes back
    /// is given its last address again while it is free.
    pub fn leases(&self) -> &[Lease] {
        &self.leases
    }

    fn keep_fitting(&mut self) {
        let mut addresses = HashSet::new();
        let mut clients = HashSet::new();
        let pool = &self.pool;
        self.leases.retain(|lease| {
            pool.allows(lease)
                && addresses.insert(lease.address)
                && clients.insert(lease.client.clone())
        });
    }

    /// What to answer `packet`, a datagram to the server's port, at `now`
    /// and the time of day `wall`, in seconds since 1970; and what the
    /// answer changed. A packet that is no request this server answers
    /// gets no answer.
    pub fn handle(&mut self, packet: &[u8], now: Instant, wall: u64) -> (Reply, Option<Event>) {
        self.offers.retain(|(_, made)| now.saturating_duration_since(*made) < OFFER_HOLD);
        self.declined.retain(|(_, at)| now.saturating_duration_since(*at) < DECLINE_WAIT);
        let Some(request) = Request::parse(packet) else { return (Reply::None, None) };
        if request.relayed {
            let event = (!self.relayed).then_some(Event::Relayed);
            self.relayed = true;
            return (Reply::None, event);
        }
        match request.kind {
            DhcpMessageType::Discover => self.discover(&request, now, wall),
            DhcpMessageType::Request => self.request(&request, wall),
            DhcpMessageType::Decline => self.decline(&request, now),
            DhcpMessageType::Release => self.release(&request, wall),
            DhcpMessageType::Inform => self.inform(&request),
            _ => (Reply::None, None),
        }
    }

    fn discover(&mut self, request: &Request, now: Instant, wall: u64) -> (Reply, Option<Event>) {
        let Some(address) = self.choose(request, wall) else {
            let event = (!self.full).then_some(Event::Full { mac: request.mac });
            self.full = true;
            return (Reply::None, event);
        };
        self.full = false;
        let offer = self.lease_of(request, address, wall);
        self.offers.retain(|(held, _)| held.client != request.client);
        self.offers.push((offer, now));
        (self.reply(request, &Answer::Offer(address)), None)
    }

    /// The address to offer: the client's reservation, else the one it
    /// had last, else the one it asks for, else the lowest that is free.
    fn choose(&self, request: &Request, wall: u64) -> Option<Ipv4Addr> {
        let declined = |address: &Ipv4Addr| !self.declined.iter().any(|(wait, _)| wait == address);
        let reserved = self.pool.reservation_of(request.mac).map(|r| r.address).filter(declined);
        let last = self.leases.iter().find(|lease| lease.client == request.client);
        reserved
            .or_else(|| last.map(|lease| lease.address).filter(|a| self.usable(*a, request, wall)))
            .or_else(|| request.requested.filter(|a| self.usable(*a, request, wall)))
            .or_else(|| self.lowest_free(request, wall))
    }

    /// Whether the client may be given `address`, an address of the range
    /// that is not reserved for another, declined, offered to another or
    /// leased to another.
    fn usable(&self, address: Ipv4Addr, request: &Request, wall: u64) -> bool {
        self.pool.in_range(address)
            && !self.pool.reserved_to_other(address, request.mac)
            && !self.declined.iter().any(|(wait, _)| *wait == address)
            && !self
                .offers
                .iter()
                .any(|(offer, _)| offer.address == address && offer.client != request.client)
            && !self.leases.iter().any(|lease| {
                lease.address == address && lease.client != request.client && lease.ends > wall
            })
    }

    /// The lowest address of the range that no one has: of those that
    /// were never given, else the one whose lease ended longest ago.
    fn lowest_free(&self, request: &Request, wall: u64) -> Option<Ipv4Addr> {
        let mut busy: HashSet<Ipv4Addr> = self.pool.reserved.iter().map(|r| r.address).collect();
        busy.extend(self.declined.iter().map(|(address, _)| *address));
        busy.extend(
            self.offers
                .iter()
                .filter(|(offer, _)| offer.client != request.client)
                .map(|(offer, _)| offer.address),
        );
        // An address whose lease has not ended is busy; one that has is
        // kept for when no other is left.
        let mut ended: HashMap<Ipv4Addr, u64> = HashMap::new();
        for lease in self.leases.iter().filter(|lease| lease.client != request.client) {
            if lease.ends > wall {
                busy.insert(lease.address);
            } else {
                ended.insert(lease.address, lease.ends);
            }
        }
        let range = u32::from(self.pool.first)..=u32::from(self.pool.last);
        let free = range.map(Ipv4Addr::from).filter(|address| !busy.contains(address));
        let never_given = free.clone().find(|address| !ended.contains_key(address));
        never_given.or_else(|| free.min_by_key(|address| ended.get(address).copied()))
    }

    fn request(&mut self, request: &Request, wall: u64) -> (Reply, Option<Event>) {
        if let Some(server) = request.server {
            return self.selecting(request, server, wall);
        }
        if !request.ciaddr.is_unspecified() {
            return self.renewing(request, wall);
        }
        match request.requested {
            Some(address) => self.rebooting(request, address, wall),
            None => (Reply::None, None),
        }
    }

    /// A REQUEST that answers an offer, naming the server: another's
    /// takes the offer back, this server's is acknowledged when it asks
    /// for the address it was offered.
    fn selecting(
        &mut self,
        request: &Request,
        server: Ipv4Addr,
        wall: u64,
    ) -> (Reply, Option<Event>) {
        let offered = self.offers.iter().position(|(offer, _)| offer.client == request.client);
        let offer = offered.map(|at| self.offers.remove(at).0);
        if server != self.pool.address {
            return (Reply::None, None);
        }
        match (offer, request.requested) {
            (Some(offer), Some(address)) if offer.address == address => {
                self.acknowledge(request, address, wall)
            }
            _ => (self.reply(request, &Answer::Nak), None),
        }
    }

    /// A REQUEST of a client that has an address and asks to keep it
    /// (RENEWING, or REBINDING when broadcast).
    fn renewing(&mut self, request: &Request, wall: u64) -> (Reply, Option<Event>) {
        if self.holds(request, request.ciaddr) {
            self.acknowledge(request, request.ciaddr, wall)
        } else {
            (self.reply(request, &Answer::Nak), None)
        }
    }

    /// A REQUEST of a client that restarted and asks for the address it
    /// had (INIT-REBOOT): refused when it is wrong for this network or
    /// another's, and left unanswered when the server knows nothing of
    /// the client, as a server of other clients may.
    fn rebooting(
        &mut self,
        request: &Request,
        address: Ipv4Addr,
        wall: u64,
    ) -> (Reply, Option<Event>) {
        let other = self.pool.reserved_to_other(address, request.mac)
            || self.leases.iter().any(|lease| {
                lease.address == address && lease.client != request.client && lease.ends > wall
            });
        if !self.pool.network().contains(address) || other {
            return (self.reply(request, &Answer::Nak), None);
        }
        let known = self.pool.reservation_of(request.mac).is_some()
            || self.leases.iter().any(|lease| lease.client == request.client);
        if !known {
            return (Reply::None, None);
        }
        if self.holds(request, address) {
            self.acknowledge(request, address, wall)
        } else {
            (self.reply(request, &Answer::Nak), None)
        }
    }

    /// Whether the client holds `address`: a lease of its own, ended or
    /// not, as no one else has been given it, or its reservation.
    fn holds(&self, request: &Request, address: Ipv4Addr) -> bool {
        self.pool.reservation_of(request.mac).is_some_and(|r| r.address == address)
            || self
                .leases
                .iter()
                .any(|lease| lease.client == request.client && lease.address == address)
    }

    /// The lease the client has for `address` from `wall` on.
    fn lease_of(&self, request: &Request, address: Ipv4Addr, wall: u64) -> Lease {
        let reserved = self.pool.reservation_of(request.mac).and_then(|r| r.name.clone());
        let before = self.leases.iter().find(|lease| lease.client == request.client);
        let kept = before.and_then(|lease| lease.name.clone());
        Lease {
            client: request.client.clone(),
            mac: request.mac,
            address,
            ends: wall.saturating_add(self.pool.lease.as_secs()),
            name: reserved.or_else(|| request.name.clone()).or(kept),
        }
    }

    /// Gives the client `address` for a lease from `wall`, and
    /// acknowledges it. It has one lease: an earlier one of its own and
    /// another client's that ended on this address go.
    fn acknowledge(
        &mut self,
        request: &Request,
        address: Ipv4Addr,
        wall: u64,
    ) -> (Reply, Option<Event>) {
        let lease = self.lease_of(request, address, wall);
        let renewed = self.leases.iter().any(|before| {
            before.client == request.client && before.address == address && before.ends > wall
        });
        self.leases.retain(|before| before.client != lease.client && before.address != address);
        self.leases.push(lease.clone());
        self.full = false;
        let event = if renewed { Event::Renewed } else { Event::Leased(lease) };
        (self.reply(request, &Answer::Ack(address)), Some(event))
    }

    /// A DECLINE: the client found the address it was given in use. The
    /// address is left alone for ten minutes, and the lease goes. Only an
    /// address the client holds or was offered can be declined, so that a
    /// client cannot take the pool's addresses away from the others.
    fn decline(&mut self, request: &Request, now: Instant) -> (Reply, Option<Event>) {
        let Some(address) = request.requested else { return (Reply::None, None) };
        if request.server.is_some_and(|server| server != self.pool.address) {
            return (Reply::None, None);
        }
        let offered = self
            .offers
            .iter()
            .any(|(offer, _)| offer.client == request.client && offer.address == address);
        if !offered && !self.holds(request, address) {
            return (Reply::None, None);
        }
        self.leases.retain(|lease| !(lease.client == request.client && lease.address == address));
        self.offers.retain(|(offer, _)| offer.client != request.client);
        self.declined.push((address, now));
        let name = request.name.clone();
        (Reply::None, Some(Event::Declined { address, mac: request.mac, name }))
    }

    /// A RELEASE: the lease ends now, and its address stays the client's
    /// first choice while it is free.
    fn release(&mut self, request: &Request, wall: u64) -> (Reply, Option<Event>) {
        if request.server.is_some_and(|server| server != self.pool.address) {
            return (Reply::None, None);
        }
        let held = self.leases.iter_mut().find(|lease| {
            lease.client == request.client && lease.address == request.ciaddr && lease.ends > wall
        });
        let Some(lease) = held else { return (Reply::None, None) };
        lease.ends = wall;
        (Reply::None, Some(Event::Released(lease.clone())))
    }

    /// An INFORM: a client that has an address of its own asks for the
    /// network's settings. It gets no lease, and its address must be on
    /// this network, as the answer goes to it.
    fn inform(&self, request: &Request) -> (Reply, Option<Event>) {
        if request.ciaddr.is_unspecified() || !self.pool.network().contains(request.ciaddr) {
            return (Reply::None, None);
        }
        (self.reply(request, &Answer::Inform), None)
    }

    /// The message that answers `request`, sent to the client's address
    /// when it has one on this network, else to everyone on it: a client
    /// with no address cannot take a reply to one. A refusal always goes
    /// to everyone, as RFC 2131 (4.1) has it for a request that came
    /// without a relay.
    fn reply(&self, request: &Request, answer: &Answer) -> Reply {
        let Some(message) = self.message(request, answer) else { return Reply::None };
        let on_network = |ip: &Ipv4Addr| !ip.is_unspecified() && self.pool.network().contains(*ip);
        let to = Some(request.ciaddr)
            .filter(on_network)
            .filter(|_| !matches!(answer, Answer::Nak))
            .unwrap_or(Ipv4Addr::BROADCAST);
        Reply::Send { to: SocketAddrV4::new(to, CLIENT_PORT), message }
    }

    /// The bytes of the message: the options always sent, and those the
    /// pool has a value for.
    fn message(&self, request: &Request, answer: &Answer) -> Option<Vec<u8>> {
        let pool = &self.pool;
        let (kind, given) = match answer {
            Answer::Offer(address) => (DhcpMessageType::Offer, Some(*address)),
            Answer::Ack(address) => (DhcpMessageType::Ack, Some(*address)),
            Answer::Inform => (DhcpMessageType::Ack, None),
            Answer::Nak => (DhcpMessageType::Nak, None),
        };
        let seconds = u32::try_from(pool.lease.as_secs()).unwrap_or(u32::MAX);
        let rebind = u32::try_from(u64::from(seconds) * 7 / 8).unwrap_or(seconds);
        let (renew_time, rebind_time) = ((seconds / 2).to_be_bytes(), rebind.to_be_bytes());
        let time = pool.time.map(|time| time.octets());
        let mut options = Vec::new();
        let ok = !matches!(answer, Answer::Nak);
        if given.is_some() {
            options.push(DhcpOption { kind: OPTION_RENEWAL_TIME, data: &renew_time });
            options.push(DhcpOption { kind: OPTION_REBINDING_TIME, data: &rebind_time });
        }
        if let (true, Some(domain)) = (ok, &pool.domain) {
            options.push(DhcpOption { kind: OPTION_DOMAIN, data: domain.as_bytes() });
        }
        if let (true, Some(time)) = (ok, &time) {
            options.push(DhcpOption { kind: OPTION_NTP_SERVERS, data: time });
        }
        if ok && pool.metered {
            options.push(DhcpOption { kind: OPTION_VENDOR, data: ANDROID_METERED });
        }
        let dns = (ok && !pool.dns.is_empty())
            .then(|| pool.dns.iter().copied().take(DHCP_MAX_DNS_SERVER_COUNT).collect());
        let repr = DhcpRepr {
            message_type: kind,
            transaction_id: request.xid,
            secs: 0,
            client_hardware_address: EthernetAddress(request.mac),
            // An acknowledgement of a client that has an address names it;
            // an offer and a refusal leave it out (RFC 2131, table 3).
            client_ip: match answer {
                Answer::Ack(_) | Answer::Inform => request.ciaddr,
                Answer::Offer(_) | Answer::Nak => Ipv4Addr::UNSPECIFIED,
            },
            your_ip: given.unwrap_or(Ipv4Addr::UNSPECIFIED),
            server_ip: Ipv4Addr::UNSPECIFIED,
            router: pool.router.filter(|_| ok),
            subnet_mask: ok.then(|| Ipv4Addr::from(pool.network().mask())),
            relay_agent_ip: Ipv4Addr::UNSPECIFIED,
            broadcast: request.broadcast,
            requested_ip: None,
            client_identifier: None,
            server_identifier: Some(pool.address),
            parameter_request_list: None,
            dns_servers: dns,
            max_size: None,
            lease_duration: given.map(|_| seconds),
            renew_duration: None,
            rebind_duration: None,
            additional_options: &options,
        };
        let mut bytes = vec![0; repr.buffer_len().max(dhcp::MESSAGE_MIN)];
        repr.emit(&mut DhcpPacket::new_unchecked(&mut bytes[..])).ok()?;
        Some(bytes)
    }

    /// The lines of the file `netd leases` reads for this card, at `wall`:
    /// `card address mac name ends`, with `-` for no name, a line for each
    /// lease that has not ended, and `reserved` in place of `ends` for
    /// the reservations that have none.
    pub fn listing(&self, wall: u64) -> String {
        let mut rows: Vec<(Ipv4Addr, String)> = Vec::new();
        let name = |name: &Option<String>| name.clone().unwrap_or_else(|| "-".into());
        let pool = &self.pool;
        for lease in self.leases.iter().filter(|lease| lease.ends > wall) {
            let row = format!(
                "{} {} {} {} {}",
                pool.card,
                lease.address,
                mac_text(lease.mac),
                name(&lease.name),
                lease.ends
            );
            rows.push((lease.address, row));
        }
        for reservation in &pool.reserved {
            let leased = self.leases.iter().any(|lease| {
                lease.mac == reservation.mac
                    && lease.address == reservation.address
                    && lease.ends > wall
            });
            if !leased {
                let row = format!(
                    "{} {} {} {} reserved",
                    pool.card,
                    reservation.address,
                    mac_text(reservation.mac),
                    name(&reservation.name)
                );
                rows.push((reservation.address, row));
            }
        }
        rows.sort();
        rows.into_iter().map(|(_, row)| row + "\n").collect()
    }
}

/// The leases as netd keeps them in a card's file, a line each, by
/// address: `mac address ends client name`, the client's identifier in
/// hexadecimal or `-` when it is `01` and the MAC address, and `-` for no
/// name.
pub fn leases_text(leases: &[Lease]) -> String {
    let mut sorted: Vec<&Lease> = leases.iter().collect();
    sorted.sort_by_key(|lease| lease.address);
    let mut text = String::from("# netd(8)'s DHCP leases: mac address ends client name\n");
    for lease in sorted {
        let default = [&[1][..], &lease.mac[..]].concat();
        let client = if lease.client == default {
            "-".to_string()
        } else {
            lease.client.iter().fold(String::new(), |hex, byte| hex + &format!("{byte:02x}"))
        };
        text += &format!(
            "{} {} {} {client} {}\n",
            mac_text(lease.mac),
            lease.address,
            lease.ends,
            lease.name.as_deref().unwrap_or("-")
        );
    }
    text
}

/// The leases a file holds ([`leases_text`]); a line that is not one is
/// left out.
pub fn parse_leases(text: &str) -> Vec<Lease> {
    text.lines().filter(|line| !line.starts_with('#')).filter_map(parse_lease).collect()
}

fn parse_lease(line: &str) -> Option<Lease> {
    let [mac, address, ends, client, name] = line.split_whitespace().collect::<Vec<_>>()[..] else {
        return None;
    };
    let mac = mac_of(mac)?;
    let client = if client == "-" { [&[1][..], &mac[..]].concat() } else { hex_bytes(client)? };
    if !(2..=CLIENT_ID_MAX).contains(&client.len()) {
        return None;
    }
    let name = match name {
        "-" => None,
        name if valid_name(name) => Some(name.to_string()),
        _ => return None,
    };
    Some(Lease { client, mac, address: address.parse().ok()?, ends: ends.parse().ok()?, name })
}

/// The bytes `text` gives in hexadecimal.
fn hex_bytes(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len()).step_by(2).map(|at| u8::from_str_radix(&text[at..at + 2], 16).ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVER: Ipv4Addr = Ipv4Addr::new(192, 168, 10, 1);
    const LAPTOP: [u8; 6] = [0x52, 0x54, 0, 0x12, 0x34, 0x56];
    const PHONE: [u8; 6] = [0x52, 0x54, 0, 0x12, 0x34, 0x57];
    const PRINTER: [u8; 6] = [0x52, 0x54, 0, 0x12, 0x34, 0x58];
    const WALL: u64 = 1_800_000_000;
    const LEASE: u64 = 12 * 3600;

    fn address(last: u8) -> Ipv4Addr {
        Ipv4Addr::new(192, 168, 10, last)
    }

    /// A pool of four addresses, 192.168.10.100 to .103, with every
    /// option set and a printer's reservation outside the range.
    fn pool() -> Pool {
        Pool {
            card: "net1".into(),
            address: SERVER,
            prefix: 24,
            first: address(100),
            last: address(103),
            lease: Duration::from_secs(LEASE),
            router: Some(address(254)),
            dns: vec![SERVER, Ipv4Addr::new(9, 9, 9, 9)],
            domain: Some("lab.example".into()),
            time: Some(SERVER),
            metered: false,
            reserved: vec![Reservation {
                mac: PRINTER,
                address: address(20),
                name: Some("printer".into()),
            }],
        }
    }

    fn server() -> Server {
        Server::new(pool(), Vec::new())
    }

    /// What a client sends.
    #[derive(Clone, Copy)]
    struct Ask<'a> {
        kind: DhcpMessageType,
        mac: [u8; 6],
        ciaddr: Ipv4Addr,
        giaddr: Ipv4Addr,
        requested: Option<Ipv4Addr>,
        server: Option<Ipv4Addr>,
        id: Option<&'a [u8]>,
        name: Option<&'a str>,
    }

    fn ask(kind: DhcpMessageType, mac: [u8; 6]) -> Ask<'static> {
        Ask {
            kind,
            mac,
            ciaddr: Ipv4Addr::UNSPECIFIED,
            giaddr: Ipv4Addr::UNSPECIFIED,
            requested: None,
            server: None,
            id: None,
            name: None,
        }
    }

    fn packet(ask: &Ask) -> Vec<u8> {
        let mut options = Vec::new();
        if let Some(id) = ask.id {
            options.push(DhcpOption { kind: OPTION_CLIENT_IDENTIFIER, data: id });
        }
        if let Some(name) = ask.name {
            options.push(DhcpOption { kind: OPTION_HOST_NAME, data: name.as_bytes() });
        }
        let repr = DhcpRepr {
            message_type: ask.kind,
            transaction_id: 0x1234_5678,
            secs: 0,
            client_hardware_address: EthernetAddress(ask.mac),
            client_ip: ask.ciaddr,
            your_ip: Ipv4Addr::UNSPECIFIED,
            server_ip: Ipv4Addr::UNSPECIFIED,
            router: None,
            subnet_mask: None,
            relay_agent_ip: ask.giaddr,
            broadcast: ask.ciaddr.is_unspecified(),
            requested_ip: ask.requested,
            client_identifier: None,
            server_identifier: ask.server,
            parameter_request_list: None,
            dns_servers: None,
            max_size: None,
            lease_duration: None,
            renew_duration: None,
            rebind_duration: None,
            additional_options: &options,
        };
        let mut bytes = vec![0; repr.buffer_len().max(dhcp::MESSAGE_MIN)];
        repr.emit(&mut DhcpPacket::new_unchecked(&mut bytes[..])).unwrap();
        bytes
    }

    /// What a message from the server holds.
    struct Message {
        kind: DhcpMessageType,
        yiaddr: Ipv4Addr,
        ciaddr: Ipv4Addr,
        xid: u32,
        options: HashMap<u8, Vec<u8>>,
        length: usize,
    }

    impl Message {
        fn option(&self, kind: u8) -> Option<&[u8]> {
            self.options.get(&kind).map(Vec::as_slice)
        }

        fn address_option(&self, kind: u8) -> Option<Ipv4Addr> {
            let [a, b, c, d] = self.option(kind)?[..4] else { return None };
            Some(Ipv4Addr::new(a, b, c, d))
        }

        fn seconds(&self, kind: u8) -> Option<u32> {
            Some(u32::from_be_bytes(self.option(kind)?.try_into().ok()?))
        }
    }

    fn read(bytes: &[u8]) -> Message {
        let packet = DhcpPacket::new_checked(bytes).unwrap();
        let options: HashMap<u8, Vec<u8>> =
            packet.options().map(|option| (option.kind, option.data.to_vec())).collect();
        Message {
            kind: DhcpMessageType::from(options[&OPTION_MESSAGE_TYPE][0]),
            yiaddr: packet.your_ip(),
            ciaddr: packet.client_ip(),
            xid: packet.transaction_id(),
            options,
            length: bytes.len(),
        }
    }

    /// Where the reply goes and what it says.
    fn sent(reply: &Reply) -> (SocketAddrV4, Message) {
        let Reply::Send { to, message } = reply else { panic!("no reply: {reply:?}") };
        (*to, read(message))
    }

    fn handle(server: &mut Server, ask: &Ask, now: Instant, wall: u64) -> (Reply, Option<Event>) {
        server.handle(&packet(ask), now, wall)
    }

    fn broadcast() -> SocketAddrV4 {
        SocketAddrV4::new(Ipv4Addr::BROADCAST, CLIENT_PORT)
    }

    /// Runs the client's DISCOVER and REQUEST: the address it got.
    fn lease(server: &mut Server, mac: [u8; 6], now: Instant, wall: u64) -> Ipv4Addr {
        lease_as(server, ask(DhcpMessageType::Discover, mac), now, wall)
    }

    fn lease_as(server: &mut Server, discover: Ask, now: Instant, wall: u64) -> Ipv4Addr {
        let (reply, event) = handle(server, &discover, now, wall);
        assert_eq!(event, None);
        let (_, offer) = sent(&reply);
        assert_eq!(offer.kind, DhcpMessageType::Offer);
        let request = Ask {
            kind: DhcpMessageType::Request,
            requested: Some(offer.yiaddr),
            server: Some(SERVER),
            ..discover
        };
        let (reply, event) = handle(server, &request, now, wall);
        let (_, ack) = sent(&reply);
        assert_eq!(ack.kind, DhcpMessageType::Ack);
        assert_eq!(ack.yiaddr, offer.yiaddr);
        assert!(matches!(event, Some(Event::Leased(_) | Event::Renewed)), "{event:?}");
        offer.yiaddr
    }

    #[test]
    fn a_new_client_is_offered_the_lowest_free_address_and_gets_it() {
        let (now, mut server) = (Instant::now(), server());
        let (reply, event) =
            handle(&mut server, &ask(DhcpMessageType::Discover, LAPTOP), now, WALL);
        assert_eq!(event, None);
        let (to, offer) = sent(&reply);
        assert_eq!(to, broadcast());
        assert_eq!(
            (offer.kind, offer.yiaddr, offer.xid),
            (DhcpMessageType::Offer, address(100), 0x1234_5678)
        );
        assert!(offer.length >= dhcp::MESSAGE_MIN);
        // Nothing is leased until the client asks for the offer.
        assert!(server.leases().is_empty());

        let request = Ask {
            requested: Some(address(100)),
            server: Some(SERVER),
            name: Some("laptop"),
            ..ask(DhcpMessageType::Request, LAPTOP)
        };
        let (reply, event) = handle(&mut server, &request, now, WALL);
        let (to, ack) = sent(&reply);
        assert_eq!((to, ack.kind, ack.yiaddr), (broadcast(), DhcpMessageType::Ack, address(100)));
        let expected = Lease {
            client: [&[1][..], &LAPTOP[..]].concat(),
            mac: LAPTOP,
            address: address(100),
            ends: WALL + LEASE,
            name: Some("laptop".into()),
        };
        assert_eq!(event, Some(Event::Leased(expected.clone())));
        assert_eq!(server.leases(), [expected]);
    }

    #[test]
    fn an_offer_names_no_address_the_client_has() {
        let (now, mut server) = (Instant::now(), server());
        let asking = Ask { ciaddr: address(77), ..ask(DhcpMessageType::Discover, LAPTOP) };
        let (reply, _) = handle(&mut server, &asking, now, WALL);
        let (_, offer) = sent(&reply);
        assert_eq!(
            (offer.kind, offer.yiaddr, offer.ciaddr),
            (DhcpMessageType::Offer, address(100), Ipv4Addr::UNSPECIFIED)
        );
    }

    #[test]
    fn the_options_always_sent_and_those_the_pool_has_a_value_for() {
        let (now, mut server) = (Instant::now(), server());
        let (reply, _) = handle(&mut server, &ask(DhcpMessageType::Discover, LAPTOP), now, WALL);
        let (_, offer) = sent(&reply);
        assert_eq!(offer.address_option(OPTION_SERVER_IDENTIFIER), Some(SERVER));
        assert_eq!(offer.seconds(51), Some(43_200));
        // T1 is half the lease, T2 seven eighths of it.
        assert_eq!(offer.seconds(OPTION_RENEWAL_TIME), Some(21_600));
        assert_eq!(offer.seconds(OPTION_REBINDING_TIME), Some(37_800));
        assert_eq!(offer.address_option(1), Some(Ipv4Addr::new(255, 255, 255, 0)));
        assert_eq!(offer.address_option(3), Some(address(254)));
        assert_eq!(offer.option(6), Some(&[192, 168, 10, 1, 9, 9, 9, 9][..]));
        assert_eq!(offer.option(OPTION_DOMAIN), Some(&b"lab.example"[..]));
        assert_eq!(offer.address_option(OPTION_NTP_SERVERS), Some(SERVER));
        assert_eq!(offer.option(OPTION_VENDOR), None);

        let bare = Pool {
            router: None,
            dns: Vec::new(),
            domain: None,
            time: None,
            metered: true,
            ..pool()
        };
        let mut server = Server::new(bare, Vec::new());
        let (reply, _) = handle(&mut server, &ask(DhcpMessageType::Discover, LAPTOP), now, WALL);
        let (_, offer) = sent(&reply);
        let mut kinds: Vec<u8> = offer.options.keys().copied().collect();
        kinds.sort_unstable();
        assert_eq!(kinds, [1, OPTION_VENDOR, 51, OPTION_MESSAGE_TYPE, 54, 58, 59]);
        assert_eq!(offer.option(OPTION_VENDOR), Some(ANDROID_METERED));
    }

    #[test]
    fn a_reserved_client_gets_its_address_and_no_one_else_does() {
        let (now, mut server) = (Instant::now(), server());
        let (reply, _) = handle(&mut server, &ask(DhcpMessageType::Discover, PRINTER), now, WALL);
        assert_eq!(sent(&reply).1.yiaddr, address(20));
        let named = Ask {
            requested: Some(address(20)),
            server: Some(SERVER),
            name: Some("kitchen"),
            ..ask(DhcpMessageType::Request, PRINTER)
        };
        let (_, event) = handle(&mut server, &named, now, WALL);
        // The reservation's name wins over the one the client gives.
        assert!(
            matches!(event, Some(Event::Leased(ref lease)) if lease.name.as_deref() == Some("printer"))
        );

        // A client that asks for it is given another, as are all in turn.
        let asking = Ask { requested: Some(address(20)), ..ask(DhcpMessageType::Discover, LAPTOP) };
        let (reply, _) = handle(&mut server, &asking, now, WALL);
        assert_eq!(sent(&reply).1.yiaddr, address(100));
    }

    #[test]
    fn a_returning_client_gets_the_address_it_had() {
        let (now, mut server) = (Instant::now(), server());
        assert_eq!(lease(&mut server, LAPTOP, now, WALL), address(100));
        assert_eq!(lease(&mut server, PHONE, now, WALL), address(101));
        // The lease has ended, and no one else has the address.
        let later = WALL + LEASE + 1;
        let (reply, _) = handle(&mut server, &ask(DhcpMessageType::Discover, LAPTOP), now, later);
        assert_eq!(sent(&reply).1.yiaddr, address(100));
        // A client that asks for another address still gets its own.
        let asking =
            Ask { requested: Some(address(103)), ..ask(DhcpMessageType::Discover, LAPTOP) };
        let (reply, _) = handle(&mut server, &asking, now, later);
        assert_eq!(sent(&reply).1.yiaddr, address(100));
    }

    #[test]
    fn a_client_gets_the_address_it_asks_for_when_that_is_free() {
        let (now, mut server) = (Instant::now(), server());
        let asking = |requested| Ask {
            requested: Some(requested),
            ..ask(DhcpMessageType::Discover, LAPTOP)
        };
        let (reply, _) = handle(&mut server, &asking(address(102)), now, WALL);
        assert_eq!(sent(&reply).1.yiaddr, address(102));
        // Outside the range, or the card's own, it is the lowest free.
        for wanted in [address(50), SERVER, address(104)] {
            let (reply, _) = handle(&mut server, &asking(wanted), now, WALL);
            assert_eq!(sent(&reply).1.yiaddr, address(100));
        }
        // Another's offer is not free: the laptop's last was address 100.
        let phone = Ask { requested: Some(address(100)), ..ask(DhcpMessageType::Discover, PHONE) };
        let (reply, _) = handle(&mut server, &phone, now, WALL);
        assert_eq!(sent(&reply).1.yiaddr, address(101));
    }

    #[test]
    fn an_offer_is_held_for_a_minute() {
        let (now, mut server) = (Instant::now(), server());
        let (reply, _) = handle(&mut server, &ask(DhcpMessageType::Discover, LAPTOP), now, WALL);
        assert_eq!(sent(&reply).1.yiaddr, address(100));
        let (reply, _) = handle(&mut server, &ask(DhcpMessageType::Discover, PHONE), now, WALL);
        assert_eq!(sent(&reply).1.yiaddr, address(101));
        let after = now + OFFER_HOLD;
        let (reply, _) = handle(&mut server, &ask(DhcpMessageType::Discover, PHONE), after, WALL);
        assert_eq!(sent(&reply).1.yiaddr, address(100));
    }

    #[test]
    fn an_address_whose_lease_ended_is_used_only_after_the_free_ones() {
        let pool = Pool { last: address(102), ..pool() };
        let (now, mut server) = (Instant::now(), Server::new(pool, Vec::new()));
        assert_eq!(lease(&mut server, LAPTOP, now, WALL), address(100));
        assert_eq!(lease(&mut server, PHONE, now, WALL + LEASE / 2), address(101));
        let later = WALL + LEASE + 1;
        let third = [0x52, 0x54, 0, 0x12, 0x34, 0x60];
        // 100 has ended and 102 was never given: 102 first.
        assert_eq!(lease(&mut server, third, now, later), address(102));
        let fourth = [0x52, 0x54, 0, 0x12, 0x34, 0x61];
        assert_eq!(lease(&mut server, fourth, now, later), address(100));
        // The laptop's record went with its address.
        assert!(server.leases().iter().all(|lease| lease.mac != LAPTOP));
    }

    #[test]
    fn a_full_pool_makes_no_offer_and_says_so_once() {
        let pool = Pool { last: address(100), ..pool() };
        let (now, mut server) = (Instant::now(), Server::new(pool, Vec::new()));
        assert_eq!(lease(&mut server, LAPTOP, now, WALL), address(100));
        let (reply, event) = handle(&mut server, &ask(DhcpMessageType::Discover, PHONE), now, WALL);
        assert_eq!((reply, event), (Reply::None, Some(Event::Full { mac: PHONE })));
        let (reply, event) = handle(&mut server, &ask(DhcpMessageType::Discover, PHONE), now, WALL);
        assert_eq!((reply, event), (Reply::None, None));
        // Once an address is free, and the pool is full again, it says so
        // again.
        let release = Ask {
            ciaddr: address(100),
            server: Some(SERVER),
            ..ask(DhcpMessageType::Release, LAPTOP)
        };
        let later = WALL + 10;
        assert!(matches!(handle(&mut server, &release, now, later).1, Some(Event::Released(_))));
        let (reply, _) = handle(&mut server, &ask(DhcpMessageType::Discover, PHONE), now, later);
        assert_eq!(sent(&reply).1.yiaddr, address(100));
        let third = [0x52, 0x54, 0, 0x12, 0x34, 0x60];
        let (reply, event) =
            handle(&mut server, &ask(DhcpMessageType::Discover, third), now, later);
        assert_eq!((reply, event), (Reply::None, Some(Event::Full { mac: third })));
        let (_, event) = handle(&mut server, &ask(DhcpMessageType::Discover, third), now, later);
        assert_eq!(event, None);
    }

    #[test]
    fn a_request_that_names_another_server_takes_the_offer_back() {
        let (now, mut server) = (Instant::now(), server());
        let (reply, _) = handle(&mut server, &ask(DhcpMessageType::Discover, LAPTOP), now, WALL);
        let offered = sent(&reply).1.yiaddr;
        let other = Ipv4Addr::new(192, 168, 10, 2);
        let request = |server| Ask {
            requested: Some(offered),
            server: Some(server),
            ..ask(DhcpMessageType::Request, LAPTOP)
        };
        assert_eq!(handle(&mut server, &request(other), now, WALL), (Reply::None, None));
        // The offer is gone: asking this server for it is refused.
        let (reply, event) = handle(&mut server, &request(SERVER), now, WALL);
        let (to, nak) = sent(&reply);
        assert_eq!((to, nak.kind, event), (broadcast(), DhcpMessageType::Nak, None));
        assert_eq!(nak.address_option(OPTION_SERVER_IDENTIFIER), Some(SERVER));
        assert_eq!(nak.yiaddr, Ipv4Addr::UNSPECIFIED);
        assert_eq!(nak.option(3), None);
    }

    #[test]
    fn a_request_for_another_address_than_the_offer_is_refused() {
        let (now, mut server) = (Instant::now(), server());
        handle(&mut server, &ask(DhcpMessageType::Discover, LAPTOP), now, WALL);
        let request = Ask {
            requested: Some(address(101)),
            server: Some(SERVER),
            ..ask(DhcpMessageType::Request, LAPTOP)
        };
        let (reply, event) = handle(&mut server, &request, now, WALL);
        assert_eq!((sent(&reply).1.kind, event), (DhcpMessageType::Nak, None));
        assert!(server.leases().is_empty());
        // So is a request that asks for nothing.
        let nothing = Ask { requested: None, ..request };
        handle(&mut server, &ask(DhcpMessageType::Discover, LAPTOP), now, WALL);
        let (reply, _) = handle(&mut server, &nothing, now, WALL);
        assert_eq!(sent(&reply).1.kind, DhcpMessageType::Nak);
    }

    #[test]
    fn a_client_that_restarts_has_its_address_confirmed_or_refused() {
        let (now, mut server) = (Instant::now(), server());
        assert_eq!(lease(&mut server, LAPTOP, now, WALL), address(100));
        let reboot =
            |mac, wanted| Ask { requested: Some(wanted), ..ask(DhcpMessageType::Request, mac) };
        // It holds it: an acknowledgement that extends the lease.
        let (reply, event) = handle(&mut server, &reboot(LAPTOP, address(100)), now, WALL + 100);
        let (to, ack) = sent(&reply);
        assert_eq!((to, ack.kind, ack.yiaddr), (broadcast(), DhcpMessageType::Ack, address(100)));
        assert_eq!(event, Some(Event::Renewed));
        assert_eq!(server.leases()[0].ends, WALL + 100 + LEASE);
        // Another network's address, another client's, and one it does not
        // hold are refused.
        for wanted in [Ipv4Addr::new(10, 0, 0, 5), address(100), address(101)] {
            let mac = if wanted == address(100) { PHONE } else { LAPTOP };
            let (reply, _) = handle(&mut server, &reboot(mac, wanted), now, WALL);
            assert_eq!(sent(&reply).1.kind, DhcpMessageType::Nak, "{wanted}");
        }
        // A client the server has no record of is left to the servers that
        // do, even for an address of this network, but not for another's.
        let stranger = [0x52, 0x54, 0, 0x12, 0x34, 0x99];
        assert_eq!(
            handle(&mut server, &reboot(stranger, address(150)), now, WALL),
            (Reply::None, None)
        );
        let (reply, _) =
            handle(&mut server, &reboot(stranger, Ipv4Addr::new(10, 0, 0, 5)), now, WALL);
        assert_eq!(sent(&reply).1.kind, DhcpMessageType::Nak);
        // A reserved client is known without a lease.
        let (reply, _) = handle(&mut server, &reboot(PRINTER, address(20)), now, WALL);
        assert_eq!(sent(&reply).1.kind, DhcpMessageType::Ack);
    }

    #[test]
    fn a_renewing_client_gets_more_time_and_a_stranger_is_refused() {
        let (now, mut server) = (Instant::now(), server());
        assert_eq!(lease(&mut server, LAPTOP, now, WALL), address(100));
        let renew = |mac, held| Ask { ciaddr: held, ..ask(DhcpMessageType::Request, mac) };
        let (reply, event) =
            handle(&mut server, &renew(LAPTOP, address(100)), now, WALL + LEASE / 2);
        let (to, ack) = sent(&reply);
        // The answer goes to the client's address, which it echoes.
        assert_eq!(to, SocketAddrV4::new(address(100), CLIENT_PORT));
        assert_eq!(
            (ack.kind, ack.yiaddr, ack.ciaddr),
            (DhcpMessageType::Ack, address(100), address(100))
        );
        assert_eq!(event, Some(Event::Renewed));
        assert_eq!(server.leases()[0].ends, WALL + LEASE / 2 + LEASE);
        // Another client's address, or one no one holds, is refused, in a
        // message to everyone, as RFC 2131 (4.1) has every refusal go.
        for (mac, held) in [(PHONE, address(100)), (LAPTOP, address(101))] {
            let (reply, event) = handle(&mut server, &renew(mac, held), now, WALL);
            let (to, nak) = sent(&reply);
            assert_eq!((to, nak.kind, event), (broadcast(), DhcpMessageType::Nak, None));
            assert_eq!(nak.ciaddr, Ipv4Addr::UNSPECIFIED);
        }
        // A lease that ended, with its address still the client's, is
        // extended, which is the client's address coming back.
        let ended = WALL + 3 * LEASE;
        let (reply, event) = handle(&mut server, &renew(LAPTOP, address(100)), now, ended);
        assert_eq!(sent(&reply).1.kind, DhcpMessageType::Ack);
        assert!(matches!(event, Some(Event::Leased(_))));
    }

    #[test]
    fn a_reply_to_an_address_off_the_network_goes_to_everyone() {
        let (now, mut server) = (Instant::now(), server());
        let renew =
            Ask { ciaddr: Ipv4Addr::new(10, 1, 2, 3), ..ask(DhcpMessageType::Request, LAPTOP) };
        let (reply, _) = handle(&mut server, &renew, now, WALL);
        let (to, nak) = sent(&reply);
        assert_eq!((to, nak.kind), (broadcast(), DhcpMessageType::Nak));
    }

    #[test]
    fn a_declined_address_is_skipped_for_ten_minutes() {
        let (now, mut server) = (Instant::now(), server());
        assert_eq!(lease(&mut server, LAPTOP, now, WALL), address(100));
        let decline = Ask {
            requested: Some(address(100)),
            server: Some(SERVER),
            name: Some("laptop"),
            ..ask(DhcpMessageType::Decline, LAPTOP)
        };
        let (reply, event) = handle(&mut server, &decline, now, WALL);
        let said =
            Event::Declined { address: address(100), mac: LAPTOP, name: Some("laptop".into()) };
        assert_eq!((reply, event), (Reply::None, Some(said)));
        assert!(server.leases().is_empty());
        // Not to the client that declined it, and not to another.
        let (reply, _) = handle(&mut server, &ask(DhcpMessageType::Discover, LAPTOP), now, WALL);
        assert_eq!(sent(&reply).1.yiaddr, address(101));
        let nine = now + Duration::from_secs(599);
        let (reply, _) = handle(&mut server, &ask(DhcpMessageType::Discover, PHONE), nine, WALL);
        assert_eq!(sent(&reply).1.yiaddr, address(101));
        let ten = now + DECLINE_WAIT;
        let (reply, _) = handle(&mut server, &ask(DhcpMessageType::Discover, PHONE), ten, WALL);
        assert_eq!(sent(&reply).1.yiaddr, address(100));
    }

    #[test]
    fn a_decline_of_an_address_the_client_does_not_have_is_ignored() {
        let (now, mut server) = (Instant::now(), server());
        assert_eq!(lease(&mut server, LAPTOP, now, WALL), address(100));
        let decline = |mac, wanted, to| Ask {
            requested: Some(wanted),
            server: Some(to),
            ..ask(DhcpMessageType::Decline, mac)
        };
        for ask in [
            // Another client's address, one that is no one's, and another
            // server's decline.
            decline(PHONE, address(100), SERVER),
            decline(LAPTOP, address(102), SERVER),
            decline(LAPTOP, address(100), Ipv4Addr::new(192, 168, 10, 2)),
            Ask { requested: None, ..decline(LAPTOP, address(100), SERVER) },
        ] {
            assert_eq!(handle(&mut server, &ask, now, WALL), (Reply::None, None));
        }
        // The pool is whole: the next client's lowest is still 101.
        let (reply, _) = handle(&mut server, &ask(DhcpMessageType::Discover, PHONE), now, WALL);
        assert_eq!(sent(&reply).1.yiaddr, address(101));
    }

    #[test]
    fn a_released_address_stays_the_clients_first_choice_while_it_is_free() {
        let (now, mut server) = (Instant::now(), server());
        assert_eq!(lease(&mut server, LAPTOP, now, WALL), address(100));
        let release = |mac| Ask {
            ciaddr: address(100),
            server: Some(SERVER),
            ..ask(DhcpMessageType::Release, mac)
        };
        // Another client's release does nothing.
        assert_eq!(handle(&mut server, &release(PHONE), now, WALL), (Reply::None, None));
        let (reply, event) = handle(&mut server, &release(LAPTOP), now, WALL + 60);
        assert_eq!(reply, Reply::None);
        let Some(Event::Released(lease)) = event else { panic!("{event:?}") };
        assert_eq!((lease.address, lease.ends), (address(100), WALL + 60));
        // A release of a lease that has ended is nothing new.
        assert_eq!(handle(&mut server, &release(LAPTOP), now, WALL + 61), (Reply::None, None));
        // The next client gets a free address, not the released one, and
        // the laptop gets its own again.
        let (reply, _) =
            handle(&mut server, &ask(DhcpMessageType::Discover, PHONE), now, WALL + 61);
        assert_eq!(sent(&reply).1.yiaddr, address(101));
        let (reply, _) =
            handle(&mut server, &ask(DhcpMessageType::Discover, LAPTOP), now, WALL + 61);
        assert_eq!(sent(&reply).1.yiaddr, address(100));
    }

    #[test]
    fn an_inform_gets_the_options_and_no_lease() {
        let (now, mut server) = (Instant::now(), server());
        let inform = Ask { ciaddr: address(77), ..ask(DhcpMessageType::Inform, LAPTOP) };
        let (reply, event) = handle(&mut server, &inform, now, WALL);
        let (to, ack) = sent(&reply);
        assert_eq!(
            (to, ack.kind, ack.yiaddr, event),
            (
                SocketAddrV4::new(address(77), CLIENT_PORT),
                DhcpMessageType::Ack,
                Ipv4Addr::UNSPECIFIED,
                None
            )
        );
        assert_eq!(ack.address_option(3), Some(address(254)));
        assert_eq!(ack.option(6), Some(&[192, 168, 10, 1, 9, 9, 9, 9][..]));
        assert_eq!(ack.address_option(OPTION_NTP_SERVERS), Some(SERVER));
        for lease_option in [51, OPTION_RENEWAL_TIME, OPTION_REBINDING_TIME] {
            assert_eq!(ack.option(lease_option), None);
        }
        assert!(server.leases().is_empty());
        // An address of another network, or none, gets no answer.
        for ciaddr in [Ipv4Addr::new(8, 8, 8, 8), Ipv4Addr::UNSPECIFIED] {
            let inform = Ask { ciaddr, ..ask(DhcpMessageType::Inform, LAPTOP) };
            assert_eq!(handle(&mut server, &inform, now, WALL), (Reply::None, None));
        }
    }

    #[test]
    fn a_client_is_known_by_its_identifier_before_its_mac_address() {
        let (now, mut server) = (Instant::now(), server());
        let first = Ask { id: Some(&[0, b'a', b'b']), ..ask(DhcpMessageType::Discover, LAPTOP) };
        let second = Ask { id: Some(&[0, b'c', b'd']), ..ask(DhcpMessageType::Discover, LAPTOP) };
        assert_eq!(lease_as(&mut server, first, now, WALL), address(100));
        assert_eq!(lease_as(&mut server, second, now, WALL), address(101));
        let again = Ask { id: Some(&[0, b'a', b'b']), ..ask(DhcpMessageType::Discover, LAPTOP) };
        let (reply, _) = handle(&mut server, &again, now, WALL);
        assert_eq!(sent(&reply).1.yiaddr, address(100));
        // With none, it is `01` and the MAC address, as hardware type 1
        // and the MAC address make it.
        let plain = Ask {
            id: Some(&[&[1][..], &LAPTOP[..]].concat()),
            ..ask(DhcpMessageType::Discover, LAPTOP)
        };
        assert_eq!(lease_as(&mut server, plain, now, WALL), address(102));
        assert_eq!(lease(&mut server, LAPTOP, now, WALL), address(102));
    }

    #[test]
    fn host_names_are_kept_when_they_are_plain() {
        let (now, mut server) = (Instant::now(), server());
        let with = |name| Ask { name: Some(name), ..ask(DhcpMessageType::Discover, LAPTOP) };
        let named = |server: &mut Server, name| {
            lease_as(server, with(name), now, WALL);
            server.leases()[0].name.clone()
        };
        assert_eq!(named(&mut server, "alice-laptop.lan"), Some("alice-laptop.lan".into()));
        // A name with other bytes in it is not kept, and the one the
        // lease had stays.
        assert_eq!(named(&mut server, "a b"), Some("alice-laptop.lan".into()));
        assert_eq!(named(&mut server, "<script>"), Some("alice-laptop.lan".into()));
        assert_eq!(host_name(b"laptop\0"), Some("laptop".into()));
        assert_eq!(host_name(&[b'a'; 64]), None);
        assert_eq!(host_name(b""), None);
        assert_eq!(host_name(&[0xff, 0xfe]), None);
    }

    #[test]
    fn leases_are_written_and_read_back() {
        let leases = vec![
            Lease {
                client: [&[1][..], &LAPTOP[..]].concat(),
                mac: LAPTOP,
                address: address(101),
                ends: WALL,
                name: Some("laptop".into()),
            },
            Lease {
                client: vec![0, 0xab, 0xcd],
                mac: PHONE,
                address: address(100),
                ends: WALL + 5,
                name: None,
            },
        ];
        let text = leases_text(&leases);
        assert_eq!(
            text,
            "# netd(8)'s DHCP leases: mac address ends client name\n\
             52:54:00:12:34:57 192.168.10.100 1800000005 00abcd -\n\
             52:54:00:12:34:56 192.168.10.101 1800000000 - laptop\n"
        );
        let mut read = parse_leases(&text);
        read.sort_by_key(|lease| lease.address);
        assert_eq!(read, [leases[1].clone(), leases[0].clone()]);
        // A line that is no lease is left out.
        let damaged = "52:54:00:12:34:57 192.168.10.100 soon - -\n\
                       52:54:00:12:34 192.168.10.100 5 - -\n\
                       52:54:00:12:34:57 192.168.10.100 5 zz -\n\
                       52:54:00:12:34:57 192.168.10.100 5 - a<b\n\
                       52:54:00:12:34:57 192.168.10.100 5\n\
                       52:54:00:12:34:57 192.168.10.100 5 - -\n";
        assert_eq!(parse_leases(damaged).len(), 1);
        assert_eq!(parse_leases(""), Vec::<Lease>::new());
    }

    #[test]
    fn a_server_keeps_the_leases_that_still_fit_its_pool() {
        let lease = |mac: [u8; 6], address: Ipv4Addr, id: u8| Lease {
            client: vec![0, id],
            mac,
            address,
            ends: WALL,
            name: None,
        };
        let leases = vec![
            lease(LAPTOP, address(100), 1),
            // The same address, and the same client, as others.
            lease(PHONE, address(100), 2),
            lease(PHONE, address(101), 1),
            // Outside the range, the card's own, and another's reserved.
            lease(PHONE, address(150), 3),
            lease(PHONE, SERVER, 4),
            lease(PHONE, address(20), 5),
            // The printer's own address, outside the range, is its.
            lease(PRINTER, address(20), 6),
        ];
        let server = Server::new(pool(), leases.clone());
        assert_eq!(server.leases(), [leases[0].clone(), leases[6].clone()]);
        let mut server = Server::new(pool(), leases.clone());
        // With a narrower range, the laptop's address is outside it; the
        // printer's reservation still holds.
        server.set_pool(Pool { first: address(101), last: address(103), ..pool() });
        assert_eq!(server.leases(), [leases[6].clone()]);
    }

    #[test]
    fn a_changed_pool_keeps_the_offers_that_still_fit_it() {
        let (now, mut server) = (Instant::now(), server());
        let discover = |server: &mut Server, mac| {
            sent(&handle(server, &ask(DhcpMessageType::Discover, mac), now, WALL).0).1.yiaddr
        };
        assert_eq!(discover(&mut server, LAPTOP), address(100));
        assert_eq!(discover(&mut server, PHONE), address(101));
        let decline =
            Ask { requested: Some(address(100)), ..ask(DhcpMessageType::Decline, LAPTOP) };
        assert!(matches!(handle(&mut server, &decline, now, WALL).1, Some(Event::Declined { .. })));
        // New DNS servers, and a range that no longer holds 101.
        server.set_pool(Pool { dns: vec![address(5)], last: address(100), ..pool() });
        let request = |mac, wanted| Ask {
            requested: Some(wanted),
            server: Some(SERVER),
            ..ask(DhcpMessageType::Request, mac)
        };
        let (reply, _) = handle(&mut server, &request(PHONE, address(101)), now, WALL);
        assert_eq!(sent(&reply).1.kind, DhcpMessageType::Nak);
        // The offer of 100 was declined by then, and the decline holds.
        let (reply, event) = handle(&mut server, &ask(DhcpMessageType::Discover, PHONE), now, WALL);
        assert_eq!((reply, event), (Reply::None, Some(Event::Full { mac: PHONE })));
        server.set_pool(Pool { dns: vec![address(6)], ..pool() });
        let (reply, _) = handle(&mut server, &ask(DhcpMessageType::Discover, PHONE), now, WALL);
        let (_, offer) = sent(&reply);
        assert_eq!((offer.yiaddr, offer.option(6)), (address(101), Some(&[192, 168, 10, 6][..])));
        let (reply, event) = handle(&mut server, &request(PHONE, address(101)), now, WALL);
        assert_eq!(sent(&reply).1.kind, DhcpMessageType::Ack);
        assert!(matches!(event, Some(Event::Leased(_))));
    }

    #[test]
    fn what_is_not_a_request_for_this_server_is_ignored() {
        let (now, mut server) = (Instant::now(), server());
        let good = packet(&ask(DhcpMessageType::Discover, LAPTOP));
        assert!(matches!(server.handle(&good, now, WALL).0, Reply::Send { .. }));
        let mutated = |edit: &dyn Fn(&mut Vec<u8>)| {
            let mut bytes = good.clone();
            edit(&mut bytes);
            bytes
        };
        let ignored = [
            // Too short, too long, and empty.
            good[..REQUEST_MIN - 1].to_vec(),
            mutated(&|bytes| bytes.resize(REQUEST_MAX + 1, 0)),
            Vec::new(),
            // A reply, not Ethernet, a hardware address of another length,
            // no cookie.
            mutated(&|bytes| bytes[0] = 2),
            mutated(&|bytes| bytes[1] = 6),
            mutated(&|bytes| bytes[2] = 8),
            mutated(&|bytes| bytes[236] = 0),
            // A multicast and an empty hardware address.
            mutated(&|bytes| bytes[28] = 0x53),
            mutated(&|bytes| bytes[28..34].fill(0)),
            // No message type, and one that is a server's.
            mutated(&|bytes| bytes[240] = 0),
            mutated(&|bytes| bytes[242] = 2),
            // An option that runs past the end of the packet.
            mutated(&|bytes| {
                bytes.truncate(REQUEST_MIN);
                bytes.extend([OPTION_MESSAGE_TYPE, 1]);
            }),
            // An identifier no client has.
            mutated(&|bytes| {
                bytes.truncate(REQUEST_MIN);
                bytes.extend([OPTION_MESSAGE_TYPE, 1, 1, OPTION_CLIENT_IDENTIFIER, 200]);
                bytes.extend([7; 200]);
                bytes.push(255);
            }),
        ];
        for bytes in ignored {
            assert_eq!(server.handle(&bytes, now, WALL), (Reply::None, None), "{bytes:02x?}");
        }
        assert!(server.leases().is_empty());
    }

    #[test]
    fn a_relayed_request_is_ignored_and_said_once() {
        let (now, mut server) = (Instant::now(), server());
        let relayed = Ask { giaddr: address(2), ..ask(DhcpMessageType::Discover, LAPTOP) };
        assert_eq!(handle(&mut server, &relayed, now, WALL), (Reply::None, Some(Event::Relayed)));
        assert_eq!(handle(&mut server, &relayed, now, WALL), (Reply::None, None));
    }

    #[test]
    fn no_packet_makes_the_server_panic() {
        let (now, mut server) = (Instant::now(), server());
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = || {
            seed ^= seed >> 12;
            seed ^= seed << 25;
            seed ^= seed >> 27;
            seed.wrapping_mul(0x2545_f491_4f6c_dd1d)
        };
        let kinds = [
            DhcpMessageType::Discover,
            DhcpMessageType::Request,
            DhcpMessageType::Decline,
            DhcpMessageType::Release,
            DhcpMessageType::Inform,
        ];
        for round in 0..3000_u64 {
            let kind = kinds[(next() % 5) as usize];
            let mut bytes = packet(&Ask {
                ciaddr: if next() % 2 == 0 { address(100) } else { Ipv4Addr::UNSPECIFIED },
                requested: Some(address(100)),
                server: (next() % 2 == 0).then_some(SERVER),
                id: Some(&[1, 2, 3]),
                ..ask(kind, LAPTOP)
            });
            for _ in 0..(next() % 6) {
                let at = (next() as usize) % bytes.len();
                bytes[at] = next() as u8;
            }
            bytes.truncate(REQUEST_MIN + (next() as usize) % (bytes.len() - REQUEST_MIN + 1));
            server.handle(&bytes, now, WALL + round);
        }
    }

    #[test]
    fn the_listing_names_leases_and_reservations() {
        let (now, mut server) = (Instant::now(), server());
        let named = Ask { name: Some("laptop"), ..ask(DhcpMessageType::Discover, LAPTOP) };
        lease_as(&mut server, named, now, WALL);
        assert_eq!(lease(&mut server, PHONE, now, WALL + 5), address(101));
        assert_eq!(
            server.listing(WALL + 10),
            format!(
                "net1 192.168.10.20 52:54:00:12:34:58 printer reserved\n\
                 net1 192.168.10.100 52:54:00:12:34:56 laptop {}\n\
                 net1 192.168.10.101 52:54:00:12:34:57 - {}\n",
                WALL + LEASE,
                WALL + 5 + LEASE
            )
        );
        // A lease that has ended is not listed, a reservation always.
        let later = WALL + 2 * LEASE;
        assert_eq!(
            server.listing(later),
            "net1 192.168.10.20 52:54:00:12:34:58 printer reserved\n"
        );
        assert_eq!(lease(&mut server, PRINTER, now, WALL), address(20));
        assert!(server.listing(WALL + 10).contains("52:54:00:12:34:58 printer 1800043200\n"));
    }

    #[test]
    fn a_range_is_checked_against_the_network() {
        let network = Cidr { ip: SERVER, prefix: 24 };
        assert_eq!(check_range(network, address(100), address(199)), Ok(()));
        assert_eq!(check_range(network, address(2), address(254)), Ok(()));
        let wrong = |first, last| check_range(network, first, last).unwrap_err();
        assert_eq!(wrong(address(9), address(5)), "ends before it begins");
        assert_eq!(
            wrong(address(100), Ipv4Addr::new(192, 168, 11, 5)),
            "is not inside the card's network, 192.168.10.0/24"
        );
        assert_eq!(
            wrong(address(1), address(5)),
            "holds the network's own address, its broadcast address or the card's own"
        );
        assert_eq!(
            wrong(address(0), address(5)),
            "holds the network's own address, its broadcast address or the card's own"
        );
        assert_eq!(
            wrong(address(250), address(255)),
            "holds the network's own address, its broadcast address or the card's own"
        );
        let big = Cidr { ip: Ipv4Addr::new(10, 0, 0, 1), prefix: 8 };
        assert_eq!(
            check_range(big, Ipv4Addr::new(10, 0, 1, 0), Ipv4Addr::new(10, 1, 0, 255)),
            Ok(())
        );
        let long =
            check_range(big, Ipv4Addr::new(10, 0, 1, 0), Ipv4Addr::new(10, 1, 1, 0)).unwrap_err();
        assert_eq!(long, "holds more than 65536 addresses");
    }
}
