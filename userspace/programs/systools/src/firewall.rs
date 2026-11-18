// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! firewall(8): the firewall's rules, and what listens (ADR 0334).
//!
//! The rules live in `/system/settings/firewall`, a rule a line, in order:
//! `allow tcp 8080`, `deny udp 5353 on net1`, `allow tcp 22 from
//! 192.168.1.0/24`, `allow program /users/alice/bin/server`. The first
//! rule that matches what another machine starts decides; when none
//! does, a ping is answered, and a connection or datagram reaches only a
//! socket a service listens on, with the inbound right Spark gives the
//! ports its file names (radix-firewall(2)). `firewall load` gives the
//! kernel the file's rules, which the `firewall` service does at boot,
//! finding the files a rule's `program` names and the interfaces its
//! `on` names; `allow`, `deny` and `remove` change the file and load it.
//!
//! `firewall status` lists every socket that listens, its program and
//! process, and whether other machines reach it: a program a person runs
//! listens for this machine alone, and status says the one command that
//! allows it from other machines, `firewall allow program PATH`, which is
//! remembered for that program. `firewall allow process N` does the same
//! for the program process N runs.
//!
//! Reading needs the firewall capability's read right, and changing the
//! rules its control right: elevate(8) gives an administrator's command
//! the capability, and the `firewall` service holds it (admin.firewall).
//!
//! authd(8) blocks the addresses that guess passwords for a while, and
//! gives the kernel's firewall the list (ADR 0479). `firewall blocked`
//! shows it, as a table or, with `--json`, as the settings of blocking.toml(5)
//! and the blocks (`report::blocked`) that the web console reads;
//! `firewall unblock ADDRESS` lifts one block, and `firewall
//! never-block [remove] NETWORK` keeps a network out of the list for good,
//! or lets it in again, in blocking.toml(5). These three ask authd, which
//! alone keeps the list and the file, so they need no capability of their
//! own: authd answers an administrator's command inside elevate(8).
//!
//! The web console reads the rules and what listens as JSON, and asks
//! which rule decides a connection, and what the kernel dropped
//! (`report`): `firewall list --json`, `status --json`, `test` and
//! `dropped`. It changes them as a person does, and may try a change
//! (ADR 0472, 0518): `--try SECONDS` after a change's words has Spark keep
//! the file and put it back unless `firewall keep` keeps the change in
//! time ([`crate::trial`]). Spark's undo restarts the `firewall` service,
//! which loads the rules of the file put back. `remove` and `replace` name
//! a rule by its words, which the page has, as well as by its number.

pub(crate) mod block;
pub(crate) mod report;

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::net::IpAddr;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use accounts::request::{self, Answer, BlockedEntry, Request};
use radix_abi::Status;
use radix_abi::errors::Kernel;
use radix_abi::firewall::{
    self as abi, Action as AbiAction, Listener as AbiListener, ListenerFlags, Match as AbiMatch,
    Rule as AbiRule,
};
use radix_abi::startup::SLOT_FIREWALL;

use spark_api::show::State;

use crate::args::{Arg, Args};
use crate::authd::blocking::{Network, Source};
use crate::sys::SYS;
use crate::trial::{self, ToUse, Tried};
use crate::util::{self, Fail, fail, fail_with, usage};
use crate::{ip, net_errors};

/// The rules, a rule a line.
pub const SETTINGS: &str = "/system/settings/firewall";

/// The mode of the settings file when it is made: everyone may read the
/// rules, as `firewall list` does.
const MODE: u32 = 0o644;

/// Spark's name for the trial of a change of the rules.
pub const TRIAL: &str = "firewall";

/// The service that loads the rules at boot, which Spark restarts to put a
/// tried change to use and to undo it.
const SERVICE: &str = "firewall";

/// The most rules the kernel holds.
const RULES_MAX: usize = abi::RULES_MAX as usize;

/// What a rule does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Allow,
    Deny,
}

/// What a rule matches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    Any,
    Tcp,
    Udp,
    Ping,
}

/// One rule, as the settings file writes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub action: Action,
    pub protocol: Protocol,
    /// The first and last port: every one for `None`.
    pub ports: Option<(u16, u16)>,
    /// The network it is from: anywhere for `None`.
    pub from: Option<(IpAddr, u8)>,
    /// The interface it comes by, by name.
    pub on: Option<String>,
    /// The program that listens for it, by its path.
    pub program: Option<String>,
}

/// Whether `name` is one an interface may have: 1 to 15 letters, digits,
/// `.`, `-` and `_` (radix-network(2)).
fn name_ok(name: &str) -> bool {
    (1..=15).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
}

/// `22` or `8000-8100`.
fn parse_ports(word: &str) -> Option<(u16, u16)> {
    let (first, last) = word.split_once('-').unwrap_or((word, word));
    let (first, last): (u16, u16) = (first.parse().ok()?, last.parse().ok()?);
    (first >= 1 && first <= last).then_some((first, last))
}

/// `192.168.1.0/24`, `2001:db8::/32`, or an address alone, its version's
/// whole length.
fn parse_network(word: &str) -> Option<(IpAddr, u8)> {
    let (address, prefix) = match word.split_once('/') {
        Some((address, prefix)) => (address, Some(prefix.parse::<u8>().ok()?)),
        None => (word, None),
    };
    let address: IpAddr = address.parse().ok()?;
    let most = if address.is_ipv4() { 32 } else { 128 };
    let prefix = prefix.unwrap_or(most);
    (prefix <= most).then_some((address, prefix))
}

impl Rule {
    /// A rule's words: `allow|deny [tcp|udp|ping|any] [PORT[-PORT]] [from
    /// NETWORK] [on INTERFACE] [program PATH]`, with `program` last.
    pub fn parse(words: &[&str]) -> Result<Rule, String> {
        let (action, mut rest) = match words.split_first() {
            Some((&"allow", rest)) => (Action::Allow, rest),
            Some((&"deny", rest)) => (Action::Deny, rest),
            Some((other, _)) => {
                return Err(format!("a rule starts with allow or deny, not `{other}`"));
            }
            None => return Err("a rule starts with allow or deny".into()),
        };
        let mut rule = Rule {
            action,
            protocol: Protocol::Any,
            ports: None,
            from: None,
            on: None,
            program: None,
        };
        let mut protocol = None;
        while let Some((&word, after)) = rest.split_first() {
            rest = after;
            let value = |rest: &mut &[&str], what: &str| -> Result<String, String> {
                let (&value, after) =
                    rest.split_first().ok_or_else(|| format!("`{word}` needs {what} after it"))?;
                *rest = after;
                Ok(value.to_string())
            };
            match word {
                "tcp" | "udp" | "ping" | "any" if protocol.is_none() => {
                    protocol = Some(match word {
                        "tcp" => Protocol::Tcp,
                        "udp" => Protocol::Udp,
                        "ping" => Protocol::Ping,
                        _ => Protocol::Any,
                    });
                }
                "from" if rule.from.is_none() => {
                    let network = value(&mut rest, "a network, such as 192.168.1.0/24")?;
                    rule.from = Some(
                        parse_network(&network)
                            .ok_or_else(|| format!("`{network}` is not an address or network"))?,
                    );
                }
                "on" if rule.on.is_none() => {
                    let name = value(&mut rest, "an interface's name")?;
                    if !name_ok(&name) {
                        return Err(format!("`{name}` is not an interface's name"));
                    }
                    rule.on = Some(name);
                }
                "program" => {
                    if rest.is_empty() {
                        return Err("`program` needs a program's path after it".into());
                    }
                    let path = rest.join(" ");
                    if !path.starts_with('/') {
                        return Err(format!("`program` takes an absolute path, not `{path}`"));
                    }
                    rule.program = Some(path);
                    rest = &[];
                }
                _ if rule.ports.is_none() && word.starts_with(|c: char| c.is_ascii_digit()) => {
                    rule.ports = Some(parse_ports(word).ok_or_else(|| {
                        format!("`{word}` is not a port, 1 to 65535, or a range of them")
                    })?);
                }
                _ => return Err(format!("`{word}` is not a word of a rule here")),
            }
        }
        rule.protocol = protocol.unwrap_or(Protocol::Any);
        if rule.protocol == Protocol::Ping && rule.ports.is_some() {
            return Err("a ping has no port".into());
        }
        if rule.protocol == Protocol::Ping && rule.program.is_some() {
            return Err("a ping reaches no program".into());
        }
        Ok(rule)
    }
}

/// A network as a rule's words have it: the address alone when the prefix
/// is its version's whole length.
fn network_text(address: IpAddr, prefix: u8) -> String {
    let most = if address.is_ipv4() { 32 } else { 128 };
    if prefix == most { address.to_string() } else { format!("{address}/{prefix}") }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.action {
            Action::Allow => "allow",
            Action::Deny => "deny",
        })?;
        match self.protocol {
            Protocol::Any => {}
            Protocol::Tcp => f.write_str(" tcp")?,
            Protocol::Udp => f.write_str(" udp")?,
            Protocol::Ping => f.write_str(" ping")?,
        }
        match self.ports {
            Some((first, last)) if first == last => write!(f, " {first}")?,
            Some((first, last)) => write!(f, " {first}-{last}")?,
            None => {}
        }
        if let Some((address, prefix)) = self.from {
            write!(f, " from {}", network_text(address, prefix))?;
        }
        if let Some(on) = &self.on {
            write!(f, " on {on}")?;
        }
        if let Some(program) = &self.program {
            write!(f, " program {program}")?;
        }
        Ok(())
    }
}

/// A line of the settings file: a rule, or anything else, kept as it is.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Line {
    Rule(Rule),
    Other(String),
}

/// The words of a line of the settings file: what comes before a `#`.
fn words_of(line: &str) -> Vec<&str> {
    line.split('#').next().unwrap_or("").split_whitespace().collect()
}

