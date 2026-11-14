// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Redaction (ADR 0531): what leaves a report names no person and no
//! machine. [`Redactor::redact`] goes over every line of a text, the
//! person's own words too, and replaces each piece that names someone
//! with a mark the preview shows:
//!
//! | mark        | what it replaces                                          |
//! |-------------|-----------------------------------------------------------|
//! | `[user]`    | the name of a person's account, and the name in `/users/` |
//! | `[host]`    | the machine's name and the names in the hosts file        |
//! | `[IPv4]`    | an IPv4 address, but loopback's and `0.0.0.0`             |
//! | `[IPv6]`    | an IPv6 address, but `::1` and `::`                       |
//! | `[MAC]`     | a hardware address                                        |
//! | `[email]`   | an email address                                          |
//! | `[Wi-Fi]`   | the name of a Wi-Fi network the machine knows             |
//! | `[serial]`  | a serial number of the machine, a disk or a USB device    |
//! | `[machine]` | the machine ID, if the machine has one                    |
//!
//! reportd(8) redacts as root, as only root reads the account files and
//! the serial numbers: it makes the draft's text, redacts it, and redacts
//! what the person edited. Names are cut as whole words, in any case, so
//! that the user `ann` leaves `channel` alone, and a version number such
//! as `1.29.0` is never an address: an address has four parts. A mark
//! already in the text stays as it is and is not counted, which makes
//! redaction repeatable: its second run cuts nothing.
//!
//! When a mark is missing from a report, `Redactor::from_system` did not
//! read the name: see the readers of this file, one for each list.

use std::cmp::Reverse;
use std::fs;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::Path;

use crate::{diskhealth, hwinfo, lsusb};

const USER: &str = "[user]";
const HOST: &str = "[host]";
const IPV4: &str = "[IPv4]";
const IPV6: &str = "[IPv6]";
const MAC: &str = "[MAC]";
const EMAIL: &str = "[email]";
const WIFI: &str = "[Wi-Fi]";
const SERIAL: &str = "[serial]";
const MACHINE: &str = "[machine]";

/// Every mark, to leave the ones already in a text alone.
const MARKS: [&str; 9] = [USER, HOST, IPV4, IPV6, MAC, EMAIL, WIFI, SERIAL, MACHINE];

/// The folder the people's homes are in, `/users/<name>`.
const HOMES: &str = "/users/";

/// The shortest name cut as a word. A name of one or two letters would cut
/// ordinary words, so it is cut in a home folder's path only.
const WORD_MIN: usize = 3;

/// The shortest serial number or machine ID worth cutting.
const SECRET_MIN: usize = 5;

/// The machine's name before anyone names it (hostname(5)): every machine
/// has it, so it says nothing about this one.
const DEFAULT_HOST: &str = "dukovo";

/// Where the C library reads the names it knows without asking DNS.
const HOSTS: &str = "/etc/hosts";

/// Where a program that makes a machine ID keeps it, as systemd and D-Bus
/// do. Dukovo makes none itself.
const MACHINE_ID: &str = "/etc/machine-id";

/// What firmware fills a serial number with when the maker left it out.
const PLACEHOLDERS: [&str; 12] = [
    "none",
    "n/a",
    "unknown",
    "default string",
    "not specified",
    "not available",
    "not applicable",
    "to be filled by o.e.m.",
    "system serial number",
    "chassis serial number",
    "no asset tag",
    "no asset information",
];

/// The names a text is cut by.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lists {
    /// The names of the people's accounts.
    pub users: Vec<String>,
    /// The machine's names.
    pub hosts: Vec<String>,
    /// The Wi-Fi networks the machine knows.
    pub wifi: Vec<String>,
    /// The serial numbers of the machine and its devices.
    pub serials: Vec<String>,
    /// The machine ID.
    pub machine: Option<String>,
}

/// A name to cut and the mark that replaces it, in lowercase ASCII for the
/// comparison.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Word {
    needle: String,
    mark: &'static str,
}

/// Cuts what names a person or a machine out of a text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redactor {
    words: Vec<Word>,
}

/// A span of text one rule cuts.
struct Cut {
    start: usize,
    end: usize,
    mark: &'static str,
}

