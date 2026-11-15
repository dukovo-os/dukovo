// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `network set`, `network keep` and `network undo` (network(8), ADR
//! 0518): a card's addresses, router, DNS servers and metering in
//! network(5)'s file, put to use at once by netd's reload, or tried with
//! an undo that Spark's timer runs ([`crate::trial`]).
//!
//! `set` edits only the card's own section, the first that names it by
//! its name or its MAC address, as netd reads it, or a new one named by
//! its MAC address at the end. It takes out the section's `address`,
//! `router`, `dns` and `dhcp` lines and writes those it was given in the
//! place of the first; every other line, comment and section stays as it
//! was. Before it writes, it reads the new file as netd will, and writes
//! nothing unless netd would set the card up as asked and find nothing
//! new to leave out.
//!
//! The web console's `network.set` action checks its values with
//! [`check`], so that the page, the API and this command refuse the same
//! values with the same words.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs;
use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;

use crate::dhcp;
use crate::dhcp_server::mac_of;
use crate::ip::{self, Cidr, Cidr6};
use crate::net_errors;
use crate::netd::{self, Setup, unicast6};
use crate::service::{ask, said};
use crate::trial::{self, ToUse, Tried};
use crate::util::{self, Fail, fail, usage};

/// Spark's name for the trial of the network settings.
pub const TRIAL: &str = "network";

/// The service that reads the settings, and reloads them.
const NETD: &str = "netd";

/// The longest name of a card.
const NAME_MAX: usize = 15;

/// The addresses, and the DNS servers, a card may be given.
const ADDRESSES_MAX: usize = 4;
const DNS_MAX: usize = 3;

/// The mode of the settings file when it is not there yet.
const MODE: u32 = 0o644;

/// The words that begin a key after the addresses.
const KEYWORDS: [&str; 3] = ["router", "dns", "metered"];

/// The usage of `set`.
pub const USAGE: &str = "network set CARD dhcp [metered auto|yes|no] [--try SECONDS]\n       \
     network set CARD fixed ADDRESS/PREFIX... [router ADDRESS] [dns ADDRESS...] \
     [metered auto|yes|no] [--try SECONDS]";

/// What `network set` asks for, as its words give it, before [`check`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Set {
    /// The card, by its name or its MAC address.
    pub card: String,
    /// Whether it asks DHCP, or has addresses of its own.
    pub mode: Mode,
    /// Its addresses, `ADDRESS/PREFIX`, for [`Mode::Fixed`].
    pub addresses: Vec<String>,
    /// Its router, for [`Mode::Fixed`].
    pub router: Option<String>,
    /// Its DNS servers, for [`Mode::Fixed`].
    pub dns: Vec<String>,
    /// `auto`, `yes` or `no`, when it is to change.
    pub metered: Option<String>,
}

/// How a card gets its address.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// By DHCP.
    #[default]
    Dhcp,
    /// Addresses of its own.
    Fixed,
}

/// A value that cannot work: its field, as the console's action names
/// it, and why, as a sentence.
#[derive(Debug, PartialEq, Eq)]
pub struct Bad {
    /// `card`, `addresses`, `router`, `dns` or `metered`.
    pub field: &'static str,
    /// Why.
    pub why: String,
}

impl Bad {
    fn new(field: &'static str, why: impl Into<String>) -> Bad {
        Bad { field, why: why.into() }
    }
}

/// A [`Set`] whose values are checked, as netd's settings say them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    card: String,
    mode: Mode,
    addresses: Vec<Cidr>,
    addresses6: Vec<Cidr6>,
    router: Option<IpAddr>,
    dns: Vec<IpAddr>,
    metered: Option<&'static str>,
}

