// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Blocking the addresses that guess passwords (ADR 0479).
//!
//! authd counts each failed sign-in from the network for the address it
//! came from, whatever account it named: a wrong password over SSH, a
//! wrong sharing password, a wrong password at the web console, a wrong
//! help-session code, and an SSH connection that tried to sign in, was
//! refused each time, and ended. An address is an IPv4 address or an
//! IPv6 network of 64 bits, since one machine holds a whole /64; an IPv4
//! address written as IPv6 counts as the IPv4 one.
//!
//! When `tries` of them fail within `within`, the address is blocked from
//! the sign-in ports, those sshd, smbd and admind listen on as their
//! service files say, for `first`; a block that comes within a day of the
//! end of the one before lasts twice as long, up to `longest`. The kernel's
//! firewall drops what the address starts there, before any rule
//! (radix-firewall(2) `set_blocked`), and stops once the time is up,
//! even if authd is late. A connection open already is not ended: its
//! next password is refused unread.
//!
//! Never blocked: loopback, the networks in `never`, and, while their
//! session lasts, an address an administrator signed in from over the
//! network, so that a mistyped password cannot lock out the person
//! fixing the machine. A right password clears its address's count, but
//! not its blocks of the day.
//!
//! The settings are [`SETTINGS`], read again whenever the file changes;
//! without it, blocking is on with the values of [`Settings::default`].
//! The blocks are kept in [`STATE`], so that they outlast a restart of
//! authd or of the machine; the counts are kept in memory alone.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;
use std::time::SystemTime;

use accounts::request::{self, Answer, BlockedEntry};
use manifest::service;
use radix_abi::firewall::{
    self as abi, BLOCK_PORTS, BLOCK_SECONDS_MAX, BLOCKS_MAX, Block as AbiBlock,
};
use radix_abi::startup::SLOT_FIREWALL;
use serde::Deserialize;

use super::{Authd, Files, Notices, Peer, read_text, replace, shown};
use crate::servicefiles;
use crate::sys::SYS;

/// The settings: `on`, `tries`, `within`, `first`, `longest` and
/// `never`, blocking.toml(5).
pub(crate) const SETTINGS: &str = "/system/settings/blocking.toml";

/// The blocks, one a line: `ADDRESS UNTIL STARTED TIMES WHY`, as
/// `203.0.113.9 1794652320 1794651720 1 5 wrong passwords over SSH in 2
/// minutes, for root`, where UNTIL is when it ends and STARTED when it
/// began, in seconds since 1970, and TIMES how many blocks of the address
/// came one after another, each within a day of the end of the one
/// before, this one counted. A block is kept for a day after it ends, so
/// that the next one lasts longer.
pub(super) const STATE: &str = "/system/state/authd/blocked";

/// The services whose ports a block closes: those people sign in to.
const SIGN_IN_SERVICES: [&str; 3] = ["sshd", "smbd", "admind"];

/// The port a block closes when no sign-in service's file names one.
const SSH_PORT: u16 = 22;

/// The most addresses counted, and the most blocks kept: past either,
/// the oldest goes, so that a guesser with many addresses cannot fill
/// authd's memory.
const KEPT_MAX: usize = 4096;

/// The most networks `never` holds.
const NEVER_MAX: usize = 256;

/// A day, in seconds: how long a block is remembered after it ends, and
/// the most a count's window may be.
const DAY: u64 = 86_400;

/// The least `within` and `first` may be, in seconds.
const SHORTEST: u64 = 60;

/// The most `tries` may be.
const TRIES_MAX: u32 = 100;

/// The fewest `tries` may be: one mistyped password never blocks.
const TRIES_MIN: u32 = 2;

/// The most account names a block's reason lists.
const NAMES_SHOWN: usize = 3;

/// Where failed sign-ins come from: an IPv4 address, or the first 64 bits
/// of an IPv6 address, its network. Ordered as the kernel keeps its
/// blocks: the IPv4 addresses first, by number, then the IPv6 networks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Source {
    V4(Ipv4Addr),
    V6(u64),
}

impl Source {
    /// The source `address` is in: none for loopback, which is never
    /// counted, and for the unspecified address.
    pub fn of(address: IpAddr) -> Option<Source> {
        let address = address.to_canonical();
        if address.is_loopback() || address.is_unspecified() {
            return None;
        }
        Some(match address {
            IpAddr::V4(address) => Source::V4(address),
            IpAddr::V6(address) => Source::V6((u128::from(address) >> 64) as u64),
        })
    }

    /// The source `text` names: an IPv4 address, or an IPv6 network of
    /// 64 bits, as `2001:db8:4:7::/64`, or any address in it.
    pub fn parse(text: &str) -> Option<Source> {
        let (address, prefix) = match text.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix)),
            None => (text, None),
        };
        let address: IpAddr = address.parse().ok()?;
        match (address.to_canonical(), prefix) {
            (IpAddr::V4(_), None | Some("32")) | (IpAddr::V6(_), None | Some("64")) => {
                Source::of(address)
            }
            _ => None,
        }
    }

    /// Whether `address` is in it.
    pub fn holds(self, address: IpAddr) -> bool {
        Source::of(address) == Some(self)
    }

    /// The kernel's Block structure of it, lasting `seconds` and closing
    /// the ranges of ports `ports`, at most [`BLOCK_PORTS`] of them.
    fn encode(self, seconds: u64, ports: &[(u16, u16)]) -> [u8; AbiBlock::SIZE] {
        let mut bytes = [0u8; AbiBlock::SIZE];
        let mut put = |at: usize, value: &[u8]| bytes[at..at + value.len()].copy_from_slice(value);
        match self {
            Source::V4(address) => {
                put(AbiBlock::OFFSET_FAMILY, &4u32.to_le_bytes());
                put(AbiBlock::OFFSET_FROM, &address.octets());
            }
            Source::V6(network) => {
                put(AbiBlock::OFFSET_FAMILY, &6u32.to_le_bytes());
                put(AbiBlock::OFFSET_FROM, &network.to_be_bytes());
            }
        }
        let ports = &ports[..ports.len().min(BLOCK_PORTS as usize)];
        put(AbiBlock::OFFSET_PORT_COUNT, &(ports.len() as u32).to_le_bytes());
        put(AbiBlock::OFFSET_SECONDS, &seconds.to_le_bytes());
        for (at, &(first, last)) in ports.iter().enumerate() {
            put(AbiBlock::OFFSET_FIRST_PORTS + 2 * at, &first.to_le_bytes());
            put(AbiBlock::OFFSET_LAST_PORTS + 2 * at, &last.to_le_bytes());
        }
        bytes
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::V4(address) => write!(f, "{address}"),
            Source::V6(network) => {
                write!(f, "{}/64", Ipv6Addr::from(u128::from(*network) << 64))
            }
        }
    }
}