/// The settings file's lines, and what is wrong with those that are not
/// rules or comments, with their numbers.
fn parse_settings(text: &str) -> (Vec<Line>, Vec<String>) {
    let mut lines = Vec::new();
    let mut wrong = Vec::new();
    for (at, line) in text.lines().enumerate() {
        let words = words_of(line);
        if words.is_empty() {
            lines.push(Line::Other(line.to_string()));
            continue;
        }
        match Rule::parse(&words) {
            Ok(rule) => lines.push(Line::Rule(rule)),
            Err(why) => {
                wrong.push(format!("{SETTINGS} line {}: {why}; it is left out", at + 1));
                lines.push(Line::Other(line.to_string()));
            }
        }
    }
    (lines, wrong)
}

/// The rules of the settings file's `text` that name the card they come
/// by, each as the file writes it, with the card's name: what a restore
/// onto other hardware says may do nothing there (backup(8)).
pub(crate) fn rules_on_cards(text: &str) -> Vec<(String, String)> {
    let (lines, _) = parse_settings(text);
    rules(&lines).filter_map(|rule| Some((rule.to_string(), rule.on.clone()?))).collect()
}

/// The rules among `lines`.
fn rules(lines: &[Line]) -> impl Iterator<Item = &Rule> {
    lines.iter().filter_map(|line| match line {
        Line::Rule(rule) => Some(rule),
        Line::Other(_) => None,
    })
}

/// The file `lines` make.
fn text_of(lines: &[Line]) -> String {
    let mut text = String::new();
    for line in lines {
        match line {
            Line::Rule(rule) => text.push_str(&rule.to_string()),
            Line::Other(other) => text.push_str(other),
        }
        text.push('\n');
    }
    text
}

/// A connection, datagram or ping that another machine starts, as far as
/// it is known: what the rules of the file are held against.
#[derive(Clone, Copy, Debug)]
struct Packet<'a> {
    protocol: Protocol,
    /// The port it goes to; a ping has none.
    port: u16,
    /// Where it comes from; `None` when that is not known.
    from: Option<IpAddr>,
    /// The interface it comes by, by name; `None` when that is not known.
    on: Option<&'a str>,
    /// The path of the program that listens for it; `None` when that is
    /// not known, as it is not before a connection comes.
    program: Option<&'a str>,
}

/// Whether `address` is in `network`, of the same version: the kernel's
/// test (radix-firewall(2)).
fn within(address: IpAddr, network: IpAddr, prefix: u8) -> bool {
    match (address, network) {
        (IpAddr::V4(address), IpAddr::V4(network)) => {
            let mask = u32::MAX.checked_shl(32 - u32::from(prefix.min(32))).unwrap_or(0);
            u32::from(address) & mask == u32::from(network) & mask
        }
        (IpAddr::V6(address), IpAddr::V6(network)) => {
            let mask = u128::MAX.checked_shl(128 - u32::from(prefix.min(128))).unwrap_or(0);
            u128::from(address) & mask == u128::from(network) & mask
        }
        _ => false,
    }
}

/// Whether a rule matches a packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fit {
    Yes,
    No,
    /// It depends on what is not known: where the packet comes from, by
    /// which interface, or which program listens for it, when `packet`
    /// does not say.
    Maybe,
}

/// How `rule` fits `packet`, as the kernel matches them: the protocol, the
/// port (a ping matches only a rule for every port), the network, the
/// interface and the program.
fn fit(rule: &Rule, packet: &Packet) -> Fit {
    let protocol = rule.protocol == Protocol::Any || rule.protocol == packet.protocol;
    let port = if packet.protocol == Protocol::Ping {
        rule.ports.is_none()
    } else {
        rule.ports.is_none_or(|(first, last)| (first..=last).contains(&packet.port))
    };
    let from = match (rule.from, packet.from) {
        (Some((network, prefix)), Some(from)) => Some(within(from, network, prefix)),
        (Some(_), None) => None,
        (None, _) => Some(true),
    };
    let on = match (&rule.on, packet.on) {
        (Some(name), Some(on)) => Some(name == on),
        (Some(_), None) => None,
        (None, _) => Some(true),
    };
    // No program listens for a ping, so a rule for a program never has it.
    let program = match (&rule.program, packet.program) {
        (Some(_), _) if packet.protocol == Protocol::Ping => Some(false),
        (Some(path), Some(program)) => Some(path == program),
        (Some(_), None) => None,
        (None, _) => Some(true),
    };
    // A fact that rules the rule out does so whatever is not known.
    if !protocol || !port || [from, on, program].contains(&Some(false)) {
        Fit::No
    } else if [from, on, program].contains(&None) {
        Fit::Maybe
    } else {
        Fit::Yes
    }
}

/// The rule that decides `packet`, if one does: the first of the rules, in
/// order, that fits it, as its number from 0 among the rules and what it
/// does. The rules before it that might apply, as `packet` does not say
/// enough to tell, are [`passed_over`].
fn decides(lines: &[Line], packet: &Packet) -> Option<(usize, Action)> {
    walk(lines, packet).0
}

/// The numbers, from 0, of the rules before the one that decides `packet`
/// (all of them when none does) that might apply: they depend on what
/// `packet` does not say, so the decision holds unless one of them
/// applies.
fn passed_over(lines: &[Line], packet: &Packet) -> Vec<usize> {
    walk(lines, packet).1
}

/// The rule that decides `packet` and the rules passed over before it.
fn walk(lines: &[Line], packet: &Packet) -> (Option<(usize, Action)>, Vec<usize>) {
    let mut passed = Vec::new();
    for (at, rule) in rules(lines).enumerate() {
        match fit(rule, packet) {
            Fit::Yes => return (Some((at, rule.action)), passed),
            Fit::Maybe => passed.push(at),
            Fit::No => {}
        }
    }
    (None, passed)
}

/// What the settings say of a TCP connection to a port: whether it is let
/// in, and which rule says so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opening {
    /// Other machines may start a connection.
    pub open: bool,
    /// The number, from 1 as `firewall list` shows it, of the rule that
    /// decided; none when no rule did and the socket's own right did.
    pub rule: Option<usize>,
}

/// Whether other machines may start a TCP connection to `port`, by the
/// settings `text` says, for a socket that listens there, and the rule that
/// decides: `with_inbound`, when its network capability has the inbound
/// right, as a service's has for the ports its file names (ADR 0334); not
/// when it is a command's of an administrator.
///
/// The first rule that matches decides, as the kernel reads them. A rule
/// that names a network, an interface or a program is passed over, since
/// none of those is known before a connection comes: only a rule for every
/// source decides.
pub fn lets_tcp_in(text: &str, port: u16, with_inbound: bool) -> Opening {
    let (lines, _) = parse_settings(text);
    let packet = Packet { protocol: Protocol::Tcp, port, from: None, on: None, program: None };
    match decides(&lines, &packet) {
        Some((at, action)) => Opening { open: action == Action::Allow, rule: Some(at + 1) },
        None => Opening { open: with_inbound, rule: None },
    }
}

/// A program, by its file: its mount's and its own numbers.
type Program = (u64, u64);

/// What `rule` names, found: the program's file and the interface's
/// number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Found {
    program: Option<Program>,
    interface: u32,
}

/// Finds what `rule` names on this machine.
fn find(rule: &Rule) -> Result<Found, String> {
    let program = match &rule.program {
        Some(path) => {
            let metadata = fs::metadata(path).map_err(|e| format!("no program at {path}: {e}"))?;
            if !metadata.is_file() {
                return Err(format!("{path} is not a program's file"));
            }
            Some((metadata.dev(), metadata.ino()))
        }
        None => None,
    };
    let interface = match &rule.on {
        Some(name) => fs::read_to_string(Path::new(ip::INTERFACES).join(name).join("ifindex"))
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .ok_or_else(|| format!("no interface named {name}"))?,
        None => 0,
    };
    Ok(Found { program, interface })
}

/// `rule`, with what it names found, as a Rule structure.
fn encode(rule: &Rule, found: Found) -> [u8; AbiRule::SIZE] {
    let mut bytes = [0u8; AbiRule::SIZE];
    let action = match rule.action {
        Action::Allow => AbiAction::Allow,
        Action::Deny => AbiAction::Deny,
    };
    let protocol = match rule.protocol {
        Protocol::Any => AbiMatch::Any,
        Protocol::Tcp => AbiMatch::Tcp,
        Protocol::Udp => AbiMatch::Udp,
        Protocol::Ping => AbiMatch::Ping,
    };
    let (first, last) = rule.ports.unwrap_or((0, u16::MAX));
    let mut put = |at: usize, value: &[u8]| bytes[at..at + value.len()].copy_from_slice(value);
    put(AbiRule::OFFSET_ACTION, &action.raw().to_le_bytes());
    put(AbiRule::OFFSET_PROTOCOL, &protocol.raw().to_le_bytes());
    put(AbiRule::OFFSET_FIRST_PORT, &first.to_le_bytes());
    put(AbiRule::OFFSET_LAST_PORT, &last.to_le_bytes());
    match rule.from {
        Some((IpAddr::V4(address), prefix)) => {
            put(AbiRule::OFFSET_FAMILY, &4u32.to_le_bytes());
            put(AbiRule::OFFSET_FROM, &address.octets());
            put(AbiRule::OFFSET_PREFIX, &[prefix]);
        }
        Some((IpAddr::V6(address), prefix)) => {
            put(AbiRule::OFFSET_FAMILY, &6u32.to_le_bytes());
            put(AbiRule::OFFSET_FROM, &address.octets());
            put(AbiRule::OFFSET_PREFIX, &[prefix]);
        }
        None => {}
    }
    put(AbiRule::OFFSET_INTERFACE, &found.interface.to_le_bytes());
    let (mount, file) = found.program.unwrap_or((0, 0));
    put(AbiRule::OFFSET_MOUNT, &mount.to_le_bytes());
    put(AbiRule::OFFSET_FILE, &file.to_le_bytes());
    bytes
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    let mut word = [0u8; 8];
    word.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(word)
}