/// A text in pieces: what is left to look at, and the marks already put.
enum Piece<'a> {
    Text(&'a str),
    Mark(&'static str),
}

impl Redactor {
    /// A redactor for the names in `lists`.
    pub fn new(lists: Lists) -> Redactor {
        let mut words = Vec::new();
        let named = |names: &[String], mark, min| {
            names
                .iter()
                .filter(|name| name.chars().count() >= min)
                .map(|name| Word { needle: name.to_ascii_lowercase(), mark })
                .collect::<Vec<_>>()
        };
        words.extend(named(&lists.users, USER, WORD_MIN));
        words.extend(named(&lists.hosts, HOST, WORD_MIN));
        words.extend(named(&lists.wifi, WIFI, WORD_MIN));
        words.extend(named(&lists.serials, SERIAL, 1));
        words.extend(named(&lists.machine.into_iter().collect::<Vec<_>>(), MACHINE, 1));
        Redactor { words }
    }

    /// The redactor for this machine: the names of the people's accounts,
    /// the machine's names, the Wi-Fi networks it knows, its serial
    /// numbers and its machine ID. It reads files only root reads, so it
    /// runs in reportd.
    pub fn from_system() -> Redactor {
        let passwd = fs::read_to_string(accounts::PASSWD).unwrap_or_default();
        let hosts = fs::read_to_string(HOSTS).unwrap_or_default();
        Redactor::new(Lists {
            users: person_names(&passwd),
            hosts: host_names(&crate::util::hostname(), &hosts),
            wifi: wifi_names(),
            serials: serial_numbers(),
            machine: machine_id(),
        })
    }

    /// `text` with each piece that names someone replaced by its mark, and
    /// how many pieces that was.
    pub fn redact(&self, text: &str) -> (String, u32) {
        let mut cuts = 0;
        let mut pieces = split_marks(text);
        pieces = apply(pieces, &mut cuts, emails);
        pieces = apply(pieces, &mut cuts, hardware_addresses);
        pieces = apply(pieces, &mut cuts, ipv6_addresses);
        pieces = apply(pieces, &mut cuts, ipv4_addresses);
        pieces = apply(pieces, &mut cuts, homes);
        pieces = apply(pieces, &mut cuts, |text| self.words(text));
        let text = pieces
            .iter()
            .map(|piece| match piece {
                Piece::Text(text) => *text,
                Piece::Mark(mark) => *mark,
            })
            .collect();
        (text, cuts)
    }

    /// The names of the lists in `text`, as whole words in any case; where
    /// two overlap, the one that starts first, then the longer, is cut.
    fn words(&self, text: &str) -> Vec<Cut> {
        let lower = text.to_ascii_lowercase();
        let mut found = Vec::new();
        for word in &self.words {
            for (start, _) in lower.match_indices(word.needle.as_str()) {
                let end = start + word.needle.len();
                if whole_word(&lower, start, end) {
                    found.push(Cut { start, end, mark: word.mark });
                }
            }
        }
        found.sort_by_key(|cut| (cut.start, Reverse(cut.end - cut.start)));
        let mut kept: Vec<Cut> = Vec::new();
        for cut in found {
            if kept.last().is_none_or(|last| cut.start >= last.end) {
                kept.push(cut);
            }
        }
        kept
    }
}

/// `text` as text and the marks it holds already.
fn split_marks(text: &str) -> Vec<Piece<'_>> {
    let mut pieces = Vec::new();
    let mut from = 0;
    let mut at = 0;
    while let Some(open) = text[at..].find('[') {
        let open = at + open;
        match MARKS.iter().find(|mark| text[open..].starts_with(**mark)) {
            Some(mark) => {
                if open > from {
                    pieces.push(Piece::Text(&text[from..open]));
                }
                pieces.push(Piece::Mark(mark));
                at = open + mark.len();
                from = at;
            }
            None => at = open + 1,
        }
    }
    if from < text.len() {
        pieces.push(Piece::Text(&text[from..]));
    }
    pieces
}

/// The text pieces of `pieces` with what `rule` finds in each cut out, and
/// the number of cuts added to `count`. A rule gives its cuts in order and
/// apart.
fn apply<'a>(
    pieces: Vec<Piece<'a>>,
    count: &mut u32,
    rule: impl Fn(&str) -> Vec<Cut>,
) -> Vec<Piece<'a>> {
    let mut out = Vec::new();
    for piece in pieces {
        let Piece::Text(text) = piece else {
            out.push(piece);
            continue;
        };
        let mut at = 0;
        for cut in rule(text) {
            if cut.start > at {
                out.push(Piece::Text(&text[at..cut.start]));
            }
            out.push(Piece::Mark(cut.mark));
            *count += 1;
            at = cut.end;
        }
        if at < text.len() {
            out.push(Piece::Text(&text[at..]));
        }
    }
    out
}

