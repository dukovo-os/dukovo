// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The sensors area of `hardware check`: the temperatures and fans of the
//! hardware monitors at rest, against the limits the monitors give. The
//! cpu area reads the same temperatures under load ([`temperatures`] and
//! [`heat_check`]), as sensors(1) does ([`crate::sensors`]), so that the
//! two agree on what a chip is called and what its limits are.
//!
//! A temperature is over its limit at its high limit (a warning) or its
//! critical limit (a failure); a limit the monitor does not give does not
//! count. A fan that reads 0 RPM with no minimum is not judged: the
//! motherboard's unused headers read so.

use std::path::Path;

use super::check::{Area, Check, Problem};
use crate::sensors::{self, Chip, Feature, Kind};

/// One temperature, with the limits its monitor gives.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Temperature {
    /// The chip, as sensors(1) names it.
    pub(crate) chip: String,
    pub(crate) label: String,
    pub(crate) celsius: f64,
    pub(crate) high: Option<f64>,
    pub(crate) critical: Option<f64>,
    /// The monitor's own alarms, which are set while a limit is passed.
    pub(crate) high_alarm: bool,
    pub(crate) critical_alarm: bool,
}

/// One fan.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Fan {
    pub(crate) label: String,
    pub(crate) rpm: f64,
    pub(crate) minimum: Option<f64>,
    pub(crate) alarm: bool,
    pub(crate) fault: bool,
}

fn flag(feature: &Feature, chip: &Chip, file: &str) -> bool {
    feature.value(chip, file) == Some(1.0)
}

/// `chip`'s temperatures; a sensor that reports a fault is left out,
/// since what it reads means nothing.
fn chip_temperatures(chip: &Chip) -> Vec<Temperature> {
    sensors::features(chip)
        .into_iter()
        .filter(|feature| feature.kind == Kind::Temp && !flag(feature, chip, "fault"))
        .filter_map(|feature| {
            Some(Temperature {
                chip: chip.name(),
                label: feature.label(chip),
                celsius: feature.value(chip, "input")?,
                high: feature.value(chip, "max"),
                critical: feature.value(chip, "crit"),
                high_alarm: flag(&feature, chip, "alarm"),
                critical_alarm: flag(&feature, chip, "crit_alarm"),
            })
        })
        .collect()
}

fn chip_fans(chip: &Chip) -> Vec<Fan> {
    sensors::features(chip)
        .into_iter()
        .filter(|feature| feature.kind == Kind::Fan)
        .filter_map(|feature| {
            Some(Fan {
                label: feature.label(chip),
                rpm: feature.value(chip, "input")?,
                minimum: feature.value(chip, "min"),
                alarm: flag(&feature, chip, "alarm"),
                fault: flag(&feature, chip, "fault"),
            })
        })
        .collect()
}

/// The temperatures of the chips in `hwmon` whose name `keep` accepts.
pub(crate) fn temperatures(hwmon: &Path, keep: &dyn Fn(&str) -> bool) -> Vec<Temperature> {
    sensors::chips(hwmon)
        .iter()
        .filter(|chip| keep(&chip.name()))
        .flat_map(chip_temperatures)
        .collect()
}

fn degrees(celsius: f64) -> String {
    format!("{celsius:.1} °C")
}

/// The check of temperatures `readings`: a failure if one is at its
/// critical limit, a warning if one is within `margin` degrees of its high
/// limit (0 for over it), else a pass that names the hottest. `problem` is
/// the one a failure or warning is.
pub(crate) fn heat_check(
    area: Area,
    name: &str,
    readings: &[Temperature],
    margin: f64,
    problem: Problem,
) -> Check {
    let critical =
        |t: &&Temperature| t.critical_alarm || t.critical.is_some_and(|c| t.celsius >= c);
    let warm =
        |t: &&Temperature| t.high_alarm || t.high.is_some_and(|high| t.celsius >= high - margin);
    if let Some(t) = readings.iter().find(critical) {
        let limit = t.critical.map_or(String::new(), |c| format!(" of {}", degrees(c)));
        let detail = format!("{} is at {}, its critical limit{limit}", t.label, degrees(t.celsius));
        return Check::failed(area, name, problem, detail);
    }
    if let Some(t) = readings.iter().find(warm) {
        let how = if margin > 0.0 { format!("within {margin} °C of") } else { "over".to_string() };
        let limit = t.high.map_or(String::new(), |high| format!(" of {}", degrees(high)));
        let detail =
            format!("{} is at {}, {how} its high limit{limit}", t.label, degrees(t.celsius));
        return Check::warning(area, name, problem, detail);
    }
    let Some(hottest) = readings.iter().max_by(|a, b| a.celsius.total_cmp(&b.celsius)) else {
        return Check::skipped(area, name, "the monitor gives no temperature".into());
    };
    let limit =
        hottest.high.map_or(String::new(), |high| format!(" (high limit {})", degrees(high)));
    let detail = format!("hottest is {} at {}{limit}", hottest.label, degrees(hottest.celsius));
    Check::passed(area, name, detail)
}