/// A network of `never`: an address and how many of its first bits name
/// the network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Network {
    address: IpAddr,
    prefix: u8,
}

impl Network {
    /// The network `text` names: `192.168.1.0/24`, `2001:db8::/48`, or
    /// one address alone. Its bits past the prefix must be zero, so that
    /// `192.168.1.5/24` is not taken for what it does not say.
    ///
    /// # Errors
    ///
    /// Why it is not a network.
    pub fn parse(text: &str) -> Result<Network, String> {
        let wrong = || {
            format!(
                "`{}` is not a network: write an address, or one and its prefix, as \
                 192.168.1.0/24 or 2001:db8::/48",
                text.escape_debug()
            )
        };
        let (address, prefix) = match text.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix)),
            None => (text, None),
        };
        let address: IpAddr = address.parse().map_err(|_| wrong())?;
        let bits = if address.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            None => bits,
            Some(prefix) if !prefix.is_empty() && prefix.bytes().all(|b| b.is_ascii_digit()) => {
                match prefix.parse::<u8>() {
                    Ok(prefix) if prefix <= bits => prefix,
                    _ => return Err(wrong()),
                }
            }
            Some(_) => return Err(wrong()),
        };
        let network = Network { address, prefix };
        if network.first_bits(address) != bits_of(address) {
            return Err(format!(
                "`{text}` has bits set past its prefix: its network is {}",
                Network { address: network.masked(), prefix }
            ));
        }
        Ok(network)
    }

    /// `address`'s first `prefix` bits, the others zero, as a number.
    fn first_bits(self, address: IpAddr) -> u128 {
        let bits = if address.is_ipv4() { 32 } else { 128 };
        let value = bits_of(address);
        let kept = bits - u32::from(self.prefix);
        if kept >= bits { 0 } else { value >> kept << kept }
    }

    /// Its address with the bits past the prefix zero.
    fn masked(self) -> IpAddr {
        let bits = self.first_bits(self.address);
        match self.address {
            IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::from(bits as u32)),
            IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::from(bits)),
        }
    }

    /// Whether `address` is in it.
    pub fn holds(self, address: IpAddr) -> bool {
        let address = address.to_canonical();
        address.is_ipv4() == self.address.is_ipv4()
            && self.first_bits(address) == bits_of(self.address)
    }

    /// Whether it and `source` share an address: a block of `source`
    /// would stop one of its.
    pub fn meets(self, source: Source) -> bool {
        match (source, self.address) {
            (Source::V4(address), IpAddr::V4(_)) => self.holds(IpAddr::V4(address)),
            (Source::V6(network), IpAddr::V6(ours)) => {
                let theirs = u128::from(network) << 64;
                // The shorter prefix decides: one network holds the other.
                let shorter = Network { address: IpAddr::V6(ours), prefix: self.prefix.min(64) };
                shorter.first_bits(IpAddr::V6(Ipv6Addr::from(theirs)))
                    == shorter.first_bits(IpAddr::V6(ours))
            }
            _ => false,
        }
    }
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bits = if self.address.is_ipv4() { 32 } else { 128 };
        if self.prefix == bits {
            write!(f, "{}", self.address)
        } else {
            write!(f, "{}/{}", self.address, self.prefix)
        }
    }
}

/// `address` as a number.
fn bits_of(address: IpAddr) -> u128 {
    match address {
        IpAddr::V4(address) => u128::from(u32::from(address)),
        IpAddr::V6(address) => u128::from(address),
    }
}

/// What blocking.toml(5) sets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Settings {
    /// Whether addresses are blocked at all.
    pub on: bool,
    /// How many failed sign-ins block an address.
    pub tries: u32,
    /// The seconds they fail within.
    pub within: u64,
    /// How long the first block of a day lasts, in seconds.
    pub first: u64,
    /// The most a block lasts, in seconds.
    pub longest: u64,
    /// The networks never blocked.
    pub never: Vec<Network>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { on: true, tries: 5, within: 600, first: 600, longest: DAY, never: Vec::new() }
    }
}

/// blocking.toml(5) as it is written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSettings {
    on: Option<bool>,
    tries: Option<u32>,
    within: Option<String>,
    first: Option<String>,
    longest: Option<String>,
    never: Option<Vec<String>>,
}

impl Settings {
    /// The settings `text` sets, each one it leaves out the default's.
    ///
    /// # Errors
    ///
    /// Why it does not read, or a value is out of its range.
    pub fn parse(text: &str) -> Result<Settings, String> {
        let raw: RawSettings = toml::from_str(text).map_err(|e| e.message().to_string())?;
        let mut settings = Settings::default();
        let seconds = |key: &str, text: Option<String>, default: u64| match text {
            None => Ok(default),
            Some(text) => {
                manifest::schedule::duration(&text).map_err(|why| format!("{key}: {why}"))
            }
        };
        settings.on = raw.on.unwrap_or(settings.on);
        settings.tries = raw.tries.unwrap_or(settings.tries);
        if !(TRIES_MIN..=TRIES_MAX).contains(&settings.tries) {
            return Err(format!("tries is {TRIES_MIN} to {TRIES_MAX}"));
        }
        settings.within = seconds("within", raw.within, settings.within)?;
        if !(SHORTEST..=DAY).contains(&settings.within) {
            return Err("within is a minute to a day".into());
        }
        settings.first = seconds("first", raw.first, settings.first)?;
        settings.longest = seconds("longest", raw.longest, settings.longest)?;
        if settings.first < SHORTEST || settings.first > settings.longest {
            return Err("first is a minute or more, and no longer than longest".into());
        }
        if settings.longest > BLOCK_SECONDS_MAX {
            return Err(format!("longest is {} days at most", BLOCK_SECONDS_MAX / DAY));
        }
        let never = raw.never.unwrap_or_default();
        if never.len() > NEVER_MAX {
            return Err(format!("never lists {NEVER_MAX} networks at most"));
        }
        settings.never = never.iter().map(|text| Network::parse(text)).collect::<Result<_, _>>()?;
        Ok(settings)
    }
}

