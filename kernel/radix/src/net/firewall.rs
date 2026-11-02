// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The firewall (ADR 0334): what the interfaces let in from other
//! machines.
//!
//! Each card's, bridge's and VLAN's stack asks [`admit`] about each packet
//! that would start something (radix-net's `filter`): a TCP segment that
//! opens a connection, a UDP datagram that answers nothing the machine
//! sent, a ping. `lo` asks nothing: what this machine sends itself, to
//! any of its addresses, always arrives. The answer is the first of the
//! rules that matches it, which radix-firewall(2)'s `set_rules` gives,
//! in order; and when none does, a ping is let in, and a connection or
//! datagram only if a socket listens for it whose network capability
//! has the inbound right for its port. Spark gives that right to the
//! services whose files ask to listen (service.toml(5)), so that sshd's
//! port 22 is open while sshd runs and closed when it does not; a
//! program a user runs listens without it, for this machine alone, until
//! a rule allows its program from other machines.
//!
//! A rule may name a program: the file it runs from, by its mount and
//! file numbers, as stat(2) gives them (st_dev and st_ino). It matches
//! what comes to a port that program's socket listens on, which the
//! kernel knows from the program each process last ran (radix-process(2)'s
//! `exec`), so that a program cannot pass for another by its name.
//! firewall(8) finds the numbers of the paths its settings name.
//!
//! What is not let in is dropped without an answer, counted, and logged
//! once the interface has let go of its stack: up to [`LOGGED_EACH`]
//! packets a second, and how many more were dropped.

use core::fmt::Write as _;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use radix_klog::{TextBuf, info};
use radix_net::IpAddress;
use radix_net::filter::{Inbound, Protocol as Kind};
use radix_sync::Mutex;

use super::numbered;
use super::ports::{Protocol, Versions};

/// A program, as the file it runs from: its mount's number and its own
/// (stat(2)'s st_dev and st_ino).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Program {
    pub mount: u64,
    pub id: u64,
}

impl Program {
    /// No program the kernel knows: one started before any `exec`.
    pub const NONE: Program = Program { mount: 0, id: 0 };
}

/// What a rule does with what it matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Allow,
    Deny,
}

/// What a rule matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Match {
    /// Connections, datagrams and pings.
    Any,
    Tcp,
    Udp,
    Ping,
}

/// One of the firewall's rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    pub action: Action,
    pub protocol: Match,
    /// The ports it is for, the first and the last: every one for (0,
    /// 65535). A ping has none, and matches only a rule for every port.
    pub ports: (u16, u16),
    /// The network it is from, an address and its prefix's length: from
    /// anywhere for `None`.
    pub from: Option<(IpAddress, u8)>,
    /// The interface it comes by, by the kernel's number: any for `None`.
    pub interface: Option<usize>,
    /// The program that listens for it: any, or none, for `None`.
    pub program: Option<Program>,
}

/// The most rules the firewall holds.
pub const RULES_MAX: usize = 128;

/// The rules, in order.
struct Rules {
    rules: [Option<Rule>; RULES_MAX],
    count: usize,
}

static RULES: Mutex<Rules> = Mutex::new(Rules { rules: [None; RULES_MAX], count: 0 });

/// Whether any rule was given: until one is, only the listeners decide.
static ANY_RULES: AtomicBool = AtomicBool::new(false);

/// Packets dropped since the machine started.
static DROPPED: AtomicU64 = AtomicU64::new(0);

/// Gives the firewall `rules`, in place of those it had.
///
/// # Errors
///
/// `Err` if there are more than [`RULES_MAX`].
pub fn set_rules(rules: &[Rule]) -> Result<(), ()> {
    if rules.len() > RULES_MAX {
        return Err(());
    }
    let mut held = RULES.lock();
    // A place at a time: the table is too large for a stack frame.
    for (at, place) in held.rules.iter_mut().enumerate() {
        *place = rules.get(at).copied();
    }
    held.count = rules.len();
    ANY_RULES.store(!rules.is_empty(), Ordering::Release);
    Ok(())
}

/// The rule numbered `at`, from 0, if there is one.
pub fn rule(at: usize) -> Option<Rule> {
    RULES.lock().rules.get(at).copied().flatten()
}

/// How many rules there are, and how many packets were dropped.
pub fn counts() -> (usize, u64) {
    (RULES.lock().count, DROPPED.load(Ordering::Relaxed))
}

