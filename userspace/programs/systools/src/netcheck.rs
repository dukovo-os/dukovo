// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! netcheck(8): where a machine's network stops working (ADR 0489). It
//! walks the layers in the order a packet needs them, the link, the
//! address, the gateway, the DNS servers, the clock, a packet of the
//! card's full size and, when asked, the update server, and stops at the
//! first that fails: the layers after it cannot say anything true. A
//! warning does not stop it. Each result that is not a pass has an ID
//! (`NET-102`), whose page in the support center says what to do, and
//! one sentence of advice.
//!
//! Nothing leaves the local network unless `--internet` is given: the
//! gateway and the DNS servers on a card's network are asked, and the
//! update server the system follows (`updates.toml`, ADR 0466) only with
//! it. The facts come from what the other tools read: `ip`'s interfaces
//! and routes, netd's settings, resolved's servers, timed's status and
//! `ping`'s echo.
//!
//! The firewall layer of the design is not here: the firewall filters
//! only what other machines start (ADR 0334), so no rule of it can stop
//! this machine's own DNS, HTTPS or time, which docs/deferred.md says.
//!
//! When it misbehaves, run the tool it asks of the layer by hand: `ip
//! link`, `ip route`, `ping`, `resolvectl status`, `timedatectl
//! timesync-status`. The `netcheck` boot test plants a fault in each
//! layer.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::net::{IpAddr, SocketAddr, SocketAddrV6, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

use netproto::dns;
use serde::Serialize;

use crate::args::{Arg, Args};
use crate::dig::{self, Asking};
use crate::ip::{self, Interface, Kind};
use crate::netd::{self, Setup};
use crate::ping::{self, Echo};
use crate::resolved::{self, Kind as Asked, OverTls, Server};
use crate::sysupdate::online;
use crate::timed;
use crate::tls;
use crate::util::{self, Fail, fail_with, usage};

const USAGE: &str = "Usage: netcheck [--internet] [--card NAME] [--json]\n  \
                     --internet   also check the update server, beyond the local network\n  \
                     --card NAME  check only this network card\n  \
                     --json       the results as JSON";

/// The exit statuses: every layer passed, one failed, only warnings, and
/// the check could not run.
const PASSED: i32 = 0;
const FAILED: i32 = 1;
const WARNED: i32 = 2;
const COULD_NOT_RUN: i32 = 3;

/// How long a gateway or a DNS server has to answer.
const ANSWER_WAIT: Duration = Duration::from_secs(2);

/// How many times each is asked.
const TRIES: u32 = 2;

/// How long a connection to the update server has to be made.
const CONNECT_WAIT: Duration = Duration::from_secs(4);

/// The most addresses of each version tried for the update server.
const ADDRESSES_TRIED: usize = 2;

/// The data bytes of the small echo request, ping's own.
const SMALL: usize = 56;

/// The bytes the headers of an echo request take, by IP version: the IP
/// header and the ICMP header.
const HEADERS_V4: usize = 20 + 8;
const HEADERS_V6: usize = 40 + 8;

/// How long after its last sync the clock is still taken as kept by a
/// time server.
const SYNC_FRESH: u64 = 86_400;

/// A layer of the network, in the order they are checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layer {
    Link,
    Address,
    Gateway,
    Dns,
    Time,
    Mtu,
    Internet,
}

const LAYERS: [Layer; 7] = [
    Layer::Link,
    Layer::Address,
    Layer::Gateway,
    Layer::Dns,
    Layer::Time,
    Layer::Mtu,
    Layer::Internet,
];

impl Layer {
    fn name(self) -> &'static str {
        match self {
            Layer::Link => "link",
            Layer::Address => "address",
            Layer::Gateway => "gateway",
            Layer::Dns => "dns",
            Layer::Time => "time",
            Layer::Mtu => "mtu",
            Layer::Internet => "internet",
        }
    }
}

/// What a layer found that is not a pass: the ID of its page, what is
/// wrong, and one sentence on what to do.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Finding {
    id: &'static str,
    what: String,
    advice: String,
}

/// How a layer came out.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Outcome {
    Passed(String),
    Warning(Finding),
    Failed(Finding),
    NotChecked(String),
}

fn failed(id: &'static str, what: String, advice: &str) -> Outcome {
    Outcome::Failed(Finding { id, what, advice: advice.to_string() })
}

fn warning(id: &'static str, what: String, advice: &str) -> Outcome {
    Outcome::Warning(Finding { id, what, advice: advice.to_string() })
}

/// What the command line asks for.
#[derive(Debug, Default)]
struct Options {
    internet: bool,
    card: Option<String>,
    json: bool,
}

/// netcheck(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    // A mistake in the command line is "could not run", not "failed":
    // the exit status 1 is for a layer that failed.
    let Some(options) = read_options(args).map_err(|fail| Fail { code: COULD_NOT_RUN, ..fail })?
    else {
        return Ok(PASSED);
    };
    let machine = Machine::read(options.card.clone()).map_err(|err| {
        fail_with(COULD_NOT_RUN, format!("{}: {}", ip::INTERFACES, util::strip_os_error(&err)))
    })?;
    let found = walk(&machine, &options);
    let status = status_of(&found);
    let now = i64::try_from(util::now_ns() / 1_000_000_000).unwrap_or(0);
    let text = if options.json { json(&found, status, now) } else { plain(&found, status) };
    print!("{text}");
    Ok(status)
}

/// What the command line asks for, or `None` after `--help`.
fn read_options(args: &mut Args) -> Result<Option<Options>, Fail> {
    let mut options = Options::default();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(ref name) if name == "internet" => options.internet = true,
            Arg::Long(ref name) if name == "json" => options.json = true,
            Arg::Long(ref name) if name == "card" => options.card = Some(args.text()?),
            Arg::Long(ref name) if name == "help" => {
                println!("{USAGE}");
                return Ok(None);
            }
            Arg::Short('h') => return Err(usage(USAGE.into())),
            other => return Err(args.unknown(&other)),
        }
    }
    Ok(Some(options))
}

/// The network as the checks read it, once.
struct Machine {
    interfaces: Vec<Interface>,
    /// netd's settings, by card.
    settings: Vec<(String, Setup)>,
    /// The DNS servers resolved asks, and how it asks those named by
    /// address alone.
    servers: Vec<Server>,
    over_tls: OverTls,
    /// The only card to check, if the command line names one.
    card: Option<String>,
}

impl Machine {
    fn read(card: Option<String>) -> io::Result<Machine> {
        let interfaces = ip::read_interfaces(Path::new(ip::INTERFACES), Path::new(ip::ROUTES))?;
        let (settings, _) =
            netd::parse_settings(&fs::read_to_string(netd::SETTINGS).unwrap_or_default());
        let (servers, over_tls) = resolved::servers_asked();
        Ok(Machine { interfaces, settings, servers, over_tls, card })
    }