/// `text`, blocking.toml(5)'s, with `never` set to `never`, the rest of it
/// as it was, comments too: the `never` key's lines are replaced, or it
/// is added at the end.
pub(super) fn with_never(text: &str, never: &[Network]) -> String {
    let value = never.iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(", ");
    let line = format!("never = [{value}]");
    let mut out = String::new();
    let mut replaced = false;
    let mut lines = text.lines();
    while let Some(raw) = lines.next() {
        let key = raw.split('=').next().unwrap_or_default().trim();
        if key == "never" && raw.contains('=') && !replaced {
            // An array written over several lines ends at its `]`.
            let mut through = raw.to_string();
            while !value_ends(&through) {
                match lines.next() {
                    Some(next) => through = next.to_string(),
                    None => break,
                }
            }
            out.push_str(&line);
            out.push('\n');
            replaced = true;
        } else {
            out.push_str(raw);
            out.push('\n');
        }
    }
    if !replaced {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Whether `line` ends a TOML array: it has its `]` outside a string and
/// a comment.
fn value_ends(line: &str) -> bool {
    let mut quoted = false;
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            '#' if !quoted => return false,
            ']' if !quoted => return true,
            _ => {}
        }
    }
    false
}

/// What failed, for a block's reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Failed {
    /// A wrong password over SSH.
    SshPassword,
    /// An SSH connection that tried to sign in, was refused each time,
    /// and ended.
    SshRefused,
    /// A wrong sharing password, from a PC signing in to the shares.
    SharePassword,
    /// A wrong help-session code, from a helper signing in (ADR 0540).
    HelpCode,
}

impl Failed {
    /// What many of them are called.
    fn words(self) -> &'static str {
        match self {
            Failed::SshPassword => "wrong passwords over SSH",
            Failed::SshRefused => "refused SSH sign-ins",
            Failed::SharePassword => "wrong sharing passwords",
            Failed::HelpCode => "wrong help-session codes",
        }
    }
}

/// The failed sign-ins counted for one source.
#[derive(Debug, Default)]
struct Failures {
    /// When each failed, in seconds since 1970, and what it was: those
    /// within the window, at most `tries`.
    times: VecDeque<(u64, Failed)>,
    /// The accounts they named, the first few.
    names: Vec<String>,
    /// Whether they named more than those.
    more: bool,
}

impl Failures {
    /// The reason for a block they make: `5 wrong passwords over SSH in
    /// 2 minutes, for root, admin and test`.
    fn why(&self) -> String {
        let count = self.times.len();
        let first = self.times.front().map_or(0, |t| t.0);
        let last = self.times.back().map_or(0, |t| t.0);
        let kind = self.times.front().map(|t| t.1);
        let words = match kind {
            Some(kind) if self.times.iter().all(|t| t.1 == kind) => kind.words(),
            _ => "failed sign-ins",
        };
        let mut why = format!("{count} {words} in {}", span(last.saturating_sub(first)));
        if !self.names.is_empty() {
            let mut names = self.names.clone();
            if self.more {
                names.push("others".into());
            }
            why.push_str(", for ");
            why.push_str(&listed(&names));
        }
        why
    }
}

/// `names` as a sentence lists them: `a`, `a and b`, `a, b and c`.
fn listed(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// `seconds` in words, rounded to whole units: `40 seconds`, `2
/// minutes`, `1 hour`, `3 days`.
pub(crate) fn span(seconds: u64) -> String {
    let (count, unit) = match seconds {
        0..60 => (seconds, "second"),
        60..3600 => (seconds.div_ceil(60), "minute"),
        3600..DAY => (seconds.div_ceil(3600), "hour"),
        _ => (seconds.div_ceil(DAY), "day"),
    };
    // 59.5 minutes round to 60: say an hour.
    match (count, unit) {
        (60, "minute") => "1 hour".into(),
        (24, "hour") => "1 day".into(),
        (1, unit) => format!("1 {unit}"),
        (count, unit) => format!("{count} {unit}s"),
    }
}

/// A block of a source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Block {
    /// When it ends, in seconds since 1970.
    pub until: u64,
    /// When it began.
    pub started: u64,
    /// How many blocks of the source came one after another, each within
    /// a day of the end of the one before, this one counted.
    pub times: u32,
    /// Why, as the audit log said it.
    pub why: String,
}

/// The counts and the blocks.
#[derive(Debug, Default)]
pub(super) struct Blocking {
    /// The settings in force.
    pub settings: Settings,
    /// When the settings file was last read, as its time of change and
    /// length, so that it is read again only once it changes.
    read_as: Option<(SystemTime, u64)>,
    failures: HashMap<Source, Failures>,
    blocks: BTreeMap<Source, Block>,
}

impl Blocking {
    /// With `settings` and the blocks `kept`.
    pub fn new(settings: Settings, kept: BTreeMap<Source, Block>) -> Blocking {
        Blocking { settings, read_as: None, failures: HashMap::new(), blocks: kept }
    }

    /// Counts a failed sign-in from `from`, of `failed`, naming the
    /// account `name` if it named one, at `now`: the block it makes, if
    /// it is the one that blocks the source. A source `spared` holds, an
    /// administrator's address while their session lasts, is never
    /// counted, nor one `never` meets, nor one blocked already.
    pub fn failed(
        &mut self,
        from: IpAddr,
        failed: Failed,
        name: Option<&str>,
        spared: &[IpAddr],
        now: u64,
    ) -> Option<(Source, Block)> {
        let source = Source::of(from)?;
        if !self.settings.on || self.spares(source, spared) || self.blocked(from, now).is_some() {
            return None;
        }
        if !self.failures.contains_key(&source) && self.failures.len() >= KEPT_MAX {
            let oldest = self
                .failures
                .iter()
                .min_by_key(|(_, f)| f.times.back().map_or(0, |t| t.0))
                .map(|(source, _)| *source);
            if let Some(oldest) = oldest {
                self.failures.remove(&oldest);
            }
        }
        let within = self.settings.within;
        let failures = self.failures.entry(source).or_default();
        while failures.times.front().is_some_and(|t| now.saturating_sub(t.0) >= within) {
            failures.times.pop_front();
        }
        failures.times.push_back((now, failed));
        while failures.times.len() > self.settings.tries as usize {
            failures.times.pop_front();
        }
        if let Some(name) = name.filter(|name| !name.is_empty()) {
            if failures.names.len() < NAMES_SHOWN {
                if !failures.names.iter().any(|n| n == name) {
                    failures.names.push(name.to_string());
                }
            } else if !failures.names.iter().any(|n| n == name) {
                failures.more = true;
            }
        }
        if failures.times.len() < self.settings.tries as usize {
            return None;
        }
        let why = failures.why();
        self.failures.remove(&source);
        let times = match self.blocks.get(&source) {
            Some(before) if now < before.until + DAY => before.times.saturating_add(1),
            _ => 1,
        };
        let doubled = self.settings.first.saturating_mul(1u64 << (times - 1).min(32));
        let block =
            Block { until: now + doubled.min(self.settings.longest), started: now, times, why };
        self.keep(source, block.clone());
        Some((source, block))
    }

