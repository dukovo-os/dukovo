// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! wg(8): WireGuard's keys and interfaces, as wireguard-tools' wg shows
//! and sets them (ADR 0340), through wgd(8)'s sockets.
//!
//! `wg genkey`, `genpsk` and `pubkey` make keys; `wg show` and `showconf`
//! say what an interface has, in wg's words and formats, `dump` and the
//! single fields too; `wg set`, `setconf`, `addconf` and `syncconf`
//! change it until wgd next reads its file. The interfaces themselves
//! come from the files in /system/state/wireguard, which wgd reads.

use std::fmt::Write as _;
use std::fs;
use std::io::{self, IsTerminal, Read};
use std::os::unix::fs::MetadataExt;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::args::Args;
use crate::util::{self, Fail, fail, usage};
use crate::wireguard::{self, Config, Device, Key, Peer, Prefix};

const HELP: &str = "Usage: wg <cmd> [<args>]

WireGuard's keys, and the interfaces wgd(8) carries:
  show: what each interface, or the one named, has and has done
  showconf: an interface's settings, as a file setconf takes
  set: change an interface's settings and its peers
  setconf: give an interface a file's settings in place of its own
  addconf: add a file's settings and peers to an interface's
  syncconf: make an interface's settings a file's, keeping its sessions
  genkey: a new private key, on standard output
  genpsk: a new preshared key, on standard output
  pubkey: the public key of the private key on standard input
`wg <cmd> --help' gives the arguments of each.
";

/// wg(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    args.stop_at_operand();
    let words: Vec<String> =
        args.rest().into_iter().map(|word| word.to_string_lossy().into_owned()).collect();
    let (command, rest) = match words.split_first() {
        Some((command, rest)) => (command.as_str(), rest),
        None => ("show", &[][..]),
    };
    if rest.iter().any(|word| word == "--help" || word == "-h") {
        print!("{}", subcommand_help(command));
        return Ok(0);
    }
    match command {
        "--help" | "-h" | "help" => {
            print!("{HELP}");
            Ok(0)
        }
        "--version" | "-v" => {
            println!("wg, Dukovo systools {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        "genkey" | "genpsk" => {
            no_more(command, rest)?;
            let key = if command == "genkey" { Key::generate() } else { Key::random() };
            let key = key.map_err(|e| fail(format!("Unable to generate key: {e}")))?;
            warn_if_shared();
            println!("{}", key.base64());
            Ok(0)
        }
        "pubkey" => {
            no_more(command, rest)?;
            let mut text = String::new();
            io::stdin().read_to_string(&mut text)?;
            let key = Key::from_base64(&text)
                .ok_or_else(|| fail("Key is not the correct length or format".into()))?;
            println!("{}", key.public().base64());
            Ok(0)
        }
        "show" => show(rest),
        "showconf" => {
            let [name] = rest else { return Err(usage(subcommand_help(command))) };
            print!("{}", showconf(&get(name)?));
            Ok(0)
        }
        "set" => {
            let Some((name, words)) = rest.split_first() else {
                return Err(usage(subcommand_help(command)));
            };
            let request = set_request(words)?;
            send(name, &request)
        }
        "setconf" | "addconf" | "syncconf" => {
            let [name, file] = rest else { return Err(usage(subcommand_help(command))) };
            let config = read_config(file)?;
            let request = match command {
                "syncconf" => sync_request(&config, &get(name)?)?,
                _ => wireguard::set_request(&config, command == "setconf").map_err(fail)?,
            };
            send(name, &request)
        }
        _ => Err(usage(format!("Invalid subcommand: `{command}'\n{HELP}"))),
    }
}

/// What `wg COMMAND --help` says.
fn subcommand_help(command: &str) -> String {
    match command {
        "show" => "Usage: wg show { <interface> | all | interfaces } [public-key | private-key | \
                   listen-port | fwmark | peers | preshared-keys | endpoints | allowed-ips | \
                   latest-handshakes | transfer | persistent-keepalive | dump]\n"
            .into(),
        "showconf" => "Usage: wg showconf <interface>\n".into(),
        "set" => "Usage: wg set <interface> [listen-port <port>] [fwmark <mark>] [private-key \
                  <file path>] [peer <base64 public key> [remove] [preshared-key <file path>] \
                  [endpoint <ip>:<port>] [persistent-keepalive <interval seconds>] [allowed-ips \
                  <ip1>/<cidr1>[,<ip2>/<cidr2>]...] ]...\n"
            .into(),
        "setconf" | "addconf" | "syncconf" => {
            format!("Usage: wg {command} <interface> <configuration filename>\n")
        }
        "genkey" | "genpsk" | "pubkey" => format!("Usage: wg {command}\n"),
        _ => HELP.into(),
    }
}

