// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What firewall(8) tells the web console (ADR 0518): the rules as JSON,
//! which rule decides a connection, the listening sockets with who reaches
//! each, what the kernel dropped, and the addresses authd has blocked.
//!
//! `list --json` reads the settings file and needs no right, as `list`
//! does; `test` reads it too. `status --json` reads the kernel's rules and
//! listeners, which needs the capability's read right, so it runs under
//! elevate(8), as `status` does. `dropped` reads the kernel's records from
//! logd(8), which gives them to root alone (ADR 0252). `blocked --json`
//! asks authd, which answers an administrator's command inside elevate(8)
//! alone, and reads blocking.toml(5), which anyone may.
//!
//! The JSON's fields and their order are the interface: the structs below
//! are in the order the fields are written, and the tests name them all.
//!
//! The kernel logs what it drops in its own words (radix `net/firewall.rs`
//! `log_dropped`): `Firewall: dropped a TCP connection from ADDRESS:PORT to
//! port N on CARD`, with `a UDP datagram` for UDP, `a ping from ADDRESS on
//! CARD` for a ping, and, when a socket listens for it that has no right to
//! be reached, `: process N listens for this machine alone` after the card.
//! It logs at most five a second, then one line `dropped N more packets`.
//! [`parse_drop`] reads the first three.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::net::IpAddr;

use serde::Serialize;

use super::{
    Action, Held, Line, Listening, Packet, Protocol, Reach, Rule, SETTINGS, TRIAL, decided,
    decides, kernel_listeners, kernel_rules, name_ok, network_text, parse_settings, passed_over,
    program_path, refused, rules, words_of,
};
use accounts::request::BlockedEntry;

use crate::authd::blocking::{self, Settings};
use crate::log::parse_time;
use crate::logd::{self, Record, Request};
use crate::util::{self, Fail, fail, usage};

/// The most records of the kernel's drops `dropped` reads: the newest. The
/// kernel logs up to five a second, so a week of an attack would be three
/// million; a group of the same address, port and card is one entry, but
/// each address that knocks once is another.
const RECORDS_MAX: usize = 100_000;

/// The time `dropped` reads back to unless told.
const SINCE_DEFAULT: &str = "-24h";

/// The `firewall` trial, when one runs.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Trial {
    /// How long until it is undone unless it is kept.
    left_ms: u64,
}

/// What `firewall list --json` writes.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Listed {
    /// The rules of the file, numbered as `firewall remove` takes them.
    rules: Vec<Shown>,
    /// The lines that are neither a rule nor a comment.
    problems: Vec<Problem>,
    trial: Option<Trial>,
}

/// One rule.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Shown {
    /// Its place among the rules, from 1.
    number: usize,
    /// The rule as `firewall list` writes it.
    text: String,
    /// `allow` or `deny`.
    action: &'static str,
    /// `tcp`, `udp`, `ping` or `any`.
    protocol: &'static str,
    /// The first and the last port; null for every one.
    ports: Option<[u16; 2]>,
    /// The network, as the rule writes it; null for anywhere.
    from: Option<String>,
    /// The interface it comes by; null for any.
    on: Option<String>,
    /// The program that listens for it; null for any.
    program: Option<String>,
}

/// A line of the file that is no rule.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Problem {
    /// Its number in the file, from 1.
    line: usize,
    /// The line, as the file has it.
    text: String,
    /// What is wrong with it, as `firewall load` says.
    why: String,
}

/// `allow` or `deny`.
fn action_name(action: Action) -> &'static str {
    match action {
        Action::Allow => "allow",
        Action::Deny => "deny",
    }
}

/// `tcp`, `udp`, `ping` or `any`.
fn protocol_name(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Any => "any",
        Protocol::Tcp => "tcp",
        Protocol::Udp => "udp",
        Protocol::Ping => "ping",
    }
}

/// The rules of the file `text`, with `trial`, the time left of the
/// `firewall` trial: the file's lines that are no rule, each with why.
fn listed(text: &str, trial: Option<u64>) -> Listed {
    let (lines, _) = parse_settings(text);
    let rules = rules(&lines)
        .enumerate()
        .map(|(at, rule)| Shown {
            number: at + 1,
            text: rule.to_string(),
            action: action_name(rule.action),
            protocol: protocol_name(rule.protocol),
            ports: rule.ports.map(|(first, last)| [first, last]),
            from: rule.from.map(|(address, prefix)| network_text(address, prefix)),
            on: rule.on.clone(),
            program: rule.program.clone(),
        })
        .collect();
    let mut problems = Vec::new();
    for (at, line) in text.lines().enumerate() {
        let words = words_of(line);
        if words.is_empty() {
            continue;
        }
        if let Err(why) = Rule::parse(&words) {
            problems.push(Problem { line: at + 1, text: line.to_string(), why });
        }
    }
    Listed { rules, problems, trial: trial.map(|left_ms| Trial { left_ms }) }
}

/// The settings file, whole: none is no rules.
fn read_file() -> Result<String, Fail> {
    match fs::read_to_string(SETTINGS) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(util::failed(&SETTINGS, error)),
    }
}

/// Writes `value` as a line of JSON.
fn say(out: &mut dyn Write, value: &impl Serialize) -> Result<i32, Fail> {
    let line = serde_json::to_string(value).map_err(|error| fail(error.to_string()))?;
    writeln!(out, "{line}")?;
    Ok(0)
}

/// An address that is blocked now.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Block {
    /// The address, or the IPv6 network of 64 bits it is blocked as.
    address: String,
    /// When the block ends, in Unix milliseconds.
    until_ms: i64,
    /// How many blocks of it came one after another, each within a day of
    /// the end of the one before, this one counted.
    in_a_row: u32,
    /// Why it was blocked, in authd's words.
    why: String,
}