/// The check of the fans `fans` of one chip: a failure if one stopped, is
/// under its minimum or has its alarm or fault set.
pub(crate) fn fan_check(name: &str, fans: &[Fan]) -> Check {
    let area = Area::Sensors;
    let bad = |fan: &Fan| -> Option<String> {
        let minimum = fan.minimum.filter(|minimum| *minimum > 0.0);
        let rpm = fan.rpm;
        if fan.fault {
            Some(format!("{} reports a fault", fan.label))
        } else if rpm == 0.0 && (minimum.is_some() || fan.alarm) {
            Some(format!("{} is stopped (0 RPM)", fan.label))
        } else if let Some(minimum) = minimum.filter(|minimum| rpm < *minimum) {
            Some(format!("{} turns at {rpm:.0} RPM, under its minimum of {minimum:.0}", fan.label))
        } else if fan.alarm {
            Some(format!("{} has its alarm set at {rpm:.0} RPM", fan.label))
        } else {
            None
        }
    };
    let problems: Vec<String> = fans.iter().filter_map(bad).collect();
    if !problems.is_empty() {
        return Check::failed(area, name, Problem::FanStopped, problems.join("; "));
    }
    let shown: Vec<String> = fans
        .iter()
        .map(|fan| {
            if fan.rpm == 0.0 {
                format!("{} 0 RPM (no minimum is set, so it may be unused)", fan.label)
            } else {
                format!("{} {:.0} RPM", fan.label, fan.rpm)
            }
        })
        .collect();
    Check::passed(area, name, shown.join(", "))
}

/// The checks of the area: for each chip of `hwmon`, its temperatures at
/// rest and its fans.
pub(crate) fn run(hwmon: &Path) -> Vec<Check> {
    let mut checks = Vec::new();
    for chip in sensors::chips(hwmon) {
        let name = chip.name();
        let readings = chip_temperatures(&chip);
        if !readings.is_empty() {
            let label = format!("{name} temperatures");
            checks.push(heat_check(Area::Sensors, &label, &readings, 0.0, Problem::SensorHot));
        }
        let fans = chip_fans(&chip);
        if !fans.is_empty() {
            checks.push(fan_check(&format!("{name} fans"), &fans));
        }
    }
    if checks.is_empty() {
        let why = "the kernel shows no temperature or fan on this machine";
        checks.push(Check::skipped(Area::Sensors, "hardware monitors", why.into()));
    }
    checks
}

#[cfg(test)]
mod tests {
    use super::super::check::Outcome;
    use super::*;
    use std::fs;

    fn temperature(celsius: f64, high: Option<f64>, critical: Option<f64>) -> Temperature {
        Temperature {
            chip: "k10temp-pci-00c3".into(),
            label: "Tctl".into(),
            celsius,
            high,
            critical,
            high_alarm: false,
            critical_alarm: false,
        }
    }

    fn fan(rpm: f64, minimum: Option<f64>) -> Fan {
        Fan { label: "fan1".into(), rpm, minimum, alarm: false, fault: false }
    }

    fn at_rest(readings: &[Temperature]) -> Check {
        heat_check(Area::Sensors, "k10temp temperatures", readings, 0.0, Problem::SensorHot)
    }

    #[test]
    fn a_temperature_under_its_limits_passes_and_names_the_hottest() {
        let readings = [temperature(45.0, Some(90.0), Some(100.0)), temperature(61.5, None, None)];
        let check = at_rest(&readings);
        assert_eq!(check.outcome, Outcome::Passed);
        assert_eq!(check.detail, "hottest is Tctl at 61.5 °C");
        let check = at_rest(&readings[..1]);
        assert_eq!(check.detail, "hottest is Tctl at 45.0 °C (high limit 90.0 °C)");
    }

    #[test]
    fn a_temperature_over_its_high_limit_warns_and_at_its_critical_limit_fails() {
        let check = at_rest(&[temperature(91.0, Some(90.0), Some(100.0))]);
        assert_eq!((check.outcome, check.id.as_deref()), (Outcome::Warning, Some("HW-131")));
        assert_eq!(check.detail, "Tctl is at 91.0 °C, over its high limit of 90.0 °C");
        let check = at_rest(&[temperature(100.0, Some(90.0), Some(100.0))]);
        assert_eq!((check.outcome, check.id.as_deref()), (Outcome::Failed, Some("HW-131")));
        assert_eq!(check.detail, "Tctl is at 100.0 °C, its critical limit of 100.0 °C");
    }

