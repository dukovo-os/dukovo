// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! nic(8): what a network card says of itself, as ethtool(8) shows
//! Linux's (ADR 0341): its link's speed and duplex, its driver, its
//! rings, its offloads and its counts.
//!
//! It reads `/kernel/class/net`: Linux's `speed`, `duplex`, `carrier`,
//! `address` and `statistics`, and Radix's `card` and `link`. `nic NAME`
//! shows the link's settings as `ethtool NAME` does, `-i` the driver,
//! `-g` the rings, `-k` the offloads, `-S` the counts and `-P` the MAC
//! address, each in ethtool's words, so that what reads ethtool's output
//! reads nic's. `nic` alone shows a line for each interface. The kernel
//! uses none of a card's offloads, and its drivers take the link the card
//! negotiates, so `-s` and `-K`, which change them on Linux, say so and
//! change nothing.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::args::{Arg, Args};
use crate::ip::{INTERFACES, Kind, parse_link};
use crate::util::{Fail, fail, usage};

/// What the kernel says of an interface's card.
#[derive(Debug, Default, PartialEq, Eq)]
struct Card {
    driver: Option<String>,
    model: Option<String>,
    bus: Option<String>,
    receive_slots: Option<u32>,
    transmit_slots: Option<u32>,
    interrupt: Option<String>,
    dropped: Option<u64>,
}

/// What its `card` file says.
fn parse_card(text: &str) -> Card {
    let mut card = Card::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(' ') else { continue };
        let value = value.trim();
        match key {
            "driver" => card.driver = Some(value.to_string()),
            "model" => card.model = Some(value.to_string()),
            "bus" => card.bus = Some(value.to_string()),
            "receive_slots" => card.receive_slots = value.parse().ok(),
            "transmit_slots" => card.transmit_slots = value.parse().ok(),
            "interrupt" => card.interrupt = Some(value.to_string()),
            "dropped" => card.dropped = value.parse().ok(),
            _ => {}
        }
    }
    card
}

/// An interface, as nic reads it.
pub(crate) struct Interface {
    pub(crate) name: String,
    pub(crate) dir: PathBuf,
    kind: Kind,
    card: Card,
    /// Megabits a second, if the card says.
    pub(crate) speed: Option<u32>,
    /// `full` or `half`, if the card says.
    pub(crate) duplex: Option<String>,
    pub(crate) carrier: bool,
}

/// What `file` of `dir` says, without its newline.
fn read(dir: &Path, file: &str) -> Option<String> {
    let text = fs::read_to_string(dir.join(file)).ok()?;
    Some(text.trim_end_matches('\n').to_string())
}

/// The interface named `name` in `root`.
fn open(root: &Path, name: &str) -> Result<Interface, Fail> {
    // A name is one of the directory's, never a path out of it.
    let dir = root.join(name);
    if name.is_empty() || name.contains('/') || name == "." || name == ".." || !dir.is_dir() {
        return Err(fail(format!("{name}: no such interface")));
    }
    let (kind, _) = parse_link(&read(&dir, "link").unwrap_or_default());
    let card = parse_card(&read(&dir, "card").unwrap_or_default());
    let speed = read(&dir, "speed").and_then(|s| s.trim().parse::<i64>().ok());
    let speed = speed.and_then(|s| u32::try_from(s).ok()).filter(|&s| s != 0);
    let duplex = read(&dir, "duplex").filter(|d| d == "full" || d == "half");
    let carrier = read(&dir, "carrier").is_some_and(|c| c.trim() == "1");
    Ok(Interface { name: name.to_string(), dir, kind, card, speed, duplex, carrier })
}

/// Every interface in `root`, by name.
pub(crate) fn all(root: &Path) -> Result<Vec<Interface>, Fail> {
    let entries =
        fs::read_dir(root).map_err(|e| fail(format!("cannot read {}: {e}", root.display())))?;
    let mut names: Vec<String> =
        entries.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect();
    names.sort();
    names.iter().map(|name| open(root, name)).collect()
}

/// What drives the interface, in ethtool -i's words: its card's driver,
/// or what Linux names the driver of a link of its kind.
fn driver(interface: &Interface) -> &str {
    match (&interface.kind, &interface.card.driver) {
        (_, Some(driver)) => driver,
        (Kind::Loopback, None) => "loopback",
        (Kind::Bridge { .. }, None) => "bridge",
        (Kind::Vlan { .. }, None) => "802.1Q VLAN Support",
        (Kind::Tunnel, None) => "tun",
        (Kind::Card, None) => "",
    }
}

/// Whether the interface has a wire whose speed and duplex mean
/// something: a card's, not a bridge's, a VLAN's, a tunnel's or `lo`'s.
pub(crate) fn has_wire(interface: &Interface) -> bool {
    matches!(interface.kind, Kind::Card)
}