/// Who a socket is: the program that bound it, and its process.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Owner {
    pub program: Program,
    /// Its process's number.
    pub process: u64,
}

/// A socket that may take what other machines start: one listening for
/// TCP connections, or bound to a UDP port of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Listener {
    pub protocol: Protocol,
    pub port: u16,
    pub versions: Versions,
    /// The interface it is bound to (SO_BINDTODEVICE), by its `ifindex`:
    /// 0 for any.
    pub device: u32,
    pub owner: Owner,
    /// Whether its network capability has the inbound right for its
    /// port, which lets in what other machines start.
    pub inbound: bool,
}

impl Listener {
    /// Whether it takes what `packet` starts, of `protocol`, coming by
    /// interface `number`.
    fn takes(&self, protocol: Protocol, packet: &Inbound, number: usize) -> bool {
        let v6 = matches!(packet.to, IpAddress::Ipv6(_));
        let versions = match self.versions {
            Versions::Ipv4 => !v6,
            Versions::Ipv6 => v6,
            Versions::Both => true,
        };
        self.protocol == protocol
            && self.port == packet.port
            && versions
            && (self.device == 0 || self.device as usize == number + 1)
    }
}

/// The most listeners the firewall knows of. A socket past them listens
/// as one without the inbound right, for this machine alone, unless a
/// rule for its port that names no program allows what comes to it.
pub const LISTENERS_MAX: usize = 256;

static LISTENERS: Mutex<[Option<Listener>; LISTENERS_MAX]> = Mutex::new([None; LISTENERS_MAX]);

/// A socket's place among the listeners, until it is dropped.
#[derive(Debug)]
pub struct Listening(usize);

impl Drop for Listening {
    fn drop(&mut self) {
        if let Some(place) = LISTENERS.lock().get_mut(self.0) {
            *place = None;
        }
    }
}

/// Notes `listener`: `None` if there is no room for it.
pub fn listen(listener: Listener) -> Option<Listening> {
    let mut listeners = LISTENERS.lock();
    let at = listeners.iter().position(Option::is_none)?;
    listeners[at] = Some(listener);
    Some(Listening(at))
}

/// The listener in place `at`, from 0 to [`LISTENERS_MAX`], if one is
/// there.
pub fn listener(at: usize) -> Option<Listener> {
    LISTENERS.lock().get(at).copied().flatten()
}

impl Rule {
    /// Whether it matches `packet`, coming by interface `number`, which
    /// `listener` takes if any does.
    fn matches(&self, packet: &Inbound, number: usize, listener: Option<&Listener>) -> bool {
        let protocol = matches!(
            (self.protocol, packet.protocol),
            (Match::Any, _)
                | (Match::Tcp, Kind::Tcp)
                | (Match::Udp, Kind::Udp)
                | (Match::Ping, Kind::Ping)
        );
        let port = match packet.protocol {
            Kind::Ping => self.ports == (0, u16::MAX),
            Kind::Tcp | Kind::Udp => (self.ports.0..=self.ports.1).contains(&packet.port),
        };
        let from = self.from.is_none_or(|(network, prefix)| within(packet.from, network, prefix));
        let interface = self.interface.is_none_or(|interface| interface == number);
        let program = self.program.is_none_or(|program| {
            listener.is_some_and(|listener| listener.owner.program == program)
        });
        protocol && port && from && interface && program
    }
}

/// Whether `address` is in the network `network`/`prefix`, of its
/// version.
fn within(address: IpAddress, network: IpAddress, prefix: u8) -> bool {
    match (address, network) {
        (IpAddress::Ipv4(address), IpAddress::Ipv4(network)) => {
            let mask = u32::MAX.checked_shl(32 - u32::from(prefix.min(32))).unwrap_or(0);
            u32::from(address) & mask == u32::from(network) & mask
        }
        (IpAddress::Ipv6(address), IpAddress::Ipv6(network)) => {
            let mask = u128::MAX.checked_shl(128 - u32::from(prefix.min(128))).unwrap_or(0);
            u128::from(address) & mask == u128::from(network) & mask
        }
        _ => false,
    }
}