/// A rule the kernel holds, as `rules` gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Held {
    allow: bool,
    protocol: Protocol,
    ports: (u16, u16),
    /// Whether it is for some machines or interfaces alone.
    narrowed: bool,
    program: Option<Program>,
}

fn decode_rule(bytes: &[u8]) -> Held {
    let program = (u64_at(bytes, AbiRule::OFFSET_MOUNT), u64_at(bytes, AbiRule::OFFSET_FILE));
    Held {
        allow: u32_at(bytes, AbiRule::OFFSET_ACTION) == AbiAction::Allow.raw(),
        protocol: match AbiMatch::from_raw(u32_at(bytes, AbiRule::OFFSET_PROTOCOL)) {
            Some(AbiMatch::Tcp) => Protocol::Tcp,
            Some(AbiMatch::Udp) => Protocol::Udp,
            Some(AbiMatch::Ping) => Protocol::Ping,
            _ => Protocol::Any,
        },
        ports: (
            u16_at(bytes, AbiRule::OFFSET_FIRST_PORT),
            u16_at(bytes, AbiRule::OFFSET_LAST_PORT),
        ),
        narrowed: u32_at(bytes, AbiRule::OFFSET_FAMILY) != 0
            || u32_at(bytes, AbiRule::OFFSET_INTERFACE) != 0,
        program: (program != (0, 0)).then_some(program),
    }
}

/// A socket that listens, as `listeners` gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Listening {
    pub tcp: bool,
    pub port: u16,
    pub ipv4: bool,
    pub ipv6: bool,
    pub inbound: bool,
    pub interface: u32,
    pub process: u64,
    pub program: Option<Program>,
}

fn decode_listener(bytes: &[u8]) -> Listening {
    let flags = ListenerFlags::from_bits(u32_at(bytes, AbiListener::OFFSET_FLAGS))
        .unwrap_or(ListenerFlags::from_bits(0).unwrap_or(ListenerFlags::IPV4));
    let program =
        (u64_at(bytes, AbiListener::OFFSET_MOUNT), u64_at(bytes, AbiListener::OFFSET_FILE));
    Listening {
        tcp: u32_at(bytes, AbiListener::OFFSET_PROTOCOL) == AbiMatch::Tcp.raw(),
        port: u16_at(bytes, AbiListener::OFFSET_PORT),
        ipv4: flags.contains(ListenerFlags::IPV4),
        ipv6: flags.contains(ListenerFlags::IPV6),
        inbound: flags.contains(ListenerFlags::INBOUND),
        interface: u32_at(bytes, AbiListener::OFFSET_INTERFACE),
        process: u64_at(bytes, AbiListener::OFFSET_PROCESS),
        program: (program != (0, 0)).then_some(program),
    }
}

/// Who reaches a listener from other machines, as the rules and its
/// right say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reach {
    /// Other machines, as its service's file says.
    Service,
    /// Other machines, as a rule allows.
    Allowed,
    /// Some machines or interfaces, as a rule says.
    Some,
    /// This machine alone: it was started by a person, and no rule allows
    /// it.
    Local,
    /// This machine alone: a rule denies it.
    Denied,
}

/// Who reaches `listener`, by `rules` in order.
fn reach(listener: &Listening, rules: &[Held]) -> Reach {
    decided(listener, rules).0
}

/// Who reaches `listener`, by `rules` in order, and the number, from 0, of
/// the rule that says so: none when no rule does and its right decides.
fn decided(listener: &Listening, rules: &[Held]) -> (Reach, Option<usize>) {
    for (at, rule) in rules.iter().enumerate() {
        let protocol = match rule.protocol {
            Protocol::Any => true,
            Protocol::Tcp => listener.tcp,
            Protocol::Udp => !listener.tcp,
            Protocol::Ping => false,
        };
        let port = (rule.ports.0..=rule.ports.1).contains(&listener.port);
        let program = rule.program.is_none_or(|program| listener.program == Some(program));
        if protocol && port && program {
            let reach = match (rule.narrowed, rule.allow) {
                (true, _) => Reach::Some,
                (false, true) => Reach::Allowed,
                (false, false) => Reach::Denied,
            };
            return (reach, Some(at));
        }
    }
    (if listener.inbound { Reach::Service } else { Reach::Local }, None)
}

/// The kernel's firewall, through the capability in SLOT_FIREWALL.
fn kernel_rules() -> Result<Vec<Held>, Status> {
    let mut buffer = vec![0u8; RULES_MAX * AbiRule::SIZE];
    let count = abi::rules(&SYS, SLOT_FIREWALL, &mut buffer)?.count as usize;
    Ok(buffer.chunks(AbiRule::SIZE).take(count.min(RULES_MAX)).map(decode_rule).collect())
}

fn kernel_listeners() -> Result<Vec<Listening>, Status> {
    let mut room = 64;
    loop {
        let mut buffer = vec![0u8; room * AbiListener::SIZE];
        let count = abi::listeners(&SYS, SLOT_FIREWALL, &mut buffer)?.count as usize;
        if count <= room {
            let chunks = buffer.chunks(AbiListener::SIZE).take(count);
            return Ok(chunks.map(decode_listener).collect());
        }
        room = count;
    }
}

/// What the kernel refused, said plainly.
fn refused(status: Status) -> Fail {
    match Kernel::from_status(status) {
        Some(
            Kernel::NoSuchSlot | Kernel::EmptySlot | Kernel::WrongKind | Kernel::MissingRights,
        ) => fail_with(2, "this needs the firewall capability: run it with elevate(8)".into()),
        _ => fail_with(2, crate::sys::status_name(status.0)),
    }
}

/// Where process `process`'s program is, if a path among its arguments
/// is `program`'s file.
fn program_path(process: u64, program: Option<Program>) -> Option<String> {
    let cmdline = fs::read(format!("/processes/{process}/cmdline")).ok()?;
    let mut first = None;
    for arg in cmdline.split(|&b| b == 0).filter(|arg| !arg.is_empty()) {
        let Ok(arg) = std::str::from_utf8(arg) else { continue };
        if first.is_none() {
            first = Some(arg.to_string());
        }
        if !arg.starts_with('/') {
            continue;
        }
        let Ok(metadata) = fs::metadata(arg) else { continue };
        if Some((metadata.dev(), metadata.ino())) == program {
            return Some(arg.to_string());
        }
    }
    program.is_none().then_some(first).flatten()
}

/// A port that other machines reach, as `firewall status` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Reached {
    pub(crate) tcp: bool,
    pub(crate) port: u16,
    /// The file name of the program that listens, or `process N` when
    /// its file is not found.
    pub(crate) program: String,
    /// Whether every machine reaches it, not only the machines a rule
    /// names.
    pub(crate) everywhere: bool,
}

/// The ports of the sockets that listen for other machines, by the
/// rules the kernel holds, each protocol and port once, TCP first.
pub(crate) fn reached() -> Result<Vec<Reached>, Fail> {
    let held = kernel_rules().map_err(refused)?;
    let mut listeners = kernel_listeners().map_err(refused)?;
    listeners.sort_by_key(|l| (!l.tcp, l.port, l.process));
    let mut found: Vec<Reached> = Vec::new();
    for listener in &listeners {
        let everywhere = match reach(listener, &held) {
            Reach::Service | Reach::Allowed => true,
            Reach::Some => false,
            Reach::Local | Reach::Denied => continue,
        };
        if found.iter().any(|r| r.tcp == listener.tcp && r.port == listener.port) {
            continue;
        }
        let program = program_path(listener.process, listener.program)
            .and_then(|path| Path::new(&path).file_name().map(|name| name.to_string_lossy().into()))
            .unwrap_or_else(|| format!("process {}", listener.process));
        found.push(Reached { tcp: listener.tcp, port: listener.port, program, everywhere });
    }
    Ok(found)
}

/// The rules in the settings file's `text`, in order, as `firewall list`
/// numbers them. A line that is no rule is left out, as `firewall load`
/// leaves it out.
pub(crate) fn rules_in(text: &str) -> Vec<Rule> {
    rules(&parse_settings(text).0).cloned().collect()
}

const HELP: &str = "Usage: firewall [status] [--json]
       firewall list [--json]
       firewall allow|deny [tcp|udp|ping|any] [PORT[-PORT]] [from NETWORK] [on INTERFACE]
                           [program PATH] [--try SECONDS]
       firewall allow process NUMBER [--try SECONDS]
       firewall remove NUMBER|RULE [--try SECONDS]
       firewall replace RULE with RULE [--try SECONDS]
       firewall keep | undo
       firewall load
       firewall test tcp|udp PORT from ADDRESS [on INTERFACE] [--json]
       firewall dropped [--since TIME] [--json]
       firewall blocked [--json]
       firewall unblock ADDRESS
       firewall never-block [remove] NETWORK
  --try SECONDS  try the change: Spark undoes it after SECONDS, 10 to 3600, unless
                 `firewall keep` keeps it
";

/// The options that may end a command's words, as a rule's words never
/// start with `--`.
#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    /// `--json`: for `status`, `list` and `blocked`.
    json: bool,
    /// `--try SECONDS`: for a change.
    seconds: Option<u32>,
}