/// What `firewall blocked --json` writes: blocking.toml(5)'s settings and
/// the blocks. The settings are null when the file does not read, since
/// authd then goes on with the settings it read last, which this cannot
/// know; `wrong` says why.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Blocked {
    /// The layout's number, 1.
    format: u32,
    /// Whether addresses are blocked at all.
    on: Option<bool>,
    /// How many wrong passwords block an address.
    tries: Option<u32>,
    /// The seconds they must come within.
    within_s: Option<u64>,
    /// How long the first block lasts, in seconds.
    first_s: Option<u64>,
    /// The longest a block lasts, in seconds.
    longest_s: Option<u64>,
    /// The networks that are never blocked.
    never: Option<Vec<String>>,
    /// The blocks in force, in authd's order.
    blocks: Vec<Block>,
    /// Why the settings are null, or null.
    wrong: Option<String>,
}

/// The report of `entries`, authd's blocks, and of `settings`, what
/// blocking.toml(5) says or why it does not read.
fn blocked_of(entries: &[BlockedEntry], settings: Result<Settings, String>) -> Blocked {
    let blocks = entries
        .iter()
        .map(|entry| Block {
            address: entry.address.clone(),
            until_ms: entry.until.saturating_mul(1000),
            in_a_row: entry.times,
            why: entry.why.clone(),
        })
        .collect();
    let (settings, wrong) = match settings {
        Ok(settings) => (Some(settings), None),
        Err(wrong) => (None, Some(wrong)),
    };
    Blocked {
        format: 1,
        on: settings.as_ref().map(|settings| settings.on),
        tries: settings.as_ref().map(|settings| settings.tries),
        within_s: settings.as_ref().map(|settings| settings.within),
        first_s: settings.as_ref().map(|settings| settings.first),
        longest_s: settings.as_ref().map(|settings| settings.longest),
        never: settings
            .as_ref()
            .map(|settings| settings.never.iter().map(ToString::to_string).collect()),
        blocks,
        wrong,
    }
}

/// What blocking.toml(5) says: the defaults for a file that is not there,
/// and why it does not read for one that does not.
fn blocking_settings() -> Result<Settings, String> {
    let path = blocking::SETTINGS;
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("{path}: {}", util::strip_os_error(&error))),
    };
    Settings::parse(&text).map_err(|why| format!("{path}: {why}"))
}

/// `firewall blocked --json`, given authd's list of `entries`.
pub fn blocked(entries: &[BlockedEntry], out: &mut dyn Write) -> Result<i32, Fail> {
    say(out, &blocked_of(entries, blocking_settings()))
}

/// `firewall list --json`.
pub fn list(out: &mut dyn Write) -> Result<i32, Fail> {
    // Spark that does not answer leaves the trial out: the rules are the
    // file's.
    let trial = crate::service::trial_left_ms(TRIAL);
    say(out, &listed(&read_file()?, trial))
}

/// Who reaches a listener, in the words of the JSON.
fn reach_name(reach: Reach) -> &'static str {
    match reach {
        Reach::Allowed => "everyone",
        Reach::Some => "some",
        Reach::Service => "service",
        Reach::Local => "local",
        Reach::Denied => "denied",
    }
}

/// What `firewall status --json` writes.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Status {
    /// The sockets that listen, TCP before UDP, by port.
    listening: Vec<Listens>,
}

/// A socket that listens.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Listens {
    port: u16,
    tcp: bool,
    /// Its program's path; null when it is not found.
    program: Option<String>,
    process: u64,
    /// Who reaches it from other machines: `everyone` (a rule allows it),
    /// `some` (a rule allows some machines or interfaces), `service` (its
    /// service's file), `local` (this machine alone) or `denied` (a rule
    /// denies it).
    reach: &'static str,
    /// The number, from 1, of the rule that says so; null when none does.
    rule: Option<usize>,
}

/// The sockets of `listeners`, with who reaches each by the kernel's
/// `held` rules. `file_rules` is how many rules the settings file has: the
/// kernel leaves out a rule whose program or interface it cannot find, so
/// its numbers are the file's only while the counts agree. `program_of`
/// finds a listener's program's path.
fn status_of(
    listeners: &[Listening],
    held: &[Held],
    file_rules: Option<usize>,
    program_of: impl Fn(&Listening) -> Option<String>,
) -> Status {
    let mut sockets = listeners.to_vec();
    sockets.sort_by_key(|l| (!l.tcp, l.port, l.process));
    // One socket for both versions of IP is one entry.
    sockets.dedup_by_key(|l| (l.tcp, l.port, l.process));
    let numbered = file_rules == Some(held.len());
    let listening = sockets
        .iter()
        .map(|listener| {
            let (reach, rule) = decided(listener, held);
            Listens {
                port: listener.port,
                tcp: listener.tcp,
                program: program_of(listener),
                process: listener.process,
                reach: reach_name(reach),
                rule: rule.filter(|_| numbered).map(|at| at + 1),
            }
        })
        .collect();
    Status { listening }
}

/// `firewall status --json`.
pub fn status(out: &mut dyn Write) -> Result<i32, Fail> {
    let held = kernel_rules().map_err(refused)?;
    let listeners = kernel_listeners().map_err(refused)?;
    let file_rules = fs::read_to_string(SETTINGS).ok().map(|text| {
        let (lines, _) = parse_settings(&text);
        rules(&lines).count()
    });
    let status = status_of(&listeners, &held, file_rules, |listener| {
        program_path(listener.process, listener.program)
    });
    say(out, &status)
}