    /// The cards to check: every interface that is not the loopback, a
    /// tunnel, which has no link of its own, or a bridge's port, whose
    /// bridge has the addresses; or the one card named.
    fn cards(&self) -> Vec<&Interface> {
        self.interfaces
            .iter()
            .filter(|card| !card.loopback && card.kind != Kind::Tunnel && card.master.is_none())
            .filter(|card| self.card.as_ref().is_none_or(|name| *name == card.name))
            .collect()
    }

    /// The cards that have a link.
    fn linked(&self) -> Vec<&Interface> {
        self.cards().into_iter().filter(|card| card.carrier).collect()
    }

    /// The cards that have a link and an address.
    fn addressed(&self) -> Vec<&Interface> {
        self.linked().into_iter().filter(|card| !addresses(card).is_empty()).collect()
    }
}

/// Checks each layer in turn, stopping at the first that fails.
fn walk(machine: &Machine, options: &Options) -> Vec<(Layer, Outcome)> {
    let mut reached = Vec::new();
    let mut found = Vec::new();
    let mut stopped = false;
    for layer in LAYERS {
        let outcome = if stopped {
            Outcome::NotChecked("an earlier layer failed".into())
        } else {
            match layer {
                Layer::Link => link(machine),
                Layer::Address => address(machine),
                Layer::Gateway => gateway(machine, &mut reached),
                Layer::Dns => dns(machine, options.internet),
                Layer::Time => time(),
                Layer::Mtu => mtu(&reached),
                Layer::Internet => internet(machine, options.internet),
            }
        };
        stopped |= matches!(outcome, Outcome::Failed(_));
        found.push((layer, outcome));
    }
    found
}

/// The exit status of the results.
fn status_of(found: &[(Layer, Outcome)]) -> i32 {
    if found.iter().any(|(_, outcome)| matches!(outcome, Outcome::Failed(_))) {
        FAILED
    } else if found.iter().any(|(_, outcome)| matches!(outcome, Outcome::Warning(_))) {
        WARNED
    } else {
        PASSED
    }
}

