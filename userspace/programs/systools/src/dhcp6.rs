// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The DHCPv6 client of netd(8) (RFC 8415, ADR 0335): what a card says to
//! DHCPv6 servers and when, with no I/O of its own, as [`crate::dhcp`]
//! does for DHCPv4, so that the tests can drive it with time of their
//! own.
//!
//! A card whose routers say that other settings come from DHCPv6 (their
//! adverts' O flag) asks for those alone, with an INFORMATION-REQUEST:
//! the DNS servers and the domains names are looked up in, asked for
//! again when the server's refresh time ends, a day if it gives none.
//! One whose routers say addresses do (the M flag) asks for an address
//! too, in an IA_NA: it SOLICITs, waits a first timeout for the
//! ADVERTISEs, REQUESTs the best (the highest preference, the first of
//! those; 255 at once), RENEWs the address at T1 and REBINDs it from any
//! server at T2, and gives it up when its valid lifetime ends or a
//! server says it has no binding for it. Each message is sent again as
//! RFC 8415, 15 says: its first timeout, doubling, a tenth either way at
//! random, up to its most.
//!
//! Every message goes to All_DHCP_Relay_Agents_and_Servers (ff02::1:2)
//! port 547, from port 546, which netd sends through the card's own
//! socket. A client is known by its DUID-LL, made of the card's MAC
//! address (RFC 8415, 11.4), so that it is the same at each boot, and
//! its IA_NA by the MAC address's last four bytes. What a server says is
//! checked before it is used: the message must answer the client's
//! transaction, name the client, and name a server; an address must be
//! one a host may have, and a domain only the letters of one.

use std::net::Ipv6Addr;
use std::time::{Duration, Instant};

/// Where the client sends: All_DHCP_Relay_Agents_and_Servers.
pub const SERVERS: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 1, 2);
/// The client's and the servers' ports.
pub const CLIENT_PORT: u16 = 546;
pub const SERVER_PORT: u16 = 547;
/// The longest message the client takes.
pub const MESSAGE_MAX: usize = 1500;

/// The messages' types (RFC 8415, 7.3).
const SOLICIT: u8 = 1;
const ADVERTISE: u8 = 2;
const REQUEST: u8 = 3;
const RENEW: u8 = 5;
const REBIND: u8 = 6;
const REPLY: u8 = 7;
const INFORMATION_REQUEST: u8 = 11;

/// The options' codes (RFC 8415, 21, and RFC 3646).
const CLIENT_ID: u16 = 1;
const SERVER_ID: u16 = 2;
const IA_NA: u16 = 3;
const IA_ADDRESS: u16 = 5;
const REQUESTED: u16 = 6;
const PREFERENCE: u16 = 7;
const ELAPSED_TIME: u16 = 8;
const STATUS_CODE: u16 = 13;
const DNS_SERVERS: u16 = 23;
const DOMAIN_LIST: u16 = 24;
const REFRESH_TIME: u16 = 32;

/// The status codes the client acts on (RFC 8415, 21.13).
const SUCCESS: u16 = 0;
const NO_BINDING: u16 = 3;

/// The timeouts of each message, its first and its most, and the most
/// times a REQUEST is sent (RFC 8415, 7.6).
const SOLICIT_TIMES: Times = Times { first: 1, most: 3600 };
const REQUEST_TIMES: Times = Times { first: 1, most: 30 };
const REQUEST_TRIES: u32 = 10;
const RENEW_TIMES: Times = Times { first: 10, most: 600 };
const INFORMATION_TIMES: Times = Times { first: 1, most: 3600 };
/// How long the first SOLICIT or INFORMATION-REQUEST waits, at most.
const FIRST_DELAY_MS: u64 = 1000;
/// When to ask for the settings again, if the server does not say, and
/// the soonest (RFC 8415, 21.23).
const REFRESH_DEFAULT: u32 = 86_400;
const REFRESH_MIN: u32 = 600;
/// A lifetime of this many seconds is forever.
const FOREVER: u32 = u32::MAX;
/// The fewest seconds between renewals, so that a server's lifetimes of
/// a few seconds do not have the client ask again and again.
const RENEW_MIN: u32 = 60;

/// A message's timeouts, in seconds.
#[derive(Clone, Copy, Debug)]
struct Times {
    first: u64,
    most: u64,
}

