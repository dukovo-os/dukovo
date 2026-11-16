// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What `cert get` and certd check before the authority is asked (ADR 0513),
//! so that a mistake is told in plain words that name what to fix, not
//! after minutes of waiting and the authority's own wording.
//!
//! [`findings`] judges facts it is given, so that each rule is tested alone,
//! and says what it found of each thing, including what passed; [`check`]
//! keeps what a command needs to stop, the failures and the warnings;
//! [`gather`] reads the facts from this machine: the resolver, the
//! interfaces and the firewall's settings. `cert get --check` shows the
//! findings as they are.
//!
//! Under `--dns` the authority never connects to this machine, so only the
//! clock is checked: advice to "use --dns" would lead in a circle.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::path::Path;

use tuf::url::Url;

use super::super::machine;
use crate::firewall;
use crate::ip;

/// Where the firewall's rules are read from (firewall(8)).
const FIREWALL_SETTINGS: &str = "/system/settings/firewall";

/// What is known of the machine and the names before an order.
pub struct Facts<'a> {
    /// The time now, in seconds since 1970.
    pub now: i64,
    /// The DNS names asked for, with the addresses each leads to.
    pub names: &'a [(String, Vec<IpAddr>)],
    /// The answers are served on port 80 (HTTP-01), not by hand in DNS.
    pub http: bool,
    /// The authority is on the internet, not a private network.
    pub public_authority: bool,
    /// This machine's IPv6 addresses that are not link-local, unique local
    /// or loopback.
    pub own_ipv6: Vec<Ipv6Addr>,
    /// The firewall's settings, as the file holds them.
    pub firewall: &'a str,
    /// Whether what listens on port 80 has the inbound right: a service
    /// that names the port has, a command of an administrator has not.
    pub with_inbound: bool,
    /// Who holds port 80, when the command asks: `cert get` finds out
    /// when it binds the port, `cert get --check` asks beforehand.
    pub holder: Option<Holder>,
}

/// Who holds port 80, as far as a person who cannot bind it can tell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Holder {
    /// No socket listens on it: the order answers the authority itself.
    Free,
    /// nginx does, and an order writes its answers where nginx serves them.
    Nginx,
    /// certd does, for a renewal that runs now.
    Certd,
    /// Another program does: the service that names the port, or none.
    Other(String),
    /// A socket listens, and who it belongs to cannot be told.
    Unknown,
}

/// How a check came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Warning,
    Failed,
    /// Not made, because it does not apply to the way the names are proved.
    Skipped,
}

impl Outcome {
    /// The word `cert get --check` uses.
    pub fn word(self) -> &'static str {
        match self {
            Outcome::Passed => "passed",
            Outcome::Warning => "warning",
            Outcome::Failed => "failed",
            Outcome::Skipped => "skipped",
        }
    }
}

/// What one check found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// The error page of what is wrong, for a warning or a failure that has
    /// one.
    pub id: Option<&'static str>,
    pub outcome: Outcome,
    /// The DNS name it is about, none for a check of the machine.
    pub name: Option<String>,
    pub words: String,
}

impl Finding {
    fn new(id: Option<&'static str>, outcome: Outcome, name: Option<&str>, words: String) -> Self {
        Finding { id, outcome, name: name.map(str::to_string), words }
    }
}

/// What [`check`] found: each thing to fix, and each thing to know.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Verdict {
    pub failures: Vec<String>,
    /// The failures, each with the ID of its page at the end in parentheses
    /// when it has one, as the history keeps them.
    pub failures_with_ids: Vec<String>,
    pub warnings: Vec<String>,
}

/// Whether `ip` is an address that only a private network, this machine
/// or its own link reaches: 10/8, 172.16/12, 192.168/16, 100.64/10, 127/8,
/// 169.254/16, the unspecified address, fc00::/7, fe80::/10 and `::1`.
pub fn private_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => private_v4(ip),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(mapped) => private_v4(mapped),
            None => private_v6(ip),
        },
    }
}

fn private_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_unspecified()
        || a == 10
        || a == 127
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
}

fn private_v6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    ip.is_unspecified() || ip.is_loopback() || first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80
}