/// Reads the words that follow `network set`: the card, the mode, and
/// the keys, which a word that is not one of them makes a mistake in the
/// command line.
///
/// # Errors
///
/// A usage error, which says what the words should be.
pub fn parse(words: &[&str]) -> Result<Set, Fail> {
    let wrong = |why: String| usage(format!("{why}\nUsage: {USAGE}"));
    let (card, mode, rest) = match words {
        [card, mode, rest @ ..] => (*card, *mode, rest),
        _ => return Err(wrong("network set takes a card and dhcp or fixed".into())),
    };
    let mode = match mode {
        "dhcp" => Mode::Dhcp,
        "fixed" => Mode::Fixed,
        other => return Err(wrong(format!("`{other}` is not dhcp or fixed"))),
    };
    let mut set = Set { card: card.into(), mode, ..Set::default() };
    let mut at = 0;
    if mode == Mode::Fixed {
        // An address is a word with a prefix, or an address that lacks
        // one, which `check` asks the prefix of.
        let is_address = |word: &str| word.contains('/') || word.parse::<IpAddr>().is_ok();
        while let Some(word) = rest.get(at).filter(|word| is_address(word)) {
            set.addresses.push((*word).into());
            at += 1;
        }
    }
    let mut seen = Vec::new();
    while let Some(&key) = rest.get(at) {
        at += 1;
        if seen.contains(&key) {
            return Err(wrong(format!("{key} is given twice")));
        }
        seen.push(key);
        match key {
            "router" | "dns" if mode == Mode::Dhcp => {
                return Err(wrong(format!("{key} goes with fixed: DHCP gives the {key}")));
            }
            "router" | "metered" => {
                let value = rest
                    .get(at)
                    .filter(|word| !KEYWORDS.contains(word))
                    .ok_or_else(|| wrong(format!("{key} takes a value")))?;
                at += 1;
                if key == "router" {
                    set.router = Some((*value).into());
                } else {
                    set.metered = Some((*value).into());
                }
            }
            "dns" => {
                let first = at;
                while let Some(word) = rest.get(at).filter(|word| !KEYWORDS.contains(word)) {
                    set.dns.push((*word).into());
                    at += 1;
                }
                if at == first {
                    return Err(wrong("dns takes one or more addresses".into()));
                }
            }
            other if mode == Mode::Fixed => {
                return Err(wrong(format!(
                    "`{other}` is not ADDRESS/PREFIX, router, dns or metered"
                )));
            }
            other => return Err(wrong(format!("`{other}` is not metered"))),
        }
    }
    Ok(set)
}

/// Whether `card` may name a card: 1 to 15 letters, digits, `.`, `-` and
/// `_` that start with a letter or a digit, or a MAC address.
pub fn is_card(card: &str) -> bool {
    mac_of(card).is_some()
        || (card.len() <= NAME_MAX
            && card.starts_with(|c: char| c.is_ascii_alphanumeric())
            && card.bytes().all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b)))
}

/// Whether `ip` is an IPv4 address that only reaches its own link.
fn link_local4(ip: Ipv4Addr) -> bool {
    ip.octets()[..2] == [169, 254]
}

/// An address of the card's own, `ADDRESS/PREFIX`, checked.
fn address(text: &str) -> Result<IpAddr, Bad> {
    let bad = |why: String| Err(Bad::new("addresses", why));
    let Some((ip, prefix)) = text.split_once('/') else {
        return bad(format!(
            "{text} has no prefix: write it as ADDRESS/PREFIX, as 192.168.1.10/24"
        ));
    };
    match ip.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            let Some(cidr) = prefix
                .parse()
                .ok()
                .filter(|p| (1..=32).contains(p))
                .map(|prefix| Cidr { ip, prefix })
            else {
                return bad(format!("{text}: an IPv4 prefix is 1 to 32"));
            };
            if !dhcp::unicast(ip) || link_local4(ip) {
                return bad(format!("{ip} is not an address a card may have"));
            }
            if cidr.network() == ip && cidr.prefix < 31 {
                return bad(format!("{text} is the network's own address, not a card's"));
            }
            if cidr.broadcast() == Some(ip) {
                return bad(format!("{text} is the network's broadcast address, not a card's"));
            }
            Ok(IpAddr::V4(ip))
        }
        Ok(IpAddr::V6(ip)) => {
            if prefix.parse::<u8>().ok().is_none_or(|p| !(1..=128).contains(&p)) {
                return bad(format!("{text}: an IPv6 prefix is 1 to 128"));
            }
            if !unicast6(ip) || ip::link_local(ip) {
                return bad(format!("{ip} is not an address a card may have"));
            }
            Ok(IpAddr::V6(ip))
        }
        Err(_) => bad(format!("{ip} is not an IP address")),
    }
}

/// Whether `ip` is an address a router or a DNS server may have.
fn unicast(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => dhcp::unicast(ip),
        IpAddr::V6(ip) => unicast6(ip),
    }
}