/// `net0`, `net0 and net1`, `net0, net1 and net2`.
fn names(cards: &[&str]) -> String {
    match cards {
        [] => String::new(),
        [only] => (*only).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The link layer: a card, and a link on it.
fn link(machine: &Machine) -> Outcome {
    link_outcome(&machine.cards(), machine.card.as_deref())
}

fn link_outcome(cards: &[&Interface], named: Option<&str>) -> Outcome {
    if cards.is_empty() {
        let what = match named {
            Some(name) => format!("no network card is named {name}"),
            None => "this machine has no network card".into(),
        };
        return failed(
            "NET-101",
            what,
            "Check that a card is fitted and that Dukovo has a driver for it: lspci and lsusb \
             list the devices, and ip link the cards the system set up.",
        );
    }
    let linked: Vec<&str> =
        cards.iter().filter(|card| card.carrier).map(|card| card.name.as_str()).collect();
    if linked.is_empty() {
        let all: Vec<&str> = cards.iter().map(|card| card.name.as_str()).collect();
        let verb = if all.len() == 1 { "has" } else { "have" };
        return failed(
            "NET-102",
            format!("{} {verb} no link", names(&all)),
            "Check the cable at both ends and that the switch or router it goes to is on.",
        );
    }
    let verb = if linked.len() == 1 { "has" } else { "have" };
    Outcome::Passed(format!("{} {verb} a link", names(&linked)))
}

/// The addresses a card has that reach beyond its own network: IPv4
/// ones, and IPv6 ones that are not link-local or still being tested.
fn addresses(card: &Interface) -> Vec<String> {
    let v6 = card
        .ipv6
        .addresses
        .iter()
        .filter(|address| !address.tentative && !ip::link_local(address.cidr.ip))
        .map(|address| address.cidr.to_string());
    card.addresses.iter().map(ToString::to_string).chain(v6).collect()
}

/// The address layer: an address on a card that has a link.
fn address(machine: &Machine) -> Outcome {
    let cards: Vec<(&Interface, bool)> = machine
        .linked()
        .into_iter()
        .map(|card| {
            let setup = netd::setup_of(&machine.settings, &card.name, &card.mac);
            (card, setup.dhcp())
        })
        .collect();
    address_outcome(&cards)
}

/// `cards` are those with a link, each with whether its settings have
/// it ask DHCP.
fn address_outcome(cards: &[(&Interface, bool)]) -> Outcome {
    let have: Vec<String> = cards
        .iter()
        .filter_map(|(card, _)| {
            addresses(card).first().map(|first| format!("{} has {first}", card.name))
        })
        .collect();
    if !have.is_empty() {
        return Outcome::Passed(have.join(", "));
    }
    let asks_dhcp = cards.iter().any(|(_, dhcp)| *dhcp);
    let all: Vec<&str> = cards.iter().map(|(card, _)| card.name.as_str()).collect();
    if asks_dhcp {
        failed(
            "NET-111",
            format!("{} has no address: DHCP got no answer", names(&all)),
            "Check that a DHCP server, usually the router, is on this network; log -s netd \
             shows what the card asked.",
        )
    } else {
        failed(
            "NET-111",
            format!("{} has no address: its settings give none", names(&all)),
            "Give the card an address in /system/settings/network, or set dhcp = yes there, \
             then run service netd reload.",
        )
    }
}

/// A default route of a card with an address.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Route {
    card: String,
    index: u32,
    mtu: u32,
    gateway: IpAddr,
}

impl Route {
    /// Where an echo request to the gateway goes: a link-local IPv6
    /// gateway needs the card's number to be reached.
    fn destination(&self) -> SocketAddr {
        match self.gateway {
            IpAddr::V6(gateway) if ip::link_local(gateway) => {
                SocketAddr::V6(SocketAddrV6::new(gateway, 0, 0, self.index))
            }
            gateway => SocketAddr::new(gateway, 0),
        }
    }

    /// The data bytes of an echo request that makes a packet of the
    /// card's full size.
    fn full_size(&self) -> usize {
        let headers = if self.gateway.is_ipv6() { HEADERS_V6 } else { HEADERS_V4 };
        usize::try_from(self.mtu).unwrap_or(0).saturating_sub(headers)
    }
}

/// The default routes of the cards that have a link and an address.
fn default_routes(machine: &Machine) -> Vec<Route> {
    let mut routes = Vec::new();
    for card in machine.addressed() {
        let gateways =
            card.router.map(IpAddr::V4).into_iter().chain(card.ipv6.router().map(IpAddr::V6));
        for gateway in gateways {
            routes.push(Route {
                card: card.name.clone(),
                index: card.index,
                mtu: card.mtu,
                gateway,
            });
        }
    }
    routes
}

/// Sends `size` data bytes to `route`'s gateway, up to [`TRIES`] times,
/// until one is answered.
fn ask_gateway(route: &Route, size: usize) -> Echo {
    let mut last = Echo::Silent;
    for _ in 0..TRIES {
        match ping::echo(route.destination(), size, ANSWER_WAIT) {
            Ok(answered @ Echo::Answered(_)) => return answered,
            Ok(other) => last = other,
            Err(err) => last = Echo::Refused(util::strip_os_error(&err)),
        }
    }
    last
}

/// The gateway layer: a default route, and a gateway that answers. The
/// routes whose gateway answered go in `reached`, for the MTU layer.
fn gateway(machine: &Machine, reached: &mut Vec<Route>) -> Outcome {
    let asked: Vec<(Route, Echo)> = default_routes(machine)
        .into_iter()
        .map(|route| {
            let echo = ask_gateway(&route, SMALL);
            (route, echo)
        })
        .collect();
    reached.extend(
        asked
            .iter()
            .filter(|(_, echo)| matches!(echo, Echo::Answered(_)))
            .map(|(route, _)| route.clone()),
    );
    gateway_outcome(&machine.addressed(), &asked)
}

/// `cards` are those with a link and an address, `asked` each default
/// route with how its gateway answered.
fn gateway_outcome(cards: &[&Interface], asked: &[(Route, Echo)]) -> Outcome {
    if asked.is_empty() {
        let all: Vec<&str> = cards.iter().map(|card| card.name.as_str()).collect();
        return failed(
            "NET-121",
            format!("{} has an address but no default route", names(&all)),
            "Give the card a router: a DHCP server gives one, or set router = for the card in \
             /system/settings/network.",
        );
    }
    let answered: Vec<String> = asked
        .iter()
        .filter_map(|(route, echo)| match echo {
            Echo::Answered(took) => {
                Some(format!("{} answers in {:.1} ms", route.gateway, took.as_secs_f64() * 1000.0))
            }
            _ => None,
        })
        .collect();
    if !answered.is_empty() {
        return Outcome::Passed(answered.join(", "));
    }
    let silent: Vec<String> = asked
        .iter()
        .map(|(route, echo)| match echo {
            Echo::Refused(why) => format!("{} ({why})", route.gateway),
            _ => route.gateway.to_string(),
        })
        .collect();
    let verb = if silent.len() == 1 { "does" } else { "do" };
    failed(
        "NET-122",
        format!("the gateway {} {verb} not answer", silent.join(", ")),
        "Check that the router at that address is on and in this network, and that the card's \
         router setting or the DHCP server names the right one.",
    )
}

/// Whether `server` is on a network one of `cards` has an address in,
/// so that asking it sends nothing beyond that network.
fn on_network(server: &Server, cards: &[&Interface]) -> bool {
    match server.address.ip() {
        IpAddr::V4(ip) => {
            cards.iter().any(|card| card.addresses.iter().any(|cidr| cidr.contains(ip)))
        }
        IpAddr::V6(ip) => cards.iter().any(|card| {
            card.ipv6
                .addresses
                .iter()
                .any(|address| !ip::link_local(address.cidr.ip) && address.cidr.contains(ip))
        }),
    }
}

/// Asks `server` for the root's name servers. Any answer, even a
/// refusal, shows the server is there. A server resolved asks over TLS
/// or HTTPS only is asked for a connection.
fn server_answers(server: &Server, over_tls: OverTls) -> Result<(), String> {
    let by_connection = match server.kind {
        Asked::Address => over_tls == OverTls::Yes,
        Asked::Tls(_) | Asked::Https { .. } => true,
    };
    if by_connection {
        let mut address = server.address;
        if server.kind == Asked::Address {
            address.set_port(853);
        }
        return TcpStream::connect_timeout(&address, ANSWER_WAIT)
            .map(drop)
            .map_err(|err| util::strip_os_error(&err));
    }
    let asking = Asking {
        server: Some(server.address.ip().to_string()),
        port: server.address.port(),
        time: ANSWER_WAIT,
        tries: TRIES,
        ..Asking::default()
    };
    let message = dig::build(".", dns::NS, dns::IN, &asking)
        .map_err(|_| "its question cannot be made".to_string())?;
    dig::exchange(&message, &asking, false).map(drop).map_err(|_| "no answer".to_string())
}

/// The DNS layer: servers, and one that answers. Those off the local
/// network are asked only with `internet`.
fn dns(machine: &Machine, internet: bool) -> Outcome {
    let cards = machine.addressed();
    let asked: Vec<(String, bool)> = machine
        .servers
        .iter()
        .filter(|server| internet || on_network(server, &cards))
        .map(|server| {
            (server.address.ip().to_string(), server_answers(server, machine.over_tls).is_ok())
        })
        .collect();
    dns_outcome(machine.servers.len(), &asked)
}

/// `configured` servers, of which `asked` were, each with whether it
/// answered.
fn dns_outcome(configured: usize, asked: &[(String, bool)]) -> Outcome {
    if configured == 0 {
        return failed(
            "NET-131",
            "resolved has no DNS server: its settings name none and DHCP gave none".into(),
            "Name a server with servers = in /system/settings/resolved, or have the DHCP \
             server give one.",
        );
    }
    if asked.is_empty() {
        return Outcome::NotChecked(
            "its servers are off the local network: netcheck --internet asks them".into(),
        );
    }
    let answering: Vec<&str> =
        asked.iter().filter(|(_, answered)| *answered).map(|(server, _)| server.as_str()).collect();
    if !answering.is_empty() {
        let silent: Vec<&str> = asked
            .iter()
            .filter(|(_, answered)| !*answered)
            .map(|(server, _)| server.as_str())
            .collect();
        let mut detail = format!("{} answers", names(&answering));
        if !silent.is_empty() {
            let _ = write!(detail, "; {} does not", names(&silent));
        }
        return Outcome::Passed(detail);
    }
    let silent: Vec<&str> = asked.iter().map(|(server, _)| server.as_str()).collect();
    failed(
        "NET-132",
        format!(
            "no DNS server answers: {} {}",
            names(&silent),
            if silent.len() == 1 { "does not" } else { "do not" }
        ),
        "Check that each server is on and reachable from this network, or name another with \
         servers = in /system/settings/resolved.",
    )
}

/// The time layer: the clock set from a time server, and lately.
fn time() -> Outcome {
    let status = fs::read_to_string(timed::STATUS).ok();
    let now = util::now_ns() / 1_000_000_000;
    time_outcome(status.as_deref(), timed::last_synced(), now)
}

/// `status` is timed's status file, `last` the time it last synced, in
/// seconds since 1970, and `now` the clock's.
fn time_outcome(status: Option<&str>, last: Option<u64>, now: u64) -> Outcome {
    let synced = status.is_some_and(|text| text.starts_with("Synchronized: yes"));
    let age = last.map(|last| now.saturating_sub(last));
    let what = match (synced, age) {
        (false, _) => "timed has not set the clock from a time server since the start".to_string(),
        (true, Some(age)) if age > SYNC_FRESH => {
            let days = age / 86_400;
            format!("the clock was last set from a time server {days} days ago")
        }
        (true, _) => return Outcome::Passed("timed set the clock from a time server".into()),
    };
    warning(
        "NET-141",
        what,
        "Check that a time server can be reached: timedatectl timesync-status says what timed \
         tried, and /system/settings/timed names the servers it asks.",
    )
}

/// The MTU layer: a packet of the card's full size reaches a gateway
/// that answered a small one. `reached` are the routes whose gateway
/// did.
fn mtu(reached: &[Route]) -> Outcome {
    let sent: Vec<(&Route, Echo)> = reached
        .iter()
        .filter(|route| route.full_size() > SMALL)
        .map(|route| (route, ask_gateway(route, route.full_size())))
        .collect();
    mtu_outcome(&sent)
}

/// `sent` are the routes whose gateway answered a small packet, each
/// with how it answered one of the card's full size.
fn mtu_outcome(sent: &[(&Route, Echo)]) -> Outcome {
    if sent.is_empty() {
        return Outcome::NotChecked(
            "no gateway answered a packet of the card's full size to test".into(),
        );
    }
    if let Some((route, _)) = sent.iter().find(|(_, echo)| matches!(echo, Echo::Answered(_))) {
        return Outcome::Passed(format!("packets of {} bytes reach {}", route.mtu, route.gateway));
    }
    let route = sent[0].0;
    warning(
        "NET-161",
        format!(
            "packets of {} bytes do not reach {}, though small ones do",
            route.mtu, route.gateway
        ),
        &format!(
            "Something between this machine and the gateway, such as a switch or a tunnel, \
             carries smaller packets than {} sends, so ask whoever runs the network to allow {} \
             bytes.",
            route.card, route.mtu
        ),
    )
}

/// Where the update server is.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Target {
    host: String,
    port: u16,
    tls: bool,
}