/// What the checks found, each in the order it was made: the clock when it
/// is wrong; for each name what it leads to; port 80 in the firewall; and
/// who holds port 80.
pub fn findings(facts: &Facts<'_>) -> Vec<Finding> {
    let mut found = Vec::new();
    if let Some(why) = machine::wrong_clock(facts.now) {
        found.push(Finding::new(
            Some("CRT-105"),
            Outcome::Failed,
            None,
            format!("{why}: set it, then try again"),
        ));
    }
    if !facts.http {
        let words = |what: &str| {
            format!(
                "{what}: the authority never connects to this machine when the names are proved by DNS"
            )
        };
        for (name, _) in facts.names {
            found.push(Finding::new(
                None,
                Outcome::Skipped,
                Some(name),
                words(&format!("{name} is not looked up")),
            ));
        }
        found.push(Finding::new(None, Outcome::Skipped, None, words("port 80 is not checked")));
        return found;
    }
    for (name, addresses) in facts.names {
        found.extend(name_findings(facts, name, addresses));
    }
    let opening = firewall::lets_tcp_in(facts.firewall, 80, facts.with_inbound);
    found.push(if opening.open {
        let by = match opening.rule {
            Some(rule) => format!("firewall rule {rule}"),
            None => String::from("no firewall rule closes it"),
        };
        Finding::new(None, Outcome::Passed, None, format!("port 80 is open ({by})"))
    } else {
        Finding::new(
            Some("CRT-103"),
            Outcome::Failed,
            None,
            String::from(
                "the firewall closes port 80, which the authority must reach: open it with \
                 `elevate firewall allow tcp 80`",
            ),
        )
    });
    if let Some(holder) = &facts.holder {
        found.push(holder_finding(holder));
    }
    found
}

/// What the checks of one name found: that it leads somewhere, that the
/// authority can reach where, and that its IPv6 addresses are this
/// machine's.
fn name_findings(facts: &Facts<'_>, name: &str, addresses: &[IpAddr]) -> Vec<Finding> {
    if addresses.is_empty() {
        return vec![Finding::new(
            Some("CRT-101"),
            Outcome::Failed,
            Some(name),
            format!(
                "{name} does not resolve to an address: give it an A record (and AAAA for IPv6) \
                 that leads to this machine"
            ),
        )];
    }
    let mut found = Vec::new();
    let private =
        addresses.iter().find(|address| facts.public_authority && private_address(**address));
    found.push(match private {
        Some(address) => Finding::new(
            Some("CRT-102"),
            Outcome::Failed,
            Some(name),
            format!(
                "{name} leads to {address}, a private address the authority cannot reach: give \
                 it this machine's public address, or use --dns"
            ),
        ),
        None => {
            let list: Vec<String> = addresses.iter().map(IpAddr::to_string).collect();
            Finding::new(
                None,
                Outcome::Passed,
                Some(name),
                format!("{name} leads to {}", list.join(", ")),
            )
        }
    });
    let v6: Vec<Ipv6Addr> = addresses
        .iter()
        .filter_map(|address| match address {
            IpAddr::V6(address) => Some(*address),
            IpAddr::V4(_) => None,
        })
        .collect();
    if let Some(first) = v6.first()
        && !v6.iter().any(|address| facts.own_ipv6.contains(address))
    {
        let words = match facts.own_ipv6.first() {
            None => format!(
                "{name} has an IPv6 address and this machine has none: if {first} is not this \
                 machine, the authority, which tries IPv6 first, may not reach it"
            ),
            Some(own) => {
                let own_all: Vec<String> = facts.own_ipv6.iter().map(Ipv6Addr::to_string).collect();
                format!(
                    "{name}'s AAAA record points to {first}, which is not this machine (it has \
                     {}): the authority tries IPv6 first, so it would check that machine: remove \
                     the AAAA record or point it to {own}",
                    own_all.join(", ")
                )
            }
        };
        found.push(Finding::new(Some("CRT-104"), Outcome::Warning, Some(name), words));
    }
    found
}