/// `words` without the options that end them, and those options.
fn options_after<'a, 'w>(words: &'a [&'w str]) -> Result<(&'a [&'w str], Options), Fail> {
    let mut options = Options::default();
    let mut rest = words;
    loop {
        rest = match rest {
            [head @ .., "--json"] if !options.json => {
                options.json = true;
                head
            }
            [head @ .., "--try", text] if options.seconds.is_none() => {
                options.seconds = Some(trial::seconds(text)?);
                head
            }
            [head @ .., last] if options.seconds.is_none() && last.starts_with("--try=") => {
                options.seconds = Some(trial::seconds(&last["--try=".len()..])?);
                head
            }
            _ => break,
        };
    }
    if let Some(word) = rest.iter().find(|word| word.starts_with("--")) {
        return Err(usage(format!(
            "`{word}` is not a word here: --json and --try SECONDS come after the \
             command's words, once each"
        )));
    }
    Ok((rest, options))
}

/// Runs firewall.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let mut words = Vec::new();
    args.stop_at_operand();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Operand(word) => {
                words.push(word.to_string_lossy().into_owned());
                words.extend(args.rest().into_iter().map(|w| w.to_string_lossy().into_owned()));
            }
            Arg::Long(ref name) if name == "help" => {
                print!("{HELP}");
                return Ok(0);
            }
            other => return Err(args.unknown(&other)),
        }
    }
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let mut out = io::stdout().lock();
    // `test` and `dropped` read their own options, which may come anywhere.
    match words.split_first() {
        Some((&"test", rest)) => return report::test(rest, &mut out),
        Some((&"dropped", rest)) => return report::dropped(rest, &mut out),
        _ => {}
    }
    let (words, Options { json, seconds }) = options_after(&words)?;
    let first = words.first().copied();
    if json && !matches!(first, None | Some("status" | "list" | "blocked")) {
        return Err(usage("--json goes with status, list and blocked".into()));
    }
    if seconds.is_some() && !matches!(first, Some("allow" | "deny" | "remove" | "replace")) {
        return Err(usage("--try goes with allow, deny, remove and replace".into()));
    }
    match words.split_first() {
        None | Some((&"status", [])) if json => report::status(&mut out),
        None | Some((&"status", [])) => status(&mut out),
        Some((&"list", [])) if json => report::list(&mut out),
        Some((&"list", [])) => list(&mut out),
        Some((&"load", [])) => load(&mut out, true),
        Some((&"help", _)) => {
            write!(out, "{HELP}")?;
            Ok(0)
        }
        Some((&verb @ ("keep" | "undo"), [])) => end(verb, &mut out),
        Some((&("blocked" | "unblock" | "never-block"), _)) => blocks(words, json, &mut out),
        Some((&"remove", [number])) if number.starts_with(|c: char| c.is_ascii_digit()) => {
            remove_number(number, seconds, &mut out)
        }
        Some((&"remove", rule)) => remove_rule(rule, seconds, &mut out),
        Some((&"replace", rest)) => replace(rest, seconds, &mut out),
        Some((&"allow", ["process", number])) => allow_process(number, seconds, &mut out),
        Some((&("allow" | "deny"), _)) => add(&rule_of(words)?, seconds, &mut out),
        Some((other, _)) => Err(usage(format!(
            "no command '{other}': status, list, allow, deny, remove, replace, keep, \
             undo, load, test, dropped, blocked, unblock or never-block"
        ))),
    }
}

/// The rule `words` say, or a usage error that says why they do not.
fn rule_of(words: &[&str]) -> Result<Rule, Fail> {
    Rule::parse(words).map_err(usage)
}