/// The server of the update settings, `https://host[:port]/path`.
fn target_of(server: &str) -> Option<Target> {
    let (tls, rest) = match server.strip_prefix("https://") {
        Some(rest) => (true, rest),
        None => (false, server.strip_prefix("http://")?),
    };
    let authority = rest.split('/').next()?;
    let authority = authority.rsplit('@').next()?;
    let (host, port) = util::split_port(authority, if tls { 443 } else { 80 }).ok()?;
    (!host.is_empty()).then(|| Target { host: host.to_string(), port, tls })
}

/// The internet layer: the update server resolves, answers on its port,
/// and its certificate is trusted now.
fn internet(machine: &Machine, asked: bool) -> Outcome {
    if !asked {
        return Outcome::NotChecked(
            "it sends nothing off the local network: netcheck --internet checks the update \
             server"
                .into(),
        );
    }
    let server = match online::Settings::load() {
        Ok(settings) => settings.server,
        Err(fail) => {
            return Outcome::NotChecked(format!(
                "the update settings cannot be read: {}",
                fail.message
            ));
        }
    };
    let Some(target) = target_of(&server) else {
        return Outcome::NotChecked(format!("{server} is not an address it can check"));
    };
    let has_v4 = machine.addressed().iter().any(|card| !card.addresses.is_empty());
    let has_v6 = machine
        .addressed()
        .iter()
        .any(|card| card.ipv6.addresses.iter().any(|a| !a.tentative && !ip::link_local(a.cidr.ip)));
    let addresses = match resolve(&target) {
        Ok(addresses) => addresses,
        Err(outcome) => return outcome,
    };
    let reached = connect(&target, &addresses, has_v4, has_v6);
    if let Some(outcome) = connect_outcome(&target, &reached) {
        return outcome;
    }
    let first = reached.iter().find_map(|(address, result)| result.is_ok().then_some(*address));
    if let (true, Some(address)) = (target.tls, first)
        && let Err(outcome) = trusted(&target, address)
    {
        return outcome;
    }
    if let Some(outcome) = family_outcome(&target, &reached, has_v6) {
        return outcome;
    }
    let how = if target.tls {
        " and its certificate is trusted"
    } else {
        " (plain http, no certificate)"
    };
    Outcome::Passed(format!("{} answers on port {}{how}", target.host, target.port))
}

/// The update server's addresses, or the failure NET-171.
fn resolve(target: &Target) -> Result<Vec<IpAddr>, Outcome> {
    if let Ok(address) = target.host.parse::<IpAddr>() {
        return Ok(vec![address]);
    }
    (target.host.as_str(), target.port)
        .to_socket_addrs()
        .map(|found| found.map(|address| address.ip()).collect())
        .map_err(|_| unresolved(target))
}

/// NET-171: the update server's name has no address.
fn unresolved(target: &Target) -> Outcome {
    failed(
        "NET-171",
        format!("the name {} does not resolve", target.host),
        &format!(
            "Check the update server's address in /system/settings/sysupdate/updates.toml and \
             that DNS works: resolvectl query {} asks resolved.",
            target.host
        ),
    )
}

/// Tries to connect to the first [`ADDRESSES_TRIED`] addresses of each
/// IP version the machine has, and gives each address with how it went.
fn connect(
    target: &Target,
    addresses: &[IpAddr],
    has_v4: bool,
    has_v6: bool,
) -> Vec<(IpAddr, Result<(), String>)> {
    let mut reached = Vec::new();
    for version6 in [false, true] {
        if (version6 && !has_v6) || (!version6 && !has_v4) {
            continue;
        }
        for address in addresses.iter().filter(|a| a.is_ipv6() == version6).take(ADDRESSES_TRIED) {
            let result =
                TcpStream::connect_timeout(&SocketAddr::new(*address, target.port), CONNECT_WAIT)
                    .map(drop)
                    .map_err(|err| util::strip_os_error(&err));
            reached.push((*address, result));
        }
    }
    reached
}

/// NET-172, when no address answered. `reached` is each address tried,
/// with how it went.
fn connect_outcome(target: &Target, reached: &[(IpAddr, Result<(), String>)]) -> Option<Outcome> {
    if reached.iter().any(|(_, result)| result.is_ok()) {
        return None;
    }
    let why = match reached {
        [] => "it has no address of a version this machine has an address of".to_string(),
        tried => tried
            .iter()
            .filter_map(|(address, result)| {
                result.as_ref().err().map(|why| format!("{address}: {why}"))
            })
            .collect::<Vec<_>>()
            .join("; "),
    };
    Some(failed(
        "NET-172",
        format!("{} does not answer on port {} ({why})", target.host, target.port),
        &format!(
            "Check that the server is up and that nothing between this machine and it blocks \
             port {}.",
            target.port
        ),
    ))
}

/// Makes a TLS connection to the update server at `address`, checking
/// its certificate against the roots the system trusts: the failure
/// NET-173, NET-174 or NET-172 if it cannot be made.
fn trusted(target: &Target, address: IpAddr) -> Result<(), Outcome> {
    let name = tls::server_name(Some(&target.host), SocketAddr::new(address, target.port))
        .map_err(|err| certificate_outcome(target, &err))?;
    let config = tls::verified(tls::roots(&[], "netcheck"), &[]);
    tls::connect(SocketAddr::new(address, target.port), name, &config, CONNECT_WAIT)
        .map(drop)
        .map_err(|err| certificate_outcome(target, &err))
}