    #[test]
    fn a_limit_the_monitor_does_not_give_does_not_count() {
        assert_eq!(at_rest(&[temperature(120.0, None, None)]).outcome, Outcome::Passed);
    }

    #[test]
    fn the_monitors_own_alarms_count_as_passing_a_limit() {
        let mut high = temperature(60.0, Some(90.0), None);
        high.high_alarm = true;
        assert_eq!(at_rest(&[high]).outcome, Outcome::Warning);
        let mut critical = temperature(60.0, Some(90.0), None);
        critical.critical_alarm = true;
        assert_eq!(at_rest(&[critical]).outcome, Outcome::Failed);
    }

    #[test]
    fn under_load_a_temperature_near_its_high_limit_warns() {
        let load = |celsius| {
            heat_check(
                Area::Cpu,
                "temperature",
                &[temperature(celsius, Some(90.0), Some(100.0))],
                5.0,
                Problem::CpuHot,
            )
        };
        assert_eq!(load(84.9).outcome, Outcome::Passed);
        let check = load(85.0);
        assert_eq!((check.outcome, check.id.as_deref()), (Outcome::Warning, Some("HW-123")));
        assert_eq!(check.detail, "Tctl is at 85.0 °C, within 5 °C of its high limit of 90.0 °C");
        assert_eq!(load(100.0).outcome, Outcome::Failed);
    }

    #[test]
    fn no_temperature_is_skipped() {
        assert_eq!(at_rest(&[]).outcome, Outcome::Skipped);
    }

    #[test]
    fn a_fan_that_turns_passes_and_one_with_no_minimum_at_zero_is_not_judged() {
        let check = fan_check("chip fans", &[fan(1200.0, Some(300.0)), fan(0.0, None)]);
        assert_eq!(check.outcome, Outcome::Passed);
        assert_eq!(
            check.detail,
            "fan1 1200 RPM, fan1 0 RPM (no minimum is set, so it may be unused)"
        );
        // A minimum of 0 is no minimum.
        assert_eq!(fan_check("chip fans", &[fan(0.0, Some(0.0))]).outcome, Outcome::Passed);
    }

    #[test]
    fn a_fan_that_stopped_or_turns_too_slowly_fails() {
        let check = fan_check("chip fans", &[fan(0.0, Some(300.0))]);
        assert_eq!((check.outcome, check.id.as_deref()), (Outcome::Failed, Some("HW-132")));
        assert_eq!(check.detail, "fan1 is stopped (0 RPM)");
        let check = fan_check("chip fans", &[fan(120.0, Some(300.0))]);
        assert_eq!(check.detail, "fan1 turns at 120 RPM, under its minimum of 300");
        let alarmed = Fan { alarm: true, ..fan(900.0, None) };
        assert_eq!(fan_check("chip fans", &[alarmed]).outcome, Outcome::Failed);
        let faulty = Fan { fault: true, ..fan(900.0, None) };
        assert_eq!(fan_check("chip fans", &[faulty]).detail, "fan1 reports a fault");
    }

    #[test]
    fn the_checks_come_from_the_files_of_each_chip() {
        let hwmon = std::env::temp_dir().join(format!("hardware-hwmon-{}", std::process::id()));
        let _ = fs::remove_dir_all(&hwmon);
        let chip = hwmon.join("hwmon0");
        fs::create_dir_all(&chip).unwrap();
        for (file, text) in [
            ("name", "k10temp"),
            ("temp1_input", "45000"),
            ("temp1_label", "Tctl"),
            ("temp1_max", "90000"),
            ("temp1_crit", "100000"),
            ("fan1_input", "0"),
            ("fan1_min", "300"),
        ] {
            fs::write(chip.join(file), format!("{text}\n")).unwrap();
        }
        let checks = run(&hwmon);
        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].name, "k10temp-pci-00c3 temperatures");
        assert_eq!(checks[0].outcome, Outcome::Passed);
        assert_eq!(checks[1].name, "k10temp-pci-00c3 fans");
        assert_eq!(checks[1].outcome, Outcome::Failed);
        let readings = temperatures(&hwmon, &|name| name.starts_with("k10temp-"));
        assert_eq!(readings.len(), 1);
        assert!(temperatures(&hwmon, &|name| name.starts_with("coretemp-")).is_empty());
        let _ = fs::remove_dir_all(&hwmon);
        let none = run(&hwmon);
        assert_eq!(none.len(), 1);
        assert_eq!(none[0].outcome, Outcome::Skipped);
    }
}