/// The settings file's lines.
fn read_settings() -> Result<(Vec<Line>, Vec<String>), Fail> {
    match fs::read_to_string(SETTINGS) {
        Ok(text) => Ok(parse_settings(&text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok((Vec::new(), Vec::new())),
        Err(e) => Err(crate::util::failed(&SETTINGS, e)),
    }
}

/// A change of the rules is refused while one is being tried, since
/// Spark's undo would put back the file as it was before both (NET-177).
/// Spark that does not answer runs no trial: only a change that is tried
/// needs it.
fn not_tried(seconds: Option<u32>) -> Result<(), Fail> {
    match trial::running(TRIAL) {
        Ok(Some(_)) => Err(net_errors::firewall_is_tried()),
        Ok(None) => Ok(()),
        Err(_) if seconds.is_none() => Ok(()),
        Err(refused) => Err(refused),
    }
}

/// Whether the `firewall` service has loaded the rules, as a trial needs:
/// Spark's undo restarts the service that ran, and a service that is
/// stopped or failed would not load the rules put back.
fn loaded(state: State) -> bool {
    matches!(state, State::Done | State::Running | State::Starting)
}

/// What a change that is tried needs: root, for Spark's control socket,
/// and a `firewall` service that has loaded the rules.
fn may_try(words: &str) -> Result<(), Fail> {
    if util::own_user() != 0 {
        return Err(fail(format!(
            "only root tries a change of the rules: run it with elevate, as \
             `elevate firewall {words} --try 60`"
        )));
    }
    let shown = crate::service::shown(Some(SERVICE))?;
    match shown.services.first().map(|service| service.state) {
        Some(state) if loaded(state) => Ok(()),
        _ => Err(fail(
            "the firewall service has not loaded the rules, so an undo could not load \
             them: `service firewall start` first"
                .into(),
        )),
    }
}

/// Puts `lines` in the settings file and the kernel, and says `said`: at
/// once, or tried for `seconds` as the `firewall` trial, which restarting
/// the service loads. A tried change that the service does not load is
/// undone at once, and says so, with 1.
fn put(lines: &[Line], seconds: Option<u32>, said: &str, out: &mut dyn Write) -> Result<i32, Fail> {
    let text = text_of(lines);
    let Some(seconds) = seconds else {
        trial::replace(Path::new(SETTINGS), text.as_bytes(), MODE)?;
        load(out, false)?;
        writeln!(out, "firewall: {said}")?;
        return Ok(0);
    };
    let to_use = ToUse::Restart(SERVICE);
    match trial::try_text(TRIAL, Path::new(SETTINGS), text.as_bytes(), MODE, seconds, to_use)? {
        Tried::InUse => {
            writeln!(out, "firewall: {said}")?;
            let at = trial::undone_at(seconds);
            writeln!(out, "firewall: Spark undoes it at {at} unless it is kept: `firewall keep`")?;
            Ok(0)
        }
        Tried::Undone { why, undone } => {
            out.flush()?;
            eprintln!("firewall: {why}");
            writeln!(out, "firewall: {undone}")?;
            Ok(1)
        }
    }
}

/// `firewall keep` and `firewall undo`: what the trial of the rules comes
/// to.
fn end(verb: &str, out: &mut dyn Write) -> Result<i32, Fail> {
    if util::own_user() != 0 {
        return Err(fail(format!(
            "only root keeps or undoes a change of the rules: run it with elevate, as \
             `elevate firewall {verb}`"
        )));
    }
    let words = trial::end(verb, TRIAL, || net_errors::nothing_is_tried("the firewall's rules"))?;
    match verb {
        "keep" => writeln!(out, "firewall: kept: the rules stay as they are")?,
        _ => writeln!(out, "firewall: {words}")?,
    }
    Ok(0)
}

/// `firewall load`: gives the kernel the settings file's rules, those it
/// can find what they name for. `admin put-back` runs it too, after it
/// writes the file.
pub(crate) fn load(out: &mut dyn Write, say: bool) -> Result<i32, Fail> {
    let (lines, wrong) = read_settings()?;
    for line in &wrong {
        eprintln!("firewall: {line}");
    }
    let mut bytes = Vec::new();
    let mut given = 0;
    for rule in rules(&lines) {
        match find(rule) {
            Ok(found) if given < RULES_MAX => {
                bytes.extend_from_slice(&encode(rule, found));
                given += 1;
            }
            Ok(_) => eprintln!("firewall: `{rule}` is past the {RULES_MAX} rules it holds"),
            Err(why) => eprintln!("firewall: `{rule}` is left out: {why}"),
        }
    }
    abi::set_rules(&SYS, SLOT_FIREWALL, &bytes).map_err(refused)?;
    if say {
        writeln!(out, "firewall: {given} rules")?;
    }
    Ok(0)
}

/// `firewall list`: the settings file's rules, numbered.
fn list(out: &mut dyn Write) -> Result<i32, Fail> {
    let (lines, _) = read_settings()?;
    let mut any = false;
    for (number, rule) in rules(&lines).enumerate() {
        writeln!(out, "{:>3}  {rule}", number + 1)?;
        any = true;
    }
    if !any {
        writeln!(
            out,
            "No rules: what services listen on is open, and the rest is for this machine alone."
        )?;
    }
    Ok(0)
}

/// The checks every change makes before it reads the file: none while a
/// change is tried, and what a change that is tried needs. `words` are
/// the command's, for the refusal's example.
fn may_change(seconds: Option<u32>, words: &str) -> Result<(), Fail> {
    if seconds.is_some() {
        may_try(words)?;
    }
    not_tried(seconds)
}

/// `firewall allow` and `deny`: adds `rule` at the end, and loads.
fn add(rule: &Rule, seconds: Option<u32>, out: &mut dyn Write) -> Result<i32, Fail> {
    find(rule).map_err(fail)?;
    may_change(seconds, &rule.to_string())?;
    let (mut lines, _) = read_settings()?;
    match added(&mut lines, rule)? {
        Some(number) => put(&lines, seconds, &format!("rule {number} is `{rule}`"), out),
        None => {
            writeln!(out, "firewall: `{rule}` is a rule already")?;
            Ok(0)
        }
    }
}

/// Adds `rule` at the end of `lines`, and gives its number from 1; none
/// when it is a rule already.
fn added(lines: &mut Vec<Line>, rule: &Rule) -> Result<Option<usize>, Fail> {
    if rules(lines).any(|held| held == rule) {
        return Ok(None);
    }
    if rules(lines).count() >= RULES_MAX {
        return Err(fail(format!("there are {RULES_MAX} rules, the most it holds")));
    }
    lines.push(Line::Rule(rule.clone()));
    Ok(Some(rules(lines).count()))
}

/// The place in `lines` of the rule that `pick` picks, with the rule.
fn place_of(lines: &[Line], pick: impl Fn(usize, &Rule) -> bool) -> Option<(usize, Rule)> {
    let mut number = 0;
    lines.iter().enumerate().find_map(|(at, line)| match line {
        Line::Rule(rule) => {
            number += 1;
            pick(number, rule).then(|| (at, rule.clone()))
        }
        Line::Other(_) => None,
    })
}

/// `firewall remove N`.
fn remove_number(number: &str, seconds: Option<u32>, out: &mut dyn Write) -> Result<i32, Fail> {
    let number: usize = number
        .parse()
        .ok()
        .filter(|&n| n >= 1)
        .ok_or_else(|| usage(format!("`{number}` is not a rule's number")))?;
    may_change(seconds, &format!("remove {number}"))?;
    let (mut lines, _) = read_settings()?;
    let (at, gone) = place_of(&lines, |n, _| n == number)
        .ok_or_else(|| fail(format!("there is no rule {number}")))?;
    lines.remove(at);
    put(&lines, seconds, &format!("`{gone}` is gone"), out)
}

/// `firewall remove RULE`: the first rule that is the same rule as the
/// words, in whatever order they come (NET-180 when none is).
fn remove_rule(words: &[&str], seconds: Option<u32>, out: &mut dyn Write) -> Result<i32, Fail> {
    let rule = rule_of(words)?;
    may_change(seconds, &format!("remove {rule}"))?;
    let (mut lines, _) = read_settings()?;
    let (at, _) = place_of(&lines, |_, held| *held == rule)
        .ok_or_else(|| net_errors::no_such_rule(&words.join(" ")))?;
    lines.remove(at);
    put(&lines, seconds, &format!("`{rule}` is gone"), out)
}

/// `firewall replace RULE with RULE`: the first rule that is the same
/// rule as the first words, put in its place by the second, which keeps
/// its order (NET-180 when none is).
fn replace(words: &[&str], seconds: Option<u32>, out: &mut dyn Write) -> Result<i32, Fail> {
    const FORM: &str = "firewall: replace takes a rule, `with` and a rule: firewall replace \
                        allow tcp 22 with allow tcp 22 from 192.168.1.0/24";
    let at = words.iter().position(|&word| word == "with").ok_or_else(|| usage(FORM.into()))?;
    let (old, new) = (&words[..at], &words[at + 1..]);
    if old.is_empty() || new.is_empty() {
        return Err(usage(FORM.into()));
    }
    let (old_rule, new_rule) = (rule_of(old)?, rule_of(new)?);
    find(&new_rule).map_err(fail)?;
    may_change(seconds, &format!("replace {old_rule} with {new_rule}"))?;
    let (mut lines, _) = read_settings()?;
    let (at, _) = place_of(&lines, |_, held| *held == old_rule)
        .ok_or_else(|| net_errors::no_such_rule(&old.join(" ")))?;
    if old_rule == new_rule {
        writeln!(out, "firewall: `{new_rule}` is the rule already")?;
        return Ok(0);
    }
    lines[at] = Line::Rule(new_rule.clone());
    put(&lines, seconds, &format!("`{old_rule}` is now `{new_rule}`"), out)
}

/// `firewall allow process N`: allows from other machines the program
/// process N runs, by its path.
fn allow_process(number: &str, seconds: Option<u32>, out: &mut dyn Write) -> Result<i32, Fail> {
    let process: u64 =
        number.parse().map_err(|_| usage(format!("`{number}` is not a process number")))?;
    let listeners = kernel_listeners().map_err(refused)?;
    let listener = listeners
        .iter()
        .find(|listener| listener.process == process)
        .ok_or_else(|| fail(format!("process {process} listens on nothing")))?;
    let path = program_path(process, listener.program)
        .filter(|_| listener.program.is_some())
        .ok_or_else(|| fail(format!("the program of process {process} is not found")))?;
    let rule = Rule {
        action: Action::Allow,
        protocol: Protocol::Any,
        ports: None,
        from: None,
        on: None,
        program: Some(path),
    };
    add(&rule, seconds, out)
}

/// What `firewall blocked`, `unblock` and `never-block` ask of authd (ADR
/// 0479).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Blocks {
    /// The addresses blocked now.
    Show,
    /// Lift the block of a source.
    Unblock(Source),
    /// Keep `network` out of the blocks, or let them block it again when
    /// `remove`.
    NeverBlock { network: Network, remove: bool },
}

impl Blocks {
    /// The command in `words`, `blocked`, `unblock ADDRESS` or `never-block
    /// [remove] NETWORK`, with its address or network read the way authd
    /// reads it, so that a mistake is told here in one plain line and
    /// authd is asked only what it takes.
    ///
    /// # Errors
    ///
    /// What is wrong with the words.
    pub(crate) fn parse(words: &[&str]) -> Result<Blocks, String> {
        match words {
            ["blocked"] => Ok(Blocks::Show),
            ["unblock", address] => {
                Source::parse(address).map(Blocks::Unblock).ok_or_else(|| not_an_address(address))
            }
            ["never-block", "remove", network] => {
                Ok(Blocks::NeverBlock { network: Network::parse(network)?, remove: true })
            }
            // A network named `remove` is no network, so a lone `remove`
            // is a command with its network left out.
            ["never-block", network] if *network != "remove" => {
                Ok(Blocks::NeverBlock { network: Network::parse(network)?, remove: false })
            }
            ["blocked", ..] => Err("blocked takes nothing more".into()),
            ["unblock", ..] => Err("unblock takes one address, as `unblock 203.0.113.9`".into()),
            _ => Err("never-block takes a network, as `never-block 192.168.1.0/24`, or remove \
                      and a network"
                .into()),
        }
    }

    /// What authd is asked.
    fn request(&self) -> Request {
        match self {
            Blocks::Show => Request::Blocked,
            Blocks::Unblock(source) => Request::Unblock { address: source.to_string() },
            Blocks::NeverBlock { network, remove } => {
                Request::NeverBlock { network: network.to_string(), remove: *remove }
            }
        }
    }

    /// The line that says it is done, for a command that changes
    /// something, from authd's `answer`: none for the list.
    fn done(&self, answer: &str) -> Option<String> {
        let already = answer == request::NEVER_AS_IT_WAS;
        Some(match self {
            Blocks::Show => return None,
            Blocks::Unblock(source) => format!("firewall: {source} is no longer blocked"),
            Blocks::NeverBlock { network, remove: false } if already => {
                format!("firewall: {network} is never blocked already")
            }
            Blocks::NeverBlock { network, remove: false } => {
                format!("firewall: {network} is never blocked")
            }
            Blocks::NeverBlock { network, remove: true } if already => {
                format!("firewall: {network} is not in never, and may be blocked")
            }
            Blocks::NeverBlock { network, remove: true } => {
                format!("firewall: {network} may be blocked again")
            }
        })
    }
}

/// Why `word` is not an address that can be blocked.
fn not_an_address(word: &str) -> String {
    format!(
        "`{}` is not an address that is blocked: write an IPv4 address, an IPv6 address, which \
         stands for its network of 64 bits, or that network as 2001:db8:4:7::/64",
        word.escape_debug()
    )
}

/// `firewall blocked`, `unblock` and `never-block`: ask authd, which keeps
/// the blocks and blocking.toml(5), and say what it answers: for `blocked`
/// a table, or with `json` the report the web console reads.
fn blocks(words: &[&str], json: bool, out: &mut dyn Write) -> Result<i32, Fail> {
    let command = Blocks::parse(words).map_err(usage)?;
    let answer = match crate::account::ask(&command.request())? {
        Answer::Done(answer) => answer,
        Answer::Refused(why) => return Err(fail(why)),
    };
    match command.done(&answer) {
        Some(done) => writeln!(out, "{done}")?,
        None if json => return report::blocked(&BlockedEntry::parse_list(&answer), out),
        None => {
            let entries = BlockedEntry::parse_list(&answer);
            let now = (util::now_ns() / 1_000_000_000) as i64;
            write!(out, "{}", blocked_table(&entries, now, util::local_time))?;
        }
    }
    Ok(0)
}

/// The table `firewall blocked` shows: each address, until when it is
/// blocked, how many blocks in a row it has had, each within a day of the
/// one before, and why. `now`
/// is the time in seconds since 1970, and `show` writes a time in a
/// strftime(3) format, in the local time zone.
fn blocked_table(entries: &[BlockedEntry], now: i64, show: impl Fn(i64, &str) -> String) -> String {
    if entries.is_empty() {
        return "No address is blocked.\n".into();
    }
    let mut rows = vec![["Blocked", "Until", "In a row", "Why"].map(String::from)];
    rows.extend(entries.iter().map(|entry| {
        [
            entry.address.clone(),
            until_text(entry.until, now, &show),
            entry.times.to_string(),
            entry.why.clone(),
        ]
    }));
    // The last column, the reason, runs to the end of the line.
    let width: [usize; 3] =
        std::array::from_fn(|c| rows.iter().map(|row| row[c].chars().count()).max().unwrap_or(0));
    let mut table = String::new();
    for row in &rows {
        table.push_str(&format!(
            "{:<w0$}  {:<w1$}  {:<w2$}  {}\n",
            row[0],
            row[1],
            row[2],
            row[3],
            w0 = width[0],
            w1 = width[1],
            w2 = width[2]
        ));
    }
    table
}

/// When a block ends, as the table says it: the local time of day, such as
/// `14:32`, when that is today, and otherwise the date too.
fn until_text(until: i64, now: i64, show: &impl Fn(i64, &str) -> String) -> String {
    const DATE: &str = "%Y-%m-%d";
    if show(until, DATE) == show(now, DATE) {
        show(until, "%H:%M")
    } else {
        show(until, "%Y-%m-%d %H:%M")
    }
}

/// `firewall status`: the rules' count, what was dropped, and each
/// socket that listens, with who reaches it.
fn status(out: &mut dyn Write) -> Result<i32, Fail> {
    let state = abi::state(&SYS, SLOT_FIREWALL).map_err(refused)?;
    let held = kernel_rules().map_err(refused)?;
    let mut listeners = kernel_listeners().map_err(refused)?;
    listeners.sort_by_key(|l| (!l.tcp, l.port, l.process));
    listeners.dedup_by_key(|l| (l.tcp, l.port, l.process, l.ipv4, l.ipv6));
    writeln!(
        out,
        "Firewall: {} rules; {} packets dropped since the machine started",
        state.rules, state.dropped
    )?;
    writeln!(
        out,
        "Other machines reach what services listen on, and what the rules allow; pings are answered."
    )?;
    if listeners.is_empty() {
        writeln!(out, "\nNothing listens.")?;
        return Ok(0);
    }
    writeln!(out)?;
    writeln!(out, "{:<14} {:>7}  {:<32} Reached from", "Listening", "Process", "Program")?;
    for listener in &listeners {
        let path = program_path(listener.process, listener.program);
        let what = format!(
            "{} {}{}",
            if listener.tcp { "tcp" } else { "udp" },
            listener.port,
            match (listener.ipv4, listener.ipv6) {
                (true, false) => " (v4)",
                (false, true) => " (v6)",
                _ => "",
            }
        );
        let reach = reach(listener, &held);
        let from = match reach {
            Reach::Service => "other machines (its service)",
            Reach::Allowed => "other machines (a rule)",
            Reach::Some => "some machines (a rule)",
            Reach::Local => "this machine only",
            Reach::Denied => "this machine only (a rule)",
        };
        writeln!(
            out,
            "{what:<14} {:>7}  {:<32} {from}",
            listener.process,
            path.as_deref().unwrap_or("?")
        )?;
        if reach == Reach::Local {
            match &path {
                Some(path) if listener.program.is_some() => writeln!(
                    out,
                    "{:24}To allow it from other machines: firewall allow program {path}",
                    ""
                )?,
                _ => writeln!(
                    out,
                    "{:24}To allow it from other machines: firewall allow process {}",
                    "", listener.process
                )?,
            }
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(text: &str) -> Result<Rule, String> {
        Rule::parse(&text.split_whitespace().collect::<Vec<_>>())
    }

    #[test]
    fn rules_are_read_and_written_back_alike() {
        for text in [
            "allow tcp 22",
            "deny udp 5353 on net1",
            "allow tcp 8000-8100 from 192.168.1.0/24",
            "allow from 2001:db8::/32",
            "deny ping from 10.0.0.1",
            "allow program /users/alice/bin/my server",
            "allow tcp 8080 on br0 program /applications/commands/web",
            "deny",
        ] {
            assert_eq!(rule(text).unwrap().to_string(), text);
        }
        assert_eq!(rule("allow 22 tcp").unwrap().to_string(), "allow tcp 22");
        assert_eq!(rule("allow any 1-65535").unwrap().to_string(), "allow 1-65535");
    }

    #[test]
    fn a_connection_to_a_port_is_let_in_by_the_first_rule_for_every_source() {
        let rules = "# kept\nallow tcp 8000-8100\ndeny tcp 80 from 10.0.0.0/8\n\
                     deny tcp 80 program /x\nallow tcp 80\ndeny tcp 80\n";
        let open = |text: &str, port: u16, inbound: bool| lets_tcp_in(text, port, inbound).open;
        assert!(open(rules, 80, false));
        assert!(open(rules, 8080, false));
        assert!(!open(rules, 443, false));
        assert!(open(rules, 443, true));
        // A rule for a network, an interface or a program does not decide.
        assert!(open("deny 80 on net0\n", 80, true));
        assert!(!open("deny tcp 80\nallow tcp 80\n", 80, true));
        assert!(!open("allow udp 80\n", 80, false));
        assert!(!open("allow ping\n", 80, false));
        assert!(open("allow\n", 80, false));
        assert!(!open("", 80, false));
        assert!(open("", 80, true));
        assert!(!open("not a rule\n", 80, false));
    }

    #[test]
    fn the_rule_that_decides_is_told_by_its_number_in_the_list() {
        // The numbers are those `firewall list` shows: comments and blank
        // lines are not counted.
        let rules = "# kept\nallow tcp 8000-8100\n\ndeny tcp 80 from 10.0.0.0/8\n\
                     deny tcp 80 program /x\nallow tcp 80\ndeny tcp 80\n";
        assert_eq!(lets_tcp_in(rules, 80, false), Opening { open: true, rule: Some(4) });
        assert_eq!(lets_tcp_in(rules, 8080, false), Opening { open: true, rule: Some(1) });
        assert_eq!(
            lets_tcp_in("allow tcp 22\ndeny tcp 80\n", 80, true),
            Opening { open: false, rule: Some(2) }
        );
        // No rule decides: the socket's own right does.
        assert_eq!(lets_tcp_in("allow tcp 22\n", 80, true), Opening { open: true, rule: None });
        assert_eq!(lets_tcp_in("", 80, false), Opening { open: false, rule: None });
    }

    fn packet<'a>(
        protocol: Protocol,
        port: u16,
        from: Option<&str>,
        on: Option<&'a str>,
    ) -> Packet<'a> {
        Packet { protocol, port, from: from.map(|from| from.parse().unwrap()), on, program: None }
    }

    #[test]
    fn the_first_rule_that_fits_decides_and_the_rules_before_it_that_might_are_passed_over() {
        let (lines, _) = parse_settings(
            "allow tcp 22 from 192.168.1.0/24\n\
             deny tcp 22 on net1\n\
             deny tcp 8000-8100 program /x\n\
             allow udp 53\n\
             deny tcp 22\n\
             allow\n",
        );
        let tcp = |port, from, on| packet(Protocol::Tcp, port, from, on);
        let decided = |packet: &Packet| decides(&lines, packet);
        // The network, from here and from elsewhere.
        assert_eq!(decided(&tcp(22, Some("192.168.1.9"), None)), Some((0, Action::Allow)));
        assert_eq!(decided(&tcp(22, Some("203.0.113.9"), Some("net0"))), Some((4, Action::Deny)));
        // By the interface, in its turn.
        assert_eq!(decided(&tcp(22, Some("203.0.113.9"), Some("net1"))), Some((1, Action::Deny)));
        // Nothing of the rules before the last fits.
        assert_eq!(decided(&tcp(80, Some("203.0.113.9"), None)), Some((5, Action::Allow)));
        // Without a source, the rules that name a network or interface might
        // apply: the ones before the decider are passed over.
        let unknown = tcp(22, None, None);
        assert_eq!(decided(&unknown), Some((4, Action::Deny)));
        assert_eq!(passed_over(&lines, &unknown), [0, 1]);
        assert_eq!(
            passed_over(&lines, &tcp(22, Some("203.0.113.9"), Some("net0"))),
            Vec::<usize>::new()
        );
        // A rule for a program might apply to what comes to its ports.
        assert_eq!(passed_over(&lines, &tcp(8080, Some("203.0.113.9"), None)), [2]);
        assert_eq!(passed_over(&lines, &tcp(9000, Some("203.0.113.9"), None)), Vec::<usize>::new());
        // UDP is by its own rules, and the version of IP is the network's.
        assert_eq!(
            decided(&packet(Protocol::Udp, 53, Some("::1"), None)),
            Some((3, Action::Allow))
        );
        assert_eq!(
            decided(&packet(Protocol::Udp, 54, Some("::1"), None)),
            Some((5, Action::Allow))
        );
    }

    /// A question of `cases.txt`: `tcp|udp PORT` or `ping`, then `from
    /// ADDRESS on CARD [program PATH]`.
    fn asked(words: &str) -> Packet<'_> {
        let words: Vec<&str> = words.split_whitespace().collect();
        let (protocol, port, rest) = match words[..] {
            ["ping", ref rest @ ..] => (Protocol::Ping, 0, rest),
            ["tcp", port, ref rest @ ..] => (Protocol::Tcp, port.parse().unwrap(), rest),
            ["udp", port, ref rest @ ..] => (Protocol::Udp, port.parse().unwrap(), rest),
            _ => panic!("{words:?}"),
        };
        let (from, on, program) = match rest {
            ["from", from, "on", on] => (from, on, None),
            ["from", from, "on", on, "program", path] => (from, on, Some(*path)),
            _ => panic!("{words:?}"),
        };
        Packet { protocol, port, from: Some(from.parse().unwrap()), on: Some(on), program }
    }

    /// The cases the Firewall page's model holds against too: the rule
    /// that decides each question, by the rules of its block.
    #[test]
    fn the_shared_cases_are_decided_as_the_page_decides_them() {
        let mut count = 0;
        for block in include_str!("firewall/cases.txt").split("\n\n") {
            let mut rules = String::new();
            for line in block.lines().filter(|line| !line.starts_with('#')) {
                let Some(question) = line.strip_prefix("> ") else {
                    rules += line;
                    rules += "\n";
                    continue;
                };
                let (lines, wrong) = parse_settings(&rules);
                assert!(wrong.is_empty(), "{wrong:?}");
                let (packet, want) = question.split_once(" => ").expect(line);
                let got = match decides(&lines, &asked(packet)) {
                    Some((at, action)) => {
                        format!(
                            "{} {}",
                            at + 1,
                            if action == Action::Allow { "allow" } else { "deny" }
                        )
                    }
                    None => "none".to_string(),
                };
                assert_eq!(got, want, "{rules}{line}");
                count += 1;
            }
        }
        assert!(count >= 30, "{count} questions");
    }

    #[test]
    fn a_rule_for_a_program_fits_when_the_program_is_known_and_might_when_not() {
        let (lines, _) = parse_settings("deny program /x\nallow\n");
        let mut packet = packet(Protocol::Tcp, 80, Some("203.0.113.9"), Some("net0"));
        assert_eq!(decides(&lines, &packet), Some((1, Action::Allow)));
        assert_eq!(passed_over(&lines, &packet), [0]);
        packet.program = Some("/x");
        assert_eq!(decides(&lines, &packet), Some((0, Action::Deny)));
        packet.program = Some("/y");
        assert_eq!(decides(&lines, &packet), Some((1, Action::Allow)));
        assert!(passed_over(&lines, &packet).is_empty());
    }

    #[test]
    fn options_end_the_words_once_each_and_a_change_s_seconds_are_spark_s() {
        let read =
            |words: &[&str]| options_after(words).map(|(rest, options)| (rest.join(" "), options));
        let tried = |seconds| Options { json: false, seconds: Some(seconds) };
        assert_eq!(read(&["list"]).unwrap(), ("list".into(), Options::default()));
        assert_eq!(
            read(&["list", "--json"]).unwrap(),
            ("list".into(), Options { json: true, seconds: None })
        );
        assert_eq!(read(&["allow", "tcp", "7", "--try", "30"]).unwrap().1, tried(30));
        assert_eq!(read(&["allow", "tcp", "7", "--try=600"]).unwrap().1, tried(600));
        assert_eq!(
            read(&["remove", "allow", "tcp", "7", "--try", "10"]).unwrap().0,
            "remove allow tcp 7"
        );
        // Both, in either order.
        let both = Options { json: true, seconds: Some(60) };
        assert_eq!(read(&["x", "--json", "--try", "60"]).unwrap().1, both);
        assert_eq!(read(&["x", "--try", "60", "--json"]).unwrap().1, both);
        for words in [
            &["allow", "tcp", "7", "--try", "5"][..],
            &["allow", "tcp", "7", "--try", "3601"],
            &["allow", "tcp", "7", "--try"],
            &["allow", "--try", "30", "tcp", "7"],
            &["list", "--json", "--json"],
            &["allow", "tcp", "7", "--try", "30", "--try", "40"],
            &["allow", "tcp", "7", "--quiet"],
        ] {
            assert!(read(words).unwrap_err().usage, "{words:?}");
        }
    }

    #[test]
    fn a_rule_is_found_by_its_number_or_as_a_rule_in_any_order_of_its_words() {
        let (lines, _) = parse_settings(
            "# the house's rules\nallow tcp 22 from 192.168.1.0/24\n\ndeny tcp 22\nallow tcp 22 from 192.168.1.0/24\n",
        );
        let rule = |text: &str| Rule::parse(&text.split_whitespace().collect::<Vec<_>>()).unwrap();
        // The first that is the same rule, at its place among the lines.
        let wanted = rule("allow from 192.168.1.0/24 22 tcp");
        assert_eq!(place_of(&lines, |_, held| *held == wanted).map(|(at, _)| at), Some(1));
        assert_eq!(place_of(&lines, |n, _| n == 2), Some((3, rule("deny tcp 22"))));
        assert_eq!(place_of(&lines, |n, _| n == 4), None);
        assert_eq!(place_of(&lines, |_, held| *held == rule("deny udp 22")), None);
    }

    #[test]
    fn a_rule_is_added_once_at_the_end_up_to_the_most_the_kernel_holds() {
        let (mut lines, _) = parse_settings("allow tcp 22\n# a comment\n");
        let rule = Rule::parse(&["deny", "tcp", "23"]).unwrap();
        assert_eq!(added(&mut lines, &rule).unwrap(), Some(2));
        assert_eq!(added(&mut lines, &rule).unwrap(), None);
        assert_eq!(text_of(&lines), "allow tcp 22\n# a comment\ndeny tcp 23\n");
        let mut full: Vec<Line> = (1..=RULES_MAX)
            .map(|port| Line::Rule(Rule::parse(&["deny", &port.to_string()]).unwrap()))
            .collect();
        let refused = added(&mut full, &Rule::parse(&["allow"]).unwrap()).unwrap_err();
        assert!(refused.message.contains("the most it holds"), "{}", refused.message);
    }

    #[test]
    fn a_trial_needs_rules_that_the_service_loaded() {
        for state in [State::Done, State::Running, State::Starting] {
            assert!(loaded(state), "{state:?}");
        }
        for state in
            [State::Stopped, State::Disabled, State::Failed, State::Pending, State::Waiting]
        {
            assert!(!loaded(state), "{state:?}");
        }
    }

    #[test]
    fn a_ping_fits_only_rules_for_every_port_and_none_for_a_program() {
        let (lines, _) = parse_settings(
            "deny 22\ndeny program /x\ndeny tcp\ndeny ping from 10.0.0.0/8\nallow ping\n",
        );
        let ping = |from| packet(Protocol::Ping, 0, Some(from), None);
        assert_eq!(decides(&lines, &ping("10.0.0.5")), Some((3, Action::Deny)));
        assert_eq!(decides(&lines, &ping("203.0.113.9")), Some((4, Action::Allow)));
        assert!(passed_over(&lines, &ping("203.0.113.9")).is_empty());
        let (lines, _) = parse_settings("deny\n");
        assert_eq!(decides(&lines, &ping("203.0.113.9")), Some((0, Action::Deny)));
    }

    #[test]
    fn an_address_is_in_a_network_of_its_own_version_only() {
        let within = |address: &str, network: &str, prefix| {
            within(address.parse().unwrap(), network.parse().unwrap(), prefix)
        };
        assert!(within("192.168.1.9", "192.168.1.0", 24));
        assert!(!within("192.168.2.9", "192.168.1.0", 24));
        assert!(within("10.1.2.3", "0.0.0.0", 0));
        assert!(within("192.168.1.9", "192.168.1.9", 32));
        assert!(!within("192.168.1.10", "192.168.1.9", 32));
        assert!(within("2001:db8::9", "2001:db8::", 32));
        assert!(!within("2001:db9::9", "2001:db8::", 32));
        assert!(within("fe80::1", "::", 0));
        assert!(!within("::ffff:192.168.1.9", "192.168.1.0", 24));
        assert!(!within("192.168.1.9", "::", 0));
    }

    #[test]
    fn a_listener_is_reached_by_the_first_rule_that_fits_it_and_the_number_says_which() {
        let listener = Listening {
            tcp: true,
            port: 8000,
            ipv4: true,
            ipv6: false,
            inbound: true,
            interface: 0,
            process: 7,
            program: Some((3, 9)),
        };
        let held = |text: &str| {
            decode_rule(&encode(
                &rule(text).unwrap(),
                Found { program: Some((3, 9)), interface: 0 },
            ))
        };
        let rules = [held("allow udp 8000"), held("deny tcp 9000"), held("deny tcp 8000-8100")];
        assert_eq!(decided(&listener, &rules), (Reach::Denied, Some(2)));
        assert_eq!(decided(&listener, &rules[..2]), (Reach::Service, None));
        assert_eq!(decided(&Listening { inbound: false, ..listener }, &[]), (Reach::Local, None));
        assert_eq!(
            decided(&listener, &[held("allow tcp 8000 from 10.0.0.0/8")]),
            (Reach::Some, Some(0))
        );
    }

    #[test]
    fn a_network_is_written_as_the_rule_has_it() {
        assert_eq!(network_text("192.168.1.0".parse().unwrap(), 24), "192.168.1.0/24");
        assert_eq!(network_text("192.168.1.9".parse().unwrap(), 32), "192.168.1.9");
        assert_eq!(network_text("2001:db8::".parse().unwrap(), 32), "2001:db8::/32");
        assert_eq!(network_text("2001:db8::9".parse().unwrap(), 128), "2001:db8::9");
    }

    #[test]
    fn wrong_rules_say_why() {
        for (text, why) in [
            ("permit tcp 22", "starts with allow or deny"),
            ("allow tcp 0", "not a port"),
            ("allow tcp 90-80", "not a port"),
            ("allow tcp 22 from 10.0.0.0/33", "not an address"),
            ("allow ping 22", "no port"),
            ("allow program bin/server", "absolute path"),
            ("allow tcp 22 on", "needs an interface"),
            ("allow tcp 22 udp", "not a word"),
            ("allow on net0/x", "not an interface"),
        ] {
            let err = rule(text).unwrap_err();
            assert!(err.contains(why), "{text}: {err}");
        }
    }

    #[test]
    fn the_settings_file_keeps_its_comments() {
        let text = "# The rules.\nallow tcp 22   # ssh\n\nnonsense here\ndeny udp 5353\n";
        let (lines, wrong) = parse_settings(text);
        assert_eq!(rules(&lines).count(), 2);
        assert_eq!(wrong.len(), 1);
        assert!(wrong[0].contains("line 4"), "{wrong:?}");
        assert_eq!(text_of(&lines), "# The rules.\nallow tcp 22\n\nnonsense here\ndeny udp 5353\n");
    }

    #[test]
    fn rules_are_encoded_as_the_kernel_reads_them() {
        let found = Found { program: Some((3, 77)), interface: 2 };
        let bytes = encode(&rule("deny tcp 8000-8100 from 10.0.0.0/8 on net0").unwrap(), found);
        assert_eq!(u32_at(&bytes, AbiRule::OFFSET_ACTION), AbiAction::Deny.raw());
        assert_eq!(u32_at(&bytes, AbiRule::OFFSET_PROTOCOL), AbiMatch::Tcp.raw());
        assert_eq!(u16_at(&bytes, AbiRule::OFFSET_FIRST_PORT), 8000);
        assert_eq!(u16_at(&bytes, AbiRule::OFFSET_LAST_PORT), 8100);
        assert_eq!(u32_at(&bytes, AbiRule::OFFSET_FAMILY), 4);
        assert_eq!(bytes[AbiRule::OFFSET_FROM..AbiRule::OFFSET_FROM + 4], [10, 0, 0, 0]);
        assert_eq!(bytes[AbiRule::OFFSET_PREFIX], 8);
        assert_eq!(u32_at(&bytes, AbiRule::OFFSET_INTERFACE), 2);
        let held = decode_rule(&bytes);
        assert_eq!(
            held,
            Held {
                allow: false,
                protocol: Protocol::Tcp,
                ports: (8000, 8100),
                narrowed: true,
                program: Some((3, 77))
            }
        );
        let all = encode(&rule("allow").unwrap(), Found::default());
        assert_eq!(decode_rule(&all).ports, (0, u16::MAX));
    }

    #[test]
    fn who_reaches_a_listener() {
        let server = Listening {
            tcp: true,
            port: 8000,
            ipv4: true,
            ipv6: true,
            inbound: false,
            interface: 0,
            process: 77,
            program: Some((3, 9)),
        };
        let sshd =
            Listening { port: 22, inbound: true, process: 41, program: Some((1, 5)), ..server };
        let held = |text: &str, program| {
            decode_rule(&encode(&rule(text).unwrap(), Found { program, interface: 0 }))
        };
        assert_eq!(reach(&server, &[]), Reach::Local);
        assert_eq!(reach(&sshd, &[]), Reach::Service);
        assert_eq!(reach(&server, &[held("allow program /x", Some((3, 9)))]), Reach::Allowed);
        assert_eq!(reach(&server, &[held("allow program /y", Some((3, 10)))]), Reach::Local);
        assert_eq!(reach(&sshd, &[held("deny tcp 22", None)]), Reach::Denied);
        assert_eq!(reach(&sshd, &[held("allow tcp 22 from 10.0.0.0/8", None)]), Reach::Some);
        assert_eq!(reach(&sshd, &[held("allow udp 22", None)]), Reach::Service);
        assert_eq!(reach(&sshd, &[held("deny ping", None)]), Reach::Service);
    }

    /// Times in UTC, so that the tests do not depend on the time zone of
    /// the machine they run on.
    fn utc(seconds: i64, format: &str) -> String {
        util::time_text(seconds, format, true)
    }

    /// 2026-11-14 14:00:00 UTC.
    const NOW: i64 = 1_794_664_800;

    fn entry(address: &str, until: i64, times: u32, why: &str) -> BlockedEntry {
        BlockedEntry { address: address.into(), until, times, why: why.into() }
    }

    #[test]
    fn the_blocked_table_lines_up_its_columns_and_ends_with_the_reason() {
        let entries = [
            // 14:32 today.
            entry("203.0.113.9", NOW + 32 * 60, 2, "5 wrong passwords over SSH in 2 minutes"),
            // 14:05 today.
            entry("2001:db8:4:7::/64", NOW + 5 * 60, 1, "5 refused SSH sign-ins in 4 minutes"),
        ];
        assert_eq!(
            blocked_table(&entries, NOW, utc),
            "Blocked            Until  In a row  Why\n\
             203.0.113.9        14:32  2         5 wrong passwords over SSH in 2 minutes\n\
             2001:db8:4:7::/64  14:05  1         5 refused SSH sign-ins in 4 minutes\n"
        );
    }

    #[test]
    fn a_block_that_ends_another_day_says_the_date() {
        let entries = [
            entry("203.0.113.9", NOW + 86_400, 3, "why"),
            entry("198.51.100.4", NOW + 600, 1, "why"),
        ];
        assert_eq!(
            blocked_table(&entries, NOW, utc),
            "Blocked       Until             In a row  Why\n\
             203.0.113.9   2026-11-15 14:00  3         why\n\
             198.51.100.4  14:10             1         why\n"
        );
    }

    #[test]
    fn today_is_the_day_the_clock_shows_not_the_next_twenty_four_hours() {
        // 23:50 to 00:10 is twenty minutes, and the next day.
        let (late, after_midnight) = (1_794_700_200, 1_794_701_400);
        assert_eq!(until_text(late + 300, late, &utc), "23:55");
        assert_eq!(until_text(after_midnight, late, &utc), "2026-11-15 00:10");
        // A block that ended a moment ago is told as the time of day too.
        assert_eq!(until_text(late - 600, late, &utc), "23:40");
    }

    #[test]
    fn nothing_blocked_is_one_plain_line() {
        assert_eq!(blocked_table(&[], NOW, utc), "No address is blocked.\n");
    }

    #[test]
    fn authds_answer_is_read_into_the_table() {
        let answer = BlockedEntry::list(&[
            entry("203.0.113.9", NOW + 60, 1, "5 wrong passwords over SSH in 1 minute, for root"),
            entry("198.51.100.4", NOW + 120, 2, "5 wrong sharing passwords in 3 minutes"),
        ]);
        let table = blocked_table(&BlockedEntry::parse_list(&answer), NOW, utc);
        assert_eq!(table.lines().count(), 3);
        assert!(
            table.contains("14:01  1         5 wrong passwords over SSH in 1 minute, for root")
        );
        assert!(table.contains("14:02  2         5 wrong sharing passwords in 3 minutes"));
        // An answer with no blocks, as authd gives for none, is none.
        assert_eq!(
            blocked_table(&BlockedEntry::parse_list(""), NOW, utc),
            "No address is blocked.\n"
        );
    }

    fn parsed(text: &str) -> Result<Blocks, String> {
        Blocks::parse(&text.split_whitespace().collect::<Vec<_>>())
    }

    #[test]
    fn unblock_takes_an_address_or_the_network_of_an_ipv6_one() {
        for (text, address) in [
            ("unblock 203.0.113.9", "203.0.113.9"),
            ("unblock 203.0.113.9/32", "203.0.113.9"),
            // An IPv6 address is blocked as its /64, so that is what is lifted.
            ("unblock 2001:db8:4:7::5", "2001:db8:4:7::/64"),
            ("unblock 2001:db8:4:7::/64", "2001:db8:4:7::/64"),
            // An IPv4 address written as IPv6 is the IPv4 one.
            ("unblock ::ffff:203.0.113.9", "203.0.113.9"),
        ] {
            let command = parsed(text).unwrap();
            assert_eq!(command.request(), Request::Unblock { address: address.into() }, "{text}");
            assert_eq!(
                command.done("").unwrap(),
                format!("firewall: {address} is no longer blocked")
            );
        }
    }

    #[test]
    fn unblock_refuses_what_is_no_address_that_is_blocked() {
        for text in [
            "unblock example.org",
            "unblock 203.0.113.0/24",
            "unblock 2001:db8::/48",
            "unblock 127.0.0.1",
            "unblock ::1",
            "unblock 0.0.0.0",
            "unblock 203.0.113.9/",
        ] {
            let why = parsed(text).unwrap_err();
            assert!(why.contains("is not an address that is blocked"), "{text}: {why}");
        }
        assert!(parsed("unblock").unwrap_err().contains("takes one address"));
        assert!(parsed("unblock 203.0.113.9 203.0.113.10").unwrap_err().contains("one address"));
    }

    #[test]
    fn never_block_takes_a_network_and_remove_gives_it_back() {
        let command = parsed("never-block 192.168.1.0/24").unwrap();
        assert_eq!(
            command.request(),
            Request::NeverBlock { network: "192.168.1.0/24".into(), remove: false }
        );
        assert_eq!(
            command.done(request::NEVER_CHANGED).unwrap(),
            "firewall: 192.168.1.0/24 is never blocked"
        );
        // A second time, which authd finds as asked, is no failure.
        assert_eq!(
            command.done(request::NEVER_AS_IT_WAS).unwrap(),
            "firewall: 192.168.1.0/24 is never blocked already"
        );
        let command = parsed("never-block remove 192.168.1.0/24").unwrap();
        assert_eq!(
            command.request(),
            Request::NeverBlock { network: "192.168.1.0/24".into(), remove: true }
        );
        assert_eq!(
            command.done(request::NEVER_CHANGED).unwrap(),
            "firewall: 192.168.1.0/24 may be blocked again"
        );
        assert_eq!(
            command.done(request::NEVER_AS_IT_WAS).unwrap(),
            "firewall: 192.168.1.0/24 is not in never, and may be blocked"
        );
        // One address is a network of its own, and is written as it was.
        assert_eq!(
            parsed("never-block 198.51.100.4").unwrap().request(),
            Request::NeverBlock { network: "198.51.100.4".into(), remove: false }
        );
        assert_eq!(
            parsed("never-block 2001:db8::/48").unwrap().request(),
            Request::NeverBlock { network: "2001:db8::/48".into(), remove: false }
        );
    }

    #[test]
    fn never_block_says_what_is_wrong_with_its_network() {
        let why = parsed("never-block 192.168.1.5/24").unwrap_err();
        assert!(why.contains("its network is 192.168.1.0/24"), "{why}");
        let why = parsed("never-block office").unwrap_err();
        assert!(why.contains("is not a network"), "{why}");
        // A lone `remove` is the command without its network, not a network.
        assert!(parsed("never-block remove").unwrap_err().contains("takes a network"));
        assert!(parsed("never-block").unwrap_err().contains("takes a network"));
        assert!(
            parsed("never-block 10.0.0.0/8 10.1.0.0/16").unwrap_err().contains("takes a network")
        );
        assert!(
            parsed("never-block remove 10.0.0.0/8 more").unwrap_err().contains("takes a network")
        );
    }

    #[test]
    fn blocked_asks_for_the_list_and_nothing_more() {
        let command = parsed("blocked").unwrap();
        assert_eq!(command.request(), Request::Blocked);
        assert_eq!(command.done(""), None);
        assert!(parsed("blocked now").unwrap_err().contains("takes nothing more"));
    }
}