/// The checks of `set` that need no machine: each value's words, and
/// that the router is on one of the card's networks.
///
/// # Errors
///
/// The first value that cannot work.
pub fn check(set: &Set) -> Result<Checked, Bad> {
    if !is_card(&set.card) {
        let why = "a card is named by 1 to 15 letters, digits, ., - and _, or by its MAC address";
        return Err(Bad::new("card", why));
    }
    let metered = match set.metered.as_deref() {
        None => None,
        Some("auto") => Some("auto"),
        Some("yes") => Some("yes"),
        Some("no") => Some("no"),
        Some(other) => {
            return Err(Bad::new("metered", format!("metered is auto, yes or no, not {other}")));
        }
    };
    let mut checked = Checked {
        card: set.card.clone(),
        mode: set.mode,
        addresses: Vec::new(),
        addresses6: Vec::new(),
        router: None,
        dns: Vec::new(),
        metered,
    };
    if set.mode == Mode::Dhcp {
        if !set.addresses.is_empty() || set.router.is_some() || !set.dns.is_empty() {
            return Err(Bad::new(
                "addresses",
                "a card that asks DHCP is given no addresses, router or DNS servers",
            ));
        }
        return Ok(checked);
    }
    if set.addresses.is_empty() || set.addresses.len() > ADDRESSES_MAX {
        let why = format!("a card has 1 to {ADDRESSES_MAX} addresses of its own");
        return Err(Bad::new("addresses", why));
    }
    let mut seen = HashSet::new();
    for text in &set.addresses {
        let ip = address(text)?;
        if !seen.insert(ip) {
            return Err(Bad::new("addresses", format!("{ip} is given twice")));
        }
        // The prefix was checked by `address`.
        match ip {
            IpAddr::V4(_) => checked.addresses.extend(Cidr::parse(text)),
            IpAddr::V6(_) => checked.addresses6.extend(Cidr6::parse(text)),
        }
    }
    if let Some(text) = &set.router {
        let router = text.parse::<IpAddr>().ok().filter(|&ip| unicast(ip)).ok_or_else(|| {
            Bad::new("router", format!("{text} is not an address a router may have"))
        })?;
        if seen.contains(&router) {
            return Err(Bad::new(
                "router",
                format!("{router} is the card's own address, not its router"),
            ));
        }
        let on_network = match router {
            IpAddr::V4(ip) => checked.addresses.iter().any(|cidr| cidr.contains(ip)),
            IpAddr::V6(ip) => {
                !checked.addresses6.is_empty()
                    && (ip::link_local(ip)
                        || checked.addresses6.iter().any(|cidr| cidr.contains(ip)))
            }
        };
        if !on_network {
            let why =
                format!("the router {router} is on none of the networks of the card's addresses");
            return Err(Bad::new("router", why));
        }
        checked.router = Some(router);
    }
    if set.dns.len() > DNS_MAX {
        return Err(Bad::new("dns", format!("a card has at most {DNS_MAX} DNS servers")));
    }
    for text in &set.dns {
        let server = text.parse::<IpAddr>().ok().filter(|&ip| unicast(ip)).ok_or_else(|| {
            Bad::new("dns", format!("{text} is not an address a DNS server may have"))
        })?;
        if checked.dns.contains(&server) {
            return Err(Bad::new("dns", format!("{server} is given twice")));
        }
        checked.dns.push(server);
    }
    Ok(checked)
}

/// A card of the machine: the name the kernel gives it, and its MAC
/// address, in lower case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub name: String,
    pub mac: String,
}

/// The card `wanted` names, by its name or its MAC address, of those netd
/// sets up.
fn card_of(interfaces: &[ip::Interface], wanted: &str) -> Option<Card> {
    interfaces
        .iter()
        .filter(|interface| netd::sets_up(interface))
        .find(|interface| interface.name == wanted || interface.mac.eq_ignore_ascii_case(wanted))
        .map(|interface| Card {
            name: interface.name.clone(),
            mac: interface.mac.to_ascii_lowercase(),
        })
}

/// The key of a settings line, if it is one: not a comment, a section or
/// blank.
pub(crate) fn key_of(line: &str) -> Option<&str> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
        return None;
    }
    line.split_once('=').map(|(key, _)| key.trim())
}

/// The section a line starts, in lower case, if it starts one.
pub(crate) fn section_name(line: &str) -> Option<String> {
    let line = line.trim();
    line.strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .map(|name| name.trim().to_ascii_lowercase())
}

/// The lines `checked` writes in the card's section, in network(5)'s
/// order.
fn lines_of(checked: &Checked) -> Vec<String> {
    let mut lines = Vec::new();
    if checked.mode == Mode::Fixed {
        for cidr in &checked.addresses {
            lines.push(format!("address = {cidr}\n"));
        }
        for cidr in &checked.addresses6 {
            lines.push(format!("address = {cidr}\n"));
        }
        if let Some(router) = checked.router {
            lines.push(format!("router = {router}\n"));
        }
        for server in &checked.dns {
            lines.push(format!("dns = {server}\n"));
        }
    }
    if let Some(metered) = checked.metered {
        lines.push(format!("metered = {metered}\n"));
    }
    lines
}