/// What the check of who holds port 80 found.
fn holder_finding(holder: &Holder) -> Finding {
    let (id, outcome, words) = match holder {
        Holder::Free => (
            None,
            Outcome::Passed,
            String::from("port 80 is free: the order answers the authority itself"),
        ),
        Holder::Nginx => {
            (None, Outcome::Passed, String::from("nginx holds port 80 and serves the answer"))
        }
        Holder::Certd => (
            None,
            Outcome::Warning,
            String::from(
                "certd holds port 80 while it renews a certificate: if the order fails, try \
                 again in a minute",
            ),
        ),
        Holder::Other(program) => (
            Some("CRT-106"),
            Outcome::Failed,
            format!(
                "port 80 is held by {program}, which is not nginx or certd: stop it for the few \
                 minutes the order takes, or use --dns"
            ),
        ),
        Holder::Unknown => (
            None,
            Outcome::Warning,
            String::from(
                "a program holds port 80 and what it is cannot be told from here: if it is not \
                 nginx, the order fails",
            ),
        ),
    };
    Finding::new(id, outcome, None, words)
}

/// Judges `facts`: what stops an order and what a person should know.
pub fn check(facts: &Facts<'_>) -> Verdict {
    let mut verdict = Verdict::default();
    for finding in findings(facts) {
        match finding.outcome {
            Outcome::Failed => {
                verdict.failures_with_ids.push(match finding.id {
                    Some(id) => format!("{} ({id})", finding.words),
                    None => finding.words.clone(),
                });
                verdict.failures.push(finding.words);
            }
            Outcome::Warning => verdict.warnings.push(finding.words),
            Outcome::Passed | Outcome::Skipped => {}
        }
    }
    verdict
}

/// The addresses `name` leads to by the system's resolver, none when it
/// does not resolve.
pub fn lookup(name: &str) -> Vec<IpAddr> {
    let Ok(found) = (name, 80).to_socket_addrs() else { return Vec::new() };
    let mut addresses: Vec<IpAddr> = found.map(|address| address.ip()).collect();
    addresses.sort();
    addresses.dedup();
    addresses
}

/// This machine's addresses on the internet's IPv6: those that are not
/// link-local, unique local or loopback, and not still being tried.
pub fn global_ipv6_addresses() -> Vec<Ipv6Addr> {
    let Ok(interfaces) = ip::read_interfaces(Path::new(ip::INTERFACES), Path::new(ip::ROUTES))
    else {
        return Vec::new();
    };
    interfaces
        .iter()
        .filter(|interface| !interface.loopback)
        .flat_map(|interface| interface.ipv6.addresses.iter())
        .filter(|address| !address.tentative && !private_v6(address.cidr.ip))
        .map(|address| address.cidr.ip)
        .collect()
}

/// Reads the facts of this machine for `names`, to be asked of the
/// authority at `directory`: with `http`, resolves each name and reads the
/// interfaces and the firewall. Let's Encrypt is on the internet without
/// being looked up, so that a machine that cannot resolve names yet still
/// finds that a name of its own leads nowhere the authority reaches.
pub fn gather<'a>(
    now: i64,
    names: &'a [(String, Vec<IpAddr>)],
    http: bool,
    directory: &Url,
    firewall_text: &'a str,
    with_inbound: bool,
) -> Facts<'a> {
    let public_authority = http
        && (super::is_public_authority(&directory.to_string()) || {
            let addresses = lookup(directory.host());
            !addresses.is_empty() && addresses.iter().all(|address| !private_address(*address))
        });
    Facts {
        now,
        names,
        http,
        public_authority,
        own_ipv6: if http { global_ipv6_addresses() } else { Vec::new() },
        firewall: firewall_text,
        with_inbound,
        holder: None,
    }
}

/// The kernel's tables of TCP sockets (radix-proc(5)).
const TCP_TABLES: [&str; 2] = ["/processes/net/tcp", "/processes/net/tcp6"];

