// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What `cert get` and certd check before the authority is asked (ADR 0513),
//! so that a mistake is told in plain words that name what to fix, not
//! after minutes of waiting and the authority's own wording.
//!
//! [`check`] judges facts it is given, so that each rule is tested alone;
//! [`gather`] reads the facts from this machine: the resolver, the
//! interfaces and the firewall's settings.
//!
//! Under `--dns` the authority never connects to this machine, so only the
//! clock is checked: advice to "use --dns" would lead in a circle.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::path::Path;

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
    /// This machine has an IPv6 address that is not link-local, unique
    /// local or loopback.
    pub global_ipv6: bool,
    /// The firewall's settings, as the file holds them.
    pub firewall: &'a str,
    /// Whether what listens on port 80 has the inbound right: a service
    /// that names the port has, a command of an administrator has not.
    pub with_inbound: bool,
}

/// What [`check`] found: each thing to fix, and each thing to know.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Verdict {
    pub failures: Vec<String>,
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

/// Judges `facts`.
pub fn check(facts: &Facts<'_>) -> Verdict {
    let mut verdict = Verdict::default();
    if let Some(why) = machine::wrong_clock(facts.now) {
        verdict.failures.push(format!("{why}: set it, then try again"));
    }
    if !facts.http {
        return verdict;
    }
    for (name, addresses) in facts.names {
        if addresses.is_empty() {
            verdict.failures.push(format!(
                "{name} does not resolve to an address: give it an A record (and AAAA for IPv6) \
                 that leads to this machine"
            ));
            continue;
        }
        if facts.public_authority
            && let Some(address) = addresses.iter().find(|address| private_address(**address))
        {
            verdict.failures.push(format!(
                "{name} leads to {address}, a private address the authority cannot reach: give \
                 it this machine's public address, or use --dns"
            ));
        }
        if !facts.global_ipv6
            && let Some(address) = addresses.iter().find(|address| address.is_ipv6())
        {
            verdict.warnings.push(format!(
                "{name} has an IPv6 address and this machine has none: if {address} is not this \
                 machine, the authority, which tries IPv6 first, may not reach it"
            ));
        }
    }
    if !firewall::lets_tcp_in(facts.firewall, 80, facts.with_inbound) {
        verdict.failures.push(String::from(
            "the firewall closes port 80, which the authority must reach: open it with \
             `elevate firewall allow tcp 80`",
        ));
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

/// Whether this machine has an address on the internet's IPv6: one that
/// is not link-local, unique local or loopback.
pub fn has_global_ipv6() -> bool {
    let Ok(interfaces) = ip::read_interfaces(Path::new(ip::INTERFACES), Path::new(ip::ROUTES))
    else {
        return false;
    };
    interfaces.iter().filter(|interface| !interface.loopback).any(|interface| {
        interface
            .ipv6
            .addresses
            .iter()
            .any(|address| !address.tentative && !private_v6(address.cidr.ip))
    })
}

/// Reads the facts of this machine for `names`, to be asked of the
/// authority at `directory_host`: with `http`, resolves each name and
/// reads the interfaces and the firewall.
pub fn gather<'a>(
    now: i64,
    names: &'a [(String, Vec<IpAddr>)],
    http: bool,
    directory_host: &str,
    firewall_text: &'a str,
    with_inbound: bool,
) -> Facts<'a> {
    let public_authority = http && {
        let addresses = lookup(directory_host);
        !addresses.is_empty() && addresses.iter().all(|address| !private_address(*address))
    };
    Facts {
        now,
        names,
        http,
        public_authority,
        global_ipv6: http && has_global_ipv6(),
        firewall: firewall_text,
        with_inbound,
    }
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

    fn facts<'a>(names: &'a [(String, Vec<IpAddr>)], firewall: &'a str) -> Facts<'a> {
        Facts {
            now: NOW,
            names,
            http: true,
            public_authority: true,
            global_ipv6: true,
            firewall,
            with_inbound: true,
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
        without.global_ipv6 = false;
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
