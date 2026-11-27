// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What `hardware power` says: each battery, AC adapter, USB-C supply and
//! UPS the kernel shows in `/kernel/class/power_supply` (ADR 0741), with
//! the charge, what it is doing and how long it lasts, and for a UPS the
//! charge and run time at which the machine shuts down.
//!
//! A value the supply does not say fails to read, as with Linux's, and is
//! `null`.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use serde::Serialize;

use super::battery::POWER_SUPPLY;

/// What `hardware power --json` says. With no supply the list is empty.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Power {
    /// The version of this document: 1.
    format: u32,
    pub(crate) supplies: Vec<Supply>,
}

/// A power supply.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Supply {
    /// As the kernel names it: `BAT0`, `AC`, `ups0`.
    pub(crate) name: String,
    /// `ups`, `battery`, `mains` or `usb`.
    pub(crate) kind: &'static str,
    /// Its model and maker, as it says them.
    pub(crate) model: Option<String>,
    pub(crate) maker: Option<String>,
    /// For a UPS or an adapter, whether the mains is there; for a USB-C
    /// port, whether a supply powers the machine through it.
    pub(crate) online: Option<bool>,
    /// A battery's or UPS's `Charging`, `Discharging`, `Full` or `Not
    /// charging`.
    pub(crate) status: Option<String>,
    /// Its charge, in percent.
    pub(crate) percent: Option<u64>,
    /// How long the charge lasts at the load now, in seconds.
    pub(crate) seconds_left: Option<u64>,
    /// What the load draws, in percent of what the UPS gives.
    pub(crate) load: Option<u64>,
    /// The charge and run time at which the UPS's battery is low.
    pub(crate) low_percent: Option<u64>,
    pub(crate) low_seconds: Option<u64>,
    /// Whether the machine shuts down when the UPS's battery is low: not
    /// when the kernel was started with `ups=show`.
    pub(crate) shuts_down: Option<bool>,
    /// Whether the battery should be replaced.
    pub(crate) replace: Option<bool>,
}

fn text(dir: &Path, file: &str) -> Option<String> {
    Some(fs::read_to_string(dir.join(file)).ok()?.trim().to_string())
}

fn number(dir: &Path, file: &str) -> Option<u64> {
    text(dir, file)?.parse().ok()
}

fn flag(dir: &Path, file: &str) -> Option<bool> {
    number(dir, file).map(|n| n != 0)
}

/// The supplies in `root`, by name, the UPSes first.
pub(crate) fn gather(root: &Path) -> Power {
    let mut dirs: Vec<_> = fs::read_dir(root)
        .map_or_else(|_| Vec::new(), |entries| entries.flatten().map(|e| e.path()).collect());
    dirs.sort();
    let mut supplies: Vec<Supply> = dirs.iter().filter_map(|dir| supply(dir)).collect();
    supplies.sort_by_key(|supply| supply.kind != "ups");
    Power { format: 1, supplies }
}

/// Where the kernel shows the power supplies.
pub(crate) fn system() -> Power {
    gather(Path::new(POWER_SUPPLY))
}

/// The supply in `dir`, if the kernel says what kind it is.
fn supply(dir: &Path) -> Option<Supply> {
    let kind = match text(dir, "type")?.as_str() {
        "UPS" => "ups",
        "Battery" => "battery",
        "Mains" => "mains",
        "USB" => "usb",
        _ => return None,
    };
    let named = |file| text(dir, file).filter(|value| !value.is_empty());
    Some(Supply {
        name: dir.file_name()?.to_string_lossy().into_owned(),
        kind,
        model: named("model_name"),
        maker: named("manufacturer"),
        online: flag(dir, "online"),
        status: named("status"),
        percent: number(dir, "capacity"),
        seconds_left: number(dir, "time_to_empty_now"),
        load: number(dir, "load"),
        low_percent: number(dir, "capacity_alert_min"),
        low_seconds: number(dir, "time_to_empty_alert_min"),
        shuts_down: flag(dir, "shuts_down"),
        replace: text(dir, "health").map(|health| health == "Dead"),
    })
}

/// `90 s`, `25 min`, `1 h 05 min`.
pub(crate) fn duration(seconds: u64) -> String {
    match seconds {
        0..120 => format!("{seconds} s"),
        120..3600 => format!("{} min", seconds / 60),
        _ => format!("{} h {:02} min", seconds / 3600, seconds / 60 % 60),
    }
}

impl Power {
    pub(crate) fn json(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }

    /// A paragraph for each supply.
    pub(crate) fn text(&self) -> String {
        let mut out = String::new();
        if self.supplies.is_empty() {
            out.push_str("No battery, adapter or UPS.\n");
        }
        for supply in &self.supplies {
            let _ = writeln!(out, "{}", supply.line());
        }
        out
    }
}