/// Whether a character belongs to a word: names are cut whole.
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether `text[start..end]` is not part of a longer word, at the sides
/// where it starts or ends with a word character.
fn whole_word(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    let first = text[start..end].chars().next();
    let last = text[start..end].chars().next_back();
    let apart = |inside: Option<char>, outside: Option<char>| {
        !inside.is_some_and(is_word) || !outside.is_some_and(is_word)
    };
    apart(first, before) && apart(last, after)
}

/// Whether a byte of an email address's part before the `@`. Bytes of
/// multi-byte characters count, as internationalized addresses have them.
fn is_local(byte: u8) -> bool {
    byte >= 0x80 || byte.is_ascii_alphanumeric() || b"._%+-".contains(&byte)
}

/// Email addresses: a name, `@`, and a domain with a dot in it.
fn emails(text: &str) -> Vec<Cut> {
    let bytes = text.as_bytes();
    let mut cuts: Vec<Cut> = Vec::new();
    for (at, _) in text.match_indices('@') {
        let floor = cuts.last().map_or(0, |last| last.end);
        let start =
            bytes[floor..at].iter().rposition(|&b| !is_local(b)).map_or(floor, |p| floor + p + 1);
        let domain = bytes[at + 1..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || **b == b'.' || **b == b'-')
            .count();
        let domain = text[at + 1..at + 1 + domain].trim_end_matches(['.', '-']);
        let labels: Vec<&str> = domain.split('.').collect();
        let last_is_name = labels
            .last()
            .is_some_and(|l| l.len() >= 2 && l.bytes().all(|b| b.is_ascii_alphabetic()));
        if start < at && labels.len() >= 2 && labels.iter().all(|l| !l.is_empty()) && last_is_name {
            cuts.push(Cut { start, end: at + 1 + domain.len(), mark: EMAIL });
        }
    }
    cuts
}

/// Whether `bytes` at `at` are a hardware address, six pairs of hexadecimal
/// digits with the same `:` or `-` between them, and whether it stands
/// alone: no letter, digit or the separator on either side, so that part
/// of a longer list of bytes, as a fingerprint is, is not cut.
fn hardware_address_at(bytes: &[u8], at: usize) -> bool {
    let Some(window) = bytes.get(at..at + 17) else { return false };
    let separator = window[2];
    let pairs = window.chunks(3).enumerate().all(|(n, chunk)| {
        chunk[0].is_ascii_hexdigit()
            && chunk[1].is_ascii_hexdigit()
            && (n == 5 || chunk[2] == separator)
    });
    let apart =
        |byte: Option<&u8>| byte.is_none_or(|b| !b.is_ascii_alphanumeric() && *b != separator);
    pairs
        && (separator == b':' || separator == b'-')
        && apart(at.checked_sub(1).and_then(|before| bytes.get(before)))
        && apart(bytes.get(at + 17))
}

fn hardware_addresses(text: &str) -> Vec<Cut> {
    let bytes = text.as_bytes();
    let mut cuts = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if hardware_address_at(bytes, at) {
            cuts.push(Cut { start: at, end: at + 17, mark: MAC });
            at += 17;
        } else {
            at += 1;
        }
    }
    cuts
}

/// The bytes of an IPv6 address's text, with the IPv4 tail it may end in.
fn is_ipv6_byte(byte: u8) -> bool {
    byte.is_ascii_hexdigit() || byte == b':' || byte == b'.'
}

/// How long the IPv6 address at the start of `run` is, leaving out the
/// full stop or colon that ends a sentence or a log's field after it; none
/// when it is not one, or one that stays: `::1` and `::`. An address has a
/// digit, so that a path such as `a::b` is none.
fn ipv6_length(run: &str) -> Option<usize> {
    let mut candidate = run;
    loop {
        if let Ok(address) = candidate.parse::<Ipv6Addr>() {
            let kept = address.is_loopback() || address.is_unspecified();
            let digits = candidate.bytes().any(|b| b.is_ascii_digit());
            return (!kept && digits).then_some(candidate.len());
        }
        candidate = candidate.strip_suffix(['.', ':'])?;
    }
}