/// What a failed TLS connection to the update server means: the clock
/// is wrong, the certificate is not one the system trusts, or the
/// connection failed otherwise.
fn certificate_outcome(target: &Target, error: &io::Error) -> Outcome {
    let host = &target.host;
    if timed::certificate_time(error) {
        let now = i64::try_from(util::now_ns() / 1_000_000_000).unwrap_or(0);
        return failed(
            "NET-174",
            format!(
                "{host}'s certificate is not valid by this machine's clock, which says {}",
                util::time_text(now, "%F %T UTC", true)
            ),
            "Check the date: timed keeps the clock right once a time server answers, and \
             timedatectl set-time sets it by hand.",
        );
    }
    let inner = error.get_ref().and_then(|inner| inner.downcast_ref::<rustls::Error>());
    let Some(rustls::Error::InvalidCertificate(problem)) = inner else {
        return failed(
            "NET-172",
            format!(
                "{host} answers on port {}, but the TLS connection fails: {}",
                target.port,
                util::strip_os_error(error)
            ),
            "Check that the server speaks TLS on that port and that nothing between this machine \
             and it cuts the connection.",
        );
    };
    let words = match problem {
        rustls::CertificateError::UnknownIssuer => {
            "its certificate is signed by an authority this system does not trust"
        }
        rustls::CertificateError::NotValidForName
        | rustls::CertificateError::NotValidForNameContext { .. } => {
            "its certificate is for another name"
        }
        _ => "its certificate is not one this system trusts",
    };
    failed(
        "NET-173",
        format!("something answers for {host}, but {words}"),
        "A proxy or a sign-in page may be answering in the server's place: sign in to the \
         network's page if it has one, or add its authority to the system's certificates \
         (dukovo-certificates(7)).",
    )
}

/// NET-175, when the update server answered over IPv4 and not over IPv6
/// though the machine has an IPv6 address and the server one.
fn family_outcome(
    target: &Target,
    reached: &[(IpAddr, Result<(), String>)],
    has_v6: bool,
) -> Option<Outcome> {
    let version = |six: bool| reached.iter().filter(move |(address, _)| address.is_ipv6() == six);
    let v4_works = version(false).any(|(_, result)| result.is_ok());
    let v6_tried = has_v6 && version(true).next().is_some();
    let v6_works = version(true).any(|(_, result)| result.is_ok());
    (v4_works && v6_tried && !v6_works).then(|| {
        warning(
            "NET-175",
            format!("{} answers over IPv4 but not over IPv6", target.host),
            "Check that the router gives IPv6 addresses and routes, or turn IPv6 off for the \
             card with ipv6 = no in /system/settings/network.",
        )
    })
}

/// The results as text, a line a layer, with the advice and the page of
/// each that is not a pass.
fn plain(found: &[(Layer, Outcome)], status: i32) -> String {
    let mut text = String::new();
    for (layer, outcome) in found {
        let name = layer.name();
        match outcome {
            Outcome::Passed(detail) => {
                let _ = writeln!(text, "{name:<9} {:<12} {detail}", "passed");
            }
            Outcome::NotChecked(why) => {
                let _ = writeln!(text, "{name:<9} {:<12} {why}", "not checked");
            }
            Outcome::Warning(finding) | Outcome::Failed(finding) => {
                let word = if matches!(outcome, Outcome::Failed(_)) { "FAILED" } else { "warning" };
                let _ = writeln!(text, "{name:<9} {word:<12} {}: {}", finding.id, finding.what);
                let _ = writeln!(text, "{:<22} {}", "", finding.advice);
                let _ = writeln!(
                    text,
                    "{:<22} Help: https://dukovo.org/support/{}/errors/{}.html",
                    "",
                    env!("CARGO_PKG_VERSION"),
                    finding.id
                );
            }
        }
    }
    let summary = match status {
        PASSED => "netcheck: every layer passed.".to_string(),
        WARNED => "netcheck: passed, with warnings.".to_string(),
        _ => {
            let layer = found
                .iter()
                .find(|(_, outcome)| matches!(outcome, Outcome::Failed(_)))
                .map_or("", |(layer, _)| layer.name());
            format!("netcheck: failed at the {layer} layer; the layers after it are not checked.")
        }
    };
    text.push_str(&summary);
    text.push('\n');
    text
}

/// One layer's result, as `--json` gives it.
#[derive(Serialize)]
struct Line<'a> {
    layer: &'static str,
    result: &'static str,
    id: Option<&'static str>,
    detail: &'a str,
    advice: Option<&'a str>,
}

/// What `--json` gives: the version, the time, the exit status's name
/// and every layer.
#[derive(Serialize)]
struct Report<'a> {
    version: &'static str,
    time: String,
    status: &'static str,
    checks: Vec<Line<'a>>,
}