/// An address a server gave, and its lifetimes, in seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Address {
    pub ip: Ipv6Addr,
    pub preferred: u32,
    pub valid: u32,
}

/// What a server's ADVERTISE or REPLY said.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answer {
    /// Its DUID.
    pub server: Vec<u8>,
    pub preference: u8,
    /// The message's status, and the IA_NA's.
    pub status: u16,
    pub ia_status: u16,
    /// The IA_NA's address, and when to renew and rebind it.
    pub address: Option<Address>,
    pub t1: u32,
    pub t2: u32,
    pub dns: Vec<Ipv6Addr>,
    pub domains: Vec<String>,
    /// When to ask an INFORMATION-REQUEST's settings again.
    pub refresh: Option<u32>,
}

/// What a lease gives a card: an address, and the settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lease {
    pub address: Address,
    pub server: Vec<u8>,
    /// Seconds after it was given to renew and to rebind it.
    pub t1: u32,
    pub t2: u32,
}

/// Where a card is among RFC 8415's states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Asking for the settings alone (INFORMATION-REQUEST).
    Informing,
    /// Has them, until they are asked again.
    Informed,
    /// Asking for servers (SOLICIT), and taking their ADVERTISEs.
    Soliciting,
    /// Requesting an address of the best (REQUEST).
    Requesting,
    /// Has an address.
    Bound,
    /// Asking its server for more time (RENEW).
    Renewing,
    /// Asking any server (REBIND).
    Rebinding,
}

/// What the client has netd do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Send `message` to [`SERVERS`] port 547.
    Send(Vec<u8>),
    /// Give the card the lease's address, in place of an earlier one's.
    Bind(Lease),
    /// Take the address away: it ended, or a server said it is no more.
    Unbind,
    /// Use these DNS servers and domains, which a server gave.
    Settings { dns: Vec<Ipv6Addr>, domains: Vec<String> },
}

/// One card's DHCPv6 client.
#[derive(Debug)]
pub struct Client {
    duid: Vec<u8>,
    iaid: [u8; 4],
    /// Whether it asks for an address (the M flag), or the settings alone.
    managed: bool,
    phase: Phase,
    /// The exchange's transaction number, when it began, for the
    /// messages' elapsed time, and the timeout of the last message.
    xid: [u8; 3],
    began: Instant,
    timeout: Duration,
    /// When to act next, and the messages sent in this phase.
    next: Instant,
    tries: u32,
    /// The best ADVERTISE so far, while soliciting.
    best: Option<Answer>,
    /// The lease, and when it was given.
    lease: Option<(Lease, Instant)>,
    random: u64,
}

