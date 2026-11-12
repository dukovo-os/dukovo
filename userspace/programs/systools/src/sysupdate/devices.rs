// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The devices each good start had, for the health check's per-device
//! checks (ADR 0468): a new version or firmware set that loses a device
//! the last good start had fails its try. The record holds the network
//! cards the kernel drives itself, on PCI or Hyper-V's VMBus, by where
//! they are and their MAC address, and the partitions the root's Btrfs is
//! on; a USB card, a card a program drives and every other disk come and
//! go with their owner, so they are not in it. It is kept in the state,
//! `/system/state/sysupdate/devices`, a line each, its fields split by
//! tabs:
//!
//! ```text
//! card    0000:00:03.0 52:54:00:12:34:56    Intel 82540EM
//! root    vda3
//! ```

use std::fmt::Write as _;
use std::path::Path;

use super::place::{self, Root};
use crate::util::Fail;

/// Where the record of the last good start is kept.
pub const RECORD: &str = "/system/state/sysupdate/devices";

/// Where the kernel lists the network interfaces (radix-sysfs(5)).
const NET: &str = "/kernel/class/net";

/// A device the record holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// `card` or `root`.
    pub kind: String,
    /// What finds it again: a card's PCI address and MAC address, or a
    /// partition's name.
    pub key: String,
    /// What it is, for messages: a card's model.
    pub about: String,
}

impl Device {
    /// The device in words, for the owner.
    #[must_use]
    pub fn described(&self) -> String {
        match self.kind.as_str() {
            "card" => {
                let (bus, mac) = self.key.split_once(' ').unwrap_or((&self.key, ""));
                let model = if self.about.is_empty() { "network card" } else { &self.about };
                format!("the network card {model} at {bus} ({mac})")
            }
            "root" => format!("the root's partition {}", self.key),
            other => format!("the {other} {}", self.key),
        }
    }
}

/// The record's text.
#[must_use]
pub fn text(devices: &[Device]) -> String {
    let mut text = String::new();
    for device in devices {
        let _ = writeln!(text, "{}\t{}\t{}", device.kind, device.key, device.about);
    }
    text
}

/// A record's devices; lines it cannot read are left out.
#[must_use]
pub fn parse(text: &str) -> Vec<Device> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            let (kind, key) = (fields.next()?, fields.next()?);
            (!kind.is_empty() && !key.is_empty()).then(|| Device {
                kind: kind.to_string(),
                key: key.to_string(),
                about: fields.next().unwrap_or_default().to_string(),
            })
        })
        .collect()
}

/// The devices of `before` that `now` lacks.
#[must_use]
pub fn missing<'a>(before: &'a [Device], now: &[Device]) -> Vec<&'a Device> {
    before.iter().filter(|d| !now.iter().any(|n| n.kind == d.kind && n.key == d.key)).collect()
}

/// A card as the kernel's `card` file of `/kernel/class/net/<name>`
/// says: one the kernel drives itself, which says where it is, with its
/// MAC address `address`.
fn card(card: &str, address: &str) -> Option<Device> {
    let field = |key: &str| {
        card.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix(' ')).map(str::trim)
    };
    let bus = field("bus")?;
    Some(Device {
        kind: "card".into(),
        key: format!("{bus} {}", address.trim()),
        about: field("model").unwrap_or_default().to_string(),
    })
}

/// The devices this start has.
///
/// # Errors
///
/// If the root's disks cannot be listed.
pub fn present(root: &Root) -> Result<Vec<Device>, Fail> {
    let mut devices = Vec::new();
    let mut names: Vec<String> = std::fs::read_dir(NET).map_or_else(
        |_| Vec::new(),
        |entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        },
    );
    names.sort();
    for name in names {
        let dir = Path::new(NET).join(&name);
        let read = |file: &str| std::fs::read_to_string(dir.join(file)).unwrap_or_default();
        if let Some(device) = card(&read("card"), &read("address")) {
            devices.push(device);
        }
    }
    for partition in place::root_partitions(root)? {
        devices.push(Device { kind: "root".into(), key: partition, about: String::new() });
    }
    Ok(devices)
}

/// The record of the last good start.
#[must_use]
pub fn recorded() -> Option<Vec<Device>> {
    std::fs::read_to_string(RECORD).ok().map(|text| parse(&text))
}

/// Keeps `devices` as the record of the last good start, whole or not at
/// all.
pub fn record(devices: &[Device]) {
    let path = Path::new(RECORD);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let partial = path.with_extension("partial");
    let _ = std::fs::write(&partial, text(devices)).and_then(|()| std::fs::rename(&partial, path));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_on_pci_is_recorded_and_missed() {
        let intel = card(
            "driver radix-intel-net\nmodel Intel 82540EM\nbus 0000:00:03.0\nreceive_slots 256\n",
            "52:54:00:12:34:56\n",
        )
        .unwrap();
        assert_eq!(intel.key, "0000:00:03.0 52:54:00:12:34:56");
        assert_eq!(
            intel.described(),
            "the network card Intel 82540EM at 0000:00:03.0 (52:54:00:12:34:56)"
        );
        // A USB adapter and a card a program drives say no place.
        assert_eq!(card("driver radix-usb\n", "52:54:00:00:00:01\n"), None);
        assert_eq!(card("driver program\n", "52:54:00:00:00:02\n"), None);
        let root = Device { kind: "root".into(), key: "vda3".into(), about: String::new() };
        assert_eq!(root.described(), "the root's partition vda3");
        let before = vec![intel.clone(), root.clone()];
        assert_eq!(parse(&text(&before)), before);
        assert_eq!(parse("card\n\nroot\tvdb3\n").len(), 1);
        assert!(missing(&before, &before).is_empty());
        assert_eq!(missing(&before, &[root]), [&intel]);
        // A card that moved to another slot is another card.
        let moved = Device { key: "0000:00:04.0 52:54:00:12:34:56".into(), ..intel.clone() };
        assert_eq!(missing(&before[..1], &[moved]), [&intel]);
    }
}
