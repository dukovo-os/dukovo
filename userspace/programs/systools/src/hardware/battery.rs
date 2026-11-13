// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The battery area of `hardware check`: how much charge each battery
//! still holds when full, against what it held new. The kernel shows
//! them as Linux's ACPI drivers do in `/kernel/class/power_supply`:
//! `energy_full` and `energy_full_design` in µWh, or for a battery that
//! counts in amperes `charge_full` and `charge_full_design` in µAh. A
//! battery under 80% of its design capacity is worn out. Most servers
//! and desktops have none, which is skipped.

use std::fs;
use std::path::Path;

use super::check::{Area, Check, Problem};

/// Where the kernel shows the power supplies.
pub(crate) const POWER_SUPPLY: &str = "/kernel/class/power_supply";

/// Under this share of its design capacity a battery is worn out.
const WORN_PERCENT: u64 = 80;

/// What a battery says of its capacity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Battery {
    pub(crate) name: String,
    pub(crate) model: Option<String>,
    /// What it holds full now, and what it held new, in the same unit.
    pub(crate) full: Option<u64>,
    pub(crate) design: Option<u64>,
    pub(crate) cycles: Option<u64>,
}

/// The file `file` of `dir`; a value the battery does not know fails to
/// read, as with Linux's.
fn text(dir: &Path, file: &str) -> Option<String> {
    Some(fs::read_to_string(dir.join(file)).ok()?.trim().to_string())
}

fn number(dir: &Path, file: &str) -> Option<u64> {
    text(dir, file)?.parse().ok()
}

/// The batteries in `root`, by name.
pub(crate) fn batteries(root: &Path) -> Vec<Battery> {
    let Ok(entries) = fs::read_dir(root) else { return Vec::new() };
    let mut dirs: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
    dirs.sort();
    dirs.iter()
        .filter(|dir| text(dir, "type").as_deref() == Some("Battery"))
        .map(|dir| {
            let pair = |full: &str, design: &str| (number(dir, full), number(dir, design));
            let (full, design) = match pair("energy_full", "energy_full_design") {
                (Some(full), Some(design)) => (Some(full), Some(design)),
                _ => pair("charge_full", "charge_full_design"),
            };
            Battery {
                name: dir
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                model: text(dir, "model_name").filter(|model| !model.is_empty()),
                full,
                design,
                cycles: number(dir, "cycle_count").filter(|cycles| *cycles > 0),
            }
        })
        .collect()
}

/// The check of one battery.
pub(crate) fn check(battery: &Battery) -> Check {
    let (Some(full), Some(design)) = (battery.full, battery.design.filter(|design| *design > 0))
    else {
        let why = "it does not say its capacity, as with an empty bay";
        return Check::skipped(Area::Battery, &battery.name, why.into());
    };
    let percent = full * 100 / design;
    let mut detail = format!("holds {percent}% of its design capacity");
    if let Some(cycles) = battery.cycles {
        detail.push_str(&format!(", after {cycles} charge cycles"));
    }
    if let Some(model) = &battery.model {
        detail = format!("{model}: {detail}");
    }
    if percent < WORN_PERCENT {
        return Check::warning(Area::Battery, &battery.name, Problem::BatteryWorn, detail);
    }
    Check::passed(Area::Battery, &battery.name, detail)
}

/// The checks of the area: one for each battery in `root`.
pub(crate) fn run(root: &Path) -> Vec<Check> {
    let found = batteries(root);
    if found.is_empty() {
        return vec![Check::skipped(
            Area::Battery,
            "battery",
            "this machine has no battery".into(),
        )];
    }
    found.iter().map(check).collect()
}

#[cfg(test)]
mod tests {
    use super::super::check::Outcome;
    use super::*;

    fn battery(full: u64, design: u64) -> Battery {
        Battery {
            name: "BAT0".into(),
            model: Some("DELL 7FHHV".into()),
            full: Some(full),
            design: Some(design),
            cycles: Some(213),
        }
    }

    #[test]
    fn a_battery_that_holds_most_of_its_capacity_passes() {
        let check = check(&battery(36_000_000, 40_000_000));
        assert_eq!(check.outcome, Outcome::Passed);
        assert_eq!(check.name, "BAT0");
        assert_eq!(
            check.detail,
            "DELL 7FHHV: holds 90% of its design capacity, after 213 charge cycles"
        );
    }

    #[test]
    fn a_battery_under_80_percent_is_worn() {
        assert_eq!(check(&battery(32_000_000, 40_000_000)).outcome, Outcome::Passed);
        let worn = check(&battery(31_999_999, 40_000_000));
        assert_eq!((worn.outcome, worn.id.as_deref()), (Outcome::Warning, Some("HW-141")));
        assert!(worn.detail.contains("holds 79% of its design capacity"), "{}", worn.detail);
    }

    #[test]
    fn a_battery_that_says_nothing_of_its_capacity_is_skipped() {
        let empty = Battery { full: None, ..battery(0, 1) };
        assert_eq!(check(&empty).outcome, Outcome::Skipped);
        let nameless = Battery { design: Some(0), ..battery(5, 1) };
        assert_eq!(check(&nameless).outcome, Outcome::Skipped);
    }

    #[test]
    fn batteries_are_read_from_the_files_of_the_power_supplies() {
        let root = std::env::temp_dir().join(format!("hardware-power-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let write = |dir: &str, files: &[(&str, &str)]| {
            fs::create_dir_all(root.join(dir)).unwrap();
            for (file, text) in files {
                fs::write(root.join(dir).join(file), format!("{text}\n")).unwrap();
            }
        };
        write("AC", &[("type", "Mains"), ("online", "1")]);
        write(
            "BAT0",
            &[("type", "Battery"), ("energy_full", "30000000"), ("energy_full_design", "40000000")],
        );
        write(
            "BAT1",
            &[
                ("type", "Battery"),
                ("charge_full", "2000000"),
                ("charge_full_design", "2500000"),
                ("cycle_count", "0"),
                ("model_name", "Pack"),
            ],
        );
        let found = batteries(&root);
        assert_eq!(found.len(), 2);
        assert_eq!(
            (found[0].name.as_str(), found[0].full, found[0].design),
            ("BAT0", Some(30_000_000), Some(40_000_000))
        );
        assert_eq!(
            (found[1].full, found[1].design, found[1].cycles),
            (Some(2_000_000), Some(2_500_000), None)
        );
        let checks = run(&root);
        assert_eq!(checks[0].outcome, Outcome::Warning);
        assert_eq!(checks[1].outcome, Outcome::Passed);
        assert_eq!(checks[1].detail, "Pack: holds 80% of its design capacity");
        let _ = fs::remove_dir_all(&root);
        let none = run(&root);
        assert_eq!((none.len(), none[0].outcome), (1, Outcome::Skipped));
    }
}