/// A connection to ask about, and how to answer.
#[derive(Debug, PartialEq, Eq)]
struct Question {
    protocol: Protocol,
    port: u16,
    from: IpAddr,
    on: Option<String>,
    json: bool,
}

/// What `firewall test` was asked: `tcp|udp PORT from ADDRESS [on CARD]`
/// and `--json`, which may come anywhere among them.
fn question(words: &[&str]) -> Result<Question, Fail> {
    const FORM: &str = "test takes tcp or udp, a port, from and an address, and on and a card if \
                        the connection comes by one: firewall test tcp 22 from 192.168.1.20 on net0";
    let mut json = false;
    let mut plain: Vec<&str> = Vec::new();
    for &word in words {
        match word {
            "--json" => json = true,
            word if word.starts_with('-') => {
                return Err(usage(format!("test has no option {word}: {FORM}")));
            }
            word => plain.push(word),
        }
    }
    let (protocol, port, from, on) = match plain[..] {
        [protocol, port, "from", from] => (protocol, port, from, None),
        [protocol, port, "from", from, "on", card] => (protocol, port, from, Some(card)),
        _ => return Err(usage(FORM.to_string())),
    };
    let protocol = match protocol {
        "tcp" => Protocol::Tcp,
        "udp" => Protocol::Udp,
        other => return Err(usage(format!("`{other}` is not tcp or udp: {FORM}"))),
    };
    let port = port
        .parse::<u16>()
        .ok()
        .filter(|&port| port >= 1)
        .ok_or_else(|| usage(format!("`{port}` is not a port, 1 to 65535")))?;
    let from = from
        .parse::<IpAddr>()
        .map_err(|_| usage(format!("`{from}` is not an address: a network is not one here")))?;
    if let Some(card) = on
        && !name_ok(card)
    {
        return Err(usage(format!("`{card}` is not an interface's name")));
    }
    Ok(Question { protocol, port, from, on: on.map(str::to_string), json })
}

/// What `firewall test --json` writes.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Tested {
    /// The number, from 1, of the rule that decides; null when none does.
    rule: Option<usize>,
    /// That rule as `firewall list` writes it.
    text: Option<String>,
    /// Whether it lets the connection in; null when no rule decides.
    allowed: Option<bool>,
    /// The numbers, from 1, of the rules before it that might apply
    /// instead: they name a program, which `test` does not know listens, or
    /// an interface when none was given.
    passed: Vec<usize>,
}

/// Which rule of `lines` decides what `question` asks.
fn tested(lines: &[Line], question: &Question) -> Tested {
    let packet = Packet {
        protocol: question.protocol,
        port: question.port,
        from: Some(question.from),
        on: question.on.as_deref(),
        program: None,
    };
    let passed = passed_over(lines, &packet).into_iter().map(|at| at + 1).collect();
    match decides(lines, &packet) {
        Some((at, action)) => Tested {
            rule: Some(at + 1),
            text: rules(lines).nth(at).map(ToString::to_string),
            allowed: Some(action == Action::Allow),
            passed,
        },
        None => Tested { rule: None, text: None, allowed: None, passed },
    }
}

/// `tested` in words.
fn tested_text(tested: &Tested, lines: &[Line], question: &Question) -> String {
    let mut text = match (tested.rule, tested.allowed, &tested.text) {
        (Some(rule), Some(allowed), Some(words)) => {
            format!("Rule {rule} {} it: {words}\n", if allowed { "allows" } else { "denies" })
        }
        _ => format!(
            "No rule decides it: it reaches port {} only while a service listens there with the \
             right to be reached.\n",
            question.port
        ),
    };
    if !tested.passed.is_empty() {
        let named: Vec<String> = tested
            .passed
            .iter()
            .filter_map(|&number| {
                rules(lines).nth(number - 1).map(|rule| format!("{number} ({rule})"))
            })
            .collect();
        text += &format!(
            "Passed over, as they name what test does not know: rule {}\n",
            named.join(", rule ")
        );
    }
    text
}

/// `firewall test tcp|udp PORT from ADDRESS [on CARD] [--json]`: which rule
/// of the file decides a new connection.
pub fn test(words: &[&str], out: &mut dyn Write) -> Result<i32, Fail> {
    let question = question(words)?;
    let (lines, _) = parse_settings(&read_file()?);
    let answer = tested(&lines, &question);
    if question.json {
        return say(out, &answer);
    }
    write!(out, "{}", tested_text(&answer, &lines, &question))?;
    Ok(0)
}

/// A packet the kernel logged as dropped.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Knock {
    protocol: &'static str,
    /// The address it came from, without its port.
    from: String,
    /// The port it went to: none for a ping.
    port: Option<u16>,
    card: String,
    /// What the kernel says after the card, if it does.
    why: Option<String>,
}

/// The drop a line of the kernel's log says, if it says one: not the line
/// that counts those it did not log.
fn parse_drop(text: &str) -> Option<Knock> {
    let rest = text.strip_prefix("Firewall: dropped ")?;
    let (protocol, rest) = [
        ("tcp", "a TCP connection from "),
        ("udp", "a UDP datagram from "),
        ("ping", "a ping from "),
    ]
    .into_iter()
    .find_map(|(protocol, words)| rest.strip_prefix(words).map(|rest| (protocol, rest)))?;
    let (source, rest) = rest.split_once(' ')?;
    // A connection's source ends with its port, as `[2001:db8::9]:51234`
    // or `203.0.113.9:51234`; a ping's is the address alone.
    let address = if protocol == "ping" {
        source
    } else if let Some(inside) = source.strip_prefix('[') {
        inside.split_once("]:")?.0
    } else {
        source.rsplit_once(':')?.0
    };
    let from = address.parse::<IpAddr>().ok()?.to_string();
    let (port, rest) = if protocol == "ping" {
        (None, rest)
    } else {
        let (port, rest) = rest.strip_prefix("to port ")?.split_once(' ')?;
        (Some(port.parse::<u16>().ok()?), rest)
    };
    let rest = rest.strip_prefix("on ")?;
    let (card, why) = match rest.split_once(": ") {
        Some((card, why)) => (card, Some(why.to_string())),
        None => (rest, None),
    };
    (!card.is_empty()).then(|| Knock { protocol, from, port, card: card.to_string(), why })
}