    /// Keeps `block` of `source`, the oldest block going if there are
    /// [`KEPT_MAX`].
    fn keep(&mut self, source: Source, block: Block) {
        if !self.blocks.contains_key(&source) && self.blocks.len() >= KEPT_MAX {
            let oldest = self.blocks.iter().min_by_key(|(_, b)| b.started).map(|(s, _)| *s);
            if let Some(oldest) = oldest {
                self.blocks.remove(&oldest);
            }
        }
        self.blocks.insert(source, block);
    }

    /// A right password from `from`: its count is cleared.
    pub fn succeeded(&mut self, from: IpAddr) {
        if let Some(source) = Source::of(from) {
            self.failures.remove(&source);
        }
    }

    /// The seconds `from` is blocked for still, if it is.
    pub fn blocked(&self, from: IpAddr, now: u64) -> Option<u64> {
        if !self.settings.on {
            return None;
        }
        let block = self.blocks.get(&Source::of(from)?)?;
        (now < block.until).then(|| block.until - now)
    }

    /// Whether `source` is never blocked: `never` meets it, or it holds
    /// an address of `spared`.
    fn spares(&self, source: Source, spared: &[IpAddr]) -> bool {
        self.settings.never.iter().any(|network| network.meets(source))
            || spared.iter().any(|&address| source.holds(address))
    }

    /// Lifts the block of `source`, and forgets the blocks before it, so
    /// that the next is a first: whether it was blocked.
    pub fn unblock(&mut self, source: Source, now: u64) -> bool {
        self.failures.remove(&source);
        self.blocks.remove(&source).is_some_and(|block| now < block.until)
    }

    /// The blocks in force at `now`, in order, for `blocked`: those
    /// `never` or `spared` spare are left out, as the kernel leaves them.
    pub fn entries(&self, spared: &[IpAddr], now: u64) -> Vec<BlockedEntry> {
        self.in_force(spared, now)
            .map(|(source, block)| BlockedEntry {
                address: source.to_string(),
                until: block.until as i64,
                times: block.times,
                why: block.why.clone(),
            })
            .collect()
    }

    /// The blocks in force at `now`, those spared left out.
    fn in_force<'a>(
        &'a self,
        spared: &'a [IpAddr],
        now: u64,
    ) -> impl Iterator<Item = (Source, &'a Block)> + 'a {
        self.blocks
            .iter()
            .filter(move |(source, block)| {
                self.settings.on && now < block.until && !self.spares(**source, spared)
            })
            .map(|(source, block)| (*source, block))
    }

    /// Forgets the blocks that ended a day ago, and the counts whose
    /// failures are all older than the window.
    pub fn tidy(&mut self, now: u64) {
        self.blocks.retain(|_, block| now < block.until + DAY);
        let within = self.settings.within;
        self.failures.retain(|_, failures| {
            failures.times.back().is_some_and(|t| now.saturating_sub(t.0) < within)
        });
    }

    /// The kernel's list at `now`: Block structures, in order, each
    /// closing `ports`. At most [`BLOCKS_MAX`]: past it, those that end
    /// last are kept.
    pub fn kernel_list(&self, spared: &[IpAddr], ports: &[(u16, u16)], now: u64) -> Vec<u8> {
        let mut blocks: Vec<(Source, &Block)> = self.in_force(spared, now).collect();
        if blocks.len() > BLOCKS_MAX as usize {
            blocks.sort_by_key(|(_, block)| std::cmp::Reverse(block.until));
            blocks.truncate(BLOCKS_MAX as usize);
            blocks.sort_by_key(|(source, _)| *source);
        }
        let mut bytes = Vec::with_capacity(blocks.len() * AbiBlock::SIZE);
        for (source, block) in blocks {
            // A clock set back makes no block longer than the longest.
            let seconds = (block.until - now).clamp(1, self.settings.longest);
            bytes.extend_from_slice(&source.encode(seconds, ports));
        }
        bytes
    }

    /// The text of [`STATE`].
    pub fn kept_text(&self) -> String {
        let mut text = String::new();
        for (source, block) in &self.blocks {
            let Block { until, started, times, why } = block;
            text.push_str(&format!("{source} {until} {started} {times} {why}\n"));
        }
        text
    }

    /// The settings file's time of change and length, if it is there.
    fn settings_file() -> Option<(SystemTime, u64)> {
        let meta = fs::metadata(SETTINGS).ok()?;
        Some((meta.modified().ok()?, meta.len()))
    }

    /// Reads [`SETTINGS`] again if it changed since it was last read: the
    /// settings that changed, in words, for the audit log, or why it does
    /// not read, in which case the settings stay as they were.
    pub fn read_settings(&mut self) -> Option<Result<String, String>> {
        let now = Blocking::settings_file();
        if now == self.read_as {
            return None;
        }
        self.read_as = now;
        let read = match now {
            None => Ok(Settings::default()),
            Some(_) => fs::read_to_string(SETTINGS)
                .map_err(|err| err.to_string())
                .and_then(|text| Settings::parse(&text)),
        };
        Some(match read {
            Ok(settings) if settings == self.settings => Ok(String::new()),
            Ok(settings) => {
                let said = said(&settings);
                self.settings = settings;
                Ok(said)
            }
            Err(why) => Err(format!("{SETTINGS}: {why}; the settings stay as they were")),
        })
    }
}

/// `settings` in words, for the audit log.
fn said(settings: &Settings) -> String {
    if !settings.on {
        return "off".into();
    }
    let never = settings.never.iter().map(ToString::to_string).collect::<Vec<_>>();
    let never =
        if never.is_empty() { String::new() } else { format!(", never {}", never.join(" ")) };
    format!(
        "on: {} tries within {} block for {}, doubling to {}{never}",
        settings.tries,
        span(settings.within),
        span(settings.first),
        span(settings.longest)
    )
}

/// The blocks `text`, [`STATE`]'s, keeps, without those that ended a day
/// before `now`; a line that does not read is skipped.
pub(super) fn parse_kept(text: &str, now: u64) -> BTreeMap<Source, Block> {
    let mut blocks = BTreeMap::new();
    for line in text.lines() {
        let words: Vec<&str> = line.splitn(5, ' ').collect();
        let [source, until, started, times, why] = words[..] else { continue };
        let (Some(source), Ok(until), Ok(started), Ok(times)) = (
            Source::parse(source),
            until.parse::<u64>(),
            started.parse::<u64>(),
            times.parse::<u32>(),
        ) else {
            continue;
        };
        if times == 0 || started > until || now >= until.saturating_add(DAY) {
            continue;
        }
        blocks.insert(source, Block { until, started, times, why: why.to_string() });
    }
    blocks
}