fn no_more(command: &str, rest: &[String]) -> Result<(), Fail> {
    if rest.is_empty() { Ok(()) } else { Err(usage(subcommand_help(command))) }
}

/// Says so if a key is going into a file others may read, as wg does.
fn warn_if_shared() {
    if let Ok(meta) = fs::metadata("/processes/self/fd/1")
        && meta.is_file()
        && meta.mode() & 0o007 != 0
    {
        eprintln!(
            "Warning: others may read the file this key is going to.\nA umask of 077 \
             keeps them out."
        );
    }
}

/// Interface `name` as wgd says it is.
fn get(name: &str) -> Result<Device, Fail> {
    let answer = wireguard::ask(name, "get=1\n\n").map_err(|e| access(name, &e))?;
    wireguard::parse_device(&answer)
        .map_err(|why| fail(format!("Unable to access interface {name}: {why}")))
}

/// Why interface `name` could not be reached.
fn access(name: &str, error: &io::Error) -> Fail {
    let why = util::strip_os_error(error);
    if why.starts_with("Unable to access") {
        fail(why)
    } else {
        fail(format!("Unable to access interface {name}: {why}"))
    }
}

/// Sends interface `name` the `set=1` request `request`.
fn send(name: &str, request: &str) -> Result<i32, Fail> {
    let answer = wireguard::ask(name, request).map_err(|e| access(name, &e))?;
    match answer.lines().find_map(|line| line.strip_prefix("errno=")) {
        Some("0") => Ok(0),
        Some(errno) => Err(fail(format!(
            "Unable to modify interface {name}: {}",
            wireguard::errno_text(errno)
        ))),
        None => Err(fail(format!("Unable to modify interface {name}: wgd did not say"))),
    }
}

/// A configuration file, for setconf, addconf and syncconf.
fn read_config(path: &str) -> Result<Config, Fail> {
    let text = fs::read_to_string(path).map_err(|e| util::failed(&path, e))?;
    let mut warn = |why: String| eprintln!("wg: {path}: {why}");
    let config =
        wireguard::parse_config(&text, &mut warn).map_err(|why| fail(format!("{path}: {why}")))?;
    if !config.addresses.is_empty() {
        eprintln!(
            "wg: {path}: Address is wg-quick's, which wg passes over; wgd gives an interface \
             those of its file in {}",
            wireguard::STATE
        );
    }
    Ok(config)
}

/// The `set=1` request that gives the interface `config` as `wg
/// syncconf` does: its peers the file does not name are removed, the
/// others changed only where they differ.
fn sync_request(config: &Config, device: &Device) -> Result<String, Fail> {
    let mut text = String::from("set=1\n");
    let key = config.private_key.unwrap_or(Key([0; 32]));
    if device.private_key != config.private_key {
        let _ = writeln!(text, "private_key={}", key.hex());
    }
    if let Some(port) = config.listen_port.filter(|port| *port != device.listen_port) {
        let _ = writeln!(text, "listen_port={port}");
    }
    if config.fwmark.unwrap_or(0) != device.fwmark {
        let _ = writeln!(text, "fwmark={}", config.fwmark.unwrap_or(0));
    }
    for peer in &device.peers {
        if !config.peers.iter().any(|p| p.public_key == peer.public_key) {
            let _ = writeln!(text, "public_key={}\nremove=true", peer.public_key.hex());
        }
    }
    for peer in &config.peers {
        wireguard::peer_lines(&mut text, peer).map_err(fail)?;
    }
    text.push('\n');
    Ok(text)
}

/// A key from the file at `path`, for `wg set`: `/dev/stdin` reads it
/// from what wg is given.
fn key_file(path: &str, what: &str) -> Result<Key, Fail> {
    let mut text = String::new();
    if path == "/dev/stdin" {
        io::stdin().read_to_string(&mut text)?;
    } else {
        text = fs::read_to_string(path).map_err(|e| util::failed(&path, e))?;
    }
    if text.trim().is_empty() {
        // An empty file takes the key away, as wg's does.
        return Ok(Key([0; 32]));
    }
    Key::from_base64(&text)
        .ok_or_else(|| fail(format!("{what} is not the correct length or format: `{path}'")))
}

