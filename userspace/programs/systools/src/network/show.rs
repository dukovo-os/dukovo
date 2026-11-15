// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `network show`: each network card with what it has now (as ip(8) reads
//! it from the kernel), what network(5) says of it, and its DHCP lease.
//!
//! Anyone may run it: the kernel's `/kernel/class/net`, the settings file
//! and netd's resolv.conf and client lease files are readable by everyone,
//! and Spark's `show` answer is too. What a person may not read is left
//! out and does not fail the command: a lease that cannot be read is null.
//!
//! The JSON is what the web console's Networking page reads, so its fields
//! and their order are the interface: [`Report`] and the structs below it
//! are in the order the fields are written, and each test of this file
//! names them all.

use std::fs;
use std::path::PathBuf;

use serde::Serialize;

use crate::dhcp::Lease;
use crate::dhcp_server::mac_of;
use crate::ip::{self, Interface, Origin};
use crate::net_errors;
use crate::netd::{self, Setup};
use crate::util::{self, Fail, fail};

/// The trial of the network settings, Spark's name for it.
const TRIAL: &str = "network";

/// The keys of a section that [`Settings`] has a field for: the other keys
/// of the section are named, and the editor leaves them as they are.
const KEYS: [&str; 5] = ["address", "router", "dns", "dhcp", "metered"];

/// Where `network show` reads, so that the tests can read their own.
struct Sources {
    /// The kernel's cards, and its routes.
    interfaces: PathBuf,
    routes: PathBuf,
    /// network(5)'s file.
    settings: PathBuf,
    /// netd's lease files, one a card.
    leases: PathBuf,
    /// The name servers netd merges from every card.
    resolv: PathBuf,
}

impl Sources {
    /// The places of a running machine.
    fn system() -> Sources {
        Sources {
            interfaces: PathBuf::from(ip::INTERFACES),
            routes: PathBuf::from(ip::ROUTES),
            settings: PathBuf::from(netd::SETTINGS),
            leases: PathBuf::from(netd::LEASES),
            resolv: PathBuf::from(netd::RESOLV),
        }
    }
}

/// What `network show --json` writes.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Report {
    /// The cards, in the order of the kernel's numbers.
    cards: Vec<Card>,
    /// The `network` trial, null when the settings are not being tried.
    trial: Option<Trial>,
}

/// One card.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Card {
    name: String,
    /// Its MAC address, in lower case.
    mac: String,
    /// Whether its link is up.
    up: bool,
    /// Megabits a second, null while the link is down or the card does
    /// not say.
    speed_mbps: Option<u32>,
    /// What it has now: its IPv4 addresses, then its IPv6 ones, those
    /// still being tested left out, as they are not its yet.
    addresses: Vec<Held>,
    /// Its default routers: IPv4, then IPv6.
    routers: Vec<String>,
    /// The name servers the machine asks, as netd writes them: one list
    /// for all the cards, which is the same on each.
    dns: Vec<String>,
    settings: Settings,
    /// The DHCP lease it holds, null when it holds none or the person may
    /// not read netd's file of it.
    lease: Option<Leased>,
}

/// An address a card has.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Held {
    address: String,
    prefix: u8,
    /// Where it came from: `dhcp` (DHCP or DHCPv6), `fixed` (network(5)'s
    /// `address`), `router` (made from what a router advertised),
    /// `link-local`, or `other` (`ip address add`, or the kernel's `ip=`).
    from: &'static str,
}

/// What network(5) says of a card.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Settings {
    /// The name of the section that sets the card up, as the file has it
    /// (a name or a MAC address); null when none does and the card takes
    /// the defaults.
    section: Option<String>,
    /// `dhcp` (it asks DHCP), `fixed` (addresses of its own, no DHCP) or
    /// `none` (netd leaves it as it finds it).
    mode: &'static str,
    /// Its own addresses, IPv4 then IPv6, as `ADDRESS/PREFIX`.
    addresses: Vec<String>,
    /// Its own router: the IPv4 one, else the IPv6 one.
    router: Option<String>,
    dns: Vec<String>,
    /// `auto`, `yes` or `no`.
    metered: &'static str,
    /// Whether the card serves DHCP (`dhcp-server`), which cannot ask it.
    dhcp_server: bool,
    /// The names of the section's other keys, in the file's order.
    other: Vec<String>,
}