/// The ports the sign-in services listen on, as their files and the
/// overrides over them say: at most [`BLOCK_PORTS`] ranges, those close
/// together joined if there are more. Port 22 when none says.
pub(super) fn sign_in_ports() -> Vec<(u16, u16)> {
    let mut ports = Vec::new();
    for name in SIGN_IN_SERVICES {
        let Ok(text) = servicefiles::text(name) else { continue };
        let over_path =
            Path::new(servicefiles::DIR).join(service::OVERRIDES).join(format!("{name}.toml"));
        let over = fs::read_to_string(over_path)
            .ok()
            .and_then(|text| service::Override::from_toml(&text).ok());
        if let Ok(service) = service::from_toml_over(&text, over.as_ref()) {
            ports.extend(service.permissions.network.listen.iter().map(|p| (p.first, p.last)));
        }
    }
    joined(ports)
}

/// `ports` sorted, those that overlap or touch joined, then the two
/// closest together joined until there are [`BLOCK_PORTS`]; port 22 if
/// there are none.
fn joined(mut ports: Vec<(u16, u16)>) -> Vec<(u16, u16)> {
    if ports.is_empty() {
        return vec![(SSH_PORT, SSH_PORT)];
    }
    ports.sort_unstable();
    let mut out: Vec<(u16, u16)> = Vec::new();
    for (first, last) in ports {
        match out.last_mut() {
            Some(prev) if u32::from(first) <= u32::from(prev.1) + 1 => prev.1 = prev.1.max(last),
            _ => out.push((first, last)),
        }
    }
    while out.len() > BLOCK_PORTS as usize {
        let at = (0..out.len() - 1).min_by_key(|&i| out[i + 1].0 - out[i].1).unwrap_or(0);
        out[at].1 = out[at + 1].1;
        out.remove(at + 1);
    }
    out
}

/// Gives the kernel `list`, [`Blocking::kernel_list`]'s.
///
/// # Errors
///
/// Why the kernel refused it.
pub(super) fn give(list: &[u8]) -> Result<(), String> {
    abi::set_blocked(&SYS, SLOT_FIREWALL, list)
        .map_err(|err| format!("the firewall did not take the blocks: {err:?}"))
}

/// Seconds since 1970, now.
pub(super) fn unix_now() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The notice an administrator's next sign-in shows, `count` addresses
/// blocked since their last.
pub(super) fn notice(count: u32) -> String {
    let addresses =
        if count == 1 { "1 address was".to_string() } else { format!("{count} addresses were") };
    format!(
        "{NOTICE_START}{addresses} blocked for guessing passwords: `elevate firewall blocked` lists \
         those still blocked."
    )
}

/// How [`notice`]'s text begins.
const NOTICE_START: &str = "Since you last signed in, ";

/// How many addresses `text`, a notice, says were blocked, if it is
/// [`notice`]'s.
pub(super) fn notice_count(text: &str) -> Option<u32> {
    let rest = text.strip_prefix(NOTICE_START)?;
    if !rest.contains(" blocked for guessing passwords") {
        return None;
    }
    rest.split(' ').next()?.parse().ok()
}

impl Authd {
    /// Reads the blocks kept and the settings, and gives the kernel the
    /// blocks in force: as authd starts.
    pub(super) fn start_blocking(&mut self) {
        let now = unix_now();
        let kept = parse_kept(&read_text(STATE).unwrap_or_default(), now);
        self.blocking = Blocking::new(Settings::default(), kept);
        self.read_blocking_settings();
        if !self.blocking.blocks.is_empty() {
            eprintln!(
                "authd: {} addresses are blocked still",
                self.blocking.entries(&[], now).len()
            );
        }
        self.blocks_changed(&[], now);
    }

    /// Reads blocking.toml(5) again if it changed: whether its settings
    /// did.
    fn read_blocking_settings(&mut self) -> bool {
        match self.blocking.read_settings() {
            Some(Ok(said)) if !said.is_empty() => {
                audit!("blocking addresses that guess passwords: {said}");
                true
            }
            Some(Err(why)) => {
                eprintln!("authd: {why}");
                false
            }
            _ => false,
        }
    }

    /// Before each request: if blocking.toml(5) changed, the kernel is
    /// given the blocks its settings leave.
    pub(super) fn check_blocking_settings(&mut self, files: &Files) {
        if self.read_blocking_settings() {
            let spared = self.spared(files);
            self.blocks_changed(&spared, unix_now());
        }
    }

    /// The addresses administrators are signed in from over SSH, while
    /// their sessions last: never blocked.
    fn spared(&self, files: &Files) -> Vec<IpAddr> {
        self.sessions
            .values()
            .filter(|session| session.remote)
            .filter(|session| {
                files.user(&session.name).is_some_and(|u| accounts::is_admin(u, &files.groups))
            })
            .filter_map(|session| session.from.parse().ok())
            .collect()
    }

    /// Keeps the blocks, and gives the kernel those in force.
    fn blocks_changed(&mut self, spared: &[IpAddr], now: u64) {
        self.blocking.tidy(now);
        if let Err(why) = replace(STATE, &self.blocking.kept_text(), 0o600) {
            eprintln!("authd: {why}");
        }
        let list = self.blocking.kernel_list(spared, &sign_in_ports(), now);
        if let Err(why) = give(&list) {
            eprintln!("authd: {why}");
        }
    }

    /// Why a password from `from` is refused unread, if the address is
    /// blocked: it is not checked, so that a block ends the guessing on
    /// a connection open before it too.
    pub(super) fn blocked_why(&self, from: &str) -> Option<String> {
        let left = self.blocking.blocked(from.parse().ok()?, unix_now())?;
        Some(format!("{} for {} more", request::BLOCKED, span(left)))
    }

    /// A right password from `from`: its address's count is cleared.
    pub(super) fn sign_in_right(&mut self, from: &str) {
        if let Ok(address) = from.parse() {
            self.blocking.succeeded(address);
        }
    }

    /// Counts a failed sign-in from `from`, of `failed`, naming `name` if
    /// it named an account; once it blocks the address, the block goes to
    /// the audit log and the kernel, and each administrator's next
    /// sign-in is told.
    pub(super) fn sign_in_failed(
        &mut self,
        files: &Files,
        from: &str,
        failed: Failed,
        name: Option<&str>,
    ) {
        let Ok(address) = from.parse::<IpAddr>() else { return };
        self.check_blocking_settings(files);
        let spared = self.spared(files);
        let now = unix_now();
        let name = name.map(shown);
        let Some((source, block)) =
            self.blocking.failed(address, failed, name.as_deref(), &spared, now)
        else {
            return;
        };
        audit!("blocked {source} for {}: {}", span(block.until - block.started), block.why);
        self.blocks_changed(&spared, now);
        let mut notices = Notices::read();
        let admins =
            files.users.entries().filter(|u| u.is_person() && accounts::is_admin(u, &files.groups));
        for admin in admins {
            notices.count_block(&admin.name);
        }
        if let Err(why) = notices.write() {
            eprintln!("authd: {why}");
        }
    }