/// The `set=1` request `wg set`'s words make.
fn set_request(words: &[String]) -> Result<String, Fail> {
    let mut text = String::from("set=1\n");
    let mut words = words.iter().map(String::as_str);
    let mut in_peer = false;
    let wanted = |word: &str| usage(format!("{word} needs a value\n{}", subcommand_help("set")));
    while let Some(word) = words.next() {
        let mut value = || words.next().ok_or_else(|| wanted(word));
        match word {
            "listen-port" if !in_peer => {
                let port: u16 =
                    value()?.parse().map_err(|_| fail("Listen port is not a valid port".into()))?;
                let _ = writeln!(text, "listen_port={port}");
            }
            "fwmark" if !in_peer => {
                let mark = value()?;
                let mark = if mark == "off" {
                    0
                } else {
                    wireguard::parse_number(mark).ok_or_else(|| {
                        fail(format!("Fwmark is neither a number nor off: `{mark}'"))
                    })?
                };
                let _ = writeln!(text, "fwmark={mark}");
            }
            "private-key" if !in_peer => {
                let key = key_file(value()?, "Key")?;
                let _ = writeln!(text, "private_key={}", key.hex());
            }
            "peer" => {
                let key = value()?;
                let key = Key::from_base64(key).ok_or_else(|| {
                    fail(format!("Key is not the correct length or format: `{key}'"))
                })?;
                let _ = writeln!(text, "public_key={}", key.hex());
                in_peer = true;
            }
            "remove" if in_peer => text.push_str("remove=true\n"),
            "preshared-key" if in_peer => {
                let key = key_file(value()?, "Key")?;
                let _ = writeln!(text, "preshared_key={}", key.hex());
            }
            "endpoint" if in_peer => {
                let endpoint = wireguard::resolve_endpoint(value()?)
                    .map_err(|why| fail(format!("Unable to parse endpoint: {why}")))?;
                let _ = writeln!(text, "endpoint={}", wireguard::endpoint_text(endpoint));
            }
            "persistent-keepalive" if in_peer => {
                let seconds = value()?;
                let seconds: u16 = if seconds == "off" {
                    0
                } else {
                    seconds.parse().map_err(|_| {
                        fail(format!(
                            "Persistent keepalive interval is neither 0/off nor 1-65535: `{seconds}'"
                        ))
                    })?
                };
                let _ = writeln!(text, "persistent_keepalive_interval={seconds}");
            }
            "allowed-ips" if in_peer => {
                text.push_str("replace_allowed_ips=true\n");
                for prefix in value()?.split(',').filter(|p| !p.trim().is_empty()) {
                    let parsed = Prefix::parse(prefix)
                        .ok_or_else(|| fail(format!("Unable to parse IP address: `{prefix}'")))?;
                    let _ = writeln!(text, "allowed_ip={}", parsed.network());
                }
            }
            _ => {
                return Err(usage(format!("Invalid argument: {word}\n{}", subcommand_help("set"))));
            }
        }
    }
    text.push('\n');
    Ok(text)
}

/// The fields `wg show` prints alone.
const FIELDS: &[&str] = &[
    "public-key",
    "private-key",
    "listen-port",
    "fwmark",
    "peers",
    "preshared-keys",
    "endpoints",
    "allowed-ips",
    "latest-handshakes",
    "persistent-keepalive",
    "transfer",
    "dump",
];

