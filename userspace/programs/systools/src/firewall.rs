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

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::net::IpAddr;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use radix_abi::Status;
use radix_abi::errors::Kernel;
use radix_abi::firewall::{
    self as abi, Action as AbiAction, Listener as AbiListener, ListenerFlags, Match as AbiMatch,
    Rule as AbiRule,
};
use radix_abi::startup::SLOT_FIREWALL;

use crate::args::{Arg, Args};
use crate::ip;
use crate::sys::SYS;
use crate::util::{Fail, fail, fail_with, usage};

/// The rules, a rule a line.
pub const SETTINGS: &str = "/system/settings/firewall";

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
            let most = if address.is_ipv4() { 32 } else { 128 };
            if prefix == most {
                write!(f, " from {address}")?;
            } else {
                write!(f, " from {address}/{prefix}")?;
            }
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

/// The settings file's lines, and what is wrong with those that are not
/// rules or comments, with their numbers.
fn parse_settings(text: &str) -> (Vec<Line>, Vec<String>) {
    let mut lines = Vec::new();
    let mut wrong = Vec::new();
    for (at, line) in text.lines().enumerate() {
        let words: Vec<&str> = line.split('#').next().unwrap_or("").split_whitespace().collect();
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
    for rule in rules {
        let protocol = match rule.protocol {
            Protocol::Any => true,
            Protocol::Tcp => listener.tcp,
            Protocol::Udp => !listener.tcp,
            Protocol::Ping => false,
        };
        let port = (rule.ports.0..=rule.ports.1).contains(&listener.port);
        let program = rule.program.is_none_or(|program| listener.program == Some(program));
        if protocol && port && program {
            return match (rule.narrowed, rule.allow) {
                (true, _) => Reach::Some,
                (false, true) => Reach::Allowed,
                (false, false) => Reach::Denied,
            };
        }
    }
    if listener.inbound { Reach::Service } else { Reach::Local }
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
        ) => fail_with(
            2,
            "firewall: this needs the firewall capability: run it with elevate(8)".into(),
        ),
        _ => fail_with(2, format!("firewall: {}", crate::sys::status_name(status.0))),
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

const HELP: &str = "Usage: firewall [status]
       firewall list
       firewall allow|deny [tcp|udp|ping|any] [PORT[-PORT]] [from NETWORK] [on INTERFACE]
                           [program PATH]
       firewall allow process NUMBER
       firewall remove NUMBER
       firewall load
";

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
    match words.split_first() {
        None | Some((&"status", [])) => status(&mut out),
        Some((&"list", [])) => list(&mut out),
        Some((&"load", [])) => load(&mut out, true),
        Some((&"help", _)) => {
            write!(out, "{HELP}")?;
            Ok(0)
        }
        Some((&"remove", [number])) => remove(number, &mut out),
        Some((&"allow", ["process", number])) => allow_process(number, &mut out),
        Some((&("allow" | "deny"), _)) => {
            let rule = Rule::parse(&words).map_err(|why| usage(format!("firewall: {why}")))?;
            add(&rule, &mut out)
        }
        Some((other, _)) => Err(usage(format!(
            "firewall: no command '{other}': status, list, allow, deny, remove or load"
        ))),
    }
}

/// The settings file's lines.
fn read_settings() -> Result<(Vec<Line>, Vec<String>), Fail> {
    match fs::read_to_string(SETTINGS) {
        Ok(text) => Ok(parse_settings(&text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok((Vec::new(), Vec::new())),
        Err(e) => Err(crate::util::failed(&format!("firewall: {SETTINGS}"), e)),
    }
}

/// Writes `lines` into the settings file, whole.
fn write_settings(lines: &[Line]) -> Result<(), Fail> {
    let new = format!("{SETTINGS}.new");
    fs::write(&new, text_of(lines))
        .and_then(|()| fs::rename(&new, SETTINGS))
        .map_err(|e| crate::util::failed(&format!("firewall: {SETTINGS}"), e))
}

/// `firewall load`: gives the kernel the settings file's rules, those it
/// can find what they name for.
fn load(out: &mut dyn Write, say: bool) -> Result<i32, Fail> {
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

/// `firewall allow` and `deny`: adds `rule` at the end, and loads.
fn add(rule: &Rule, out: &mut dyn Write) -> Result<i32, Fail> {
    find(rule).map_err(|why| fail(format!("firewall: {why}")))?;
    let (mut lines, _) = read_settings()?;
    if rules(&lines).any(|held| held == rule) {
        writeln!(out, "firewall: `{rule}` is a rule already")?;
        return Ok(0);
    }
    if rules(&lines).count() >= RULES_MAX {
        return Err(fail(format!("firewall: there are {RULES_MAX} rules, the most it holds")));
    }
    lines.push(Line::Rule(rule.clone()));
    write_settings(&lines)?;
    load(out, false)?;
    writeln!(out, "firewall: rule {} is `{rule}`", rules(&lines).count())?;
    Ok(0)
}

/// `firewall remove N`.
fn remove(number: &str, out: &mut dyn Write) -> Result<i32, Fail> {
    let number: usize = number
        .parse()
        .ok()
        .filter(|&n| n >= 1)
        .ok_or_else(|| usage(format!("firewall: `{number}` is not a rule's number")))?;
    let (mut lines, _) = read_settings()?;
    let at = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| matches!(line, Line::Rule(_)))
        .nth(number - 1)
        .map(|(at, _)| at)
        .ok_or_else(|| fail(format!("firewall: there is no rule {number}")))?;
    let gone = lines.remove(at);
    write_settings(&lines)?;
    load(out, false)?;
    if let Line::Rule(rule) = gone {
        writeln!(out, "firewall: `{rule}` is gone")?;
    }
    Ok(0)
}

/// `firewall allow process N`: allows from other machines the program
/// process N runs, by its path.
fn allow_process(number: &str, out: &mut dyn Write) -> Result<i32, Fail> {
    let process: u64 = number
        .parse()
        .map_err(|_| usage(format!("firewall: `{number}` is not a process number")))?;
    let listeners = kernel_listeners().map_err(refused)?;
    let listener = listeners
        .iter()
        .find(|listener| listener.process == process)
        .ok_or_else(|| fail(format!("firewall: process {process} listens on nothing")))?;
    let path = program_path(process, listener.program)
        .filter(|_| listener.program.is_some())
        .ok_or_else(|| fail(format!("firewall: the program of process {process} is not found")))?;
    let rule = Rule {
        action: Action::Allow,
        protocol: Protocol::Any,
        ports: None,
        from: None,
        on: None,
        program: Some(path),
    };
    add(&rule, out)
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
}
