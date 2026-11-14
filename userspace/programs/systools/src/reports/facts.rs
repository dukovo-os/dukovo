// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The facts a problem report starts with (ADR 0531): which Dukovo, which
//! machine, which devices, which language. They are what a maintainer asks
//! for first, and none of them names a person: no serial number, no
//! address, no account.
//!
//! reportd(8) gathers them as root and puts them at the top of every
//! draft ([`super::draft`]). Each comes from what anyone may read: the
//! kernel's version, `updates.toml`, the firmware's SMBIOS fields the
//! kernel shows in `/kernel/class/dmi/id`, the processor's CPUID, the
//! memory the kernel counts, and the vendor and device IDs of the PCI
//! functions and USB devices. A fact the machine does not give is left
//! out. When a draft misses a line, run `hwinfo`, `lspci -n` or `lsusb`
//! and see which of them fails on its own.

use std::fs;
use std::path::Path;

use crate::sysupdate::online;
use crate::{dukofetch, hwinfo, lscpu, lspci, lsusb, proc};

/// The most device IDs of each kind a report lists; a larger machine
/// shows its first.
const DEVICES_MAX: usize = 100;

/// What a report says of the machine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// Dukovo's version, `0.16.0`.
    pub version: String,
    /// The update channel the system follows: `stable`, `beta` or `nightly`.
    pub channel: String,
    /// The edition, `server`, when the kernel's version line names one.
    pub edition: Option<String>,
    /// The maker the firmware names, `Dell Inc.`.
    pub maker: Option<String>,
    /// The model the firmware names, `OptiPlex 7040`.
    pub model: Option<String>,
    /// The processor's name, without its marks and clock.
    pub processor: Option<String>,
    /// The memory the kernel counts, in KiB.
    pub memory_kib: Option<u64>,
    /// The PCI functions' vendor and device IDs, `8086:15b8`, each once.
    pub pci: Vec<String>,
    /// The USB devices' vendor and product IDs, `1d6b:0002`, each once.
    pub usb: Vec<String>,
    /// The language setting of the environment, `en_US.UTF-8`.
    pub language: Option<String>,
}

/// The facts of this machine.
pub fn gather() -> Facts {
    let release = fs::read_to_string(dukofetch::VERSION)
        .ok()
        .and_then(|text| dukofetch::parse_version(&text))
        .map(|(_, release)| release);
    let (edition, version) = match release {
        Some((name, version)) => (edition_of(&name), version),
        None => (None, env!("CARGO_PKG_VERSION").to_string()),
    };
    // The channel is a convenience of the report, not worth failing it
    // over: a settings file that cannot be read has the stable channel's
    // default shown.
    let channel = online::Settings::load()
        .map_or_else(|_| online::DEFAULT_CHANNEL.to_string(), |settings| settings.channel);
    Facts {
        version,
        channel,
        edition,
        maker: dmi_field("sys_vendor"),
        model: dmi_field("product_name"),
        processor: lscpu::model_name(&lscpu::cpuid).map(|brand| dukofetch::cpu_name(&brand)),
        memory_kib: proc::memory().ok().map(|memory| memory.total).filter(|&kib| kib > 0),
        pci: pci_ids(),
        usb: usb_ids(),
        language: language(|name| std::env::var(name).ok()),
    }
}

/// The edition in a release's name: `server` for `Dukovo Server`.
fn edition_of(name: &str) -> Option<String> {
    let edition = name.strip_prefix("Dukovo ")?.trim();
    Some(edition.to_lowercase()).filter(|edition| !edition.is_empty())
}

/// A line of an SMBIOS field the kernel shows, if it has a value.
fn dmi_field(name: &str) -> Option<String> {
    let text = fs::read_to_string(Path::new(hwinfo::DMI).join(name)).ok()?;
    Some(text.trim().to_string()).filter(|value| !value.is_empty())
}

fn pci_ids() -> Vec<String> {
    let functions = lspci::functions(Path::new(lspci::DEVICES)).unwrap_or_default();
    unique(functions.iter().map(|f| format!("{:04x}:{:04x}", f.vendor, f.device)).collect())
}

fn usb_ids() -> Vec<String> {
    unique(lsusb::ids(Path::new(lsusb::DEVICES)).unwrap_or_default())
}

/// `ids` in order, each once, and no more than [`DEVICES_MAX`].
fn unique(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids.dedup();
    ids.truncate(DEVICES_MAX);
    ids
}

/// The language setting `get` gives: `LC_ALL` beats `LANG`, as in the C
/// library, and an empty one counts as none.
fn language(get: impl Fn(&str) -> Option<String>) -> Option<String> {
    ["LC_ALL", "LANG"].iter().find_map(|name| get(name).filter(|value| !value.is_empty()))
}