/// `wg show`.
fn show(words: &[String]) -> Result<i32, Fail> {
    let (which, field) = match words {
        [] => ("all", None),
        [which] => (which.as_str(), None),
        [which, field] => (which.as_str(), Some(field.as_str())),
        _ => return Err(usage(subcommand_help("show"))),
    };
    if let Some(field) = field
        && !FIELDS.contains(&field)
    {
        return Err(usage(format!("Invalid parameter: `{field}'\n{}", subcommand_help("show"))));
    }
    if which == "interfaces" {
        if field.is_some() {
            return Err(usage(subcommand_help("show")));
        }
        let names = wireguard::interfaces();
        if !names.is_empty() {
            println!("{}", names.join(" "));
        }
        return Ok(0);
    }
    let names = if which == "all" { wireguard::interfaces() } else { vec![which.to_string()] };
    let colour = colour();
    let mut text = String::new();
    let mut code = 0;
    for (at, name) in names.iter().enumerate() {
        let device = match get(name) {
            Ok(device) => device,
            Err(failure) if which == "all" => {
                eprintln!("wg: {}", failure.message);
                code = 1;
                continue;
            }
            Err(failure) => return Err(failure),
        };
        let prefix = if which == "all" { format!("{name}\t") } else { String::new() };
        match field {
            None => {
                if at > 0 {
                    text.push('\n');
                }
                pretty(&mut text, name, &device, colour);
            }
            Some(field) => fields(&mut text, &prefix, &device, field),
        }
    }
    print!("{text}");
    Ok(code)
}

/// Whether `wg show` colours its words: on a terminal, unless
/// WG_COLOR_MODE says always or never.
fn colour() -> bool {
    match std::env::var("WG_COLOR_MODE").as_deref() {
        Ok("always") => true,
        Ok("never") => false,
        _ => io::stdout().is_terminal(),
    }
}

/// The seconds since 1970 now.
fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs())
}

/// `wg show`'s words for interface `name`.
fn pretty(text: &mut String, name: &str, device: &Device, colour: bool) {
    let paint = |code: &str, word: &str| {
        if colour { format!("\x1b[{code}m{word}\x1b[0m") } else { word.to_string() }
    };
    let label = |word: &str| paint("1", word);
    let _ = writeln!(text, "{}: {}", paint("32;1", "interface"), paint("32", name));
    if let Some(key) = device.private_key {
        let _ = writeln!(text, "  {}: {}", label("public key"), key.public().base64());
        let _ = writeln!(text, "  {}: (hidden)", label("private key"));
    }
    if device.listen_port != 0 {
        let _ = writeln!(text, "  {}: {}", label("listening port"), device.listen_port);
    }
    if device.fwmark != 0 {
        let _ = writeln!(text, "  {}: 0x{:x}", label("fwmark"), device.fwmark);
    }
    let mut peers: Vec<&Peer> = device.peers.iter().collect();
    // The latest handshakes first, as wg sorts them.
    peers.sort_by_key(|peer| std::cmp::Reverse(peer.last_handshake));
    let now = now();
    for peer in peers {
        let _ = writeln!(
            text,
            "\n{}: {}",
            paint("33;1", "peer"),
            paint("33", &peer.public_key.base64())
        );
        if peer.preshared_key.is_some() {
            let _ = writeln!(text, "  {}: (hidden)", label("preshared key"));
        }
        if let Some(endpoint) = peer.endpoint {
            let _ =
                writeln!(text, "  {}: {}", label("endpoint"), wireguard::endpoint_text(endpoint));
        }
        let allowed = if peer.allowed_ips.is_empty() {
            "(none)".to_string()
        } else {
            peer.allowed_ips.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
        };
        let _ = writeln!(text, "  {}: {allowed}", label("allowed ips"));
        if peer.last_handshake.0 != 0 {
            let _ = writeln!(
                text,
                "  {}: {}",
                label("latest handshake"),
                ago(now, peer.last_handshake.0)
            );
        }
        if peer.rx_bytes != 0 || peer.tx_bytes != 0 {
            let _ = writeln!(
                text,
                "  {}: {} received, {} sent",
                label("transfer"),
                bytes(peer.rx_bytes),
                bytes(peer.tx_bytes)
            );
        }
        if peer.keepalive != 0 {
            let _ = writeln!(
                text,
                "  {}: every {}",
                label("persistent keepalive"),
                seconds(u64::from(peer.keepalive))
            );
        }
    }
}