fn ipv6_addresses(text: &str) -> Vec<Cut> {
    let bytes = text.as_bytes();
    let mut cuts = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if !is_ipv6_byte(bytes[at]) {
            at += 1;
            continue;
        }
        let start = at;
        while at < bytes.len() && is_ipv6_byte(bytes[at]) {
            at += 1;
        }
        let before = text[..start].chars().next_back();
        let after = text[at..].chars().next();
        let alone = !before.is_some_and(is_word) && !after.is_some_and(is_word);
        if let Some(length) = ipv6_length(&text[start..at]).filter(|_| alone) {
            cuts.push(Cut { start, end: start + length, mark: IPV6 });
        }
    }
    cuts
}

/// Whether `run` is an IPv4 address that stays or goes: four parts of one
/// to three digits, each up to 255, and not loopback's or `0.0.0.0`.
/// None when it is not an address at all.
fn ipv4_goes(run: &str) -> Option<bool> {
    let parts: Vec<&str> = run.split('.').collect();
    if parts.len() != 4 || parts.iter().any(|p| p.is_empty() || p.len() > 3) {
        return None;
    }
    let octets: Vec<u8> = parts.iter().map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let address = Ipv4Addr::new(octets[0], octets[1], octets[2], octets[3]);
    Some(!address.is_loopback() && !address.is_unspecified())
}

fn ipv4_addresses(text: &str) -> Vec<Cut> {
    let bytes = text.as_bytes();
    let mut cuts = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if !bytes[at].is_ascii_digit() {
            at += 1;
            continue;
        }
        let start = at;
        while at < bytes.len() && (bytes[at].is_ascii_digit() || bytes[at] == b'.') {
            at += 1;
        }
        // A full stop after the address ends a sentence.
        let run = text[start..at].trim_end_matches('.');
        let before = text[..start].chars().next_back();
        let after = text[start + run.len()..].chars().next();
        // A version such as `v1.2.3.4` or `libfoo-1.2.3.4` has a name before it.
        let alone = !before.is_some_and(|c| is_word(c) || c == '-') && !after.is_some_and(is_word);
        if alone && ipv4_goes(run) == Some(true) {
            cuts.push(Cut { start, end: start + run.len(), mark: IPV4 });
        }
    }
    cuts
}

/// The name of a home in `/users/`, not one that starts with a dot, as
/// `/users/.removed` does: it is no account's.
fn homes(text: &str) -> Vec<Cut> {
    let mut cuts = Vec::new();
    for (at, _) in text.match_indices(HOMES) {
        let inside_a_longer_path =
            text[..at].chars().next_back().is_some_and(|c| is_word(c) || "-.~".contains(c));
        let start = at + HOMES.len();
        let name = text[start..]
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || b"_-.".contains(b))
            .count();
        let name = text[start..start + name].trim_end_matches('.');
        if !inside_a_longer_path && !name.is_empty() && !name.starts_with('.') {
            cuts.push(Cut { start, end: start + name.len(), mark: USER });
        }
    }
    cuts
}

/// The names of the people's accounts in the passwd file `text`: those of
/// persons, not root's or a service's, whose names are ordinary words.
fn person_names(passwd: &str) -> Vec<String> {
    accounts::users(passwd)
        .entries()
        .filter(|user| user.is_person())
        .map(|user| user.name.clone())
        .collect()
}

/// The machine's names: its own, whole and as its first label, and those
/// in the hosts file `hosts`, but the default name and the loopback's.
fn host_names(hostname: &str, hosts: &str) -> Vec<String> {
    let mut names = vec![hostname.to_string()];
    names.extend(hostname.split('.').next().map(str::to_string));
    for line in hosts.lines() {
        let line = line.split('#').next().unwrap_or_default();
        names.extend(
            line.split_whitespace()
                .skip(1)
                .filter(|name| name.parse::<std::net::IpAddr>().is_err())
                .map(str::to_string),
        );
    }
    names.retain(|name| {
        let lower = name.to_ascii_lowercase();
        lower != DEFAULT_HOST && !lower.contains("localhost") && !lower.starts_with("ip6-")
    });
    names.sort();
    names.dedup();
    names
}