/// Who holds port 80, by whether a socket listens on it, the services that
/// run with the ports each names, and whether nginx runs; none of the
/// services when Spark cannot be asked.
///
/// A socket's owner is not in the kernel's tables, so the services that run
/// and name the port stand for it: nginx first, since an order writes its
/// answers where nginx serves them, then any other.
pub fn holder_from(
    listening: bool,
    services: Option<&[(String, Vec<String>)]>,
    nginx_runs: bool,
) -> Holder {
    if !listening {
        return Holder::Free;
    }
    let Some(services) = services else { return Holder::Unknown };
    let names: Vec<&str> = services
        .iter()
        .filter(|(_, listen)| names_port(listen, 80))
        .map(|(name, _)| name.as_str())
        .collect();
    if names.contains(&"nginx") || (names.is_empty() && nginx_runs) {
        return Holder::Nginx;
    }
    match names.iter().find(|name| **name != "certd") {
        Some(other) => Holder::Other((*other).to_string()),
        None if names.is_empty() => Holder::Other(String::from("another program")),
        None => Holder::Certd,
    }
}

/// Whether one of `listen`, each a port or a range as `service show`
/// writes it ("80", "8000-8010"), holds `port`.
fn names_port(listen: &[String], port: u32) -> bool {
    listen.iter().any(|item| match item.split_once('-') {
        Some((first, last)) => match (first.parse::<u32>(), last.parse::<u32>()) {
            (Ok(first), Ok(last)) => (first..=last).contains(&port),
            _ => false,
        },
        None => item.parse::<u32>() == Ok(port),
    })
}

/// Who holds port 80 on this machine now, as a person can tell: the kernel's
/// tables and what Spark says of the services, which anyone may ask.
pub fn port80_holder() -> Holder {
    let listening = TCP_TABLES
        .iter()
        .filter_map(|table| std::fs::read_to_string(table).ok())
        .any(|table| spark_api::ready::listening(&table, &[(80, 1)]));
    if !listening {
        return Holder::Free;
    }
    let shown = crate::service::shown(None).ok();
    let running: Option<Vec<(String, Vec<String>)>> = shown.as_ref().map(|shown| {
        shown
            .services
            .iter()
            .filter(|service| {
                matches!(
                    service.state,
                    spark_api::show::State::Running | spark_api::show::State::Starting
                )
            })
            .map(|service| (service.name.clone(), service.holds.listen.clone()))
            .collect()
    });
    let nginx_runs =
        running.as_ref().is_some_and(|running| running.iter().any(|(name, _)| name == "nginx"));
    holder_from(true, running.as_deref(), nginx_runs)
}