impl Supply {
    /// What it is and what it does, on a line or two.
    fn line(&self) -> String {
        let what = match self.kind {
            "ups" => "UPS",
            "battery" => "battery",
            "mains" => "AC adapter",
            _ => "USB-C supply",
        };
        let mut line = format!("{}: {what}", self.name);
        let model = match (&self.model, &self.maker) {
            (Some(model), Some(maker)) => Some(format!("{model} by {maker}")),
            (Some(one), None) | (None, Some(one)) => Some(one.clone()),
            (None, None) => None,
        };
        if let Some(model) = model {
            let _ = write!(line, ", {model}");
        }
        let mut said = Vec::new();
        match (self.kind, self.online) {
            ("ups", Some(true)) => said.push("on the mains".to_string()),
            ("ups", Some(false)) => said.push("on its battery: the mains is gone".to_string()),
            ("mains" | "usb", Some(online)) => {
                said.push(if online { "powering the machine" } else { "unplugged" }.to_string());
            }
            _ => {}
        }
        if let Some(status) = &self.status
            && !(self.kind == "ups" && status == "Discharging")
        {
            said.push(status.to_lowercase());
        }
        if let Some(percent) = self.percent {
            said.push(format!("{percent}% charged"));
        }
        if let Some(seconds) = self.seconds_left {
            said.push(format!("{} left", duration(seconds)));
        }
        if let Some(load) = self.load {
            said.push(format!("load {load}%"));
        }
        if !said.is_empty() {
            let _ = write!(line, "\n  {}", said.join(", "));
        }
        if self.replace == Some(true) {
            line.push_str("\n  its battery should be replaced");
        }
        if self.kind == "ups" {
            let at = match (self.low_percent, self.low_seconds) {
                (Some(percent), Some(seconds)) => {
                    format!("{percent}% or {} left", duration(seconds))
                }
                (Some(percent), None) => format!("{percent}%"),
                (None, Some(seconds)) => format!("{} left", duration(seconds)),
                (None, None) => "the UPS's low battery".to_string(),
            };
            match self.shuts_down {
                Some(false) => {
                    let _ = write!(line, "\n  low at {at}; ups=show keeps the machine on");
                }
                _ => {
                    let _ =
                        write!(line, "\n  shuts the machine down cleanly on its battery at {at}");
                }
            }
        }
        line
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A power_supply directory with `supplies`, each a name and its files.
    fn root(name: &str, supplies: &[(&str, &[(&str, &str)])]) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("hardware-power-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (supply, files) in supplies {
            let dir = root.join(supply);
            fs::create_dir_all(&dir).unwrap();
            for (file, value) in *files {
                fs::write(dir.join(file), format!("{value}\n")).unwrap();
            }
        }
        root
    }

    const UPS: &[(&str, &str)] = &[
        ("type", "UPS"),
        ("status", "Charging"),
        ("online", "1"),
        ("capacity", "87"),
        ("time_to_empty_now", "1500"),
        ("load", "35"),
        ("capacity_alert_min", "10"),
        ("time_to_empty_alert_min", "120"),
        ("shuts_down", "1"),
        ("health", "Good"),
        ("model_name", "CP1500PFCLCD"),
        ("manufacturer", "CPS"),
    ];

    #[test]
    fn a_ups_comes_first_with_its_charge_and_when_it_shuts_down() {
        let dir = root("ups", &[("AC", &[("type", "Mains"), ("online", "1")]), ("ups0", UPS)]);
        let power = gather(&dir);
        let names: Vec<_> = power.supplies.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["ups0", "AC"]);
        let ups = &power.supplies[0];
        assert_eq!(
            (ups.kind, ups.percent, ups.seconds_left, ups.load),
            ("ups", Some(87), Some(1500), Some(35))
        );
        assert_eq!(
            (ups.online, ups.replace, ups.shuts_down),
            (Some(true), Some(false), Some(true))
        );
        assert_eq!(
            power.text(),
            "ups0: UPS, CP1500PFCLCD by CPS\n  on the mains, charging, 87% charged, 25 min left, \
             load 35%\n  shuts the machine down cleanly on its battery at 10% or 2 min left\n\
             AC: AC adapter\n  powering the machine\n"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_ups_on_its_battery_says_the_mains_is_gone() {
        let on_battery: Vec<(&str, &str)> = UPS
            .iter()
            .map(|&(file, value)| match file {
                "online" => (file, "0"),
                "status" => (file, "Discharging"),
                "health" => (file, "Dead"),
                "shuts_down" => (file, "0"),
                _ => (file, value),
            })
            .collect();
        let dir = root("battery", &[("ups0", &on_battery)]);
        let text = gather(&dir).text();
        assert!(text.contains("on its battery: the mains is gone, 87% charged"), "{text}");
        assert!(text.contains("its battery should be replaced"), "{text}");
        assert!(text.contains("low at 10% or 2 min left; ups=show keeps the machine on"), "{text}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn values_a_supply_does_not_say_are_null() {
        let dir =
            root("bare", &[("BAT0", &[("type", "Battery")]), ("odd", &[("type", "Wireless")])]);
        let power = gather(&dir);
        assert_eq!(power.supplies.len(), 1);
        assert_eq!((power.supplies[0].percent, power.supplies[0].status.as_deref()), (None, None));
        assert_eq!(gather(&dir.join("none")).text(), "No battery, adapter or UPS.\n");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn durations_read_as_people_say_them() {
        assert_eq!(duration(90), "90 s");
        assert_eq!(duration(1500), "25 min");
        assert_eq!(duration(3900), "1 h 05 min");
    }
}