/// `nic NAME`: the link's settings, as `ethtool NAME`.
fn settings(interface: &Interface, out: &mut dyn Write) -> io::Result<()> {
    writeln!(out, "Settings for {}:", interface.name)?;
    if has_wire(interface) {
        match interface.speed.filter(|_| interface.carrier) {
            Some(mbps) => writeln!(out, "\tSpeed: {mbps}Mb/s")?,
            None => writeln!(out, "\tSpeed: Unknown!")?,
        }
        match interface.duplex.as_deref().filter(|_| interface.carrier) {
            Some("full") => writeln!(out, "\tDuplex: Full")?,
            Some(_) => writeln!(out, "\tDuplex: Half")?,
            None => writeln!(out, "\tDuplex: Unknown! (255)")?,
        }
    }
    writeln!(out, "\tLink detected: {}", if interface.carrier { "yes" } else { "no" })
}

/// `-i`: the driver, as `ethtool -i`.
fn driver_info(interface: &Interface, release: &str, out: &mut dyn Write) -> io::Result<()> {
    writeln!(out, "driver: {}", driver(interface))?;
    // Every driver is the kernel's, of its release, as Linux's in-tree
    // drivers say.
    writeln!(out, "version: {release}")?;
    writeln!(out, "firmware-version: ")?;
    writeln!(out, "expansion-rom-version: ")?;
    writeln!(out, "bus-info: {}", interface.card.bus.as_deref().unwrap_or(""))?;
    writeln!(out, "supports-statistics: yes")?;
    writeln!(out, "supports-test: no")?;
    writeln!(out, "supports-eeprom-access: no")?;
    writeln!(out, "supports-register-dump: no")?;
    writeln!(out, "supports-priv-flags: no")?;
    if let Some(model) = &interface.card.model {
        writeln!(out, "model: {model}")?;
    }
    if let Some(interrupt) = &interface.card.interrupt {
        writeln!(out, "interrupt: {interrupt}")?;
    }
    Ok(())
}

/// `-g`: the rings, as `ethtool -g`; the card's are fixed, so the most
/// it takes is what it has.
fn rings(interface: &Interface, out: &mut dyn Write) -> Result<(), Fail> {
    let (Some(rx), Some(tx)) = (interface.card.receive_slots, interface.card.transmit_slots) else {
        return Err(fail(format!(
            "Cannot get device ring settings: Operation not supported ({} has no card the kernel drives)",
            interface.name
        )));
    };
    let mut text = format!("Ring parameters for {}:\n", interface.name);
    for heading in ["Pre-set maximums:", "Current hardware settings:"] {
        text.push_str(&format!(
            "{heading}\nRX:\t\t{rx}\nRX Mini:\tn/a\nRX Jumbo:\tn/a\nTX:\t\t{tx}\n"
        ));
    }
    out.write_all(text.as_bytes()).map_err(Fail::from)
}

/// The offloads ethtool -k shows, in its order.
const FEATURES: [&str; 12] = [
    "rx-checksumming",
    "tx-checksumming",
    "scatter-gather",
    "tcp-segmentation-offload",
    "udp-fragmentation-offload",
    "generic-segmentation-offload",
    "generic-receive-offload",
    "large-receive-offload",
    "rx-vlan-offload",
    "tx-vlan-offload",
    "ntuple-filters",
    "receive-hashing",
];

/// `-k`: the offloads, as `ethtool -k`. The kernel uses none of a card's
/// and does none in software; the loopback link alone needs no
/// checksums, as Linux's `lo` says.
fn features(interface: &Interface, out: &mut dyn Write) -> io::Result<()> {
    writeln!(out, "Features for {}:", interface.name)?;
    for feature in FEATURES {
        let on = matches!(interface.kind, Kind::Loopback)
            && matches!(feature, "rx-checksumming" | "tx-checksumming");
        writeln!(out, "{feature}: {} [fixed]", if on { "on" } else { "off" })?;
    }
    Ok(())
}

/// The counts of `statistics`, in ethtool -S's order.
const STATISTICS: [&str; 8] = [
    "rx_bytes",
    "rx_packets",
    "rx_errors",
    "rx_dropped",
    "tx_bytes",
    "tx_packets",
    "tx_errors",
    "tx_dropped",
];

/// `-S`: the counts, as `ethtool -S`: the stack's, and the frames the
/// card dropped if its driver counts them.
fn statistics(interface: &Interface, out: &mut dyn Write) -> io::Result<()> {
    writeln!(out, "NIC statistics:")?;
    let counts = interface.dir.join("statistics");
    for name in STATISTICS {
        let count = read(&counts, name).and_then(|c| c.trim().parse::<u64>().ok()).unwrap_or(0);
        writeln!(out, "     {name}: {count}")?;
    }
    if let Some(dropped) = interface.card.dropped {
        writeln!(out, "     card_rx_dropped: {dropped}")?;
    }
    Ok(())
}