/// The names of the Wi-Fi networks the machine knows. Dukovo has no Wi-Fi
/// yet (docs/deferred.md, Phase 17), so it keeps no network's name and the
/// list is empty; where its settings are read comes here with the driver.
fn wifi_names() -> Vec<String> {
    Vec::new()
}

/// Whether `value` is a serial number or ID that is worth cutting: long
/// enough, not one character repeated, as an unset UUID is, and not what
/// firmware writes when the maker left it out.
fn is_secret(value: &str) -> bool {
    let digits: Vec<char> = value.chars().filter(|c| *c != '-').collect();
    digits.len() >= SECRET_MIN
        && digits.iter().any(|c| *c != digits[0])
        && !PLACEHOLDERS.contains(&value.to_ascii_lowercase().as_str())
}

/// The serial numbers the machine has: the firmware's (SMBIOS) for the
/// machine, its board and its chassis, and the serial numbers of its disks
/// and USB devices.
fn serial_numbers() -> Vec<String> {
    let mut serials = Vec::new();
    for field in [
        "product_serial",
        "board_serial",
        "chassis_serial",
        "product_uuid",
        "board_asset_tag",
        "chassis_asset_tag",
    ] {
        serials.extend(read_line(&Path::new(hwinfo::DMI).join(field)));
    }
    for disk in diskhealth::disks().unwrap_or_default() {
        serials.push(diskhealth::device_text(&disk, "serial"));
    }
    for device in fs::read_dir(lsusb::DEVICES).into_iter().flatten().flatten() {
        serials.extend(read_line(&device.path().join("serial")));
    }
    serials.retain(|serial| is_secret(serial));
    serials.sort();
    serials.dedup();
    serials
}

/// The machine ID, if a program made one.
fn machine_id() -> Option<String> {
    read_line(Path::new(MACHINE_ID)).filter(|id| is_secret(id))
}

