// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The network area of `hardware check`: the link of each network card,
//! from `/kernel/class/net` as nic(8) reads it: whether it has a link,
//! at what speed and duplex, and whether its receive errors (frames with
//! a bad checksum or a wrong length) rose while the check ran, which the
//! counts of its start and end tell.
//!
//! A link at half duplex is a warning, since a card at full duplex on a
//! cable or switch port in trouble negotiates down to it. A card that
//! has no link is not a problem of the hardware that this check can
//! tell: it says so, and netcheck(8) looks at the layers above.

use std::fs;
use std::path::Path;

use super::check::{Area, Check, Problem, counted};
use crate::ip::INTERFACES;
use crate::nic::{self, Interface};

/// The receive errors of each card with a wire, by name, at one moment.
pub(crate) type Counts = Vec<(String, Option<u64>)>;

/// Where the kernel shows the network interfaces.
pub(crate) const CLASS_NET: &str = INTERFACES;

fn receive_errors(interface: &Interface) -> Option<u64> {
    fs::read_to_string(interface.dir.join("statistics/rx_errors")).ok()?.trim().parse().ok()
}

/// The receive errors of each card in `root` now.
pub(crate) fn counts(root: &Path) -> Counts {
    let Ok(interfaces) = nic::all(root) else { return Vec::new() };
    interfaces
        .iter()
        .filter(|interface| nic::has_wire(interface))
        .map(|interface| (interface.name.clone(), receive_errors(interface)))
        .collect()
}

/// The check of one card: `before` and `after` are its receive errors at
/// the start and end of the run.
pub(crate) fn check(interface: &Interface, before: Option<u64>, after: Option<u64>) -> Check {
    let name = interface.name.as_str();
    if !interface.carrier {
        return Check::skipped(
            Area::Network,
            name,
            "it has no link: no cable, or the other end is off".into(),
        );
    }
    let speed = interface
        .speed
        .map_or("at a speed it does not say".to_string(), |mbps| format!("at {mbps} Mb/s"));
    let duplex = interface.duplex.as_deref().map_or("with a duplex it does not say", |duplex| {
        if duplex == "half" { "half duplex" } else { "full duplex" }
    });
    let link = format!("link up {speed}, {duplex}");
    if interface.duplex.as_deref() == Some("half") {
        return Check::warning(Area::Network, name, Problem::LinkHalfDuplex, link);
    }
    let rose = match (before, after) {
        (Some(before), Some(after)) if after > before => Some(after - before),
        _ => None,
    };
    if let Some(rose) = rose {
        let detail = format!("{link}; {} during the check", counted(rose, "receive error"));
        return Check::warning(Area::Network, name, Problem::LinkErrors, detail);
    }
    let errors = if after.is_some() {
        "no receive errors during the check"
    } else {
        "it counts no receive errors"
    };
    Check::passed(Area::Network, name, format!("{link}; {errors}"))
}

/// The checks of the area: one for each card with a wire in `root`, with
/// `before` the counts the run began with.
pub(crate) fn run(root: &Path, before: &Counts) -> Vec<Check> {
    let interfaces = nic::all(root).unwrap_or_default();
    let cards: Vec<&Interface> = interfaces.iter().filter(|i| nic::has_wire(i)).collect();
    if cards.is_empty() {
        return vec![Check::skipped(
            Area::Network,
            "network",
            "this machine has no network card".into(),
        )];
    }
    cards
        .into_iter()
        .map(|card| {
            let start =
                before.iter().find(|(name, _)| *name == card.name).and_then(|(_, count)| *count);
            check(card, start, receive_errors(card))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::check::Outcome;
    use super::*;

    fn card(root: &Path, name: &str, files: &[(&str, &str)]) {
        let dir = root.join(name);
        fs::create_dir_all(dir.join("statistics")).unwrap();
        for (file, text) in files {
            fs::write(dir.join(file), format!("{text}\n")).unwrap();
        }
    }

    fn folder(call: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("hardware-net-{}-{call}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn up(root: &Path, name: &str, duplex: &str, errors: u64) {
        card(
            root,
            name,
            &[
                ("carrier", "1"),
                ("speed", "1000"),
                ("duplex", duplex),
                ("statistics/rx_errors", &errors.to_string()),
            ],
        );
    }

    fn check_of(root: &Path, name: &str, before: Option<u64>, after: Option<u64>) -> Check {
        let interfaces = nic::all(root).unwrap();
        let interface = interfaces.iter().find(|i| i.name == name).unwrap();
        check(interface, before, after)
    }

    #[test]
    fn a_link_at_full_duplex_without_errors_passes() {
        let root = folder("pass");
        up(&root, "eth0", "full", 0);
        let check = check_of(&root, "eth0", Some(0), Some(0));
        assert_eq!(check.outcome, Outcome::Passed);
        assert_eq!(
            check.detail,
            "link up at 1000 Mb/s, full duplex; no receive errors during the check"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_link_at_half_duplex_warns() {
        let root = folder("half");
        up(&root, "eth0", "half", 0);
        let check = check_of(&root, "eth0", Some(0), Some(0));
        assert_eq!((check.outcome, check.id.as_deref()), (Outcome::Warning, Some("HW-152")));
        assert_eq!(check.detail, "link up at 1000 Mb/s, half duplex");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn receive_errors_that_rose_during_the_check_warn() {
        let root = folder("errors");
        up(&root, "eth0", "full", 14);
        let check = check_of(&root, "eth0", Some(4), Some(14));
        assert_eq!((check.outcome, check.id.as_deref()), (Outcome::Warning, Some("HW-153")));
        assert!(check.detail.ends_with("; 10 receive errors during the check"), "{}", check.detail);
        // Errors that came before the check are not its to count.
        assert_eq!(check_of(&root, "eth0", Some(14), Some(14)).outcome, Outcome::Passed);
        let counted_none = check_of(&root, "eth0", None, None);
        assert!(counted_none.detail.ends_with("it counts no receive errors"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_card_without_a_link_is_skipped() {
        let root = folder("down");
        card(&root, "eth0", &[("carrier", "0")]);
        let check = check_of(&root, "eth0", None, None);
        assert_eq!(check.outcome, Outcome::Skipped);
        assert!(check.detail.starts_with("it has no link"), "{}", check.detail);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn each_card_with_a_wire_is_checked_against_the_counts_of_the_start() {
        let root = folder("run");
        up(&root, "eth0", "full", 3);
        up(&root, "eth1", "full", 9);
        card(&root, "lo", &[("link", "loopback")]);
        let before = vec![("eth0".to_string(), Some(3)), ("eth1".to_string(), Some(0))];
        let checks = run(&root, &before);
        assert_eq!(checks.len(), 2);
        assert_eq!((checks[0].name.as_str(), checks[0].outcome), ("eth0", Outcome::Passed));
        assert_eq!((checks[1].name.as_str(), checks[1].outcome), ("eth1", Outcome::Warning));
        assert_eq!(counts(&root), [("eth0".to_string(), Some(3)), ("eth1".to_string(), Some(9))]);
        let _ = fs::remove_dir_all(&root);
        let none = run(&root, &Vec::new());
        assert_eq!((none.len(), none[0].outcome), (1, Outcome::Skipped));
    }
}