/// Whether to let `packet` in on interface `number` (the kernel's), at
/// `now_ns`: the first rule that matches decides, else the listener.
pub fn admit(number: usize, packet: &Inbound, now_ns: u64) -> bool {
    let protocol = match packet.protocol {
        Kind::Tcp => Some(Protocol::Tcp),
        Kind::Udp => Some(Protocol::Udp),
        Kind::Ping => None,
    };
    let listener = protocol.and_then(|protocol| {
        let listeners = LISTENERS.lock();
        listeners.iter().flatten().find(|l| l.takes(protocol, packet, number)).copied()
    });
    let mut decided = None;
    if ANY_RULES.load(Ordering::Acquire) {
        let rules = RULES.lock();
        decided = rules
            .rules
            .iter()
            .flatten()
            .find(|rule| rule.matches(packet, number, listener.as_ref()))
            .map(|rule| rule.action == Action::Allow);
    }
    let allowed = decided.unwrap_or(match packet.protocol {
        Kind::Ping => true,
        Kind::Tcp | Kind::Udp => listener.is_some_and(|listener| listener.inbound),
    });
    if !allowed {
        dropped(number, packet, listener.map(|listener| listener.owner), now_ns);
    }
    allowed
}

/// The packets logged each second at most.
pub const LOGGED_EACH: usize = 5;

/// A dropped packet still to be logged.
#[derive(Clone, Copy)]
struct Dropped {
    number: usize,
    packet: Inbound,
    /// The socket that listened for it, without the inbound right.
    owner: Option<Owner>,
}

/// The drops still to be logged, and those past them this second.
struct Drops {
    waiting: [Option<Dropped>; LOGGED_EACH],
    /// The second the drops counted are in.
    second: u64,
    /// The drops in it.
    this_second: usize,
    /// Those past [`LOGGED_EACH`] in it, not logged.
    unlogged: u64,
}

static DROPS: Mutex<Drops> =
    Mutex::new(Drops { waiting: [None; LOGGED_EACH], second: 0, this_second: 0, unlogged: 0 });

/// Whether drops wait to be logged.
static WAITING: AtomicBool = AtomicBool::new(false);

/// Counts a dropped packet, and keeps it to be logged.
fn dropped(number: usize, packet: &Inbound, owner: Option<Owner>, now_ns: u64) {
    DROPPED.fetch_add(1, Ordering::Relaxed);
    let second = now_ns / 1_000_000_000;
    let mut drops = DROPS.lock();
    if drops.second != second {
        drops.second = second;
        drops.this_second = 0;
    }
    drops.this_second += 1;
    if drops.this_second > LOGGED_EACH {
        drops.unlogged += 1;
    } else if let Some(place) = drops.waiting.iter_mut().find(|place| place.is_none()) {
        *place = Some(Dropped { number, packet: *packet, owner });
    }
    WAITING.store(true, Ordering::Release);
}

/// Logs the drops kept since the last call: after a poll, once its stack
/// is let go, as no lock may be held while logging.
pub fn log_dropped() {
    if !WAITING.swap(false, Ordering::AcqRel) {
        return;
    }
    let (waiting, unlogged) = {
        let mut drops = DROPS.lock();
        let waiting = core::mem::replace(&mut drops.waiting, [None; LOGGED_EACH]);
        (waiting, core::mem::take(&mut drops.unlogged))
    };
    for drop in waiting.iter().flatten() {
        let name = numbered(drop.number).map_or("?", |interface| interface.name.as_str());
        let packet = drop.packet;
        let what = match packet.protocol {
            Kind::Tcp => "a TCP connection",
            Kind::Udp => "a UDP datagram",
            Kind::Ping => "a ping",
        };
        let mut to = TextBuf::<64>::new();
        if packet.protocol != Kind::Ping {
            let _ = write!(to, " to port {}", packet.port);
        }
        let mut from = TextBuf::<64>::new();
        match packet.from {
            IpAddress::Ipv6(address) if packet.protocol != Kind::Ping => {
                let _ = write!(from, "[{address}]:{}", packet.from_port);
            }
            address if packet.protocol != Kind::Ping => {
                let _ = write!(from, "{address}:{}", packet.from_port);
            }
            address => {
                let _ = write!(from, "{address}");
            }
        }
        match drop.owner {
            Some(owner) => info!(
                "Firewall: dropped {what} from {from}{to} on {name}: process {} listens \
                 for this machine alone",
                owner.process
            ),
            None => info!("Firewall: dropped {what} from {from}{to} on {name}"),
        }
    }
    if unlogged > 0 {
        info!("Firewall: dropped {unlogged} more packets");
    }
}