/// What `firewall dropped --json` writes.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Dropped {
    /// The start of what the groups cover, in milliseconds since 1970:
    /// the time asked for, or the first record read when there were more
    /// than [`RECORDS_MAX`].
    since_ms: u64,
    /// The most drops first.
    groups: Vec<Group>,
}

/// The drops of one address, protocol, port and card, for one reason.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Group {
    from: String,
    protocol: &'static str,
    /// Null for a ping.
    port: Option<u16>,
    card: String,
    /// What the kernel said after the card, such as `process 312 listens
    /// for this machine alone`; null when it said nothing more.
    why: Option<String>,
    /// How many lines the kernel logged: it logs at most five a second, so
    /// more packets were dropped than counted.
    count: u64,
    first_ms: u64,
    last_ms: u64,
}

/// How many drops of a kind, and when the first and the last were logged.
struct Span {
    count: u64,
    first_ms: u64,
    last_ms: u64,
}

/// The drops being counted.
#[derive(Default)]
struct Tally {
    /// The count of each kind of drop.
    groups: BTreeMap<Knock, Span>,
    /// The records read, and the time of the first.
    read: usize,
    first_ms: Option<u64>,
}

impl Tally {
    /// Counts the record made at `time_ms`, in milliseconds since 1970,
    /// which says `text`.
    fn add(&mut self, time_ms: u64, text: &str) {
        self.read += 1;
        self.first_ms = Some(self.first_ms.map_or(time_ms, |first| first.min(time_ms)));
        let Some(knock) = parse_drop(text) else { return };
        let span = self.groups.entry(knock).or_insert(Span {
            count: 0,
            first_ms: time_ms,
            last_ms: time_ms,
        });
        span.count += 1;
        span.first_ms = span.first_ms.min(time_ms);
        span.last_ms = span.last_ms.max(time_ms);
    }

    /// The groups, the most counted first, then the latest, for the
    /// records since `since_ms`.
    fn finish(self, since_ms: u64) -> Dropped {
        // The newest `RECORDS_MAX` are what logd gave: those before the
        // first are not counted.
        let since_ms = match self.first_ms {
            Some(first) if self.read >= RECORDS_MAX => first.max(since_ms),
            _ => since_ms,
        };
        let mut groups: Vec<Group> = self
            .groups
            .into_iter()
            .map(|(knock, span)| Group {
                from: knock.from,
                protocol: knock.protocol,
                port: knock.port,
                card: knock.card,
                why: knock.why,
                count: span.count,
                first_ms: span.first_ms,
                last_ms: span.last_ms,
            })
            .collect();
        groups.sort_by_key(|group| {
            (std::cmp::Reverse(group.count), std::cmp::Reverse(group.last_ms))
        });
        Dropped { since_ms, groups }
    }
}

/// What `firewall dropped` was asked: how far back, and how to answer.
#[derive(Debug, PartialEq, Eq)]
struct Look {
    /// A time as log(1) takes `--since`; the last 24 hours without one.
    since: String,
    json: bool,
}

/// `[--since TIME] [--json]`, in any order.
fn look(words: &[&str]) -> Result<Look, Fail> {
    let mut look = Look { since: SINCE_DEFAULT.to_string(), json: false };
    let mut words = words.iter().copied();
    while let Some(word) = words.next() {
        match word {
            "--json" => look.json = true,
            "--since" => {
                let time = words.next().ok_or_else(|| usage("--since needs a time".into()))?;
                look.since = time.to_string();
            }
            word => match word.strip_prefix("--since=") {
                Some(time) => look.since = time.to_string(),
                None => {
                    return Err(usage(format!(
                        "dropped takes --since TIME and --json, not {word}"
                    )));
                }
            },
        }
    }
    Ok(look)
}

/// `dropped` in words, with the times in UTC.
fn dropped_text(dropped: &Dropped) -> String {
    let since = util::time_text((dropped.since_ms / 1000) as i64, "%Y-%m-%d %H:%M:%S", true);
    if dropped.groups.is_empty() {
        return format!("The kernel dropped nothing it logged since {since} UTC.\n");
    }
    let mut rows = vec![
        ["FROM", "PROTOCOL", "PORT", "CARD", "COUNT", "LAST (UTC)", "WHY"].map(str::to_string),
    ];
    for group in &dropped.groups {
        rows.push([
            group.from.clone(),
            group.protocol.to_string(),
            group.port.map_or_else(|| "-".to_string(), |port| port.to_string()),
            group.card.clone(),
            group.count.to_string(),
            util::time_text((group.last_ms / 1000) as i64, "%Y-%m-%d %H:%M:%S", true),
            group.why.clone().unwrap_or_else(|| "-".to_string()),
        ]);
    }
    let widths: Vec<usize> =
        (0..7).map(|at| rows.iter().map(|row| row[at].len()).max().unwrap_or(0)).collect();
    let mut text = format!(
        "Dropped since {since} UTC. A count is of the lines the kernel logged, at most five a \
         second, so more were dropped.\n"
    );
    for row in &rows {
        let cells: Vec<String> =
            row.iter().zip(&widths).map(|(cell, width)| format!("{cell:width$}")).collect();
        text += cells.join("  ").trim_end();
        text += "\n";
    }
    text
}