/// The firewall's settings, or nothing when the file is not there, which
/// is no rule at all.
pub fn firewall_settings() -> String {
    std::fs::read_to_string(FIREWALL_SETTINGS).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn ip6(text: &str) -> Ipv6Addr {
        text.parse().unwrap()
    }

    #[test]
    fn lets_encrypt_is_on_the_internet_without_a_look_up() {
        for directory in [super::super::LETS_ENCRYPT, super::super::LETS_ENCRYPT_STAGING] {
            let url = Url::parse(directory).unwrap();
            let names = [(String::from("localhost"), vec![ip("127.0.0.1")])];
            let facts = gather(NOW, &names, true, &url, "", false);
            assert!(facts.public_authority, "{directory}");
            let found = findings(&facts);
            let failed: Vec<_> = found.iter().filter(|f| f.id == Some("CRT-102")).collect();
            assert_eq!(failed.len(), 1, "{directory}");
            assert!(failed[0].words.starts_with("localhost leads to 127.0.0.1, a private address"));
        }
        // Proving the names by DNS never asks whether the authority is public.
        let url = Url::parse(super::super::LETS_ENCRYPT).unwrap();
        assert!(!gather(NOW, &[], false, &url, "", false).public_authority);
    }

    fn facts<'a>(names: &'a [(String, Vec<IpAddr>)], firewall: &'a str) -> Facts<'a> {
        Facts {
            now: NOW,
            names,
            http: true,
            public_authority: true,
            own_ipv6: vec![ip6("2001:db8::7")],
            firewall,
            with_inbound: true,
            holder: None,
        }
    }

    fn one(name: &str, addresses: &[&str]) -> Vec<(String, Vec<IpAddr>)> {
        vec![(name.to_string(), addresses.iter().map(|text| ip(text)).collect())]
    }

    #[test]
    fn the_address_classes_of_a_private_network_are_told_apart() {
        for private in [
            "10.0.0.1",
            "10.255.255.255",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "100.64.0.1",
            "100.127.255.255",
            "127.0.0.1",
            "169.254.1.1",
            "0.0.0.0",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "::1",
            "::ffff:10.1.2.3",
        ] {
            assert!(private_address(ip(private)), "{private}");
        }
        for public in [
            "8.8.8.8",
            "172.15.255.255",
            "172.32.0.1",
            "100.63.255.255",
            "100.128.0.1",
            "192.169.0.1",
            "169.253.0.1",
            "11.0.0.1",
            "2001:db8::1",
            "2606:4700::1",
            "fb00::1",
            "fec0::1",
            "::ffff:8.8.8.8",
        ] {
            assert!(!private_address(ip(public)), "{public}");
        }
    }

    #[test]
    fn a_name_that_is_all_right_passes_without_a_word() {
        let names = one("a.example", &["203.0.113.7"]);
        assert_eq!(check(&facts(&names, "")), Verdict::default());
    }

    #[test]
    fn a_wrong_clock_stops_it_in_words_that_say_what_to_do() {
        let names = one("a.example", &["203.0.113.7"]);
        let mut wrong = facts(&names, "");
        wrong.now = 86_400;
        let verdict = check(&wrong);
        assert_eq!(verdict.failures, ["the clock is wrong (1970-01-02): set it, then try again"]);
    }

    #[test]
    fn a_name_without_an_address_is_told_what_record_to_make() {
        let names = one("a.example", &[]);
        assert_eq!(
            check(&facts(&names, "")).failures,
            ["a.example does not resolve to an address: give it an A record (and AAAA for \
                 IPv6) that leads to this machine"]
        );
    }

    #[test]
    fn a_private_address_fails_for_an_authority_on_the_internet_only() {
        let names = one("a.example", &["192.168.1.20", "203.0.113.7"]);
        assert_eq!(
            check(&facts(&names, "")).failures,
            ["a.example leads to 192.168.1.20, a private address the authority cannot reach: \
                 give it this machine's public address, or use --dns"]
        );
        let mut lab = facts(&names, "");
        lab.public_authority = false;
        assert_eq!(check(&lab), Verdict::default());
    }

    #[test]
    fn an_ipv6_name_on_a_machine_without_ipv6_is_a_warning_not_a_failure() {
        let names = one("a.example", &["203.0.113.7", "2001:db8::7"]);
        let mut without = facts(&names, "");
        without.own_ipv6 = Vec::new();
        let verdict = check(&without);
        assert!(verdict.failures.is_empty());
        assert_eq!(
            verdict.warnings,
            ["a.example has an IPv6 address and this machine has none: if 2001:db8::7 is not \
                 this machine, the authority, which tries IPv6 first, may not reach it"]
        );
        assert_eq!(check(&facts(&names, "")), Verdict::default());
    }

    #[test]
    fn an_ipv6_name_that_is_none_of_this_machines_addresses_names_both() {
        let names = one("www.blog.example.org", &["203.0.113.7", "2001:db8::5"]);
        let mut own = facts(&names, "");
        own.own_ipv6 = vec![ip6("2001:db8:42::10")];
        let verdict = check(&own);
        assert!(verdict.failures.is_empty());
        assert_eq!(
            verdict.warnings,
            ["www.blog.example.org's AAAA record points to 2001:db8::5, which is not this \
              machine (it has 2001:db8:42::10): the authority tries IPv6 first, so it would \
              check that machine: remove the AAAA record or point it to 2001:db8:42::10"]
        );
        let found = findings(&own);
        let warning = found.iter().find(|finding| finding.outcome == Outcome::Warning).unwrap();
        assert_eq!(warning.id, Some("CRT-104"));
        assert_eq!(warning.name.as_deref(), Some("www.blog.example.org"));
        // One of its addresses being this machine's is enough, and so is more
        // than one of this machine's.
        let names = one("a.example", &["2001:db8::5", "2001:db8:42::11"]);
        let mut several = facts(&names, "");
        several.own_ipv6 = vec![ip6("2001:db8:42::10"), ip6("2001:db8:42::11")];
        assert_eq!(check(&several), Verdict::default());
        let names = one("a.example", &["2001:db8::5"]);
        several.names = &names;
        assert!(check(&several).warnings[0].ends_with("point it to 2001:db8:42::10"));
        assert!(check(&several).warnings[0].contains("(it has 2001:db8:42::10, 2001:db8:42::11)"));
    }

    #[test]
    fn the_findings_say_what_passed_with_the_ids_of_what_did_not() {
        let names = vec![
            (String::from("a.example"), vec![ip("203.0.113.7"), ip("2001:db8::7")]),
            (String::from("b.example"), Vec::new()),
            (String::from("c.example"), vec![ip("192.168.1.20")]),
        ];
        let mut all = facts(&names, "allow tcp 22\nallow tcp 80\n");
        all.with_inbound = false;
        all.holder = Some(Holder::Free);
        let found = findings(&all);
        let rows: Vec<(Option<&str>, Outcome, Option<&str>)> =
            found.iter().map(|row| (row.id, row.outcome, row.name.as_deref())).collect();
        assert_eq!(
            rows,
            [
                (None, Outcome::Passed, Some("a.example")),
                (Some("CRT-101"), Outcome::Failed, Some("b.example")),
                (Some("CRT-102"), Outcome::Failed, Some("c.example")),
                (None, Outcome::Passed, None),
                (None, Outcome::Passed, None),
            ]
        );
        assert_eq!(found[0].words, "a.example leads to 203.0.113.7, 2001:db8::7");
        assert_eq!(found[3].words, "port 80 is open (firewall rule 2)");
        assert_eq!(found[4].words, "port 80 is free: the order answers the authority itself");
        // The failures are the same words as before, and `check` keeps them.
        assert_eq!(check(&all).failures.len(), 2);
        // A service's own right, with no rule at all.
        let names = one("a.example", &["203.0.113.7"]);
        let mut open = facts(&names, "");
        open.holder = Some(Holder::Nginx);
        let found = findings(&open);
        assert_eq!(found[1].words, "port 80 is open (no firewall rule closes it)");
        assert_eq!(found[2].words, "nginx holds port 80 and serves the answer");
    }

    #[test]
    fn a_clock_that_is_wrong_is_the_first_finding() {
        let names = one("a.example", &["203.0.113.7"]);
        let mut wrong = facts(&names, "");
        wrong.now = 86_400;
        let found = findings(&wrong);
        assert_eq!(found[0].id, Some("CRT-105"));
        assert_eq!(found[0].outcome, Outcome::Failed);
        assert_eq!(found[0].name, None);
        assert_eq!(found[0].words, "the clock is wrong (1970-01-02): set it, then try again");
    }

    #[test]
    fn under_dns_every_check_of_the_names_and_the_port_is_skipped() {
        let names = vec![
            (String::from("a.example"), Vec::new()),
            (String::from("*.a.example"), Vec::new()),
        ];
        let mut by_hand = facts(&names, "deny tcp 80\n");
        by_hand.http = false;
        by_hand.holder = Some(Holder::Other(String::from("lighttpd")));
        let found = findings(&by_hand);
        assert_eq!(found.len(), 3);
        assert!(found.iter().all(|row| row.outcome == Outcome::Skipped && row.id.is_none()));
        assert_eq!(found[0].name.as_deref(), Some("a.example"));
        assert_eq!(found[2].name, None);
        assert!(found[2].words.starts_with("port 80 is not checked: "));
        assert_eq!(check(&by_hand), Verdict::default());
    }

    #[test]
    fn who_holds_port_80_is_told_from_the_services_that_run_and_name_it() {
        let nginx = (String::from("nginx"), vec![String::from("80"), String::from("443")]);
        let certd = (String::from("certd"), vec![String::from("80")]);
        let other = (String::from("lighttpd"), vec![String::from("8000-8100")]);
        let wide = (String::from("lighttpd"), vec![String::from("70-90")]);
        let ssh = (String::from("sshd"), vec![String::from("22")]);
        let held = |services: &[(String, Vec<String>)], nginx_runs: bool| {
            holder_from(true, Some(services), nginx_runs)
        };
        assert_eq!(holder_from(false, Some(&[nginx.clone()]), true), Holder::Free);
        assert_eq!(holder_from(true, None, false), Holder::Unknown);
        assert_eq!(held(&[ssh.clone(), nginx.clone()], true), Holder::Nginx);
        assert_eq!(held(&[certd.clone(), nginx.clone()], true), Holder::Nginx);
        assert_eq!(held(&[certd.clone()], false), Holder::Certd);
        assert_eq!(held(&[wide.clone()], false), Holder::Other(String::from("lighttpd")));
        assert_eq!(held(&[certd, wide], false), Holder::Other(String::from("lighttpd")));
        // A socket nobody that runs names: nginx when it runs, as an order
        // assumes, else some program.
        assert_eq!(held(&[ssh.clone(), other.clone()], true), Holder::Nginx);
        assert_eq!(held(&[ssh, other], false), Holder::Other(String::from("another program")));
    }

    #[test]
    fn a_program_that_holds_port_80_fails_the_check_and_nginx_or_nobody_does_not() {
        let names = one("a.example", &["203.0.113.7"]);
        let mut held = facts(&names, "allow tcp 80\n");
        held.holder = Some(Holder::Other(String::from("lighttpd")));
        let found = findings(&held);
        let row = found.last().unwrap();
        assert_eq!((row.id, row.outcome), (Some("CRT-106"), Outcome::Failed));
        assert_eq!(
            row.words,
            "port 80 is held by lighttpd, which is not nginx or certd: stop it for the few \
             minutes the order takes, or use --dns"
        );
        assert_eq!(check(&held).failures.len(), 1);
        for (holder, outcome) in [
            (Holder::Free, Outcome::Passed),
            (Holder::Nginx, Outcome::Passed),
            (Holder::Certd, Outcome::Warning),
            (Holder::Unknown, Outcome::Warning),
        ] {
            held.holder = Some(holder);
            assert_eq!(findings(&held).last().unwrap().outcome, outcome);
        }
        // `cert get` that does not ask leaves the row out.
        held.holder = None;
        assert_eq!(findings(&held).len(), 2);
    }

    #[test]
    fn a_firewall_that_closes_port_80_says_the_command_that_opens_it() {
        let names = one("a.example", &["203.0.113.7"]);
        let closed = "allow tcp 22\ndeny tcp 80\n";
        let want = ["the firewall closes port 80, which the authority must reach: open it with \
             `elevate firewall allow tcp 80`"];
        assert_eq!(check(&facts(&names, closed)).failures, want);
        // A command's socket has no inbound right: only a rule opens it.
        let mut command = facts(&names, "allow tcp 22\n");
        command.with_inbound = false;
        assert_eq!(check(&command).failures, want);
        command.firewall = "allow tcp 22\nallow tcp 80\n";
        assert_eq!(check(&command), Verdict::default());
        // A service's has it: only a rule that denies closes it.
        assert_eq!(check(&facts(&names, "allow tcp 22\n")), Verdict::default());
    }

    #[test]
    fn under_dns_only_the_clock_is_checked() {
        let names = one("a.example", &[]);
        let mut by_hand = facts(&names, "deny tcp 80\n");
        by_hand.http = false;
        assert_eq!(check(&by_hand), Verdict::default());
        by_hand.now = 0;
        assert_eq!(check(&by_hand).failures.len(), 1);
    }

    #[test]
    fn each_name_is_judged_and_all_the_failures_are_told_together() {
        let names = vec![
            (String::from("a.example"), vec![ip("10.0.0.5")]),
            (String::from("b.example"), Vec::new()),
        ];
        let verdict = check(&facts(&names, "deny tcp 80\n"));
        assert_eq!(verdict.failures.len(), 3, "{:?}", verdict.failures);
    }
}