    /// An SSH connection refused each time it tried, ended: sshd's to
    /// say.
    pub(super) fn ssh_failed(
        &mut self,
        files: &Files,
        connection: &str,
        name: &str,
        peer: Peer,
    ) -> Answer {
        if !peer.is_root() {
            return Answer::Refused(
                "only sshd, which runs as root, says how a connection ended".into(),
            );
        }
        let Some(from) = super::ssh_from(connection) else {
            return Answer::Refused("a connection is named as SSH_CONNECTION names it".into());
        };
        let name = (!name.is_empty()).then_some(name);
        self.sign_in_failed(files, from, Failed::SshRefused, name);
        Answer::Done(String::new())
    }

    /// The blocks in force, for `firewall blocked`.
    pub(super) fn blocked_list(&mut self, files: &Files, peer: Peer) -> Answer {
        if !peer.is_root() {
            return Answer::Refused(
                "only an administrator, inside elevate, lists the blocked addresses".into(),
            );
        }
        let spared = self.spared(files);
        Answer::Done(BlockedEntry::list(&self.blocking.entries(&spared, unix_now())))
    }

    /// Lifts the block of `address`, for `firewall unblock`.
    pub(super) fn unblock(&mut self, files: &Files, address: &str, peer: Peer) -> Answer {
        if !peer.is_root() {
            return Answer::Refused("only an administrator, inside elevate, lifts a block".into());
        }
        let Some(source) = Source::parse(address) else {
            return Answer::Refused(format!(
                "`{}` is not an IPv4 address or an IPv6 network of 64 bits, as 203.0.113.9 or \
                 2001:db8:4:7::/64",
                shown(address)
            ));
        };
        let now = unix_now();
        if !self.blocking.unblock(source, now) {
            return Answer::Refused(format!("{source} is not blocked"));
        }
        audit!("unblocked {source}, for process {} (user {})", peer.process, peer.user);
        let spared = self.spared(files);
        self.blocks_changed(&spared, now);
        Answer::Done(source.to_string())
    }