/// A lease a card holds.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Leased {
    address: String,
    /// When it ends, in milliseconds since 1970; null for a lease that
    /// lasts for ever.
    ends_ms: Option<u64>,
}

/// A trial that runs.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Trial {
    /// How long until it is undone unless it is kept.
    left_ms: u64,
}

/// The name servers of a resolv.conf, as written.
fn nameservers(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.strip_prefix("nameserver"))
        .filter_map(|rest| rest.trim().parse::<std::net::IpAddr>().ok())
        .map(|ip| ip.to_string())
        .collect()
}

/// The names of the keys of the first section called `section` that
/// [`Settings`] has no field for, in the order of the file, each once.
/// netd takes the first section that names a card, so the later ones are
/// not read.
fn other_keys(text: &str, section: &str) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    let (mut inside, mut seen) = (false, false);
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            let named = name.trim().eq_ignore_ascii_case(section);
            inside = named && !seen;
            seen |= named;
        } else if inside
            && !line.starts_with('#')
            && let Some((key, _)) = line.split_once('=')
        {
            let key = key.trim();
            if !KEYS.contains(&key) && !keys.iter().any(|known| known == key) {
                keys.push(key.to_string());
            }
        }
    }
    keys
}

/// How network(5) sets a card up, as [`Settings`] says it.
fn settings_of(setup: &Setup, section: Option<&str>, text: &str) -> Settings {
    let mode = if setup.alone() {
        "none"
    } else if setup.dhcp() {
        "dhcp"
    } else {
        "fixed"
    };
    let addresses = setup.addresses.iter().map(ToString::to_string);
    Settings {
        section: section.map(str::to_string),
        mode,
        addresses: addresses.chain(setup.addresses6.iter().map(ToString::to_string)).collect(),
        router: setup.router.map(|ip| ip.to_string()).or(setup.router6.map(|ip| ip.to_string())),
        dns: setup.dns.iter().map(ToString::to_string).collect(),
        metered: match setup.metered {
            Some(true) => "yes",
            Some(false) => "no",
            None => "auto",
        },
        dhcp_server: setup.server.range.is_some(),
        other: section.map_or_else(Vec::new, |section| other_keys(text, section)),
    }
}

/// The lease the card holds: the one in netd's file of the card's, if the
/// card asks DHCP, the file reads, and the card has the lease's address
/// now. A lease netd kept over a restart, whose address the card has not
/// got back, is not one it holds.
fn lease_of(sources: &Sources, interface: &Interface, setup: &Setup) -> Option<(Lease, u64)> {
    if !setup.dhcp() {
        return None;
    }
    let file = sources.leases.join(netd::lease_name(mac_of(&interface.mac)?));
    let (lease, obtained) = netd::parse_lease(&fs::read_to_string(file).ok()?)?;
    interface.addresses.iter().any(|cidr| cidr.ip == lease.address.ip).then_some((lease, obtained))
}

/// The addresses the card has, each with where it came from.
fn held(interface: &Interface, setup: &Setup, lease: Option<&Lease>) -> Vec<Held> {
    let mut held = Vec::new();
    for cidr in &interface.addresses {
        let from = if setup.addresses.iter().any(|own| own.ip == cidr.ip) {
            "fixed"
        } else if lease.is_some_and(|lease| lease.address.ip == cidr.ip) {
            "dhcp"
        } else {
            "other"
        };
        held.push(Held { address: cidr.ip.to_string(), prefix: cidr.prefix, from });
    }
    // DHCPv6 gives an address as a given one, as the settings' own are.
    let dhcpv6 = setup.ipv6 != Some(false)
        && setup.dhcpv6.unwrap_or_default().asks(interface.ipv6.managed, interface.ipv6.other)
            == Some(true);
    for address in interface.ipv6.addresses.iter().filter(|address| !address.tentative) {
        let from = match address.origin {
            Origin::LinkLocal => "link-local",
            Origin::Stable | Origin::Temporary => "router",
            Origin::Given if setup.addresses6.iter().any(|own| own.ip == address.cidr.ip) => {
                "fixed"
            }
            Origin::Given if dhcpv6 => "dhcp",
            Origin::Given => "other",
        };
        held.push(Held { address: address.cidr.ip.to_string(), prefix: address.cidr.prefix, from });
    }
    held
}