/// `firewall dropped [--since TIME] [--json]`: what the kernel dropped
/// since TIME, from its log.
pub fn dropped(words: &[&str], out: &mut dyn Write) -> Result<i32, Fail> {
    let look = look(words)?;
    let since = parse_time(&look.since, util::now_ns(), false)?;
    if util::own_user() != 0 {
        return Err(fail(
            "only root reads the kernel's log records; try elevate firewall dropped".into(),
        ));
    }
    let request = Request {
        kernel: true,
        boot: "all".into(),
        since: Some(since),
        grep: Some("^Firewall: dropped ".into()),
        lines: Some(RECORDS_MAX),
        ..Request::default()
    };
    let mut tally = Tally::default();
    logd::ask(&request, |line| {
        if let Ok(record) = serde_json::from_str::<Record>(line) {
            tally.add(record.time_ns() / 1_000_000, &record.text);
        }
        Ok(true)
    })?;
    let dropped = tally.finish(since / 1_000_000);
    if look.json {
        return say(out, &dropped);
    }
    write!(out, "{}", dropped_text(&dropped))?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::firewall::{Found, decode_rule, encode};

    /// `value` as the JSON a reader gets.
    fn read(value: &impl Serialize) -> Value {
        serde_json::from_str(&serde_json::to_string(value).unwrap()).unwrap()
    }

    fn lines(text: &str) -> Vec<Line> {
        parse_settings(text).0
    }

    #[test]
    fn the_rules_are_listed_with_their_fields_and_the_lines_that_are_none() {
        let text = "# The rules.\nallow tcp 443 from 192.168.1.0/24\n\ndeny udp 5353 on net1\n\
                    allow tcp 99999\nallow tcp 8000-8100 from 2001:db8::9\nallow\n\
                    allow program /users/alice/bin/my server   # mine\n";
        assert_eq!(
            read(&listed(text, Some(41_000))),
            json!({
                "rules": [
                    {"number": 1, "text": "allow tcp 443 from 192.168.1.0/24", "action": "allow",
                     "protocol": "tcp", "ports": [443, 443], "from": "192.168.1.0/24",
                     "on": null, "program": null},
                    {"number": 2, "text": "deny udp 5353 on net1", "action": "deny",
                     "protocol": "udp", "ports": [5353, 5353], "from": null, "on": "net1",
                     "program": null},
                    {"number": 3, "text": "allow tcp 8000-8100 from 2001:db8::9",
                     "action": "allow", "protocol": "tcp", "ports": [8000, 8100],
                     "from": "2001:db8::9", "on": null, "program": null},
                    {"number": 4, "text": "allow", "action": "allow", "protocol": "any",
                     "ports": null, "from": null, "on": null, "program": null},
                    {"number": 5, "text": "allow program /users/alice/bin/my server",
                     "action": "allow", "protocol": "any", "ports": null, "from": null,
                     "on": null, "program": "/users/alice/bin/my server"},
                ],
                "problems": [{
                    "line": 5,
                    "text": "allow tcp 99999",
                    "why": "`99999` is not a port, 1 to 65535, or a range of them",
                }],
                "trial": {"left_ms": 41_000},
            })
        );
    }

    #[test]
    fn a_file_without_rules_lists_none_and_a_trial_that_does_not_run_is_null() {
        let none = json!({"rules": [], "problems": [], "trial": null});
        assert_eq!(read(&listed("# nothing\n\n", None)), none);
        assert_eq!(read(&listed("", None)), none);
    }

    fn listener(tcp: bool, port: u16, process: u64, inbound: bool, v6: bool) -> Listening {
        Listening {
            tcp,
            port,
            ipv4: !v6,
            ipv6: v6,
            inbound,
            interface: 0,
            process,
            program: Some((1, process)),
        }
    }

    /// The rule as the kernel holds it.
    fn held(text: &str) -> Held {
        let words: Vec<&str> = text.split_whitespace().collect();
        decode_rule(&encode(&Rule::parse(&words).unwrap(), Found::default()))
    }

    #[test]
    fn the_listening_sockets_say_who_reaches_each_and_by_which_rule() {
        let listeners = [
            listener(false, 5353, 40, false, false),
            listener(true, 9090, 312, true, false),
            listener(true, 9090, 312, true, true),
            listener(true, 22, 41, true, false),
            listener(true, 8000, 77, false, false),
            listener(true, 8080, 78, false, false),
            listener(true, 8443, 79, true, false),
        ];
        let rules =
            [held("deny tcp 22"), held("allow tcp 8000"), held("allow tcp 8080 from 10.0.0.0/8")];
        let path = |listener: &Listening| {
            (listener.process != 40).then(|| format!("/system/commands/p{}", listener.process))
        };
        let status = status_of(&listeners, &rules, Some(3), path);
        assert_eq!(
            read(&status),
            json!({"listening": [
                {"port": 22, "tcp": true, "program": "/system/commands/p41", "process": 41,
                 "reach": "denied", "rule": 1},
                {"port": 8000, "tcp": true, "program": "/system/commands/p77", "process": 77,
                 "reach": "everyone", "rule": 2},
                {"port": 8080, "tcp": true, "program": "/system/commands/p78", "process": 78,
                 "reach": "some", "rule": 3},
                {"port": 8443, "tcp": true, "program": "/system/commands/p79", "process": 79,
                 "reach": "service", "rule": null},
                {"port": 9090, "tcp": true, "program": "/system/commands/p312", "process": 312,
                 "reach": "service", "rule": null},
                {"port": 5353, "tcp": false, "program": null, "process": 40,
                 "reach": "local", "rule": null},
            ]})
        );
    }

    #[test]
    fn the_kernels_rule_numbers_are_the_files_only_while_the_counts_agree() {
        let sockets = [listener(true, 22, 41, true, false)];
        let rules = [held("deny tcp 22")];
        let rule = |file_rules| status_of(&sockets, &rules, file_rules, |_| None).listening[0].rule;
        assert_eq!(rule(Some(1)), Some(1));
        // The file has a rule the kernel left out, or cannot be read.
        assert_eq!(rule(Some(2)), None);
        assert_eq!(rule(None), None);
        assert!(status_of(&[], &[], Some(0), |_| None).listening.is_empty());
    }

    fn asking(protocol: Protocol, port: u16, from: &str, on: Option<&str>, json: bool) -> Question {
        Question { protocol, port, from: from.parse().unwrap(), on: on.map(str::to_string), json }
    }

    #[test]
    fn a_test_is_read_from_its_words_and_the_option_comes_anywhere() {
        let ask = |words: &[&str]| question(words).unwrap();
        let tcp = |on, json| asking(Protocol::Tcp, 22, "192.168.1.20", on, json);
        assert_eq!(ask(&["tcp", "22", "from", "192.168.1.20"]), tcp(None, false));
        assert_eq!(
            ask(&["tcp", "22", "from", "192.168.1.20", "on", "net0"]),
            tcp(Some("net0"), false)
        );
        assert_eq!(ask(&["--json", "tcp", "22", "from", "192.168.1.20"]), tcp(None, true));
        assert_eq!(
            ask(&["tcp", "22", "from", "192.168.1.20", "--json", "on", "net0"]),
            tcp(Some("net0"), true)
        );
        assert_eq!(
            ask(&["udp", "5353", "from", "fe80::1"]),
            asking(Protocol::Udp, 5353, "fe80::1", None, false)
        );
    }

    #[test]
    fn a_test_that_is_not_one_is_a_mistake_in_the_command_line() {
        for words in [
            &[][..],
            &["tcp"],
            &["tcp", "22"],
            &["tcp", "22", "from"],
            &["tcp", "22", "192.168.1.20"],
            &["ping", "22", "from", "192.168.1.20"],
            &["tcp", "0", "from", "192.168.1.20"],
            &["tcp", "70000", "from", "192.168.1.20"],
            &["tcp", "22", "from", "192.168.1.0/24"],
            &["tcp", "22", "from", "nowhere"],
            &["tcp", "22", "from", "192.168.1.20", "on"],
            &["tcp", "22", "from", "192.168.1.20", "on", "net/0"],
            &["tcp", "22", "from", "192.168.1.20", "at", "net0"],
            &["tcp", "22", "from", "192.168.1.20", "--try", "10"],
        ] {
            let error = question(words).unwrap_err();
            assert!(error.usage && error.code == 1, "{words:?}: {}", error.message);
        }
    }

    #[test]
    fn the_rule_that_decides_is_the_first_that_fits_and_the_others_are_passed_over() {
        let file = lines(
            "deny tcp 22 program /system/commands/sshd\n\
             allow tcp 22 on net1\n\
             allow tcp 22 from 192.168.1.0/24\n\
             deny tcp 22\n",
        );
        let ask = |from, on| asking(Protocol::Tcp, 22, from, on, true);
        // No card named: the rule for net1 might apply, and so might the
        // program's.
        assert_eq!(
            read(&tested(&file, &ask("192.168.1.20", None))),
            json!({"rule": 3, "text": "allow tcp 22 from 192.168.1.0/24", "allowed": true,
                   "passed": [1, 2]})
        );
        // By net0, the rule for net1 does not apply.
        assert_eq!(read(&tested(&file, &ask("192.168.1.20", Some("net0"))))["passed"], json!([1]));
        // By net1, rule 2 decides.
        assert_eq!(
            read(&tested(&file, &ask("203.0.113.9", Some("net1")))),
            json!({"rule": 2, "text": "allow tcp 22 on net1", "allowed": true, "passed": [1]})
        );
        // From elsewhere, by net0: the last rule.
        assert_eq!(
            read(&tested(&file, &ask("203.0.113.9", Some("net0")))),
            json!({"rule": 4, "text": "deny tcp 22", "allowed": false, "passed": [1]})
        );
    }

    #[test]
    fn a_connection_no_rule_decides_has_no_rule_and_no_answer() {
        let file = lines("allow tcp 443\nallow udp 8080\ndeny tcp 8080 program /x\n");
        let question = asking(Protocol::Tcp, 8080, "203.0.113.9", None, true);
        assert_eq!(
            read(&tested(&file, &question)),
            json!({"rule": null, "text": null, "allowed": null, "passed": [3]})
        );
        assert_eq!(
            read(&tested(&[], &question)),
            json!({"rule": null, "text": null, "allowed": null, "passed": []})
        );
    }

    #[test]
    fn the_test_says_its_answer_in_words() {
        let file = lines("deny tcp 22 program /x\nallow tcp 22 from 192.168.1.0/24\n");
        let ask = |port, from| asking(Protocol::Tcp, port, from, None, false);
        let question = ask(22, "192.168.1.20");
        assert_eq!(
            tested_text(&tested(&file, &question), &file, &question),
            "Rule 2 allows it: allow tcp 22 from 192.168.1.0/24\n\
             Passed over, as they name what test does not know: rule 1 (deny tcp 22 program /x)\n"
        );
        let question = ask(8080, "192.168.1.20");
        assert_eq!(
            tested_text(&tested(&file, &question), &file, &question),
            "No rule decides it: it reaches port 8080 only while a service listens there with \
             the right to be reached.\n"
        );
        let file = lines("deny tcp 22\n");
        let question = ask(22, "203.0.113.9");
        let words = tested_text(&tested(&file, &question), &file, &question);
        assert_eq!(words, "Rule 1 denies it: deny tcp 22\n");
    }

    #[test]
    fn the_kernels_words_for_a_drop_are_read_and_the_others_are_not() {
        let knock = |protocol, from: &str, port, card: &str, why: Option<&str>| {
            Some(Knock {
                protocol,
                from: from.to_string(),
                port,
                card: card.to_string(),
                why: why.map(str::to_string),
            })
        };
        for (text, want) in [
            (
                "Firewall: dropped a TCP connection from 203.0.113.9:51234 to port 22 on net0",
                knock("tcp", "203.0.113.9", Some(22), "net0", None),
            ),
            (
                "Firewall: dropped a TCP connection from [2001:db8::9]:51234 to port 443 on \
                 net1: process 312 listens for this machine alone",
                knock(
                    "tcp",
                    "2001:db8::9",
                    Some(443),
                    "net1",
                    Some("process 312 listens for this machine alone"),
                ),
            ),
            (
                "Firewall: dropped a UDP datagram from 10.0.0.1:5353 to port 5353 on br0",
                knock("udp", "10.0.0.1", Some(5353), "br0", None),
            ),
            (
                "Firewall: dropped a ping from 10.0.0.1 on net0",
                knock("ping", "10.0.0.1", None, "net0", None),
            ),
            (
                "Firewall: dropped a ping from fe80::1 on net0",
                knock("ping", "fe80::1", None, "net0", None),
            ),
            // The line for those it did not log, and lines that are not the
            // kernel's words for a drop.
            ("Firewall: dropped 12 more packets", None),
            ("Firewall: dropped a TCP connection from nowhere:1 to port 22 on net0", None),
            ("Firewall: dropped a TCP connection from 10.0.0.1:1 to port x on net0", None),
            ("Firewall: dropped a TCP connection from 10.0.0.1:1 to port 22", None),
            ("Firewall: dropped a SCTP association from 10.0.0.1:1 to port 22 on net0", None),
            ("dropped a ping from 10.0.0.1 on net0", None),
            ("", None),
        ] {
            assert_eq!(parse_drop(text), want, "{text}");
        }
    }

    #[test]
    fn drops_are_grouped_and_the_most_counted_group_comes_first() {
        let tcp = |port: &str| {
            format!("Firewall: dropped a TCP connection from 203.0.113.9:{port} to port 22 on net0")
        };
        let mut tally = Tally::default();
        for (at, port) in [(2_000, "1"), (1_000, "2"), (5_000, "3")] {
            tally.add(at, &tcp(port));
        }
        tally.add(3_000, "Firewall: dropped a UDP datagram from 203.0.113.9:7 to port 53 on net0");
        tally.add(4_000, "Firewall: dropped a ping from 198.51.100.4 on net0");
        tally.add(6_000, "Firewall: dropped a ping from 198.51.100.4 on net0");
        tally.add(7_000, &format!("{}: process 9 listens for this machine alone", tcp("4")));
        tally.add(8_000, "Firewall: dropped 40 more packets");
        assert_eq!(
            read(&tally.finish(500)),
            json!({
                "since_ms": 500,
                "groups": [
                    {"from": "203.0.113.9", "protocol": "tcp", "port": 22, "card": "net0",
                     "why": null, "count": 3, "first_ms": 1_000, "last_ms": 5_000},
                    {"from": "198.51.100.4", "protocol": "ping", "port": null, "card": "net0",
                     "why": null, "count": 2, "first_ms": 4_000, "last_ms": 6_000},
                    {"from": "203.0.113.9", "protocol": "tcp", "port": 22, "card": "net0",
                     "why": "process 9 listens for this machine alone", "count": 1,
                     "first_ms": 7_000, "last_ms": 7_000},
                    {"from": "203.0.113.9", "protocol": "udp", "port": 53, "card": "net0",
                     "why": null, "count": 1, "first_ms": 3_000, "last_ms": 3_000},
                ],
            })
        );
    }

    #[test]
    fn no_drops_is_an_empty_list_and_more_records_than_are_read_move_the_start() {
        assert_eq!(read(&Tally::default().finish(9)), json!({"since_ms": 9, "groups": []}));
        // A full read stopped at the newest records: what is before the
        // first is not counted, so the groups start there.
        let ping = "Firewall: dropped a ping from 10.0.0.1 on net0";
        let mut tally = Tally::default();
        for at in 0..RECORDS_MAX {
            tally.add(10_000 + at as u64, ping);
        }
        let dropped = tally.finish(500);
        assert_eq!(dropped.since_ms, 10_000);
        assert_eq!(dropped.groups[0].count, RECORDS_MAX as u64);
        // One short of the most leaves the time asked for.
        let mut tally = Tally::default();
        for at in 0..RECORDS_MAX - 1 {
            tally.add(10_000 + at as u64, ping);
        }
        assert_eq!(tally.finish(500).since_ms, 500);
    }

    #[test]
    fn dropped_takes_a_time_and_the_option_in_any_order() {
        let ask = |words: &[&str]| look(words).unwrap();
        let day = |json| Look { since: "-24h".into(), json };
        assert_eq!(ask(&[]), day(false));
        assert_eq!(ask(&["--json"]), day(true));
        assert_eq!(ask(&["--since", "-1h"]), Look { since: "-1h".into(), json: false });
        assert_eq!(ask(&["--json", "--since", "-7d"]), Look { since: "-7d".into(), json: true });
        assert_eq!(ask(&["--since=-1h", "--json"]), Look { since: "-1h".into(), json: true });
        for words in [&["--since"][..], &["-7d"], &["--try", "10"], &["--json=yes"]] {
            assert!(look(words).unwrap_err().usage, "{words:?}");
        }
    }

    #[test]
    fn the_drops_are_a_table_in_words() {
        let mut tally = Tally::default();
        for (at, port) in [(1_763_116_800_000, "51234"), (1_763_116_803_000, "51235")] {
            let text = format!(
                "Firewall: dropped a TCP connection from 203.0.113.9:{port} to port 22 on net0"
            );
            tally.add(at, &text);
        }
        let dropped = tally.finish(1_763_113_200_000);
        assert_eq!(
            dropped_text(&dropped),
            "Dropped since 2025-11-14 09:40:00 UTC. A count is of the lines the kernel logged, at \
             most five a second, so more were dropped.\n\
             FROM         PROTOCOL  PORT  CARD  COUNT  LAST (UTC)           WHY\n\
             203.0.113.9  tcp       22    net0  2      2025-11-14 10:40:03  -\n"
        );
        assert_eq!(
            dropped_text(&Tally::default().finish(1_763_113_200_000)),
            "The kernel dropped nothing it logged since 2025-11-14 09:40:00 UTC.\n"
        );
    }
    /// blocking.toml as the machine of the tests has it.
    const BLOCKING: &str = "tries = 5\nwithin = \"10m\"\nfirst = \"10m\"\nlongest = \"24h\"\n\
                            never = [\"192.168.1.0/24\", \"2001:db8:4:7::/64\"]\n";

    fn entry(address: &str, until: i64, times: u32, why: &str) -> BlockedEntry {
        BlockedEntry { address: address.into(), until, times, why: why.into() }
    }

    /// An address blocked for the second time in a row, and a network.
    fn entries() -> Vec<BlockedEntry> {
        vec![
            entry(
                "203.0.113.41",
                1_763_122_740,
                2,
                "3 wrong passwords over SSH in 2 minutes, for admin",
            ),
            entry(
                "2001:db8:9:1::/64",
                1_763_126_400,
                1,
                "5 wrong passwords at the console in 4 minutes",
            ),
        ]
    }

    #[test]
    fn the_blocked_report_is_the_golden_object_for_the_machine_the_tests_build() {
        let report = blocked_of(&entries(), Settings::parse(BLOCKING));
        let golden: Value = serde_json::from_str(include_str!("blocked.golden.json")).unwrap();
        assert_eq!(read(&report), golden);
    }

    #[test]
    fn the_blocked_report_has_its_fields_in_the_order_the_man_page_gives() {
        let line =
            serde_json::to_string(&blocked_of(&entries(), Settings::parse(BLOCKING))).unwrap();
        let at = |name: &str| line.find(&format!("\"{name}\":")).unwrap();
        let names = [
            "format",
            "on",
            "tries",
            "within_s",
            "first_s",
            "longest_s",
            "never",
            "blocks",
            "address",
            "until_ms",
            "in_a_row",
            "why",
            "wrong",
        ];
        let places: Vec<usize> = names.iter().map(|name| at(name)).collect();
        assert!(places.windows(2).all(|pair| pair[0] < pair[1]), "{line}");
        assert!(!line.contains('\n'));
    }

    #[test]
    fn with_no_file_and_no_blocks_it_says_the_defaults_and_an_empty_list() {
        let report = read(&blocked_of(&[], Settings::parse("")));
        assert_eq!(
            report,
            json!({
                "format": 1, "on": true, "tries": 5, "within_s": 600, "first_s": 600,
                "longest_s": 86400, "never": [], "blocks": [], "wrong": null,
            })
        );
    }

    #[test]
    fn blocking_that_is_off_says_so() {
        let report = read(&blocked_of(&[], Settings::parse("on = false\n")));
        assert_eq!(report["on"], json!(false));
        assert_eq!(report["wrong"], Value::Null);
    }

    #[test]
    fn a_file_that_does_not_read_leaves_the_settings_null_and_says_why() {
        let why = Settings::parse("tries = 1\n").map_err(|why| format!("/x: {why}"));
        let report = read(&blocked_of(&entries(), why));
        for field in ["on", "tries", "within_s", "first_s", "longest_s", "never"] {
            assert_eq!(report[field], Value::Null, "{field}");
        }
        assert_eq!(report["wrong"], json!("/x: tries is 2 to 100"));
        // What authd says of the blocks needs no file.
        assert_eq!(report["blocks"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn a_time_that_would_overflow_is_the_latest_there_is() {
        let report =
            read(&blocked_of(&[entry("203.0.113.9", i64::MAX, 1, "x")], Settings::parse("")));
        assert_eq!(report["blocks"][0]["until_ms"], json!(i64::MAX));
    }

    #[test]
    fn what_a_blocked_report_is_read_from_is_authd_s_list_and_one_file() {
        let words = "203.0.113.41 1763122740 2 3 wrong passwords over SSH in 2 minutes, for admin";
        let parsed = BlockedEntry::parse_list(words);
        assert_eq!(parsed.len(), 1);
        let report = read(&blocked_of(&parsed, Settings::parse("")));
        assert_eq!(report["blocks"][0]["address"], json!("203.0.113.41"));
        assert_eq!(report["blocks"][0]["until_ms"], json!(1_763_122_740_000_u64));
        assert_eq!(report["blocks"][0]["in_a_row"], json!(2));
    }
}