/// `text`, network(5)'s file, with the section of `card` set as `checked`
/// says: its `address`, `router`, `dns` and `dhcp` lines (and `metered`,
/// when it is given) taken out and the new ones written where the first
/// was, else after the section's last line; or a new section named by the
/// card's MAC address at the end.
fn edited(text: &str, card: &Card, checked: &Checked) -> String {
    let (settings, _) = netd::parse_settings(text);
    let section = netd::section_of(&settings, &card.name, &card.mac).map(|(name, _)| name.clone());
    let mut taken = vec!["address", "router", "dns", "dhcp"];
    if checked.metered.is_some() {
        taken.push("metered");
    }
    let new = lines_of(checked);
    let mut lines: Vec<&str> = text.split_inclusive('\n').collect();
    let Some(section) = section else {
        if new.is_empty() {
            return text.to_string();
        }
        let mut out = String::from(text);
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        if !out.is_empty() {
            out.push('\n');
        }
        let _ = writeln!(out, "[{}]\n# {}, set by network(8)", card.mac, card.name);
        out.extend(new);
        return out;
    };
    // The first header of that name is the section netd reads; a later
    // one of the same name it leaves out.
    let Some(header) =
        lines.iter().position(|line| section_name(line).as_deref() == Some(&section))
    else {
        return text.to_string();
    };
    let end = lines[header + 1..]
        .iter()
        .position(|line| section_name(line).is_some())
        .map_or(lines.len(), |at| header + 1 + at);
    let mut kept: Vec<&str> = Vec::new();
    let mut place = None;
    for line in &lines[header + 1..end] {
        if key_of(line).is_some_and(|key| taken.contains(&key)) {
            place.get_or_insert(kept.len());
        } else {
            kept.push(line);
        }
    }
    // Without a line taken out, the new ones go after the section's last
    // line that is not blank.
    let place = place.unwrap_or_else(|| {
        kept.iter().rposition(|line| !line.trim().is_empty()).map_or(0, |at| at + 1)
    });
    // The section's last line may lack its line end, at the end of the
    // file.
    let mut owned: Vec<String> = kept.iter().map(|line| (*line).to_string()).collect();
    if let Some(last) = place.checked_sub(1).and_then(|at| owned.get_mut(at))
        && !last.ends_with('\n')
    {
        last.push('\n');
    }
    owned.splice(place..place, new);
    let mut out: String = lines[..=header].concat();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.extend(owned);
    lines.drain(..end);
    out.extend(lines.iter().map(|line| (*line).to_string()));
    out
}

/// What netd says is wrong in `text`, without the line numbers, which the
/// edit moves.
fn wrong_of(text: &str) -> Vec<String> {
    let (_, wrong) = netd::parse_settings(text);
    wrong
        .into_iter()
        .map(|line| {
            let rest = line.strip_prefix(netd::SETTINGS).unwrap_or(&line);
            let rest = rest.strip_prefix(':').unwrap_or(rest);
            let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit());
            rest.trim_start_matches(':').trim().to_string()
        })
        .collect()
}

/// How netd reads the card in `text`.
fn setup_in(text: &str, card: &Card) -> Setup {
    let (settings, _) = netd::parse_settings(text);
    netd::setup_of(&settings, &card.name, &card.mac)
}

/// The new file, once it is checked that netd reads it as `checked`
/// asks: the card's addresses, router, DNS servers, metering and DHCP as
/// given, its DHCP server as it was, and nothing new left out.
///
/// # Errors
///
/// NET-178 when the card serves DHCP and cannot do so after the change;
/// otherwise what netd would leave out.
fn planned(text: &str, card: &Card, checked: &Checked) -> Result<String, Fail> {
    let before = setup_in(text, card);
    if before.server.range.is_some() && checked.mode == Mode::Dhcp {
        return Err(net_errors::serves_dhcp(&card.name));
    }
    let new = edited(text, card, checked);
    let after = setup_in(&new, card);
    if let Some((first, last)) = before.server.range
        && after.server.range.is_none()
    {
        return Err(net_errors::range_outside(&card.name, first, last));
    }
    let old_wrong = wrong_of(text);
    if let Some(why) = wrong_of(&new).into_iter().find(|why| !old_wrong.contains(why)) {
        return Err(fail(format!("netd would leave out what the change writes: {why}")));
    }
    let routers = |setup: &Setup| (setup.router.map(IpAddr::V4), setup.router6.map(IpAddr::V6));
    let asked_router = match checked.router {
        Some(IpAddr::V4(ip)) => (Some(IpAddr::V4(ip)), None),
        Some(IpAddr::V6(ip)) => (None, Some(IpAddr::V6(ip))),
        None => (None, None),
    };
    let metered = match checked.metered {
        Some("yes") => Some(true),
        Some("no") => Some(false),
        Some(_) => None,
        None => before.metered,
    };
    let as_asked = after.addresses == checked.addresses
        && after.addresses6 == checked.addresses6
        && routers(&after) == asked_router
        && after.dns == checked.dns
        && after.metered == metered
        && after.dhcp() == (checked.mode == Mode::Dhcp);
    if !as_asked {
        return Err(fail(format!(
            "netd would not read {}'s section as asked; {} is left as it is",
            card.name,
            netd::SETTINGS
        )));
    }
    Ok(new)
}