/// `wg show`'s lines of one field, each after `prefix`.
fn fields(text: &mut String, prefix: &str, device: &Device, field: &str) {
    let key_or_none = |key: Option<Key>| key.map_or("(none)".to_string(), |key| key.base64());
    let public = device.private_key.map(|key| key.public());
    match field {
        "public-key" => {
            let _ = writeln!(text, "{prefix}{}", key_or_none(public));
        }
        "private-key" => {
            let _ = writeln!(text, "{prefix}{}", key_or_none(device.private_key));
        }
        "listen-port" => {
            let _ = writeln!(text, "{prefix}{}", device.listen_port);
        }
        "fwmark" => {
            let mark =
                if device.fwmark == 0 { "off".into() } else { format!("0x{:x}", device.fwmark) };
            let _ = writeln!(text, "{prefix}{mark}");
        }
        "dump" => {
            let mark =
                if device.fwmark == 0 { "off".into() } else { format!("0x{:x}", device.fwmark) };
            let _ = writeln!(
                text,
                "{prefix}{}\t{}\t{}\t{mark}",
                key_or_none(device.private_key),
                key_or_none(public),
                device.listen_port
            );
            for peer in &device.peers {
                let _ = writeln!(
                    text,
                    "{prefix}{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    peer.public_key.base64(),
                    key_or_none(peer.preshared_key),
                    endpoint(peer),
                    allowed(peer, ","),
                    peer.last_handshake.0,
                    peer.rx_bytes,
                    peer.tx_bytes,
                    keepalive(peer)
                );
            }
        }
        _ => {
            for peer in &device.peers {
                let value = match field {
                    "peers" => String::new(),
                    "preshared-keys" => key_or_none(peer.preshared_key),
                    "endpoints" => endpoint(peer),
                    "allowed-ips" => allowed(peer, " "),
                    "latest-handshakes" => peer.last_handshake.0.to_string(),
                    "persistent-keepalive" => keepalive(peer),
                    _ => format!("{}\t{}", peer.rx_bytes, peer.tx_bytes),
                };
                let key = peer.public_key.base64();
                if value.is_empty() {
                    let _ = writeln!(text, "{prefix}{key}");
                } else {
                    let _ = writeln!(text, "{prefix}{key}\t{value}");
                }
            }
        }
    }
}

fn endpoint(peer: &Peer) -> String {
    peer.endpoint.map_or("(none)".into(), wireguard::endpoint_text)
}

fn allowed(peer: &Peer, between: &str) -> String {
    if peer.allowed_ips.is_empty() {
        return "(none)".into();
    }
    peer.allowed_ips.iter().map(ToString::to_string).collect::<Vec<_>>().join(between)
}

fn keepalive(peer: &Peer) -> String {
    if peer.keepalive == 0 { "off".into() } else { peer.keepalive.to_string() }
}

/// `wg showconf`'s text: the interface's settings as a file gives them.
fn showconf(device: &Device) -> String {
    let mut text = String::from("[Interface]\n");
    if device.listen_port != 0 {
        let _ = writeln!(text, "ListenPort = {}", device.listen_port);
    }
    if device.fwmark != 0 {
        let _ = writeln!(text, "FwMark = 0x{:x}", device.fwmark);
    }
    if let Some(key) = device.private_key {
        let _ = writeln!(text, "PrivateKey = {}", key.base64());
    }
    for peer in &device.peers {
        let _ = writeln!(text, "\n[Peer]\nPublicKey = {}", peer.public_key.base64());
        if let Some(key) = peer.preshared_key {
            let _ = writeln!(text, "PresharedKey = {}", key.base64());
        }
        if !peer.allowed_ips.is_empty() {
            let _ = writeln!(text, "AllowedIPs = {}", allowed(peer, ", "));
        }
        if let Some(endpoint) = peer.endpoint {
            let _ = writeln!(text, "Endpoint = {}", wireguard::endpoint_text(endpoint));
        }
        if peer.keepalive != 0 {
            let _ = writeln!(text, "PersistentKeepalive = {}", peer.keepalive);
        }
    }
    text
}

/// A count of bytes as wg says it: `512 B`, `1.50 KiB`.
fn bytes(count: u64) -> String {
    #[allow(clippy::cast_precision_loss)] // two decimals are shown.
    let scaled = |power: i32| count as f64 / 1024f64.powi(power);
    match count {
        0..1024 => format!("{count} B"),
        1024..1_048_576 => format!("{:.2} KiB", scaled(1)),
        1_048_576..1_073_741_824 => format!("{:.2} MiB", scaled(2)),
        1_073_741_824..1_099_511_627_776 => format!("{:.2} GiB", scaled(3)),
        _ => format!("{:.2} TiB", scaled(4)),
    }
}