impl Facts {
    /// The lines that open a report, `Label: value`. The first is always
    /// the Version line; the lines of a machine that tells nothing of
    /// itself are left out.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![self.version_line()];
        if let Some(hardware) = self.hardware() {
            lines.push(format!("Hardware: {hardware}"));
        }
        if !self.pci.is_empty() {
            lines.push(format!("Devices: {}", self.pci.join(", ")));
        }
        if !self.usb.is_empty() {
            lines.push(format!("USB devices: {}", self.usb.join(", ")));
        }
        if let Some(language) = &self.language {
            lines.push(format!("Language: {language}"));
        }
        lines
    }

    /// `Version: 0.16.0 (stable, server)`.
    fn version_line(&self) -> String {
        let mut details = vec![self.channel.as_str()];
        details.extend(self.edition.as_deref());
        format!("Version: {} ({})", self.version, details.join(", "))
    }

    /// The maker and model, the processor and the memory, as one line's
    /// value, or none when the machine names none of them.
    fn hardware(&self) -> Option<String> {
        let machine: Vec<&str> =
            [&self.maker, &self.model].into_iter().flatten().map(String::as_str).collect();
        let mut parts = Vec::new();
        if !machine.is_empty() {
            parts.push(machine.join(" "));
        }
        parts.extend(self.processor.clone());
        parts.extend(self.memory_kib.map(memory_text));
        Some(parts.join(", ")).filter(|text| !text.is_empty())
    }
}

/// A memory size as a report says it: whole GiB from three quarters of one
/// up, as the kernel counts a little less than the modules hold, and MiB
/// below.
fn memory_text(kib: u64) -> String {
    const GIB: u64 = 1024 * 1024;
    if kib < GIB / 4 * 3 {
        return format!("{} MiB", kib / 1024);
    }
    format!("{} GiB", (kib + GIB / 2) / GIB)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dell() -> Facts {
        Facts {
            version: "0.16.0".into(),
            channel: "stable".into(),
            edition: Some("server".into()),
            maker: Some("Dell Inc.".into()),
            model: Some("OptiPlex 7040".into()),
            processor: Some("Intel Core i7-6700".into()),
            memory_kib: Some(16 * 1024 * 1024 - 120_000),
            pci: vec!["8086:15b8".into(), "8086:a102".into()],
            usb: vec!["1d6b:0002".into()],
            language: Some("en_US.UTF-8".into()),
        }
    }

    #[test]
    fn a_whole_machine_gives_a_line_for_each_fact() {
        assert_eq!(
            dell().lines(),
            [
                "Version: 0.16.0 (stable, server)",
                "Hardware: Dell Inc. OptiPlex 7040, Intel Core i7-6700, 16 GiB",
                "Devices: 8086:15b8, 8086:a102",
                "USB devices: 1d6b:0002",
                "Language: en_US.UTF-8",
            ]
        );
    }

    #[test]
    fn a_machine_that_tells_nothing_gives_only_the_version() {
        let facts = Facts { version: "0.16.0".into(), channel: "beta".into(), ..Facts::default() };
        assert_eq!(facts.lines(), ["Version: 0.16.0 (beta)"]);
    }

    #[test]
    fn the_hardware_line_names_what_the_machine_gives() {
        let mut facts = dell();
        facts.maker = None;
        facts.model = Some("Standard PC".into());
        facts.memory_kib = None;
        assert_eq!(facts.hardware().unwrap(), "Standard PC, Intel Core i7-6700");
        facts.model = None;
        facts.processor = None;
        facts.memory_kib = Some(512 * 1024);
        assert_eq!(facts.hardware().unwrap(), "512 MiB");
        facts.memory_kib = None;
        assert_eq!(facts.hardware(), None);
    }

    #[test]
    fn memory_is_rounded_to_whole_gibibytes_from_one_up() {
        assert_eq!(memory_text(16 * 1024 * 1024), "16 GiB");
        assert_eq!(memory_text(8 * 1024 * 1024 - 300_000), "8 GiB");
        assert_eq!(memory_text(1000 * 1024), "1 GiB");
        assert_eq!(memory_text(300 * 1024), "300 MiB");
        assert_eq!(memory_text(700 * 1024), "700 MiB");
        assert_eq!(memory_text(800 * 1024), "1 GiB");
    }

    #[test]
    fn the_edition_is_the_release_name_after_dukovo() {
        assert_eq!(edition_of("Dukovo Server").as_deref(), Some("server"));
        assert_eq!(edition_of("Dukovo Desktop").as_deref(), Some("desktop"));
        assert_eq!(edition_of("Dukovo"), None);
        assert_eq!(edition_of("Other"), None);
    }

    #[test]
    fn the_language_is_lc_all_before_lang_and_never_empty() {
        let env = |pairs: &'static [(&str, &str)]| {
            move |name: &str| {
                pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| (*value).to_string())
            }
        };
        assert_eq!(
            language(env(&[("LANG", "C.UTF-8"), ("LC_ALL", "de_DE.UTF-8")])).as_deref(),
            Some("de_DE.UTF-8")
        );
        assert_eq!(
            language(env(&[("LANG", "C.UTF-8"), ("LC_ALL", "")])).as_deref(),
            Some("C.UTF-8")
        );
        assert_eq!(language(env(&[])), None);
    }

    #[test]
    fn device_ids_are_sorted_each_once_and_capped() {
        let ids = unique(vec!["8086:a102".into(), "8086:15b8".into(), "8086:a102".into()]);
        assert_eq!(ids, ["8086:15b8", "8086:a102"]);
        let many: Vec<String> = (0..300).map(|n| format!("8086:{n:04x}")).collect();
        assert_eq!(unique(many).len(), DEVICES_MAX);
    }

    #[test]
    fn gathering_on_any_machine_gives_at_least_the_version() {
        let facts = gather();
        assert!(!facts.version.is_empty());
        assert!(facts.lines()[0].starts_with("Version: "));
    }
}