    /// Adds `network` to blocking.toml(5)'s `never`, or takes it out
    /// when `remove`, for `firewall never-block`.
    pub(super) fn never_block(
        &mut self,
        files: &Files,
        network: &str,
        remove: bool,
        peer: Peer,
    ) -> Answer {
        if !peer.is_root() {
            return Answer::Refused(
                "only an administrator, inside elevate, changes which networks are never blocked"
                    .into(),
            );
        }
        let network = match Network::parse(network) {
            Ok(network) => network,
            Err(why) => return Answer::Refused(why),
        };
        let text = match read_text(SETTINGS) {
            Ok(text) => text,
            Err(err) => return Answer::Refused(format!("cannot read {SETTINGS}: {err}")),
        };
        let mut never = match Settings::parse(&text) {
            Ok(settings) => settings.never,
            Err(why) => {
                return Answer::Refused(format!("{SETTINGS} does not read; fix it first: {why}"));
            }
        };
        let had = never.contains(&network);
        match (remove, had) {
            (false, true) => return Answer::Refused(format!("{network} is never blocked already")),
            (true, false) => return Answer::Refused(format!("{network} is not in never")),
            (false, false) if never.len() >= NEVER_MAX => {
                return Answer::Refused(format!("never lists {NEVER_MAX} networks at most"));
            }
            (false, false) => never.push(network),
            (true, true) => never.retain(|n| *n != network),
        }
        if let Err(why) = replace(SETTINGS, &with_never(&text, &never), 0o644) {
            return Answer::Refused(why);
        }
        // Read again even if the file's time and length look the same.
        self.blocking.read_as = None;
        let said = if remove { "may be blocked again" } else { "is never blocked" };
        audit!("{network} {said}, for process {} (user {})", peer.process, peer.user);
        self.check_blocking_settings(files);
        Answer::Done(network.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALICE: &str = "alice";

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn blocking() -> Blocking {
        Blocking::new(Settings::default(), BTreeMap::new())
    }

    /// Fails `count` times from `from`, a second apart from `start`: the
    /// block the last made, if one did.
    fn fail(
        blocking: &mut Blocking,
        from: &str,
        count: u64,
        start: u64,
    ) -> Option<(Source, Block)> {
        let mut made = None;
        for n in 0..count {
            made = blocking.failed(ip(from), Failed::SshPassword, Some(ALICE), &[], start + n);
        }
        made
    }

    #[test]
    fn five_failures_in_ten_minutes_block_an_address_for_ten_minutes() {
        let mut blocking = blocking();
        assert!(fail(&mut blocking, "203.0.113.9", 4, 1000).is_none());
        let (source, block) = fail(&mut blocking, "203.0.113.9", 1, 1004).unwrap();
        assert_eq!(source.to_string(), "203.0.113.9");
        assert_eq!((block.started, block.until, block.times), (1004, 1604, 1));
        assert_eq!(block.why, "5 wrong passwords over SSH in 4 seconds, for alice");
        assert_eq!(blocking.blocked(ip("203.0.113.9"), 1004), Some(600));
        assert_eq!(blocking.blocked(ip("203.0.113.9"), 1604), None);
        assert_eq!(blocking.blocked(ip("203.0.113.10"), 1004), None);
    }

    #[test]
    fn failures_spread_wider_than_the_window_never_block() {
        let mut blocking = blocking();
        for n in 0..20 {
            // One every three minutes: never five within ten.
            assert!(fail(&mut blocking, "198.51.100.4", 1, n * 180).is_none());
        }
        assert!(blocking.blocked(ip("198.51.100.4"), 20 * 180).is_none());
    }

    #[test]
    fn a_right_password_clears_the_count() {
        let mut blocking = blocking();
        fail(&mut blocking, "203.0.113.9", 4, 0);
        blocking.succeeded(ip("203.0.113.9"));
        assert!(fail(&mut blocking, "203.0.113.9", 4, 10).is_none());
    }

    #[test]
    fn each_block_within_a_day_of_the_last_doubles_up_to_the_longest() {
        let mut blocking = blocking();
        let mut now = 0;
        let mut lasted = Vec::new();
        for _ in 0..10 {
            let (_, block) = fail(&mut blocking, "203.0.113.9", 5, now).unwrap();
            lasted.push(block.until - block.started);
            now = block.until;
        }
        assert_eq!(lasted[..4], [600, 1200, 2400, 4800]);
        assert_eq!(*lasted.last().unwrap(), DAY);
        // A day after the last ended, the next is a first again.
        let (_, block) = fail(&mut blocking, "203.0.113.9", 5, now + DAY).unwrap();
        assert_eq!((block.times, block.until - block.started), (1, 600));
    }

    #[test]
    fn an_ipv6_network_of_64_bits_is_one_source() {
        let mut blocking = blocking();
        for host in 1..=5 {
            let from = format!("2001:db8:4:7::{host:x}");
            blocking.failed(ip(&from), Failed::SshPassword, None, &[], host);
        }
        assert_eq!(blocking.blocked(ip("2001:db8:4:7:ffff::1"), 5), Some(600));
        assert_eq!(blocking.blocked(ip("2001:db8:4:8::1"), 5), None);
        let entries = blocking.entries(&[], 5);
        assert_eq!(entries[0].address, "2001:db8:4:7::/64");
        assert_eq!(entries[0].why, "5 wrong passwords over SSH in 4 seconds");
    }

    #[test]
    fn an_ipv4_address_written_as_ipv6_is_the_ipv4_one() {
        let mut blocking = blocking();
        fail(&mut blocking, "::ffff:203.0.113.9", 5, 0);
        assert_eq!(blocking.blocked(ip("203.0.113.9"), 1), Some(603));
    }

    #[test]
    fn loopback_never_counts() {
        let mut blocking = blocking();
        assert!(fail(&mut blocking, "127.0.0.1", 50, 0).is_none());
        assert!(fail(&mut blocking, "::1", 50, 0).is_none());
    }

    #[test]
    fn never_and_an_administrators_address_are_spared() {
        let mut blocking = blocking();
        blocking.settings.never = vec![Network::parse("192.168.1.0/24").unwrap()];
        assert!(fail(&mut blocking, "192.168.1.77", 50, 0).is_none());
        // An administrator signed in from 198.51.100.4: their typos do
        // not lock them out.
        let admin = [ip("198.51.100.4")];
        for n in 0..50 {
            let made = blocking.failed(ip("198.51.100.4"), Failed::SshPassword, None, &admin, n);
            assert!(made.is_none());
        }
        // An IPv6 network holding one of `never`'s is spared whole.
        blocking.settings.never = vec![Network::parse("2001:db8:4:7::5").unwrap()];
        for n in 0..5 {
            blocking.failed(ip("2001:db8:4:7::99"), Failed::SshPassword, None, &[], n);
        }
        assert!(blocking.blocked(ip("2001:db8:4:7::99"), 5).is_none());
    }

    #[test]
    fn a_block_made_before_never_named_it_is_not_given_to_the_kernel() {
        let mut blocking = blocking();
        fail(&mut blocking, "192.168.1.77", 5, 0);
        assert_eq!(blocking.kernel_list(&[], &[(22, 22)], 10).len(), AbiBlock::SIZE);
        blocking.settings.never = vec![Network::parse("192.168.1.0/24").unwrap()];
        assert!(blocking.kernel_list(&[], &[(22, 22)], 10).is_empty());
        assert!(blocking.entries(&[], 10).is_empty());
    }

    #[test]
    fn the_reason_names_what_failed_and_the_first_few_accounts() {
        let mut blocking = blocking();
        let from = ip("203.0.113.9");
        let mut made = None;
        for (n, name) in ["root", "admin", "test", "root", "oracle"].iter().enumerate() {
            made = blocking.failed(from, Failed::SshPassword, Some(name), &[], n as u64 * 30);
        }
        let (_, block) = made.unwrap();
        assert_eq!(
            block.why,
            "5 wrong passwords over SSH in 2 minutes, for root, admin, test and others"
        );
        let other = ip("203.0.113.10");
        let failed = [Failed::SshRefused, Failed::SharePassword, Failed::SshRefused];
        let mut made = None;
        for n in 0..5 {
            made = blocking.failed(other, failed[n % 3], None, &[], n as u64 * 60);
        }
        assert_eq!(made.unwrap().1.why, "5 failed sign-ins in 4 minutes");
    }

    #[test]
    fn off_counts_nothing_and_lifts_every_block() {
        let mut blocking = blocking();
        fail(&mut blocking, "203.0.113.9", 5, 0);
        blocking.settings.on = false;
        assert!(blocking.blocked(ip("203.0.113.9"), 1).is_none());
        assert!(blocking.kernel_list(&[], &[(22, 22)], 1).is_empty());
        assert!(fail(&mut blocking, "198.51.100.4", 50, 0).is_none());
    }

    #[test]
    fn unblock_lifts_a_block_and_forgets_the_day() {
        let mut blocking = blocking();
        fail(&mut blocking, "203.0.113.9", 5, 0);
        let source = Source::parse("203.0.113.9").unwrap();
        assert!(blocking.unblock(source, 1));
        assert!(blocking.blocked(ip("203.0.113.9"), 1).is_none());
        let (_, block) = fail(&mut blocking, "203.0.113.9", 5, 10).unwrap();
        assert_eq!(block.times, 1);
        assert!(!blocking.unblock(Source::parse("198.51.100.4").unwrap(), 1));
    }

    #[test]
    fn the_kernels_list_is_in_order_and_says_the_seconds_left() {
        let mut blocking = blocking();
        for from in ["2001:db8::1", "203.0.113.9", "10.0.0.1"] {
            fail(&mut blocking, from, 5, 100);
        }
        let list = blocking.kernel_list(&[], &[(22, 22), (445, 445)], 200);
        assert_eq!(list.len(), 3 * AbiBlock::SIZE);
        let block = |n: usize| &list[n * AbiBlock::SIZE..(n + 1) * AbiBlock::SIZE];
        let u32_at =
            |bytes: &[u8], at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        assert_eq!(u32_at(block(0), AbiBlock::OFFSET_FAMILY), 4);
        assert_eq!(block(0)[AbiBlock::OFFSET_FROM..AbiBlock::OFFSET_FROM + 4], [10, 0, 0, 1]);
        assert_eq!(block(1)[AbiBlock::OFFSET_FROM..AbiBlock::OFFSET_FROM + 4], [203, 0, 113, 9]);
        assert_eq!(u32_at(block(2), AbiBlock::OFFSET_FAMILY), 6);
        assert_eq!(block(2)[AbiBlock::OFFSET_FROM + 8..AbiBlock::OFFSET_FROM + 16], [0; 8]);
        let seconds = u64::from_le_bytes(
            block(0)[AbiBlock::OFFSET_SECONDS..AbiBlock::OFFSET_SECONDS + 8].try_into().unwrap(),
        );
        assert_eq!(seconds, 504);
        assert_eq!(u32_at(block(0), AbiBlock::OFFSET_PORT_COUNT), 2);
        let port = |at: usize| u16::from_le_bytes(block(0)[at..at + 2].try_into().unwrap());
        assert_eq!(port(AbiBlock::OFFSET_FIRST_PORTS + 2), 445);
        assert_eq!(port(AbiBlock::OFFSET_LAST_PORTS + 2), 445);
        assert_eq!(port(AbiBlock::OFFSET_FIRST_PORTS + 4), 0);
    }

    #[test]
    fn blocks_are_kept_and_read_back_and_those_a_day_old_dropped() {
        let mut blocking = blocking();
        fail(&mut blocking, "203.0.113.9", 5, 1000);
        fail(&mut blocking, "2001:db8:4:7::1", 5, 2000);
        let text = blocking.kept_text();
        assert_eq!(
            text.lines().next(),
            Some("203.0.113.9 1604 1004 1 5 wrong passwords over SSH in 4 seconds, for alice")
        );
        let kept = parse_kept(&text, 3000);
        assert_eq!(kept, blocking.blocks);
        // A day after the first ended, it goes; the second stays.
        let kept = parse_kept(&text, 1604 + DAY);
        assert_eq!(kept.len(), 1);
        assert!(parse_kept("nonsense\n203.0.113.9 x 1 1 why\n", 0).is_empty());
    }

    #[test]
    fn settings_read_with_defaults_and_their_ranges() {
        let settings = Settings::parse("").unwrap();
        assert_eq!(settings, Settings::default());
        let text = "on = true\ntries = 3\nwithin = \"5m\"\nfirst = \"1h\"\nlongest = \"7d\"\n\
                    never = [\"192.168.1.0/24\", \"2001:db8::/48\", \"198.51.100.4\"]\n";
        let settings = Settings::parse(text).unwrap();
        assert_eq!((settings.tries, settings.within, settings.first), (3, 300, 3600));
        assert_eq!(settings.longest, 7 * DAY);
        assert_eq!(settings.never.len(), 3);
        for wrong in [
            "tries = 1",
            "tries = 101",
            "within = \"30s\"",
            "within = \"2d\"",
            "first = \"2d\"",
            "longest = \"31d\"",
            "never = [\"192.168.1.5/24\"]",
            "never = [\"example.org\"]",
            "colour = \"red\"",
        ] {
            assert!(Settings::parse(wrong).is_err(), "{wrong}");
        }
        let why = Settings::parse("never = [\"192.168.1.5/24\"]").unwrap_err();
        assert!(why.contains("its network is 192.168.1.0/24"), "{why}");
    }

    #[test]
    fn the_file_the_system_ships_sets_the_defaults() {
        // It says them out, so that a person sees what to change, and so
        // it must say what authd does without it.
        let shipped = include_str!("../../../../defaults/blocking.toml");
        assert_eq!(Settings::parse(shipped).unwrap(), Settings::default());
    }

    #[test]
    fn networks_hold_their_addresses_and_meet_sources() {
        let lan = Network::parse("192.168.1.0/24").unwrap();
        assert!(lan.holds(ip("192.168.1.200")));
        assert!(lan.holds(ip("::ffff:192.168.1.200")));
        assert!(!lan.holds(ip("192.168.2.1")));
        assert!(!lan.holds(ip("2001:db8::1")));
        let wide = Network::parse("2001:db8::/32").unwrap();
        let one = Network::parse("2001:db8:4:7::5/128").unwrap();
        let source = Source::parse("2001:db8:4:7::/64").unwrap();
        assert!(wide.meets(source) && one.meets(source));
        assert!(!Network::parse("2001:db8:4:8::/64").unwrap().meets(source));
        assert!(Network::parse("0.0.0.0/0").unwrap().holds(ip("8.8.8.8")));
        assert_eq!(one.to_string(), "2001:db8:4:7::5");
        assert_eq!(lan.to_string(), "192.168.1.0/24");
    }

    #[test]
    fn never_is_written_in_place_keeping_the_rest() {
        let text =
            "# Blocking.\non = true\nnever = [\n  \"10.0.0.0/8\",  # the office\n]\ntries = 5\n";
        let never = [Network::parse("192.168.1.0/24").unwrap()];
        let written = with_never(text, &never);
        assert_eq!(written, "# Blocking.\non = true\nnever = [\"192.168.1.0/24\"]\ntries = 5\n");
        assert_eq!(with_never("on = true\n", &[]), "on = true\nnever = []\n");
        assert_eq!(Settings::parse(&written).unwrap().never, never);
    }

    #[test]
    fn ports_are_joined_to_four_ranges() {
        assert_eq!(joined(Vec::new()), [(22, 22)]);
        assert_eq!(joined(vec![(445, 445), (22, 22), (23, 30)]), [(22, 30), (445, 445)]);
        // Past four, the two closest together are joined: the fewest ports
        // closed that are not sign-in ports.
        let many = vec![(22, 22), (139, 139), (445, 445), (9090, 9090), (8443, 8443)];
        assert_eq!(joined(many), [(22, 139), (445, 445), (8443, 8443), (9090, 9090)]);
    }

    #[test]
    fn spans_read_in_whole_units() {
        assert_eq!(span(0), "0 seconds");
        assert_eq!(span(1), "1 second");
        assert_eq!(span(119), "2 minutes");
        assert_eq!(span(3599), "1 hour");
        assert_eq!(span(DAY), "1 day");
        assert_eq!(span(3 * DAY), "3 days");
    }

    #[test]
    fn the_notice_counts_up() {
        assert_eq!(notice_count(&notice(1)), Some(1));
        assert_eq!(notice_count(&notice(12)), Some(12));
        assert!(notice(1).contains("1 address was blocked"));
        assert!(notice(3).contains("3 addresses were blocked"));
        assert_eq!(notice_count("Since you last signed in, 4 things happened."), None);
    }

    #[test]
    fn many_sources_never_hold_more_than_the_most_kept() {
        let mut blocking = blocking();
        for n in 0..(KEPT_MAX as u32 + 100) {
            let from = IpAddr::V4(Ipv4Addr::from(0x0a00_0000 + n));
            blocking.failed(from, Failed::SshRefused, None, &[], u64::from(n));
        }
        assert_eq!(blocking.failures.len(), KEPT_MAX);
    }
}