fn json(found: &[(Layer, Outcome)], status: i32, now: i64) -> String {
    let checks = found
        .iter()
        .map(|(layer, outcome)| {
            let (result, id, detail, advice) = match outcome {
                Outcome::Passed(detail) => ("passed", None, detail.as_str(), None),
                Outcome::NotChecked(why) => ("not checked", None, why.as_str(), None),
                Outcome::Warning(f) => {
                    ("warning", Some(f.id), f.what.as_str(), Some(f.advice.as_str()))
                }
                Outcome::Failed(f) => {
                    ("failed", Some(f.id), f.what.as_str(), Some(f.advice.as_str()))
                }
            };
            Line { layer: layer.name(), result, id, detail, advice }
        })
        .collect();
    let report = Report {
        version: env!("CARGO_PKG_VERSION"),
        time: util::time_text(now, "%Y-%m-%dT%H:%M:%SZ", true),
        status: match status {
            PASSED => "passed",
            WARNED => "warning",
            _ => "failed",
        },
        checks,
    };
    let mut text = serde_json::to_string_pretty(&report).expect("a report of strings serializes");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    use crate::ip::{Address6, Cidr, Cidr6, Ipv6, Origin};

    fn card(name: &str, carrier: bool) -> Interface {
        Interface {
            name: name.into(),
            index: 2,
            mtu: 1500,
            mac: "52:54:00:12:34:56".into(),
            loopback: false,
            carrier,
            operstate: if carrier { "up" } else { "down" }.into(),
            addresses: Vec::new(),
            router: None,
            ipv6: Ipv6::default(),
            kind: Kind::Card,
            master: None,
            received: [0; 4],
            sent: [0; 4],
        }
    }

    fn addressed(mut card: Interface) -> Interface {
        card.addresses.push(Cidr { ip: Ipv4Addr::new(10, 0, 2, 15), prefix: 24 });
        card
    }

    fn route(gateway: &str, mtu: u32) -> Route {
        Route { card: "net0".into(), index: 2, mtu, gateway: gateway.parse().unwrap() }
    }

    fn finding(outcome: &Outcome) -> &Finding {
        match outcome {
            Outcome::Warning(finding) | Outcome::Failed(finding) => finding,
            other => panic!("expected a warning or a failure, got {other:?}"),
        }
    }

    /// The advice is one sentence: it ends at its only full stop.
    fn one_sentence(finding: &Finding) {
        assert!(finding.advice.ends_with('.'), "{}", finding.advice);
        assert_eq!(finding.advice.matches(". ").count(), 0, "{}", finding.advice);
    }

    #[test]
    fn a_machine_with_no_card_fails_the_link_layer() {
        let outcome = link_outcome(&[], None);
        assert!(matches!(outcome, Outcome::Failed(_)));
        assert_eq!(finding(&outcome).id, "NET-101");
        assert_eq!(finding(&outcome).what, "this machine has no network card");
        one_sentence(finding(&outcome));
        let named = link_outcome(&[], Some("net9"));
        assert_eq!(finding(&named).what, "no network card is named net9");
    }

    #[test]
    fn cards_without_a_link_fail_the_link_layer() {
        let (a, b) = (card("net0", false), card("net1", false));
        let outcome = link_outcome(&[&a, &b], None);
        assert!(matches!(outcome, Outcome::Failed(_)));
        assert_eq!(finding(&outcome).id, "NET-102");
        assert_eq!(finding(&outcome).what, "net0 and net1 have no link");
        one_sentence(finding(&outcome));
        let one = link_outcome(&[&a], None);
        assert_eq!(finding(&one).what, "net0 has no link");
    }

    #[test]
    fn one_card_with_a_link_passes_the_link_layer() {
        let (a, b) = (card("net0", false), card("net1", true));
        assert_eq!(link_outcome(&[&a, &b], None), Outcome::Passed("net1 has a link".into()));
    }

    #[test]
    fn a_card_that_asks_dhcp_and_has_no_address_fails_for_dhcp() {
        let a = card("net0", true);
        let outcome = address_outcome(&[(&a, true)]);
        assert!(matches!(outcome, Outcome::Failed(_)));
        assert_eq!(finding(&outcome).id, "NET-111");
        assert_eq!(finding(&outcome).what, "net0 has no address: DHCP got no answer");
        one_sentence(finding(&outcome));
    }

    #[test]
    fn a_card_whose_settings_give_no_address_fails_for_its_settings() {
        let a = card("net0", true);
        let outcome = address_outcome(&[(&a, false)]);
        assert_eq!(finding(&outcome).id, "NET-111");
        assert_eq!(finding(&outcome).what, "net0 has no address: its settings give none");
        one_sentence(finding(&outcome));
    }

    #[test]
    fn a_link_local_address_is_no_address() {
        let mut a = card("net0", true);
        a.ipv6.addresses.push(Address6 {
            cidr: Cidr6 { ip: "fe80::5054:ff:fe12:3456".parse().unwrap(), prefix: 64 },
            origin: Origin::LinkLocal,
            preferred: None,
            valid: None,
            tentative: false,
        });
        assert!(matches!(address_outcome(&[(&a, true)]), Outcome::Failed(_)));
    }

    #[test]
    fn an_address_passes_the_address_layer() {
        let a = addressed(card("net0", true));
        assert_eq!(address_outcome(&[(&a, true)]), Outcome::Passed("net0 has 10.0.2.15/24".into()));
        let mut v6 = card("net1", true);
        v6.ipv6.addresses.push(Address6 {
            cidr: Cidr6 { ip: Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 5), prefix: 64 },
            origin: Origin::Stable,
            preferred: None,
            valid: None,
            tentative: false,
        });
        assert_eq!(
            address_outcome(&[(&v6, true)]),
            Outcome::Passed("net1 has 2001:db8::5/64".into())
        );
    }

    #[test]
    fn a_card_with_an_address_and_no_router_fails_the_gateway_layer() {
        let a = addressed(card("net0", true));
        let outcome = gateway_outcome(&[&a], &[]);
        assert_eq!(finding(&outcome).id, "NET-121");
        assert_eq!(finding(&outcome).what, "net0 has an address but no default route");
        one_sentence(finding(&outcome));
    }

    #[test]
    fn a_gateway_that_does_not_answer_fails_the_gateway_layer() {
        let a = addressed(card("net0", true));
        let silent = gateway_outcome(&[&a], &[(route("10.0.2.99", 1500), Echo::Silent)]);
        assert!(matches!(silent, Outcome::Failed(_)));
        assert_eq!(finding(&silent).id, "NET-122");
        assert_eq!(finding(&silent).what, "the gateway 10.0.2.99 does not answer");
        one_sentence(finding(&silent));
        let refused = gateway_outcome(
            &[&a],
            &[(route("10.0.2.99", 1500), Echo::Refused("Destination Host Unreachable".into()))],
        );
        assert_eq!(
            finding(&refused).what,
            "the gateway 10.0.2.99 (Destination Host Unreachable) does not answer"
        );
    }

    #[test]
    fn a_gateway_that_answers_passes_the_gateway_layer() {
        let a = addressed(card("net0", true));
        let outcome = gateway_outcome(
            &[&a],
            &[
                (route("10.0.2.2", 1500), Echo::Answered(Duration::from_micros(1200))),
                (route("fe80::1", 1500), Echo::Silent),
            ],
        );
        assert_eq!(outcome, Outcome::Passed("10.0.2.2 answers in 1.2 ms".into()));
    }

    #[test]
    fn a_link_local_gateway_is_reached_through_its_card() {
        let scoped = route("fe80::1", 1500).destination();
        match scoped {
            SocketAddr::V6(address) => assert_eq!(address.scope_id(), 2),
            other => panic!("{other}"),
        }
        assert_eq!(route("10.0.2.2", 1500).destination(), "10.0.2.2:0".parse().unwrap());
    }

    #[test]
    fn a_full_packet_is_the_cards_mtu_less_the_headers() {
        assert_eq!(route("10.0.2.2", 1500).full_size(), 1472);
        assert_eq!(route("fe80::1", 1500).full_size(), 1452);
        assert_eq!(route("10.0.2.2", 20).full_size(), 0);
    }

    #[test]
    fn no_dns_server_fails_the_dns_layer() {
        let outcome = dns_outcome(0, &[]);
        assert_eq!(finding(&outcome).id, "NET-131");
        one_sentence(finding(&outcome));
    }

    #[test]
    fn servers_off_the_network_are_not_asked_without_internet() {
        assert!(matches!(dns_outcome(2, &[]), Outcome::NotChecked(_)));
    }

    #[test]
    fn dns_servers_that_all_stay_silent_fail_the_dns_layer() {
        let outcome = dns_outcome(1, &[("10.0.2.99".into(), false)]);
        assert_eq!(finding(&outcome).id, "NET-132");
        assert_eq!(finding(&outcome).what, "no DNS server answers: 10.0.2.99 does not");
        one_sentence(finding(&outcome));
        let two = dns_outcome(2, &[("10.0.2.98".into(), false), ("10.0.2.99".into(), false)]);
        assert_eq!(finding(&two).what, "no DNS server answers: 10.0.2.98 and 10.0.2.99 do not");
    }

    #[test]
    fn one_dns_server_that_answers_passes_the_dns_layer() {
        let outcome = dns_outcome(2, &[("10.0.2.3".into(), true), ("10.0.2.99".into(), false)]);
        assert_eq!(outcome, Outcome::Passed("10.0.2.3 answers; 10.0.2.99 does not".into()));
    }

    #[test]
    fn servers_are_on_the_network_of_a_card_with_an_address_there() {
        let a = addressed(card("net0", true));
        let server = |text: &str| resolved::parse_server(text).unwrap();
        assert!(on_network(&server("10.0.2.3"), &[&a]));
        assert!(!on_network(&server("9.9.9.9"), &[&a]));
        assert!(!on_network(&server("10.0.2.3"), &[]));
    }

    #[test]
    fn a_clock_a_time_server_set_lately_passes_the_time_layer() {
        let status = "Synchronized: yes, at 2026-11-13 10:00:00 UTC, from time.example\n";
        let now = 1_800_000_000;
        assert!(matches!(time_outcome(Some(status), Some(now - 3600), now), Outcome::Passed(_)));
        assert!(matches!(time_outcome(Some(status), None, now), Outcome::Passed(_)));
    }

    #[test]
    fn a_clock_no_time_server_set_warns() {
        let now = 1_800_000_000;
        for status in [None, Some("Synchronized: no; at 2026-11-13 no server gave the time:\n")] {
            let outcome = time_outcome(status, None, now);
            assert!(matches!(outcome, Outcome::Warning(_)));
            assert_eq!(finding(&outcome).id, "NET-141");
            assert_eq!(
                finding(&outcome).what,
                "timed has not set the clock from a time server since the start"
            );
            one_sentence(finding(&outcome));
        }
    }

    #[test]
    fn a_clock_last_set_over_a_day_ago_warns() {
        let status = "Synchronized: yes, earlier; at 2026-11-13 no server gave the time:\n";
        let now = 1_800_000_000;
        let outcome = time_outcome(Some(status), Some(now - 3 * 86_400 - 5), now);
        assert_eq!(finding(&outcome).id, "NET-141");
        assert_eq!(finding(&outcome).what, "the clock was last set from a time server 3 days ago");
        assert!(matches!(time_outcome(Some(status), Some(now - 86_400), now), Outcome::Passed(_)));
    }

    #[test]
    fn full_packets_that_do_not_arrive_warn_about_the_mtu() {
        let r = route("10.0.2.2", 1500);
        let outcome = mtu_outcome(&[(&r, Echo::Silent)]);
        assert!(matches!(outcome, Outcome::Warning(_)));
        assert_eq!(finding(&outcome).id, "NET-161");
        assert_eq!(
            finding(&outcome).what,
            "packets of 1500 bytes do not reach 10.0.2.2, though small ones do"
        );
        one_sentence(finding(&outcome));
    }

    #[test]
    fn full_packets_that_arrive_pass_the_mtu_layer() {
        let r = route("10.0.2.2", 1500);
        let outcome = mtu_outcome(&[(&r, Echo::Answered(Duration::from_millis(1)))]);
        assert_eq!(outcome, Outcome::Passed("packets of 1500 bytes reach 10.0.2.2".into()));
        assert!(matches!(mtu_outcome(&[]), Outcome::NotChecked(_)));
    }

    #[test]
    fn the_update_server_is_its_host_and_port() {
        let target = |server: &str| target_of(server).unwrap();
        assert_eq!(
            target("https://updates.dukovo.org/system/"),
            Target { host: "updates.dukovo.org".into(), port: 443, tls: true }
        );
        assert_eq!(
            target("https://127.0.0.1:8443/system/"),
            Target { host: "127.0.0.1".into(), port: 8443, tls: true }
        );
        assert_eq!(
            target("http://10.0.2.2:8080/"),
            Target { host: "10.0.2.2".into(), port: 8080, tls: false }
        );
        assert_eq!(target_of("ftp://example.org/"), None);
    }

    fn update_server() -> Target {
        Target { host: "updates.example".into(), port: 443, tls: true }
    }

    #[test]
    fn a_server_no_address_reaches_fails_the_internet_layer() {
        let address: IpAddr = "203.0.113.5".parse().unwrap();
        let reached = [(address, Err("connection timed out".to_string()))];
        let outcome = connect_outcome(&update_server(), &reached).unwrap();
        assert_eq!(finding(&outcome).id, "NET-172");
        assert_eq!(
            finding(&outcome).what,
            "updates.example does not answer on port 443 (203.0.113.5: connection timed out)"
        );
        one_sentence(finding(&outcome));
        assert!(connect_outcome(&update_server(), &[(address, Ok(()))]).is_none());
    }

    #[test]
    fn a_name_that_does_not_resolve_fails_the_internet_layer() {
        let target = Target { host: "updates.example".into(), port: 443, tls: true };
        let outcome = unresolved(&target);
        assert!(matches!(outcome, Outcome::Failed(_)));
        assert_eq!(finding(&outcome).id, "NET-171");
        assert_eq!(finding(&outcome).what, "the name updates.example does not resolve");
        one_sentence(finding(&outcome));
        let literal = Target { host: "127.0.0.1".into(), port: 443, tls: true };
        assert_eq!(resolve(&literal).unwrap(), vec!["127.0.0.1".parse::<IpAddr>().unwrap()]);
    }

    fn refused_certificate(problem: rustls::CertificateError) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidData, rustls::Error::InvalidCertificate(problem))
    }

    #[test]
    fn a_certificate_of_an_authority_nothing_trusts_is_net_173() {
        let error = refused_certificate(rustls::CertificateError::UnknownIssuer);
        let outcome = certificate_outcome(&update_server(), &error);
        assert_eq!(finding(&outcome).id, "NET-173");
        assert_eq!(
            finding(&outcome).what,
            "something answers for updates.example, but its certificate is signed by an \
             authority this system does not trust"
        );
        one_sentence(finding(&outcome));
    }

    #[test]
    fn a_certificate_that_is_its_own_authority_is_net_173_without_the_library_text() {
        // webpki refuses a certificate made by `openssl req -x509` as a
        // CA used as a server's, which is no unknown issuer.
        let error = refused_certificate(rustls::CertificateError::Other(rustls::OtherError(
            std::sync::Arc::new(io::Error::other("CaUsedAsEndEntity")),
        )));
        let outcome = certificate_outcome(&update_server(), &error);
        assert_eq!(finding(&outcome).id, "NET-173");
        assert!(finding(&outcome).what.ends_with("its certificate is not one this system trusts"));
        assert!(!finding(&outcome).what.contains("CaUsedAsEndEntity"));
    }

    #[test]
    fn a_certificate_for_another_name_is_net_173() {
        let error = refused_certificate(rustls::CertificateError::NotValidForName);
        let outcome = certificate_outcome(&update_server(), &error);
        assert_eq!(finding(&outcome).id, "NET-173");
        assert!(finding(&outcome).what.ends_with("its certificate is for another name"));
    }

    #[test]
    fn a_certificate_not_valid_by_the_clock_is_net_174() {
        for problem in [rustls::CertificateError::Expired, rustls::CertificateError::NotValidYet] {
            let outcome = certificate_outcome(&update_server(), &refused_certificate(problem));
            assert_eq!(finding(&outcome).id, "NET-174");
            assert!(finding(&outcome).what.starts_with(
                "updates.example's certificate is not valid by this machine's clock, which says "
            ));
            one_sentence(finding(&outcome));
        }
    }

    #[test]
    fn a_tls_failure_that_is_no_certificate_is_net_172() {
        let error = io::Error::new(
            io::ErrorKind::InvalidData,
            rustls::Error::AlertReceived(rustls::AlertDescription::HandshakeFailure),
        );
        let outcome = certificate_outcome(&update_server(), &error);
        assert_eq!(finding(&outcome).id, "NET-172");
        assert!(
            finding(&outcome)
                .what
                .starts_with("updates.example answers on port 443, but the TLS connection fails")
        );
        one_sentence(finding(&outcome));
    }

    #[test]
    fn ipv6_that_fails_beside_ipv4_that_works_warns() {
        let v4: IpAddr = "203.0.113.5".parse().unwrap();
        let v6: IpAddr = "2001:db8::5".parse().unwrap();
        let down = Err("connection timed out".to_string());
        let outcome =
            family_outcome(&update_server(), &[(v4, Ok(())), (v6, down.clone())], true).unwrap();
        assert!(matches!(outcome, Outcome::Warning(_)));
        assert_eq!(finding(&outcome).id, "NET-175");
        assert_eq!(finding(&outcome).what, "updates.example answers over IPv4 but not over IPv6");
        one_sentence(finding(&outcome));
        // Both work, no IPv6 on the machine, or no IPv6 asked.
        assert!(family_outcome(&update_server(), &[(v4, Ok(())), (v6, Ok(()))], true).is_none());
        assert!(family_outcome(&update_server(), &[(v4, Ok(())), (v6, down)], false).is_none());
        assert!(family_outcome(&update_server(), &[(v4, Ok(()))], true).is_none());
    }

    #[test]
    fn the_exit_status_is_the_worst_result() {
        let passed = (Layer::Link, Outcome::Passed("ok".into()));
        let warned = (Layer::Time, warning("NET-141", "late".into(), "Wait."));
        let broken = (Layer::Gateway, failed("NET-122", "dead".into(), "Fix."));
        let unchecked = (Layer::Internet, Outcome::NotChecked("not asked".into()));
        assert_eq!(status_of(&[passed.clone(), unchecked.clone()]), PASSED);
        assert_eq!(status_of(&[passed.clone(), warned.clone(), unchecked.clone()]), WARNED);
        assert_eq!(status_of(&[passed, warned, broken, unchecked]), FAILED);
    }

    #[test]
    fn the_text_names_each_layer_its_result_and_the_help_page() {
        let found = vec![
            (Layer::Link, Outcome::Passed("net0 has a link".into())),
            (
                Layer::Address,
                failed("NET-111", "net0 has no address: DHCP got no answer".into(), "Look."),
            ),
            (Layer::Gateway, Outcome::NotChecked("an earlier layer failed".into())),
        ];
        let text = plain(&found, FAILED);
        assert!(text.contains("link      passed       net0 has a link\n"), "{text}");
        assert!(
            text.contains(
                "address   FAILED       NET-111: net0 has no address: DHCP got no answer\n"
            ),
            "{text}"
        );
        assert!(text.contains("Help: https://dukovo.org/support/"), "{text}");
        assert!(text.contains("/errors/NET-111.html\n"), "{text}");
        assert!(text.contains("gateway   not checked  an earlier layer failed\n"), "{text}");
        assert!(
            text.ends_with(
                "netcheck: failed at the address layer; the layers after it are not checked.\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn the_json_has_a_line_a_layer() {
        let found = vec![
            (Layer::Link, Outcome::Passed("net0 has a link".into())),
            (Layer::Time, warning("NET-141", "late".into(), "Wait.")),
        ];
        let text = json(&found, WARNED, 0);
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["status"], "warning");
        assert_eq!(value["time"], "1970-01-01T00:00:00Z");
        assert_eq!(value["checks"][0]["layer"], "link");
        assert_eq!(value["checks"][0]["result"], "passed");
        assert!(value["checks"][0]["id"].is_null());
        assert_eq!(value["checks"][1]["id"], "NET-141");
        assert_eq!(value["checks"][1]["advice"], "Wait.");
    }

    #[test]
    fn cards_are_listed_without_the_loopback_tunnels_and_bridge_ports() {
        let mut lo = card("lo", true);
        lo.loopback = true;
        let mut wg = card("wg0", true);
        wg.kind = Kind::Tunnel;
        let mut port = card("net1", true);
        port.master = Some("br0".into());
        let machine = Machine {
            interfaces: vec![lo, wg, port, card("br0", true), card("net0", false)],
            settings: Vec::new(),
            servers: Vec::new(),
            over_tls: OverTls::default(),
            card: None,
        };
        let cards: Vec<&str> = machine.cards().iter().map(|card| card.name.as_str()).collect();
        assert_eq!(cards, ["br0", "net0"]);
        assert_eq!(machine.linked().len(), 1);
        let only = Machine { card: Some("net0".into()), ..machine };
        assert_eq!(only.cards().len(), 1);
    }

    #[test]
    fn layers_after_a_failure_are_not_checked() {
        // No card at all: the link layer fails, and nothing else runs.
        let machine = Machine {
            interfaces: Vec::new(),
            settings: Vec::new(),
            servers: Vec::new(),
            over_tls: OverTls::default(),
            card: None,
        };
        let found = walk(&machine, &Options::default());
        assert_eq!(found.len(), LAYERS.len());
        assert!(matches!(found[0].1, Outcome::Failed(_)));
        assert!(found[1..].iter().all(|(_, outcome)| matches!(outcome, Outcome::NotChecked(_))));
        assert_eq!(status_of(&found), FAILED);
    }

    #[test]
    fn the_internet_layer_sends_nothing_unless_asked() {
        let machine = Machine {
            interfaces: Vec::new(),
            settings: Vec::new(),
            servers: Vec::new(),
            over_tls: OverTls::default(),
            card: None,
        };
        assert!(matches!(internet(&machine, false), Outcome::NotChecked(_)));
    }
}