impl Client {
    /// A client for the card with the address `mac`, which asks for an
    /// address if `managed`, else for the settings alone; it starts
    /// within a second, at random. `seed` makes its numbers.
    pub fn new(mac: [u8; 6], managed: bool, seed: u64, now: Instant) -> Client {
        // DUID-LL: its type, 3, the hardware's, 1 for Ethernet, and the
        // MAC address.
        let mut duid = vec![0, 3, 0, 1];
        duid.extend_from_slice(&mac);
        let mut client = Client {
            duid,
            iaid: [mac[2], mac[3], mac[4], mac[5]],
            managed,
            phase: if managed { Phase::Soliciting } else { Phase::Informing },
            xid: [0; 3],
            began: now,
            timeout: Duration::ZERO,
            next: now,
            tries: 0,
            best: None,
            lease: None,
            // xorshift64* needs a state other than 0.
            random: seed | 1,
        };
        client.next = now + Duration::from_millis(client.random() % FIRST_DELAY_MS);
        client.start(now);
        client
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Whether it asks for an address.
    pub fn managed(&self) -> bool {
        self.managed
    }

    /// When [`Client::tick`] next has something to do.
    pub fn next(&self) -> Instant {
        self.next
    }

    /// The lease, if the card has one.
    pub fn lease(&self) -> Option<&Lease> {
        self.lease.as_ref().map(|(lease, _)| lease)
    }

    /// Its card's link came back: it may be on another network now, so it
    /// asks any server for its address again, or for the settings.
    pub fn link_up(&mut self, now: Instant) {
        self.best = None;
        self.phase = match (self.managed, &self.lease) {
            (true, Some(_)) => Phase::Rebinding,
            (true, None) => Phase::Soliciting,
            (false, _) => Phase::Informing,
        };
        self.start(now);
        self.next = now;
    }

    /// What to do at `now`: nothing before [`Client::next`].
    pub fn tick(&mut self, now: Instant) -> Vec<Action> {
        if now < self.next {
            return Vec::new();
        }
        match self.phase {
            Phase::Informed => {
                self.phase = Phase::Informing;
                self.start(now);
                self.send(now, INFORMATION_REQUEST, INFORMATION_TIMES)
            }
            Phase::Informing => self.send(now, INFORMATION_REQUEST, INFORMATION_TIMES),
            // The first timeout has passed with an ADVERTISE: request it.
            Phase::Soliciting if self.best.is_some() => {
                self.phase = Phase::Requesting;
                self.start(now);
                self.send(now, REQUEST, REQUEST_TIMES)
            }
            Phase::Soliciting => self.send(now, SOLICIT, SOLICIT_TIMES),
            Phase::Requesting if self.tries >= REQUEST_TRIES => self.solicit(now, Vec::new()),
            Phase::Requesting => self.send(now, REQUEST, REQUEST_TIMES),
            Phase::Bound | Phase::Renewing | Phase::Rebinding => self.extend(now),
        }
    }

    /// What to do with `bytes`, a message to the client's port.
    pub fn receive(&mut self, bytes: &[u8], now: Instant) -> Vec<Action> {
        let Some((kind, answer)) = self.read(bytes) else { return Vec::new() };
        match (kind, self.phase) {
            (REPLY, Phase::Informing) => {
                if answer.status != SUCCESS {
                    return Vec::new();
                }
                self.phase = Phase::Informed;
                let refresh = answer.refresh.unwrap_or(REFRESH_DEFAULT).max(REFRESH_MIN);
                self.next = if refresh == FOREVER {
                    now + Duration::from_secs(u64::from(u32::MAX))
                } else {
                    now + Duration::from_secs(u64::from(refresh))
                };
                vec![Action::Settings { dns: answer.dns, domains: answer.domains }]
            }
            (ADVERTISE, Phase::Soliciting) => {
                // One with no address, or that says it has none, is no
                // offer (RFC 8415, 18.2.9).
                if answer.status != SUCCESS
                    || answer.ia_status != SUCCESS
                    || answer.address.is_none()
                {
                    return Vec::new();
                }
                let preference = answer.preference;
                if self.best.as_ref().is_none_or(|best| preference > best.preference) {
                    self.best = Some(answer);
                }
                // The most preferred is taken at once; else the first
                // timeout's ADVERTISEs are waited for.
                if preference == 255 {
                    self.next = now;
                    return self.tick(now);
                }
                Vec::new()
            }
            (REPLY, Phase::Requesting | Phase::Renewing | Phase::Rebinding) => {
                self.replied(answer, now)
            }
            _ => Vec::new(),
        }
    }

    /// A REPLY to a REQUEST, RENEW or REBIND.
    fn replied(&mut self, answer: Answer, now: Instant) -> Vec<Action> {
        let gone = answer.ia_status == NO_BINDING
            || answer.address.is_some_and(|address| address.valid == 0);
        let Some(address) = answer
            .address
            .filter(|_| answer.status == SUCCESS && answer.ia_status == SUCCESS && !gone)
        else {
            // Refused: a lease the server no longer has goes; else the
            // client asks again, from SOLICIT, keeping one it has until
            // it ends.
            let had = self.lease.is_some() && (gone || self.phase == Phase::Requesting);
            if had {
                self.lease = None;
            }
            let actions = if had { vec![Action::Unbind] } else { Vec::new() };
            if self.lease.is_some() {
                return actions;
            }
            return self.solicit(now, actions);
        };
        let settings =
            Action::Settings { dns: answer.dns.clone(), domains: answer.domains.clone() };
        let (t1, t2) = times(answer.t1, answer.t2, address.preferred);
        let lease = Lease { address, server: answer.server, t1, t2 };
        self.lease = Some((lease.clone(), now));
        self.best = None;
        self.phase = Phase::Bound;
        self.tries = 0;
        self.next = self.renew_at().unwrap_or(now + Duration::from_secs(u64::from(u32::MAX)));
        vec![Action::Bind(lease), settings]
    }

    /// Starts again from SOLICIT, after `actions`.
    fn solicit(&mut self, now: Instant, mut actions: Vec<Action>) -> Vec<Action> {
        self.phase = Phase::Soliciting;
        self.best = None;
        self.start(now);
        self.next = now;
        actions.extend(self.tick(now));
        actions
    }

    /// Asks for more time for the address, from its server until T2 and
    /// from any after, or gives it up once it has ended.
    fn extend(&mut self, now: Instant) -> Vec<Action> {
        let Some((lease, given)) = self.lease.clone() else { return self.solicit(now, Vec::new()) };
        let after = |seconds: u32| given + Duration::from_secs(u64::from(seconds));
        if lease.address.valid != FOREVER && now >= after(lease.address.valid) {
            self.lease = None;
            return self.solicit(now, vec![Action::Unbind]);
        }
        let phase = if now >= after(lease.t2) {
            Phase::Rebinding
        } else if now >= after(lease.t1) {
            Phase::Renewing
        } else {
            self.phase = Phase::Bound;
            self.next = after(lease.t1);
            return Vec::new();
        };
        if phase != self.phase {
            self.phase = phase;
            self.start(now);
        }
        let kind = if phase == Phase::Rebinding { REBIND } else { RENEW };
        let actions = self.send(now, kind, RENEW_TIMES);
        // Renewing ends at T2, rebinding when the address does.
        let until = if phase == Phase::Renewing {
            Some(after(lease.t2))
        } else {
            (lease.address.valid != FOREVER).then(|| after(lease.address.valid))
        };
        if let Some(until) = until {
            self.next = self.next.min(until);
        }
        actions
    }

    /// Starts an exchange with a new transaction number.
    fn start(&mut self, now: Instant) {
        let random = self.random();
        self.xid = [(random >> 16) as u8, (random >> 8) as u8, random as u8];
        self.began = now;
        self.tries = 0;
        self.timeout = Duration::ZERO;
    }

    /// Sends a message of `kind` and sets when to send it again: its
    /// first timeout, then twice the last, a tenth either way at random,
    /// up to its most (RFC 8415, 15). The first SOLICIT waits a little
    /// longer than its first timeout, never less.
    fn send(&mut self, now: Instant, kind: u8, times: Times) -> Vec<Action> {
        let message = self.message(kind, now);
        let (first, most) = (Duration::from_secs(times.first), Duration::from_secs(times.most));
        let base = if self.timeout.is_zero() { first } else { (self.timeout * 2).min(most) };
        let tenth = base.as_millis() as u64 / 10;
        let random = self.random() % (2 * tenth + 1);
        let positive = kind == SOLICIT && self.timeout.is_zero();
        let jitter = if positive { random / 2 } else { random };
        self.timeout = (base + Duration::from_millis(jitter))
            .saturating_sub(Duration::from_millis(if positive { 0 } else { tenth }));
        self.next = now + self.timeout;
        self.tries += 1;
        vec![Action::Send(message)]
    }

    /// A message of `kind`, with the client's identifier, the time since
    /// the exchange began, the options it asks for, and for those about
    /// an address its IA_NA.
    fn message(&self, kind: u8, now: Instant) -> Vec<u8> {
        let mut out = vec![kind, self.xid[0], self.xid[1], self.xid[2]];
        put_option(&mut out, CLIENT_ID, &self.duid);
        let server = match kind {
            REQUEST => self.best.as_ref().map(|best| best.server.clone()),
            RENEW => self.lease.as_ref().map(|(lease, _)| lease.server.clone()),
            _ => None,
        };
        if let Some(server) = server {
            put_option(&mut out, SERVER_ID, &server);
        }
        // In hundredths of a second, the first message's 0.
        let hundredths = now.saturating_duration_since(self.began).as_millis() / 10;
        let elapsed =
            if self.tries == 0 { 0 } else { u16::try_from(hundredths).unwrap_or(u16::MAX) };
        put_option(&mut out, ELAPSED_TIME, &elapsed.to_be_bytes());
        let mut asked = vec![0, DNS_SERVERS as u8, 0, DOMAIN_LIST as u8];
        if kind == INFORMATION_REQUEST {
            asked.extend_from_slice(&REFRESH_TIME.to_be_bytes());
        }
        put_option(&mut out, REQUESTED, &asked);
        if kind != INFORMATION_REQUEST {
            let address = match kind {
                REQUEST => self.best.as_ref().and_then(|best| best.address),
                RENEW | REBIND => self.lease.as_ref().map(|(lease, _)| lease.address),
                _ => None,
            };
            // The IAID, and T1 and T2 left to the server.
            let mut ia = self.iaid.to_vec();
            ia.extend_from_slice(&[0; 8]);
            if let Some(address) = address {
                let mut inner = address.ip.octets().to_vec();
                inner.extend_from_slice(&[0; 8]);
                put_option(&mut ia, IA_ADDRESS, &inner);
            }
            put_option(&mut out, IA_NA, &ia);
        }
        out
    }

    /// `bytes` as an ADVERTISE or REPLY to this client's transaction,
    /// naming it and a server: its type, and what it says.
    fn read(&self, bytes: &[u8]) -> Option<(u8, Answer)> {
        let (&kind, rest) = bytes.split_first()?;
        if (kind != ADVERTISE && kind != REPLY) || rest.get(..3)? != self.xid {
            return None;
        }
        let mut answer = Answer::default();
        let mut client = None;
        for (code, data) in options(&rest[3..])? {
            match code {
                CLIENT_ID => client = Some(data),
                SERVER_ID if !data.is_empty() && data.len() <= 130 => answer.server = data.to_vec(),
                PREFERENCE if data.len() == 1 => answer.preference = data[0],
                STATUS_CODE if data.len() >= 2 => {
                    answer.status = u16::from_be_bytes([data[0], data[1]])
                }
                IA_NA if data.len() >= 12 && data[..4] == self.iaid => {
                    answer.t1 = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
                    answer.t2 = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
                    for (code, data) in options(&data[12..])? {
                        match code {
                            STATUS_CODE if data.len() >= 2 => {
                                answer.ia_status = u16::from_be_bytes([data[0], data[1]]);
                            }
                            IA_ADDRESS if data.len() >= 24 && answer.address.is_none() => {
                                let octets: [u8; 16] = data[..16].try_into().ok()?;
                                let word = |at: usize| {
                                    u32::from_be_bytes([
                                        data[at],
                                        data[at + 1],
                                        data[at + 2],
                                        data[at + 3],
                                    ])
                                };
                                let address = Address {
                                    ip: Ipv6Addr::from(octets),
                                    preferred: word(16),
                                    valid: word(20),
                                };
                                // Preferred longer than valid is refused
                                // (RFC 8415, 21.6).
                                if unicast(address.ip) && address.preferred <= address.valid {
                                    answer.address = Some(address);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                DNS_SERVERS => {
                    for chunk in data.chunks_exact(16).take(3) {
                        let octets: [u8; 16] = chunk.try_into().ok()?;
                        let server = Ipv6Addr::from(octets);
                        if unicast(server) || is_link_local(server) {
                            answer.dns.push(server);
                        }
                    }
                }
                DOMAIN_LIST => answer.domains = domains(data),
                REFRESH_TIME if data.len() == 4 => {
                    answer.refresh = Some(u32::from_be_bytes([data[0], data[1], data[2], data[3]]));
                }
                _ => {}
            }
        }
        if client? != self.duid.as_slice() || answer.server.is_empty() {
            return None;
        }
        Some((kind, answer))
    }

    /// When to renew the lease.
    fn renew_at(&self) -> Option<Instant> {
        let (lease, given) = self.lease.as_ref()?;
        (lease.t1 != FOREVER).then(|| *given + Duration::from_secs(u64::from(lease.t1)))
    }

    /// The next of its numbers (xorshift64*).
    fn random(&mut self) -> u64 {
        self.random ^= self.random >> 12;
        self.random ^= self.random << 25;
        self.random ^= self.random >> 27;
        self.random.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
}

/// When to renew and rebind an address the server gave T1 and T2 for:
/// those, or with 0, half and four fifths of its preferred lifetime
/// (RFC 8415, 21.4); never sooner than a minute, nor T2 before T1.
fn times(t1: u32, t2: u32, preferred: u32) -> (u32, u32) {
    if preferred == FOREVER && t1 == 0 {
        return (FOREVER, FOREVER);
    }
    let t1 = if t1 == 0 { preferred / 2 } else { t1 }.max(RENEW_MIN);
    let t2 = if t2 == 0 || t2 < t1 { (u64::from(preferred) * 4 / 5) as u32 } else { t2 };
    (t1, t2.max(t1))
}

/// Appends an option of `code` holding `data`.
fn put_option(out: &mut Vec<u8>, code: u16, data: &[u8]) {
    out.extend_from_slice(&code.to_be_bytes());
    out.extend_from_slice(&(data.len() as u16).to_be_bytes());
    out.extend_from_slice(data);
}

/// The options in `bytes`, each its code and its data: none if one runs
/// past the end.
fn options(mut bytes: &[u8]) -> Option<Vec<(u16, &[u8])>> {
    let mut found = Vec::new();
    while !bytes.is_empty() {
        let code = u16::from_be_bytes([*bytes.first()?, *bytes.get(1)?]);
        let len = usize::from(u16::from_be_bytes([*bytes.get(2)?, *bytes.get(3)?]));
        let data = bytes.get(4..4 + len)?;
        found.push((code, data));
        bytes = &bytes[4 + len..];
    }
    Some(found)
}

/// The domain names of a DOMAIN_LIST option, in DNS's form without
/// compression (RFC 8415, 10): those that are names, letters, digits,
/// `-` and `_` in labels of 63 at most, three at most.
fn domains(mut data: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    'names: while !data.is_empty() && names.len() < 3 {
        let mut labels: Vec<&str> = Vec::new();
        loop {
            let Some((&len, rest)) = data.split_first() else { break 'names };
            let len = usize::from(len);
            if len == 0 {
                data = rest;
                break;
            }
            let Some(label) = rest.get(..len) else { break 'names };
            let ok = len <= 63
                && label
                    .iter()
                    .all(|&byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
            labels.push(if ok { std::str::from_utf8(label).unwrap_or("") } else { "" });
            data = &rest[len..];
        }
        let name = labels.join(".");
        if !labels.is_empty() && labels.iter().all(|label| !label.is_empty()) && name.len() <= 253 {
            names.push(name);
        }
    }
    names
}

/// Whether `ip` is fe80::/10.
fn is_link_local(ip: Ipv6Addr) -> bool {
    ip.segments()[0] & 0xffc0 == 0xfe80
}

/// Whether a host may have `ip` from a server: a global or unique local
/// address, not link-local, loopback, multicast or IPv4-mapped.
pub fn unicast(ip: Ipv6Addr) -> bool {
    !ip.is_unspecified()
        && !ip.is_loopback()
        && !ip.is_multicast()
        && !is_link_local(ip)
        && ip.to_ipv4_mapped().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC: [u8; 6] = [0x52, 0x54, 0, 0x12, 0x34, 0x56];
    const SERVER_DUID: &[u8] = &[0, 3, 0, 1, 0x52, 0x55, 10, 0, 2, 2];

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    /// The one message in `actions`.
    fn sent(actions: &[Action]) -> Vec<u8> {
        let sends: Vec<_> = actions
            .iter()
            .filter_map(|action| match action {
                Action::Send(message) => Some(message.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(sends.len(), 1, "{actions:?}");
        sends.into_iter().next().unwrap()
    }

    /// A message's type, and its options by code.
    fn read(message: &[u8]) -> (u8, Vec<(u16, Vec<u8>)>) {
        let found = options(&message[4..]).unwrap();
        (message[0], found.into_iter().map(|(code, data)| (code, data.to_vec())).collect())
    }

    fn option(message: &[u8], code: u16) -> Option<Vec<u8>> {
        read(message).1.into_iter().find(|(c, _)| *c == code).map(|(_, data)| data)
    }

    /// A server's answer of `kind` to `to`, with `address` (and its
    /// lifetimes) in the IA_NA, `preference`, and a DNS server.
    fn answer(kind: u8, to: &[u8], address: Option<(&str, u32, u32)>, preference: u8) -> Vec<u8> {
        let mut out = vec![kind, to[1], to[2], to[3]];
        put_option(&mut out, CLIENT_ID, &option(to, CLIENT_ID).unwrap());
        put_option(&mut out, SERVER_ID, SERVER_DUID);
        if preference != 0 {
            put_option(&mut out, PREFERENCE, &[preference]);
        }
        if let Some((ip, preferred, valid)) = address {
            let mut ia = MAC[2..].to_vec();
            ia.extend_from_slice(&0u32.to_be_bytes());
            ia.extend_from_slice(&0u32.to_be_bytes());
            let mut inner = ip.parse::<Ipv6Addr>().unwrap().octets().to_vec();
            inner.extend_from_slice(&preferred.to_be_bytes());
            inner.extend_from_slice(&valid.to_be_bytes());
            put_option(&mut ia, IA_ADDRESS, &inner);
            put_option(&mut out, IA_NA, &ia);
        }
        put_option(&mut out, DNS_SERVERS, &"2001:db8::53".parse::<Ipv6Addr>().unwrap().octets());
        put_option(&mut out, DOMAIN_LIST, b"\x07example\x03org\x00\x03lan\x00");
        out
    }

    #[test]
    fn the_settings_alone_are_asked_and_asked_again_later() {
        let start = Instant::now();
        let mut client = Client::new(MAC, false, 7, start);
        assert!(client.next() < start + secs(1));
        assert_eq!(client.tick(start), []);
        let now = client.next();
        let message = sent(&client.tick(now));
        let (kind, found) = read(&message);
        assert_eq!(kind, INFORMATION_REQUEST);
        assert_eq!(
            option(&message, CLIENT_ID).unwrap(),
            [0, 3, 0, 1, 0x52, 0x54, 0, 0x12, 0x34, 0x56]
        );
        assert!(found.iter().all(|(code, _)| *code != IA_NA));
        assert_eq!(option(&message, REQUESTED).unwrap(), [0, 23, 0, 24, 0, 32]);
        assert_eq!(option(&message, ELAPSED_TIME).unwrap(), [0, 0]);
        // Not answered: sent again after about a second, then two.
        let first = client.next() - now;
        assert!(
            first >= Duration::from_millis(900) && first <= Duration::from_millis(1100),
            "{first:?}"
        );
        let again = sent(&client.tick(client.next()));
        assert_ne!(option(&again, ELAPSED_TIME).unwrap(), [0, 0]);
        // An answer to another transaction is passed over.
        let mut other = answer(REPLY, &again, None, 0);
        other[1] ^= 1;
        assert_eq!(client.receive(&other, now), []);
        let actions = client.receive(&answer(REPLY, &again, None, 0), now);
        assert_eq!(
            actions,
            [Action::Settings {
                dns: vec!["2001:db8::53".parse().unwrap()],
                domains: vec!["example.org".into(), "lan".into()],
            }]
        );
        assert_eq!(client.phase(), Phase::Informed);
        assert_eq!(client.next(), now + secs(86_400));
        assert_eq!(sent(&client.tick(now + secs(86_400)))[0], INFORMATION_REQUEST);
    }

    #[test]
    fn an_address_is_solicited_requested_renewed_and_rebound() {
        let start = Instant::now();
        let mut client = Client::new(MAC, true, 11, start);
        let now = client.next();
        let solicit = sent(&client.tick(now));
        assert_eq!(solicit[0], SOLICIT);
        let ia = option(&solicit, IA_NA).unwrap();
        assert_eq!(ia[..4], MAC[2..]);
        // The first SOLICIT waits at least its timeout (RFC 8415, 15).
        assert!(client.next() - now >= secs(1));
        // An ADVERTISE of preference 0 is held until the timeout ends.
        let advert = answer(ADVERTISE, &solicit, Some(("2001:db8::99", 3600, 7200)), 0);
        assert_eq!(client.receive(&advert, now), []);
        let request = sent(&client.tick(client.next()));
        assert_eq!(request[0], REQUEST);
        assert_eq!(option(&request, SERVER_ID).unwrap(), SERVER_DUID);
        let requested = option(&request, IA_NA).unwrap();
        assert_eq!(requested[16..32], "2001:db8::99".parse::<Ipv6Addr>().unwrap().octets());
        let given = now + secs(2);
        let reply = answer(REPLY, &request, Some(("2001:db8::99", 3600, 7200)), 0);
        let actions = client.receive(&reply, given);
        let lease = client.lease().unwrap().clone();
        assert_eq!(lease.address.ip, "2001:db8::99".parse::<Ipv6Addr>().unwrap());
        // T1 and T2 the client's: half and four fifths of the preferred.
        assert_eq!((lease.t1, lease.t2), (1800, 2880));
        assert!(matches!(&actions[..], [Action::Bind(_), Action::Settings { .. }]));
        assert_eq!(client.next(), given + secs(1800));
        // Renewed from the server at T1, rebound from any at T2.
        let renew = sent(&client.tick(given + secs(1800)));
        assert_eq!(renew[0], RENEW);
        assert_eq!(option(&renew, SERVER_ID).unwrap(), SERVER_DUID);
        assert!(client.next() <= given + secs(1800) + secs(11));
        let rebind = sent(&client.tick(given + secs(2880)));
        assert_eq!(rebind[0], REBIND);
        assert_eq!(option(&rebind, SERVER_ID), None);
        // Answered, the address goes on.
        let reply = answer(REPLY, &rebind, Some(("2001:db8::99", 3600, 7200)), 0);
        let renewed = given + secs(2900);
        assert!(matches!(&client.receive(&reply, renewed)[..], [Action::Bind(_), _]));
        assert_eq!(client.phase(), Phase::Bound);
        assert_eq!(client.next(), renewed + secs(1800));
    }

    #[test]
    fn an_address_ends_with_its_lifetime_or_the_servers_word() {
        let start = Instant::now();
        let mut client = Client::new(MAC, true, 3, start);
        let solicit = sent(&client.tick(client.next()));
        // Preference 255 is requested at once.
        let advert = answer(ADVERTISE, &solicit, Some(("2001:db8::7", 600, 900)), 255);
        let request = sent(&client.receive(&advert, start));
        assert_eq!(request[0], REQUEST);
        let reply = answer(REPLY, &request, Some(("2001:db8::7", 600, 900)), 0);
        client.receive(&reply, start);
        // Unanswered to the end of its valid lifetime: given up, and
        // solicited again.
        let mut now = start;
        let mut actions = Vec::new();
        while now < start + secs(901) {
            now = client.next().max(now);
            actions = client.tick(now);
            if actions.contains(&Action::Unbind) {
                break;
            }
        }
        assert_eq!(actions[0], Action::Unbind);
        assert_eq!(read(&sent(&actions)).0, SOLICIT);
        assert!(now >= start + secs(900) && now < start + secs(901));
        // A server that says there is no binding ends it too.
        let solicit = sent(&actions);
        let advert = answer(ADVERTISE, &solicit, Some(("2001:db8::7", 600, 900)), 255);
        let request = sent(&client.receive(&advert, now));
        client.receive(&answer(REPLY, &request, Some(("2001:db8::7", 600, 900)), 0), now);
        let renew = sent(&client.tick(now + secs(300)));
        let mut refused = answer(REPLY, &renew, None, 0);
        let mut ia = MAC[2..].to_vec();
        ia.extend_from_slice(&[0; 8]);
        put_option(&mut ia, STATUS_CODE, &NO_BINDING.to_be_bytes());
        put_option(&mut refused, IA_NA, &ia);
        let actions = client.receive(&refused, now + secs(301));
        assert_eq!(actions[0], Action::Unbind);
        assert_eq!(client.phase(), Phase::Soliciting);
    }

    #[test]
    fn answers_are_checked_before_they_are_taken() {
        let start = Instant::now();
        let mut client = Client::new(MAC, true, 5, start);
        let solicit = sent(&client.tick(client.next()));
        // Another client's, an address no host may have, and one cut short.
        let mut theirs = answer(ADVERTISE, &solicit, Some(("2001:db8::7", 600, 900)), 255);
        theirs[4 + 4 + 9] ^= 0xff;
        assert_eq!(client.receive(&theirs, start), []);
        let link_local = answer(ADVERTISE, &solicit, Some(("fe80::7", 600, 900)), 255);
        assert_eq!(client.receive(&link_local, start), []);
        let longer = answer(ADVERTISE, &solicit, Some(("2001:db8::7", 900, 600)), 255);
        assert_eq!(client.receive(&longer, start), []);
        let whole = answer(ADVERTISE, &solicit, Some(("2001:db8::7", 600, 900)), 255);
        assert_eq!(client.receive(&whole[..whole.len() - 3], start), []);
        assert_eq!(client.phase(), Phase::Soliciting);
        assert_eq!(domains(b"\x03a.b\x00\x04good\x00"), ["good"]);
        assert_eq!(domains(b"\x05short"), Vec::<String>::new());
        assert_eq!(times(0, 0, FOREVER), (FOREVER, FOREVER));
        assert_eq!(times(10, 5, 100), (60, 80));
    }
}