/// The first line of a file, trimmed, if it has one.
fn read_line(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    Some(text.lines().next()?.trim().to_string()).filter(|line| !line.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(users: &[&str], hosts: &[&str]) -> Redactor {
        let own = |names: &[&str]| names.iter().map(|n| (*n).to_string()).collect();
        Redactor::new(Lists { users: own(users), hosts: own(hosts), ..Lists::default() })
    }

    /// Each row: the text, what redacting it gives, and how many cuts.
    fn check(redactor: &Redactor, table: &[(&str, &str, u32)]) {
        for (text, wanted, cuts) in table {
            let (got, counted) = redactor.redact(text);
            assert_eq!((got.as_str(), counted), (*wanted, *cuts), "redacting {text:?}");
            // Redacting what is redacted cuts nothing more.
            assert_eq!(redactor.redact(&got), (got.clone(), 0), "redacting {got:?} again");
        }
    }

    #[test]
    fn a_users_name_is_cut_as_a_whole_word_in_any_case() {
        check(
            &names(&["ann", "bob.smith"], &[]),
            &[
                ("ann signed in", "[user] signed in", 1),
                ("Ann and ANN: ann's", "[user] and [user]: [user]'s", 3),
                ("the channel annals", "the channel annals", 0),
                ("ann_1 anna annann", "ann_1 anna annann", 0),
                ("ann-marie", "[user]-marie", 1),
                ("bob.smith@work", "[user]@work", 1),
                ("bob smith", "bob smith", 0),
                ("café ann\u{e9}", "café ann\u{e9}", 0),
            ],
        );
    }

    #[test]
    fn the_longer_of_two_overlapping_names_is_cut_once() {
        check(&names(&["ann", "annabel"], &[]), &[("annabel", "[user]", 1)]);
        check(&names(&["ann", "ann.lee"], &[]), &[("ann.lee wrote", "[user] wrote", 1)]);
    }

    #[test]
    fn a_name_of_one_or_two_letters_is_cut_in_a_path_only() {
        check(
            &names(&["al", "x"], &[]),
            &[
                ("al wrote a fix for x", "al wrote a fix for x", 0),
                ("/users/al/documents", "/users/[user]/documents", 1),
            ],
        );
    }

    #[test]
    fn homes_are_cut_whoever_lives_there() {
        check(
            &Redactor::new(Lists::default()),
            &[
                ("/users/alice/documents/a.txt", "/users/[user]/documents/a.txt", 1),
                ("cd /users/bob.", "cd /users/[user].", 1),
                ("/users/.removed/old", "/users/.removed/old", 0),
                ("/system/users/alice", "/system/users/alice", 0),
                ("/users/", "/users/", 0),
                ("/users/a /users/b", "/users/[user] /users/[user]", 2),
            ],
        );
    }

    #[test]
    fn the_hosts_names_are_cut_and_the_default_name_is_not_one() {
        let hosts = "# written by hand\n127.0.0.1 localhost\n::1 ip6-localhost ip6-loopback\n\
                     192.168.1.5 nas.home.lan nas # the NAS\n";
        let listed = host_names("studio.example.com", hosts);
        assert_eq!(listed, ["nas", "nas.home.lan", "studio", "studio.example.com"]);
        assert!(host_names("dukovo", "127.0.0.1 localhost\n").is_empty());
        let redactor = Redactor::new(Lists { hosts: listed, ..Lists::default() });
        check(
            &redactor,
            &[
                ("studio.example.com is up", "[host] is up", 1),
                ("nas: Studio mounted nas.home.lan", "[host]: [host] mounted [host]", 3),
                ("dukovo problem report", "dukovo problem report", 0),
            ],
        );
    }

    #[test]
    fn an_ipv4_address_is_cut_but_not_loopback_or_a_version() {
        check(
            &Redactor::new(Lists::default()),
            &[
                ("from 192.168.1.20 to 10.0.2.15.", "from [IPv4] to [IPv4].", 2),
                ("(10.0.2.15:8080)", "([IPv4]:8080)", 1),
                (
                    "lo 127.0.0.1 any 0.0.0.0 also 127.1.2.3",
                    "lo 127.0.0.1 any 0.0.0.0 also 127.1.2.3",
                    0,
                ),
                ("nginx/1.29.0 and 0.16.0", "nginx/1.29.0 and 0.16.0", 0),
                ("v1.2.3.4 libfoo-1.2.3.4 1.2.3.4.5", "v1.2.3.4 libfoo-1.2.3.4 1.2.3.4.5", 0),
                ("256.1.1.1 1.2.3 1..2.3 1.2.3.4x", "256.1.1.1 1.2.3 1..2.3 1.2.3.4x", 0),
                ("8.8.8.8", "[IPv4]", 1),
            ],
        );
    }

    #[test]
    fn an_ipv6_address_is_cut_but_not_loopback_a_time_or_a_path() {
        check(
            &Redactor::new(Lists::default()),
            &[
                ("2001:db8::1 and fe80::1ff:fe23:4567:890a", "[IPv6] and [IPv6]", 2),
                ("addr 2001:db8::1: refused", "addr [IPv6]: refused", 1),
                ("[2001:db8::1]:443", "[[IPv6]]:443", 1),
                ("2001:0db8:85a3:0000:0000:8a2e:0370:7334.", "[IPv6].", 1),
                ("::ffff:192.168.1.1", "[IPv6]", 1),
                ("lo ::1 and :: ", "lo ::1 and :: ", 0),
                ("at 10:02:11 and 10:02", "at 10:02:11 and 10:02", 0),
                ("std::fmt::Debug a::b dead::beef", "std::fmt::Debug a::b dead::beef", 0),
            ],
        );
    }

    #[test]
    fn a_hardware_address_is_cut_in_either_form_and_not_inside_a_longer_one() {
        check(
            &Redactor::new(Lists::default()),
            &[
                ("net0 52:54:00:12:34:56 up", "net0 [MAC] up", 1),
                ("AA-BB-CC-DD-EE-FF", "[MAC]", 1),
                ("aa:bb:cc:dd:ee:ff:00:11:22", "aa:bb:cc:dd:ee:ff:00:11:22", 0),
                ("xaa:bb:cc:dd:ee:ff", "xaa:bb:cc:dd:ee:ff", 0),
                ("aa:bb-cc:dd:ee:ff", "aa:bb-cc:dd:ee:ff", 0),
                ("(52:54:00:12:34:56)", "([MAC])", 1),
            ],
        );
    }

    #[test]
    fn an_email_address_is_cut_with_its_name_and_its_domain() {
        check(
            &names(&["ann"], &[]),
            &[
                ("write to ann@example.com.", "write to [email].", 1),
                ("a.b+c@sub.example.org and x@y.io", "[email] and [email]", 2),
                ("ann@dukovo and @mention and a@b", "[user]@dukovo and @mention and a@b", 1),
                ("jos\u{e9}@example.es", "[email]", 1),
                ("price 5@3.5", "price 5@3.5", 0),
            ],
        );
    }

    #[test]
    fn a_wifi_name_is_cut_with_its_spaces_and_in_any_case() {
        let redactor = Redactor::new(Lists {
            wifi: vec!["Home Net 5G".into(), "x".into()],
            ..Lists::default()
        });
        check(
            &redactor,
            &[
                ("joined home net 5g at 10:02", "joined [Wi-Fi] at 10:02", 1),
                ("ssid=\"Home Net 5G\"", "ssid=\"[Wi-Fi]\"", 1),
                ("Home Net 5Gx", "Home Net 5Gx", 0),
                ("x marks", "x marks", 0),
            ],
        );
    }

    #[test]
    fn serial_numbers_and_the_machine_id_are_cut() {
        let redactor = Redactor::new(Lists {
            serials: vec!["ABC12345XYZ".into(), "WD-WX11A".into()],
            machine: Some("0123456789abcdef0123456789abcdef".into()),
            ..Lists::default()
        });
        check(
            &redactor,
            &[
                ("S/N abc12345xyz, disk WD-WX11A", "S/N [serial], disk [serial]", 2),
                ("id 0123456789ABCDEF0123456789abcdef", "id [machine]", 1),
                ("ABC12345XYZ1", "ABC12345XYZ1", 0),
            ],
        );
    }

    #[test]
    fn serials_that_are_placeholders_or_unset_are_not_worth_cutting() {
        for value in [
            "",
            "0",
            "None",
            "N/A",
            "Default string",
            "To Be Filled By O.E.M.",
            "Not Specified",
            "00000000-0000-0000-0000-000000000000",
            "FFFFFFFF",
            "1234",
        ] {
            assert!(!is_secret(value), "{value:?}");
        }
        for value in ["S3Z2NB0K123456", "4c4c4544-0042-4410-8052-b7c04f525632", "ABCDE"] {
            assert!(is_secret(value), "{value:?}");
        }
    }

    #[test]
    fn only_persons_accounts_are_named() {
        let passwd = "root:x:0:0:Administrator:/:/system/commands/nukesh\n\
                      web:x:300:300::/:/system/commands/false\n\
                      alice:x:1000:1000:Alice Liddell:/users/alice:/system/commands/nukesh\n\
                      bob:x:1001:1001::/users/bob:/system/commands/nukesh\n";
        assert_eq!(person_names(passwd), ["alice", "bob"]);
    }

    #[test]
    fn marks_already_in_the_text_stay_and_are_not_counted() {
        let redactor = names(&["user", "host", "ann"], &["host"]);
        check(
            &redactor,
            &[
                (
                    "[user] [host] [IPv4] [IPv6] [MAC] [email] [Wi-Fi] [serial] [machine]",
                    "[user] [host] [IPv4] [IPv6] [MAC] [email] [Wi-Fi] [serial] [machine]",
                    0,
                ),
                ("[user] and ann and [", "[user] and [user] and [", 1),
                ("[usr] [users] 10.0.0.1", "[usr] [users] [IPv4]", 1),
            ],
        );
    }

    #[test]
    fn several_kinds_in_one_text_are_each_counted() {
        let redactor = names(&["alice"], &["studio"]);
        let text = "alice@studio has /users/alice on 10.1.2.3 (52:54:00:12:34:56), mail a@b.org";
        let (got, cuts) = redactor.redact(text);
        assert_eq!(got, "[user]@[host] has /users/[user] on [IPv4] ([MAC]), mail [email]");
        assert_eq!(cuts, 6);
    }

    #[test]
    fn a_text_with_nothing_to_cut_comes_back_as_it_was() {
        let text = "Dukovo problem report\nVersion: 0.16.0 (stable, server)\n\n== Crash\n \
                    0 ngx_http_core_run_phases+0x4c  nginx 1.29.0 +0x5a2c1c  build 7be951e4\n";
        let (got, cuts) = names(&["alice"], &["studio"]).redact(text);
        assert_eq!((got.as_str(), cuts), (text, 0));
    }
}