/// `-P`: the MAC address, as `ethtool -P`.
fn permanent(interface: &Interface, out: &mut dyn Write) -> io::Result<()> {
    let address = read(&interface.dir, "address").unwrap_or_default();
    let address = if address.is_empty() { "00:00:00:00:00:00" } else { address.trim() };
    writeln!(out, "Permanent address: {address}")
}

/// `nic` alone: a line for each interface, its link and its card.
fn summary(interfaces: &[Interface], out: &mut dyn Write) -> io::Result<()> {
    let width = interfaces.iter().map(|i| i.name.len()).max().unwrap_or(0).max(9);
    writeln!(
        out,
        "{:width$}  {:4}  {:>10}  {:7}  {:18}  MODEL",
        "INTERFACE", "LINK", "SPEED", "DUPLEX", "DRIVER"
    )?;
    for interface in interfaces {
        let link = if interface.carrier { "yes" } else { "no" };
        let up = |i: &Interface| has_wire(i) && i.carrier;
        let speed =
            interface.speed.filter(|_| up(interface)).map_or("-".into(), |s| format!("{s}Mb/s"));
        let duplex = interface.duplex.as_deref().filter(|_| up(interface)).unwrap_or("-");
        let model = interface.card.model.as_deref().unwrap_or("-");
        let mut driver = driver(interface);
        if driver.is_empty() {
            driver = "-";
        }
        writeln!(
            out,
            "{:width$}  {link:4}  {speed:>10}  {duplex:7}  {driver:18}  {model}",
            interface.name
        )?;
    }
    Ok(())
}

/// What nic was asked to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Show {
    Settings,
    Driver,
    Rings,
    Features,
    Statistics,
    Permanent,
}

/// Runs nic.
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let mut show = Show::Settings;
    let mut names = Vec::new();
    while let Some(arg) = args.next()? {
        let next = match &arg {
            Arg::Short('i') => Show::Driver,
            Arg::Long(name) if name == "driver" => Show::Driver,
            Arg::Short('g') => Show::Rings,
            Arg::Long(name) if name == "show-ring" => Show::Rings,
            Arg::Short('k') => Show::Features,
            Arg::Long(name) if name == "show-features" || name == "show-offload" => Show::Features,
            Arg::Short('S') => Show::Statistics,
            Arg::Long(name) if name == "statistics" => Show::Statistics,
            Arg::Short('P') => Show::Permanent,
            Arg::Long(name) if name == "show-permaddr" => Show::Permanent,
            Arg::Short('s') => {
                return Err(fail(
                    "-s: a card's link is the one it negotiates, and is not set here".into(),
                ));
            }
            Arg::Long(name) if name == "change" => {
                return Err(fail(
                    "--change: a card's link is the one it negotiates, and is not set here".into(),
                ));
            }
            Arg::Short('K') => {
                return Err(fail("-K: the offloads are fixed: the kernel uses none".into()));
            }
            Arg::Long(name) if name == "features" || name == "offload" => {
                return Err(fail(format!(
                    "--{name}: the offloads are fixed: the kernel uses none"
                )));
            }
            Arg::Short('h') => {
                print!("{HELP}");
                return Ok(0);
            }
            Arg::Long(name) if name == "help" => {
                print!("{HELP}");
                return Ok(0);
            }
            Arg::Operand(name) => {
                names.push(name.to_string_lossy().into_owned());
                continue;
            }
            other => return Err(args.unknown(other)),
        };
        if show != Show::Settings && show != next {
            return Err(usage("only one of -i, -g, -k, -S and -P at a time".into()));
        }
        show = next;
    }
    let root = Path::new(INTERFACES);
    let mut stdout = io::stdout().lock();
    if names.is_empty() {
        if show != Show::Settings {
            return Err(usage("missing operand: which interface".into()));
        }
        summary(&all(root)?, &mut stdout)?;
        return Ok(0);
    }
    if names.len() > 1 {
        return Err(usage(format!("one interface at a time, not {}", names.join(" "))));
    }
    let interface = open(root, &names[0])?;
    match show {
        Show::Settings => settings(&interface, &mut stdout)?,
        Show::Driver => driver_info(&interface, &crate::util::uname().1, &mut stdout)?,
        Show::Rings => rings(&interface, &mut stdout)?,
        Show::Features => features(&interface, &mut stdout)?,
        Show::Statistics => statistics(&interface, &mut stdout)?,
        Show::Permanent => permanent(&interface, &mut stdout)?,
    }
    Ok(0)
}

const HELP: &str = "\
Usage: nic [NAME]
       nic -i|-g|-k|-S|-P NAME