/// The card's default routers: its IPv4 one, then those advertised to it
/// and the one it was given, each once.
fn routers_of(interface: &Interface) -> Vec<String> {
    let advertised = interface.ipv6.routers.iter().map(|(router, _)| *router);
    let mut routers: Vec<String> = interface.router.iter().map(ToString::to_string).collect();
    for router in advertised.chain(interface.ipv6.given_router) {
        let router = router.to_string();
        if !routers.contains(&router) {
            routers.push(router);
        }
    }
    routers
}

/// The card's speed in megabits a second: the kernel's `speed` file says
/// -1 while the link is down or the card does not say.
fn speed_of(sources: &Sources, name: &str) -> Option<u32> {
    let text = fs::read_to_string(sources.interfaces.join(name).join("speed")).ok()?;
    text.trim().parse::<u32>().ok().filter(|&mbps| mbps > 0)
}

/// What `network show` says: the cards, or the one named `card` (by name
/// or by MAC address), and the trial whose time left is `trial`.
///
/// # Errors
///
/// NET-181 when no card has the name or the address; a failure when the
/// kernel's list of the cards cannot be read.
fn report(sources: &Sources, card: Option<&str>, trial: Option<u64>) -> Result<Report, Fail> {
    let interfaces = ip::read_interfaces(&sources.interfaces, &sources.routes)
        .map_err(|error| util::failed(&sources.interfaces.display(), error))?;
    let settings_text = fs::read_to_string(&sources.settings).unwrap_or_default();
    let (settings, _) = netd::parse_settings(&settings_text);
    let dns = nameservers(&fs::read_to_string(&sources.resolv).unwrap_or_default());
    let mut cards = Vec::new();
    for interface in interfaces.iter().filter(|interface| netd::sets_up(interface)) {
        if let Some(wanted) = card
            && interface.name != wanted
            && !interface.mac.eq_ignore_ascii_case(wanted)
        {
            continue;
        }
        let section = netd::section_of(&settings, &interface.name, &interface.mac);
        let setup = section.map_or_else(Setup::default, |(_, setup)| setup.clone());
        let lease = lease_of(sources, interface, &setup);
        cards.push(Card {
            name: interface.name.clone(),
            mac: interface.mac.to_ascii_lowercase(),
            up: interface.carrier,
            speed_mbps: speed_of(sources, &interface.name),
            addresses: held(interface, &setup, lease.as_ref().map(|(lease, _)| lease)),
            routers: routers_of(interface),
            dns: dns.clone(),
            settings: settings_of(&setup, section.map(|(name, _)| name.as_str()), &settings_text),
            lease: lease.map(|(lease, obtained)| Leased {
                address: lease.address.ip.to_string(),
                ends_ms: lease.ends(obtained).map(|seconds| seconds * 1000),
            }),
        });
    }
    if let Some(wanted) = card
        && cards.is_empty()
    {
        return Err(net_errors::no_such_card(wanted));
    }
    Ok(Report { cards, trial: trial.map(|left_ms| Trial { left_ms }) })
}