/// What the change is, in a line.
fn said_of(card: &Card, checked: &Checked) -> String {
    let mut words = match checked.mode {
        Mode::Dhcp => format!("{}: by DHCP", card.name),
        Mode::Fixed => {
            let addresses: Vec<String> = checked
                .addresses
                .iter()
                .map(ToString::to_string)
                .chain(checked.addresses6.iter().map(ToString::to_string))
                .collect();
            format!("{}: fixed {}", card.name, addresses.join(" "))
        }
    };
    if let Some(router) = checked.router {
        let _ = write!(words, ", router {router}");
    }
    if !checked.dns.is_empty() {
        let dns: Vec<String> = checked.dns.iter().map(ToString::to_string).collect();
        let _ = write!(words, ", dns {}", dns.join(" "));
    }
    if let Some(metered) = checked.metered {
        let _ = write!(words, ", metered {metered}");
    }
    words
}

/// Fails unless this runs as root, whose the settings file and Spark's
/// control socket are.
fn need_root(words: &str) -> Result<(), Fail> {
    if util::own_user() == 0 {
        return Ok(());
    }
    Err(fail(format!(
        "only root changes the network settings: run it with elevate, as `elevate network {words} ...`"
    )))
}

/// `network set` with the words after `set`, tried for `seconds` when
/// given.
///
/// # Errors
///
/// A usage error for words that are not the command's; a refusal for a
/// value that cannot work, a card the machine does not have (NET-181),
/// one that serves DHCP (NET-178), or while the network settings are
/// being tried (NET-176); or what failed.
pub fn run(words: &[&str], seconds: Option<u32>) -> Result<i32, Fail> {
    let set = parse(words)?;
    let checked = check(&set).map_err(|bad| fail(bad.why))?;
    need_root("set")?;
    if trial::running(TRIAL)?.is_some() {
        return Err(net_errors::network_is_tried());
    }
    let interfaces = ip::read_interfaces(Path::new(ip::INTERFACES), Path::new(ip::ROUTES))
        .map_err(|error| util::failed(&ip::INTERFACES, error))?;
    let card = card_of(&interfaces, &checked.card)
        .ok_or_else(|| net_errors::no_such_card(&checked.card))?;
    let settings = Path::new(netd::SETTINGS);
    let text = match fs::read_to_string(settings) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(util::failed(&netd::SETTINGS, error)),
    };
    let new = planned(&text, &card, &checked)?;
    println!("network: {}", said_of(&card, &checked));
    if new == text {
        println!("network: {} says so already", netd::SETTINGS);
        return Ok(0);
    }
    put(settings, &new, seconds)
}

/// Puts `new` in place of the settings at `path` and has netd read it:
/// at once, or as a trial of `seconds`.
fn put(path: &Path, new: &str, seconds: Option<u32>) -> Result<i32, Fail> {
    let Some(seconds) = seconds else {
        trial::replace(path, new.as_bytes(), MODE)?;
        let answer = ask(&format!("reload {NETD}"))?;
        println!("network: {}", said(&answer));
        return Ok(0);
    };
    match trial::try_text(TRIAL, path, new.as_bytes(), MODE, seconds, ToUse::Reload(NETD))? {
        Tried::InUse => {
            let at = trial::undone_at(seconds);
            println!("network: Spark undoes it at {at} unless it is kept: `network keep`");
            Ok(0)
        }
        Tried::Undone { why, undone } => {
            eprintln!("network: {why}");
            println!("network: {undone}");
            Ok(1)
        }
    }
}