Shows what a network card says of itself, as ethtool(8) does.
  (none)                   every interface's link, speed, duplex and driver
  NAME                     the link's speed and duplex, and whether it is up
  -i, --driver             its driver, model and PCI address
  -g, --show-ring          its rings: how many frames it holds each way
  -k, --show-features      its offloads, which the kernel does not use
  -S, --statistics         what it sent and received
  -P, --show-permaddr      its MAC address
";

#[cfg(test)]
mod tests {
    use super::*;

    /// An interface directory with `files`, under a new root.
    fn root(call: &str, files: &[(&str, &str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("nic-test-{}-{call}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (interface, file, text) in files {
            let path = root.join(interface).join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        root
    }

    fn shown(f: impl FnOnce(&mut Vec<u8>)) -> String {
        let mut out = Vec::new();
        f(&mut out);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn a_card_shows_its_link_driver_and_rings_as_ethtool() {
        let root = root(
            "card",
            &[
                ("net0", "link", "card\n"),
                ("net0", "speed", "1000\n"),
                ("net0", "duplex", "full\n"),
                ("net0", "carrier", "1\n"),
                ("net0", "address", "52:54:00:12:34:56\n"),
                (
                    "net0",
                    "card",
                    "driver radix-intel-net\nmodel Intel 82574L\nbus 0000:00:03.0\nreceive_slots 256\n\
                     transmit_slots 128\ninterrupt MSI vector 0x31\ndropped 2\n",
                ),
                ("net0", "statistics/rx_bytes", "1500\n"),
                ("net0", "statistics/tx_packets", "3\n"),
            ],
        );
        let net0 = open(&root, "net0").unwrap();
        assert_eq!(
            shown(|out| settings(&net0, out).unwrap()),
            "Settings for net0:\n\tSpeed: 1000Mb/s\n\tDuplex: Full\n\tLink detected: yes\n"
        );
        let info = shown(|out| driver_info(&net0, "0.11.0", out).unwrap());
        assert!(info.starts_with("driver: radix-intel-net\nversion: 0.11.0\n"), "{info}");
        assert!(
            info.contains("bus-info: 0000:00:03.0\n") && info.contains("model: Intel 82574L\n")
        );
        let rings = shown(|out| rings(&net0, out).unwrap());
        assert!(rings.ends_with(
            "Current hardware settings:\nRX:\t\t256\nRX Mini:\tn/a\nRX Jumbo:\tn/a\nTX:\t\t128\n"
        ));
        let counts = shown(|out| statistics(&net0, out).unwrap());
        assert!(
            counts.contains("     rx_bytes: 1500\n") && counts.contains("     tx_packets: 3\n")
        );
        assert!(counts.ends_with("     card_rx_dropped: 2\n"), "{counts}");
        assert_eq!(
            shown(|out| permanent(&net0, out).unwrap()),
            "Permanent address: 52:54:00:12:34:56\n"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_link_that_is_down_or_says_nothing_has_no_speed() {
        let root = root(
            "down",
            &[
                ("net1", "link", "card\n"),
                ("net1", "speed", "-1\n"),
                ("net1", "duplex", "unknown\n"),
                ("net1", "carrier", "0\n"),
                ("net1", "card", "driver radix-virtio\nmodel virtio\n"),
                ("lo", "link", "loopback\n"),
                ("lo", "carrier", "1\n"),
                ("wg0", "link", "tunnel\n"),
                ("wg0", "carrier", "1\n"),
            ],
        );
        let net1 = open(&root, "net1").unwrap();
        assert_eq!(
            shown(|out| settings(&net1, out).unwrap()),
            "Settings for net1:\n\tSpeed: Unknown!\n\tDuplex: Unknown! (255)\n\tLink detected: no\n"
        );
        assert!(rings(&net1, &mut Vec::new()).is_err());
        let lo = open(&root, "lo").unwrap();
        assert_eq!(
            shown(|out| settings(&lo, out).unwrap()),
            "Settings for lo:\n\tLink detected: yes\n"
        );
        let features = shown(|out| features(&lo, out).unwrap());
        assert!(features.contains("rx-checksumming: on [fixed]\n"));
        assert!(features.contains("scatter-gather: off [fixed]\n"));
        let wg0 = open(&root, "wg0").unwrap();
        assert!(shown(|out| driver_info(&wg0, "1", out).unwrap()).starts_with("driver: tun\n"));
        let lines = shown(|out| summary(&all(&root).unwrap(), out).unwrap());
        assert_eq!(lines.lines().count(), 4, "{lines}");
        assert!(open(&root, "../etc").is_err() && open(&root, "eth9").is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn card_lines_are_read_by_key() {
        let card = parse_card("driver program\n");
        assert_eq!(card.driver.as_deref(), Some("program"));
        assert_eq!(card.receive_slots, None);
        assert_eq!(
            parse_card("model Realtek RTL8168h\n").model.as_deref(),
            Some("Realtek RTL8168h")
        );
    }
}