/// `network show` as text, at `now_ms`, in milliseconds since 1970: a
/// block of lines for each card, and a line for the trial.
fn text(report: &Report, now_ms: u64) -> String {
    if report.cards.is_empty() {
        return String::from("no network card\n");
    }
    let list =
        |items: Vec<String>| if items.is_empty() { "none".to_string() } else { items.join(", ") };
    let blocks: Vec<String> = report
        .cards
        .iter()
        .map(|card| {
            let speed = card.speed_mbps.map_or_else(String::new, |mbps| format!(", {mbps} Mb/s"));
            let mut block = format!(
                "{}  {}  link {}{speed}\n",
                card.name,
                card.mac,
                if card.up { "up" } else { "down" }
            );
            let addresses = card
                .addresses
                .iter()
                .map(|held| format!("{}/{} ({})", held.address, held.prefix, held.from));
            block += &format!("  addresses  {}\n", list(addresses.collect()));
            block += &format!("  routers    {}\n", list(card.routers.clone()));
            block += &format!("  dns        {}\n", list(card.dns.clone()));
            block += &format!("  settings   {}\n", settings_line(&card.settings));
            if let Some(lease) = &card.lease {
                let ends = match lease.ends_ms {
                    None => "for ever".to_string(),
                    Some(ends) if ends <= now_ms => "has ended".to_string(),
                    Some(ends) => format!("ends in {}", util::elapsed((ends - now_ms) / 1000)),
                };
                block += &format!("  lease      {}, {ends}\n", lease.address);
            }
            block
        })
        .collect();
    let mut shown = blocks.join("\n");
    if let Some(trial) = &report.trial {
        shown += &format!(
            "\nThe network settings are being tried: {} left before they are undone unless kept.\n",
            util::elapsed(trial.left_ms / 1000)
        );
    }
    shown
}

/// A card's settings on one line.
fn settings_line(settings: &Settings) -> String {
    let mut line = match &settings.section {
        Some(section) => format!("[{section}] {}", settings.mode),
        None => format!("no section, {}", settings.mode),
    };
    if !settings.addresses.is_empty() {
        line += &format!(", addresses {}", settings.addresses.join(" "));
    }
    if let Some(router) = &settings.router {
        line += &format!(", router {router}");
    }
    if !settings.dns.is_empty() {
        line += &format!(", dns {}", settings.dns.join(" "));
    }
    line += &format!(", metered {}", settings.metered);
    if settings.dhcp_server {
        line += ", serves DHCP";
    }
    if !settings.other.is_empty() {
        line += &format!(", also {}", settings.other.join(" "));
    }
    line
}