/// `network keep` and `network undo`: what the trial of the network
/// settings comes to.
///
/// # Errors
///
/// NET-179 when nothing is being tried; Spark's refusal.
pub fn end(verb: &str) -> Result<i32, Fail> {
    need_root(verb)?;
    let words = trial::end(verb, TRIAL, || net_errors::nothing_is_tried("the network settings"))?;
    match verb {
        "keep" => println!("network: kept: the network settings stay as they are"),
        _ => println!("network: {words}"),
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(words: &[&str]) -> Set {
        parse(words).unwrap()
    }

    fn checked(words: &[&str]) -> Checked {
        check(&set(words)).unwrap()
    }

    fn bad(words: &[&str]) -> Bad {
        check(&set(words)).unwrap_err()
    }

    /// The cases the Networking page's editor checks too: each line's
    /// words, and what the first check that fails says, or `ok`.
    #[test]
    fn the_shared_cases_are_checked_as_the_page_checks_them() {
        let mut count = 0;
        for line in include_str!("cases.txt").lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (words, want) = line.split_once(" => ").expect(line);
            let words: Vec<&str> = words.split_whitespace().collect();
            let got = match check(&set(&words)) {
                Ok(_) => "ok".to_string(),
                Err(bad) => format!("{}: {}", bad.field, bad.why),
            };
            assert_eq!(got, want, "{line}");
            count += 1;
        }
        assert!(count >= 40, "{count} cases");
    }

    fn net0() -> Card {
        Card { name: "net0".into(), mac: "52:54:00:12:34:56".into() }
    }

    #[test]
    fn the_words_give_the_mode_the_addresses_and_the_keys_in_any_order() {
        assert_eq!(set(&["net0", "dhcp"]), Set { card: "net0".into(), ..Set::default() });
        let fixed = set(&[
            "net0",
            "fixed",
            "10.0.2.15/24",
            "10.0.3.15/24",
            "metered",
            "no",
            "dns",
            "10.0.2.3",
            "1.1.1.1",
            "router",
            "10.0.2.2",
        ]);
        assert_eq!(
            fixed,
            Set {
                card: "net0".into(),
                mode: Mode::Fixed,
                addresses: vec!["10.0.2.15/24".into(), "10.0.3.15/24".into()],
                router: Some("10.0.2.2".into()),
                dns: vec!["10.0.2.3".into(), "1.1.1.1".into()],
                metered: Some("no".into()),
            }
        );
        assert_eq!(set(&["net0", "dhcp", "metered", "yes"]).metered.as_deref(), Some("yes"));
    }

    #[test]
    fn words_that_are_not_the_command_s_are_a_usage_error() {
        for words in [
            &["net0"][..],
            &["net0", "static", "10.0.2.15/24"],
            &["net0", "dhcp", "router", "10.0.2.2"],
            &["net0", "dhcp", "10.0.2.15/24"],
            &["net0", "fixed", "10.0.2.15/24", "router"],
            &["net0", "fixed", "10.0.2.15/24", "router", "10.0.2.2", "10.0.2.3"],
            &["net0", "fixed", "10.0.2.15/24", "router", "10.0.2.2", "router", "10.0.2.1"],
            &["net0", "fixed", "10.0.2.15/24", "dns"],
            &["net0", "fixed", "10.0.2.15/24", "metered"],
            &["net0", "fixed", "10.0.2.15/24", "gateway", "10.0.2.2"],
        ] {
            let error = parse(words).unwrap_err();
            assert!(error.usage, "{words:?}");
            assert!(error.message.contains("Usage: network set"), "{words:?}: {}", error.message);
        }
    }

    #[test]
    fn a_card_is_a_short_name_or_a_mac_address() {
        for card in ["net0", "br0", "net0.7", "wlan_1", "52:54:00:12:34:56", "52:54:00:AB:CD:EF"] {
            assert!(is_card(card), "{card}");
        }
        for card in ["", "../x", ".x", "net0/1", "a-name-that-is-long", "net 0", "-net0"] {
            assert!(!is_card(card), "{card}");
        }
        assert_eq!(bad(&["../x", "dhcp"]).field, "card");
    }

    #[test]
    fn the_addresses_are_one_to_four_with_a_prefix_each_and_a_card_s_own() {
        assert_eq!(bad(&["net0", "fixed"]).field, "addresses");
        let five =
            ["net0", "fixed", "10.0.0.1/8", "10.0.0.2/8", "10.0.0.3/8", "10.0.0.4/8", "10.0.0.5/8"];
        assert_eq!(bad(&five).why, "a card has 1 to 4 addresses of its own");
        for (address, why) in [
            (
                "10.0.2.15",
                "10.0.2.15 has no prefix: write it as ADDRESS/PREFIX, as 192.168.1.10/24",
            ),
            ("10.0.2.15/0", "10.0.2.15/0: an IPv4 prefix is 1 to 32"),
            ("10.0.2.15/33", "10.0.2.15/33: an IPv4 prefix is 1 to 32"),
            ("2001:db8::5/129", "2001:db8::5/129: an IPv6 prefix is 1 to 128"),
            ("169.254.1.1/16", "169.254.1.1 is not an address a card may have"),
            ("fe80::1/64", "fe80::1 is not an address a card may have"),
            ("127.0.0.1/8", "127.0.0.1 is not an address a card may have"),
            ("224.0.0.1/24", "224.0.0.1 is not an address a card may have"),
            ("10.0.2.0/24", "10.0.2.0/24 is the network's own address, not a card's"),
            ("10.0.2.255/24", "10.0.2.255/24 is the network's broadcast address, not a card's"),
            ("router.local/24", "router.local is not an IP address"),
        ] {
            assert_eq!(bad(&["net0", "fixed", address]), Bad::new("addresses", why), "{address}");
        }
        let twice = bad(&["net0", "fixed", "10.0.2.15/24", "10.0.2.15/16"]);
        assert_eq!(twice.why, "10.0.2.15 is given twice");
        // A /31 has no network or broadcast address of its own.
        assert!(check(&set(&["net0", "fixed", "10.0.0.0/31"])).is_ok());
    }

    #[test]
    fn the_router_is_on_one_of_the_card_s_networks() {
        let off = bad(&["net0", "fixed", "10.0.2.15/24", "router", "192.168.9.1"]);
        assert_eq!(
            off,
            Bad::new(
                "router",
                "the router 192.168.9.1 is on none of the networks of the card's addresses"
            )
        );
        let own = bad(&["net0", "fixed", "10.0.2.15/24", "router", "10.0.2.15"]);
        assert_eq!(own.why, "10.0.2.15 is the card's own address, not its router");
        let other_version = bad(&["net0", "fixed", "10.0.2.15/24", "router", "2001:db8::1"]);
        assert_eq!(other_version.field, "router");
        // An IPv6 router on the link is on every network of the card's.
        let six = checked(&["net0", "fixed", "2001:db8::5/64", "router", "fe80::1"]);
        assert_eq!(six.router, Some("fe80::1".parse().unwrap()));
        assert_eq!(bad(&["net0", "fixed", "10.0.2.15/24", "router", "0.0.0.0"]).field, "router");
    }

    #[test]
    fn the_dns_servers_are_up_to_three_addresses() {
        let four =
            ["net0", "fixed", "10.0.2.15/24", "dns", "1.1.1.1", "8.8.8.8", "9.9.9.9", "1.0.0.1"];
        assert_eq!(bad(&four).why, "a card has at most 3 DNS servers");
        assert_eq!(bad(&["net0", "fixed", "10.0.2.15/24", "dns", "dns.example"]).field, "dns");
        assert_eq!(
            bad(&["net0", "fixed", "10.0.2.15/24", "dns", "1.1.1.1", "1.1.1.1"]).field,
            "dns"
        );
        assert_eq!(bad(&["net0", "dhcp", "metered", "sometimes"]).field, "metered");
    }

    const FILE: &str = "# The machine's cards.\n\n[net1]\naddress = 192.168.5.1/24\n\
                        ipv6 = no\n\n[52:54:00:12:34:56]\n# The uplink.\ndhcp = no\n\
                        address = 10.0.2.9/24\nrouter = 10.0.2.2\ntemporary-addresses = no\n\
                        dns = 10.0.2.3\nmetered = yes\n\n[net2]\naddress = 10.9.0.1/16\n";

    #[test]
    fn a_change_takes_the_card_s_lines_out_and_writes_the_new_where_the_first_was() {
        let fixed =
            checked(&["net0", "fixed", "10.0.2.15/24", "router", "10.0.2.2", "dns", "1.1.1.1"]);
        let new = planned(FILE, &net0(), &fixed).unwrap();
        assert_eq!(
            new,
            "# The machine's cards.\n\n[net1]\naddress = 192.168.5.1/24\nipv6 = no\n\n\
             [52:54:00:12:34:56]\n# The uplink.\naddress = 10.0.2.15/24\nrouter = 10.0.2.2\n\
             dns = 1.1.1.1\ntemporary-addresses = no\nmetered = yes\n\n[net2]\n\
             address = 10.9.0.1/16\n"
        );
        let dhcp = checked(&["net0", "dhcp", "metered", "auto"]);
        let new = planned(FILE, &net0(), &dhcp).unwrap();
        assert!(
            new.contains("# The uplink.\nmetered = auto\ntemporary-addresses = no\n\n[net2]"),
            "{new}"
        );
        assert!(setup_in(&new, &net0()).dhcp());
    }

    #[test]
    fn a_card_without_a_section_gets_one_named_by_its_mac_address_at_the_end() {
        let fixed = checked(&["net3", "fixed", "10.7.0.5/24"]);
        let card = Card { name: "net3".into(), mac: "52:54:00:aa:bb:cc".into() };
        let new = planned("[net1]\naddress = 192.168.5.1/24", &card, &fixed).unwrap();
        assert_eq!(
            new,
            "[net1]\naddress = 192.168.5.1/24\n\n[52:54:00:aa:bb:cc]\n# net3, set by network(8)\n\
             address = 10.7.0.5/24\n"
        );
        assert_eq!(
            planned("", &card, &fixed).unwrap(),
            "[52:54:00:aa:bb:cc]\n# net3, set by network(8)\naddress = 10.7.0.5/24\n"
        );
        // A card that asks DHCP by default, asked to, changes nothing.
        let dhcp = checked(&["net3", "dhcp"]);
        assert_eq!(planned("[net1]\nipv6 = no\n", &card, &dhcp).unwrap(), "[net1]\nipv6 = no\n");
    }

    #[test]
    fn the_section_netd_reads_is_the_one_changed_and_a_later_one_stays() {
        let text = "[NET0]\naddress = 10.0.2.9/24\n[52:54:00:12:34:56]\naddress = 10.0.4.4/24\n";
        let fixed = checked(&["net0", "fixed", "10.0.2.15/24"]);
        let new = planned(text, &net0(), &fixed).unwrap();
        assert_eq!(
            new,
            "[NET0]\naddress = 10.0.2.15/24\n[52:54:00:12:34:56]\naddress = 10.0.4.4/24\n"
        );
        // A section that ends the file without a line end gets one.
        let new = planned("[net0]\nipv6 = no", &net0(), &fixed).unwrap();
        assert_eq!(new, "[net0]\nipv6 = no\naddress = 10.0.2.15/24\n");
    }

    #[test]
    fn a_card_that_serves_dhcp_keeps_an_address_on_its_range_s_network() {
        let text = "[net0]\naddress = 192.168.5.1/24\ndhcp-server = 192.168.5.100-192.168.5.199\n";
        let dhcp = checked(&["net0", "dhcp"]);
        let refused = planned(text, &net0(), &dhcp).unwrap_err();
        assert!(refused.message.ends_with("(NET-178)"), "{}", refused.message);
        let elsewhere = checked(&["net0", "fixed", "10.0.2.15/24"]);
        let refused = planned(text, &net0(), &elsewhere).unwrap_err();
        assert!(
            refused.message.contains("from 192.168.5.100 to 192.168.5.199"),
            "{}",
            refused.message
        );
        assert!(refused.message.ends_with("(NET-178)"), "{}", refused.message);
        let same_network = checked(&["net0", "fixed", "192.168.5.2/24", "router", "192.168.5.254"]);
        let new = planned(text, &net0(), &same_network).unwrap();
        assert!(
            new.starts_with(
                "[net0]\naddress = 192.168.5.2/24\nrouter = 192.168.5.254\ndhcp-server"
            ),
            "{new}"
        );
    }

    #[test]
    fn what_was_wrong_before_does_not_stop_a_change_and_what_the_change_breaks_does() {
        let text = "[net0]\nipv6 = maybe\naddress = 10.0.2.9/24\n";
        let fixed = checked(&["net0", "fixed", "10.0.2.15/24"]);
        assert!(planned(text, &net0(), &fixed).is_ok());
        assert_eq!(wrong_of(text), ["'maybe' is not a ipv6 here"]);
    }

    #[test]
    fn the_change_is_said_in_a_line() {
        let fixed = checked(&[
            "net0",
            "fixed",
            "10.0.2.15/24",
            "2001:db8::5/64",
            "router",
            "10.0.2.2",
            "dns",
            "1.1.1.1",
            "metered",
            "no",
        ]);
        assert_eq!(
            said_of(&net0(), &fixed),
            "net0: fixed 10.0.2.15/24 2001:db8::5/64, router 10.0.2.2, dns 1.1.1.1, metered no"
        );
        assert_eq!(said_of(&net0(), &checked(&["net0", "dhcp"])), "net0: by DHCP");
    }
}