/// A span of seconds as wg says it: `1 minute, 5 seconds`.
fn seconds(mut left: u64) -> String {
    let units = [
        ("year", 365 * 24 * 3600),
        ("day", 24 * 3600),
        ("hour", 3600),
        ("minute", 60),
        ("second", 1),
    ];
    let mut parts = Vec::new();
    for (unit, size) in units {
        let count = left / size;
        left %= size;
        if count > 0 {
            parts.push(format!("{count} {unit}{}", if count == 1 { "" } else { "s" }));
        }
    }
    parts.join(", ")
}

/// How long ago `then` was, at `now`, as wg says it.
fn ago(now: u64, then: u64) -> String {
    match now.saturating_sub(then) {
        0 => "Now".into(),
        span => format!("{} ago", seconds(span)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_spans_are_said_as_wg_says_them() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1536), "1.50 KiB");
        assert_eq!(bytes(3 * 1_048_576), "3.00 MiB");
        assert_eq!(seconds(25), "25 seconds");
        assert_eq!(seconds(61), "1 minute, 1 second");
        assert_eq!(ago(100, 100), "Now");
        assert_eq!(ago(3700, 100), "1 hour ago");
    }

    #[test]
    fn set_is_said_in_the_protocol() {
        let key = Key::generate().unwrap().public();
        let words: Vec<String> = format!(
            "listen-port 51820 peer {} endpoint 10.0.0.2:51820 persistent-keepalive 25 \
             allowed-ips 10.9.0.2/32,fd09::2",
            key.base64()
        )
        .split(' ')
        .map(String::from)
        .collect();
        let request = set_request(&words).unwrap();
        assert_eq!(
            request,
            format!(
                "set=1\nlisten_port=51820\npublic_key={}\nendpoint=10.0.0.2:51820\n\
                 persistent_keepalive_interval=25\nreplace_allowed_ips=true\n\
                 allowed_ip=10.9.0.2/32\nallowed_ip=fd09::2/128\n\n",
                key.hex()
            )
        );
        let words = vec!["peer".to_string(), key.base64(), "colour".into(), "red".into()];
        assert!(set_request(&words).is_err());
        assert!(set_request(&["remove".to_string()]).is_err(), "remove needs a peer");
    }

    #[test]
    fn syncconf_removes_the_peers_a_file_does_not_name() {
        let (kept, gone) = (Key::generate().unwrap().public(), Key::generate().unwrap().public());
        let device = Device {
            private_key: None,
            listen_port: 51820,
            fwmark: 0,
            peers: [kept, gone]
                .into_iter()
                .map(|public_key| Peer {
                    public_key,
                    preshared_key: None,
                    endpoint: None,
                    allowed_ips: Vec::new(),
                    last_handshake: (0, 0),
                    rx_bytes: 0,
                    tx_bytes: 0,
                    keepalive: 0,
                })
                .collect(),
        };
        let text =
            format!("[Interface]\nListenPort = 51820\n[Peer]\nPublicKey = {}\n", kept.base64());
        let config = wireguard::parse_config(&text, &mut |_| {}).unwrap();
        let request = sync_request(&config, &device).unwrap();
        assert!(request.contains(&format!("public_key={}\nremove=true\n", gone.hex())));
        assert!(!request.contains("listen_port"), "unchanged: {request}");
        assert!(request.contains(&format!("public_key={}\npreshared_key=", kept.hex())));
    }

    #[test]
    fn showconf_gives_a_file_setconf_reads() {
        let key = Key::generate().unwrap();
        let device = Device {
            private_key: Some(key),
            listen_port: 51820,
            fwmark: 0,
            peers: vec![Peer {
                public_key: key.public(),
                preshared_key: None,
                endpoint: Some("10.0.0.2:51820".parse().unwrap()),
                allowed_ips: vec![Prefix::parse("10.9.0.0/24").unwrap()],
                last_handshake: (0, 0),
                rx_bytes: 0,
                tx_bytes: 0,
                keepalive: 25,
            }],
        };
        let config = wireguard::parse_config(&showconf(&device), &mut |_| {}).unwrap();
        assert_eq!(config.private_key, Some(key));
        assert_eq!(config.listen_port, Some(51820));
        assert_eq!(config.peers[0].endpoint.as_deref(), Some("10.0.0.2:51820"));
        assert_eq!(config.peers[0].keepalive, Some(25));
    }
}