/// `network show [CARD] [--json]`.
pub fn run(card: Option<&str>, json: bool) -> Result<i32, Fail> {
    // A Spark that does not answer leaves the trial out, as it does not
    // change what the cards have.
    let trial = crate::service::trial_left_ms(TRIAL);
    let report = report(&Sources::system(), card, trial)?;
    if json {
        let line = serde_json::to_string(&report).map_err(|error| fail(error.to_string()))?;
        println!("{line}");
    } else {
        print!("{}", text(&report, util::now_ns() / 1_000_000));
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::{Value, json};

    use super::*;

    /// Net0's lease: 10.0.2.15, given at 1763116400 for an hour.
    const LEASE: &str = "# netd(8)'s lease for net0.\naddress = 10.0.2.15/24\nrouter = 10.0.2.2\n\
                         dns = 10.0.2.3\nserver = 10.0.2.2\nobtained = 1763116400\nseconds = \
                         3600\nrenew = 1800\nrebind = 3150\n";

    /// A machine's files: the loopback and a card as the kernel shows
    /// them, and a lease file, in a folder of their own for each call, as
    /// the tests run at once.
    fn machine(settings: &str, lease: Option<&str>) -> Sources {
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        let call = CALLS.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("network-show-{}-{call}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let write = |name: &str, files: &[(&str, &str)]| {
            let path = dir.join("net").join(name);
            fs::create_dir_all(path.join("statistics")).unwrap();
            for (file, value) in files {
                fs::write(path.join(file), format!("{value}\n")).unwrap();
            }
        };
        write(
            "lo",
            &[
                ("ifindex", "1"),
                ("address", "00:00:00:00:00:00"),
                ("type", "772"),
                ("operstate", "unknown"),
                ("carrier", "1"),
                ("ipv4", "127.0.0.1/8"),
                ("speed", "-1"),
            ],
        );
        write(
            "net0",
            &[
                ("ifindex", "2"),
                ("address", "52:54:00:12:34:56"),
                ("type", "1"),
                ("operstate", "up"),
                ("carrier", "1"),
                ("speed", "1000"),
                ("ipv4", "10.0.2.15/24\n192.168.7.1/32"),
                (
                    "ipv6",
                    "address fe80::8c1:5bff:fe2e:a7d3/64 link-local preferred forever valid forever\n\
                     address fec0::d1f:8e01:6a3c:52b0/64 stable preferred 14390 valid 86390\n\
                     address 2001:db8::5/64 given preferred forever valid forever\n\
                     address 2001:db8::6/64 given preferred forever valid forever\n\
                     address 2001:db8::7/64 given preferred forever valid forever tentative\n\
                     router fe80::2 lifetime 1790\n\
                     router fe80::2 given\n\
                     dhcp managed",
                ),
            ],
        );
        // A bridge's port and a tunnel are not cards netd sets up.
        write(
            "net1",
            &[("ifindex", "3"), ("address", "52:54:00:12:34:57"), ("link", "master br0")],
        );
        write("wg0", &[("ifindex", "4"), ("address", "00:00:00:00:00:00"), ("link", "tunnel")]);
        fs::write(
            dir.join("route"),
            "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
             net0\t00000000\t0202000A\t0003\t0\t0\t0\t00000000\t0\t0\t0\n",
        )
        .unwrap();
        fs::write(dir.join("settings"), settings).unwrap();
        fs::write(
            dir.join("resolv.conf"),
            "# Written by netd(8)\nnameserver 10.0.2.3\nnameserver fec0::3\nnameserver bogus\n\
             search lan\n",
        )
        .unwrap();
        fs::create_dir_all(dir.join("leases")).unwrap();
        if let Some(lease) = lease {
            fs::write(dir.join("leases/52-54-00-12-34-56.lease"), lease).unwrap();
        }
        Sources {
            interfaces: dir.join("net"),
            routes: dir.join("route"),
            settings: dir.join("settings"),
            leases: dir.join("leases"),
            resolv: dir.join("resolv.conf"),
        }
    }

    /// The report as the JSON a reader gets.
    fn json_of(sources: &Sources, card: Option<&str>, trial: Option<u64>) -> Value {
        let report = report(sources, card, trial).unwrap();
        serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap()
    }

    #[test]
    fn a_card_that_asks_dhcp_has_its_lease_and_each_address_its_origin() {
        let sources = machine("# nothing\n", Some(LEASE));
        let shown = json_of(&sources, None, None);
        assert_eq!(
            shown,
            json!({
                "cards": [{
                    "name": "net0",
                    "mac": "52:54:00:12:34:56",
                    "up": true,
                    "speed_mbps": 1000,
                    "addresses": [
                        {"address": "10.0.2.15", "prefix": 24, "from": "dhcp"},
                        {"address": "192.168.7.1", "prefix": 32, "from": "other"},
                        {"address": "fe80::8c1:5bff:fe2e:a7d3", "prefix": 64, "from": "link-local"},
                        {"address": "fec0::d1f:8e01:6a3c:52b0", "prefix": 64, "from": "router"},
                        {"address": "2001:db8::5", "prefix": 64, "from": "dhcp"},
                        {"address": "2001:db8::6", "prefix": 64, "from": "dhcp"},
                    ],
                    "routers": ["10.0.2.2", "fe80::2"],
                    "dns": ["10.0.2.3", "fec0::3"],
                    "settings": {
                        "section": null,
                        "mode": "dhcp",
                        "addresses": [],
                        "router": null,
                        "dns": [],
                        "metered": "auto",
                        "dhcp_server": false,
                        "other": [],
                    },
                    "lease": {"address": "10.0.2.15", "ends_ms": 1_763_120_000_000_u64},
                }],
                "trial": null,
            })
        );
    }

    #[test]
    fn a_card_with_a_section_shows_its_settings_and_the_names_of_its_other_keys() {
        let settings = "# The lab's card.\n[net9]\ndhcp = no\n\n[52:54:00:12:34:56]\n\
                        # fixed\naddress = 10.0.2.15/24\naddress = 2001:db8::5/64\n\
                        router = 10.0.2.2\ndns = 10.0.2.3\nmetered = yes\nipv6 = yes\n\
                        dhcp-server = 10.0.2.50-10.0.2.60\nipv6 = no\n\n[net0]\ndhcp = yes\n";
        let shown = json_of(&machine(settings, Some(LEASE)), None, None);
        let card = &shown["cards"][0];
        assert_eq!(
            card["settings"],
            json!({
                "section": "52:54:00:12:34:56",
                "mode": "fixed",
                "addresses": ["10.0.2.15/24", "2001:db8::5/64"],
                "router": "10.0.2.2",
                "dns": ["10.0.2.3"],
                "metered": "yes",
                "dhcp_server": true,
                "other": ["ipv6", "dhcp-server"],
            })
        );
        // Its own address is its own, and a fixed card holds no lease.
        assert_eq!(card["lease"], Value::Null);
        assert_eq!(
            card["addresses"][0],
            json!({"address": "10.0.2.15", "prefix": 24, "from": "fixed"})
        );
        assert_eq!(
            card["addresses"][4],
            json!({"address": "2001:db8::5", "prefix": 64, "from": "fixed"})
        );
        assert_eq!(
            card["addresses"][5],
            json!({"address": "2001:db8::6", "prefix": 64, "from": "dhcp"})
        );
    }

    #[test]
    fn an_address_nothing_names_is_dhcpv6s_only_while_the_card_asks_it_for_one() {
        // The routers say DHCPv6 gives addresses (`managed`).
        for (settings, from) in [
            ("", "dhcp"),
            ("[net0]\ndhcpv6 = address\n", "dhcp"),
            ("[net0]\ndhcpv6 = no\n", "other"),
            ("[net0]\ndhcpv6 = settings\n", "other"),
            ("[net0]\nipv6 = no\n", "other"),
        ] {
            let shown = json_of(&machine(settings, None), None, None);
            let addresses = shown["cards"][0]["addresses"].as_array().unwrap().clone();
            let froms: Vec<&Value> = addresses[4..].iter().map(|held| &held["from"]).collect();
            assert_eq!(froms, [from, from], "{settings}");
        }
    }

    #[test]
    fn a_card_netd_leaves_alone_has_mode_none() {
        let shown = json_of(&machine("[net0]\ndhcp = no\nmetered = no\n", None), None, None);
        let settings = &shown["cards"][0]["settings"];
        assert_eq!((&settings["section"], &settings["mode"]), (&json!("net0"), &json!("none")));
        assert_eq!(settings["metered"], "no");
    }

    #[test]
    fn a_lease_that_lasts_for_ever_has_no_end_and_one_the_card_lost_is_not_held() {
        let forever = LEASE.replace("seconds = 3600", "seconds = 4294967295");
        let shown = json_of(&machine("", Some(&forever)), None, None);
        assert_eq!(shown["cards"][0]["lease"], json!({"address": "10.0.2.15", "ends_ms": null}));
        // The card has 10.0.2.15 and not 10.0.2.99.
        let moved = LEASE.replace("10.0.2.15/24", "10.0.2.99/24");
        let shown = json_of(&machine("", Some(&moved)), None, None);
        assert_eq!(shown["cards"][0]["lease"], Value::Null);
        assert_eq!(shown["cards"][0]["addresses"][0]["from"], "other");
        // A lease file that does not read is no lease.
        let shown = json_of(&machine("", Some("address = nonsense\n")), None, None);
        assert_eq!(shown["cards"][0]["lease"], Value::Null);
    }

    #[test]
    fn the_trial_is_its_time_left() {
        let sources = machine("", None);
        assert_eq!(json_of(&sources, None, Some(41_000))["trial"], json!({"left_ms": 41_000}));
        assert_eq!(json_of(&sources, None, None)["trial"], Value::Null);
    }

    #[test]
    fn a_card_is_named_by_its_name_or_its_mac_and_others_are_no_card() {
        let sources = machine("", None);
        for card in ["net0", "52:54:00:12:34:56", "52:54:00:12:34:56".to_uppercase().as_str()] {
            let shown = json_of(&sources, Some(card), None);
            assert_eq!(shown["cards"].as_array().unwrap().len(), 1, "{card}");
            assert_eq!(shown["cards"][0]["name"], "net0");
        }
        // The loopback, a port, a tunnel and a card that is not there.
        for card in ["lo", "net1", "wg0", "net9", ""] {
            let error = report(&sources, Some(card), None).unwrap_err();
            assert_eq!(error.code, 1, "{card}");
            assert!(error.message.ends_with("(NET-181)"), "{}", error.message);
        }
    }

    #[test]
    fn a_machine_without_cards_has_an_empty_list() {
        let sources = machine("", None);
        fs::remove_dir_all(sources.interfaces.join("net0")).unwrap();
        let shown = json_of(&sources, None, None);
        assert_eq!(shown, json!({"cards": [], "trial": null}));
        assert_eq!(text(&report(&sources, None, None).unwrap(), 0), "no network card\n");
    }

    #[test]
    fn a_speed_the_card_does_not_say_is_null() {
        let sources = machine("", None);
        for says in ["-1", "0", "fast"] {
            fs::write(sources.interfaces.join("net0/speed"), says).unwrap();
            assert_eq!(json_of(&sources, None, None)["cards"][0]["speed_mbps"], Value::Null);
        }
        fs::remove_file(sources.interfaces.join("net0/speed")).unwrap();
        assert_eq!(json_of(&sources, None, None)["cards"][0]["speed_mbps"], Value::Null);
    }

    #[test]
    fn the_other_keys_are_those_of_the_first_section_the_card_takes_once_each() {
        let text = "[a]\nipv6 = no\n[Net0]\n# comment = x\naddress = 1.2.3.4\ndhcpv6 = no\n\
                    temporary-addresses = no\ndhcpv6 = auto\nrouter = 1.2.3.1\n[net0]\nfoo = 1\n";
        assert_eq!(other_keys(text, "net0"), ["dhcpv6", "temporary-addresses"]);
        assert_eq!(other_keys(text, "a"), ["ipv6"]);
        assert!(other_keys(text, "b").is_empty());
    }

    #[test]
    fn the_name_servers_are_the_addresses_of_the_nameserver_lines() {
        let text = "# x\nnameserver 10.0.2.3\nnameserver  fec0::3 \nnameserver nope\nnameserver\n\
                    search lan\n";
        assert_eq!(nameservers(text), ["10.0.2.3", "fec0::3"]);
        assert!(nameservers("").is_empty());
    }

    #[test]
    fn the_text_says_each_card_and_the_trial() {
        let sources =
            machine("[net0]\naddress = 10.0.2.15/24\nrouter = 10.0.2.2\nipv6 = yes\n", None);
        let shown = text(&report(&sources, None, Some(41_000)).unwrap(), 0);
        assert_eq!(
            shown,
            "net0  52:54:00:12:34:56  link up, 1000 Mb/s\n\
             \x20 addresses  10.0.2.15/24 (fixed), 192.168.7.1/32 (other), fe80::8c1:5bff:fe2e:a7d3/64 \
             (link-local), fec0::d1f:8e01:6a3c:52b0/64 (router), 2001:db8::5/64 (dhcp), \
             2001:db8::6/64 (dhcp)\n\
             \x20 routers    10.0.2.2, fe80::2\n\
             \x20 dns        10.0.2.3, fec0::3\n\
             \x20 settings   [net0] fixed, addresses 10.0.2.15/24, router 10.0.2.2, metered auto, \
             also ipv6\n\
             \n\
             The network settings are being tried: 00:41 left before they are undone unless kept.\n"
        );
    }

    #[test]
    fn the_text_says_when_the_lease_ends() {
        let sources = machine("", Some(LEASE));
        let report = report(&sources, None, None).unwrap();
        // The lease ends at 1763120000, an hour and a minute from now.
        let now = 1_763_116_340_000;
        assert!(text(&report, now).contains("  lease      10.0.2.15, ends in 01:01:00\n"));
        assert!(text(&report, 1_763_120_000_000).contains("10.0.2.15, has ended\n"));
        let forever = LEASE.replace("seconds = 3600", "seconds = 4294967295");
        let report = super::report(&machine("", Some(&forever)), None, None).unwrap();
        assert!(text(&report, now).contains("  lease      10.0.2.15, for ever\n"));
    }
}
