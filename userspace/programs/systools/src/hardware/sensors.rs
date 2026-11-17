// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What `hardware sensors` says: the temperatures and fans of the
//! hardware monitors, the firmware's thermal zones, and the disks'
//! temperatures, each with the name of the series metricsd(8) keeps its
//! history under, so that a page can join them with `metrics show`.
//!
//! The monitors are read as sensors(1) reads them ([`crate::sensors`]) and
//! the zones as metricsd reads them, so that the two agree on what each is
//! called. A disk is never asked: what the diskhealth service last read of
//! it is in its live file, and a disk that sleeps says since when, so that
//! it is not woken to be looked at.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::diskhealth::{self, DiskKind, LimitsFrom, Live};
use crate::metricsd::sample::{self, DISK_CHIP};
use crate::sensors::{self, Chip, Feature, HWMON, Kind};
use crate::util;

/// Where the kernel lists the firmware's thermal zones.
const THERMAL: &str = "/kernel/class/thermal";

/// Where each source is, so that tests can read their own.
pub(crate) struct Sources {
    /// The hardware monitors.
    hwmon: PathBuf,
    /// The thermal zones.
    thermal: PathBuf,
    /// The live file of each disk.
    disks: PathBuf,
}

impl Sources {
    /// Where the system keeps them.
    pub(crate) fn system() -> Sources {
        Sources {
            hwmon: PathBuf::from(HWMON),
            thermal: PathBuf::from(THERMAL),
            disks: PathBuf::from(diskhealth::LIVE),
        }
    }
}

/// What `hardware sensors --json` says. On a machine with no sensor the
/// lists are empty.
#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct Sensors {
    /// The version of this document: 1.
    format: u32,
    sensors: Vec<Sensor>,
    zones: Vec<Zone>,
    disks: Vec<Disk>,
}

/// A temperature or a fan of a hardware monitor.
#[derive(Debug, PartialEq, Serialize)]
struct Sensor {
    /// The chip as sensors(1) names it: `k10temp-pci-00c3`.
    chip: String,
    label: String,
    /// `temp` or `fan`.
    kind: &'static str,
    /// What metricsd calls the temperature's series, `temp{chip/label}`;
    /// null for a fan, which has none.
    series: Option<String>,
    /// Degrees Celsius, or revolutions a minute; null when the sensor
    /// gives none.
    value: Option<f64>,
    /// The limits the sensor gives, null for those it does not: a
    /// temperature's high and critical, a fan's minimum.
    high: Option<f64>,
    crit: Option<f64>,
    min: Option<f64>,
    /// Whether the monitor's alarm is set, and whether the sensor says it
    /// is faulty.
    alarm: bool,
    fault: bool,
}

/// A thermal zone of the firmware's.
#[derive(Debug, PartialEq, Serialize)]
struct Zone {
    /// Its type, with its number when several have one type: `acpitz`.
    zone: String,
    /// `temp{zone/TYPE}`.
    series: String,
    /// Degrees Celsius.
    value: f64,
}

/// A disk, as the diskhealth service last read it.
#[derive(Debug, PartialEq, Serialize)]
struct Disk {
    disk: String,
    /// What the driver says it is; null where it says nothing.
    model: Option<String>,
    /// `hdd`, `ssd` or `nvme`; null for a file an older service wrote.
    kind: Option<DiskKind>,
    /// What it said at its last look, in degrees Celsius; null when it
    /// gives no temperature.
    temperature_c: Option<i64>,
    /// When that look was, in milliseconds since 1970.
    checked_ms: u64,
    /// Since when it sleeps, which means it is not read; null while it
    /// does not.
    asleep_ms: Option<u64>,
    /// The temperatures at which it is too hot; null where it gives no
    /// temperature.
    warning_c: Option<i64>,
    critical_c: Option<i64>,
    /// Whether they are the disk's own (`disk`) or the usual for its kind
    /// (`kind`).
    limits_from: Option<LimitsFrom>,
    /// `temp{disk/NAME}`.
    series: String,
}

/// The flag file `file` of `feature`.
fn flag(feature: &Feature, chip: &Chip, file: &str) -> bool {
    feature.value(chip, file).is_some_and(|value| value != 0.0)
}

/// The temperatures and fans of `chip`.
fn chip_sensors(chip: &Chip) -> Vec<Sensor> {
    let name = chip.name();
    sensors::features(chip)
        .into_iter()
        .map(|feature| {
            let label = feature.label(chip);
            let temperature = feature.kind == Kind::Temp;
            let fault = flag(&feature, chip, "fault");
            Sensor {
                kind: if temperature { "temp" } else { "fan" },
                series: temperature.then(|| sample::temperature_series(&name, &label)),
                chip: name.clone(),
                label,
                value: feature.value(chip, "input").filter(|_| !fault),
                high: feature.value(chip, "max"),
                crit: feature.value(chip, "crit"),
                min: feature.value(chip, "min"),
                alarm: flag(&feature, chip, "alarm") || flag(&feature, chip, "crit_alarm"),
                fault,
            }
        })
        .collect()
}

/// The disks that have a live file in `dir`, by name.
fn disks(dir: &Path) -> Vec<Disk> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut found: Vec<Disk> = entries
        .flatten()
        .filter(|entry| Path::new(&entry.file_name()).extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| {
            let text = fs::read_to_string(entry.path()).ok()?;
            serde_json::from_str::<Live>(&text).ok()
        })
        .map(disk_of)
        .collect();
    found.sort_by(|a, b| a.disk.cmp(&b.disk));
    found
}

/// What the live file `live` says, as a row.
fn disk_of(live: Live) -> Disk {
    let name = live.summary.disk;
    Disk {
        series: sample::temperature_series(DISK_CHIP, &name),
        disk: name,
        model: live.summary.model,
        kind: live.kind,
        temperature_c: live.summary.temperature_c,
        checked_ms: live.checked_ms,
        asleep_ms: live.asleep_ms,
        warning_c: live.limits.map(|limits| limits.warning_c),
        critical_c: live.limits.map(|limits| limits.critical_c),
        limits_from: live.limits.map(|limits| limits.from),
    }
}

/// What the machine's sensors say, read from `sources`.
pub(crate) fn gather(sources: &Sources) -> Sensors {
    Sensors {
        format: 1,
        sensors: sensors::chips(&sources.hwmon).iter().flat_map(chip_sensors).collect(),
        zones: sample::read_zones(&sources.thermal)
            .into_iter()
            .map(|zone| Zone {
                zone: zone.instance.strip_prefix("zone/").unwrap_or(&zone.instance).to_string(),
                series: format!("temp{{{}}}", zone.instance),
                value: zone.celsius,
            })
            .collect(),
        disks: disks(&sources.disks),
    }
}

impl Sensors {
    /// The JSON the command prints, on one line.
    ///
    /// # Errors
    ///
    /// The words of the writer's refusal, which a document of these types
    /// does not give.
    pub(crate) fn json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// The text a terminal shows, as it is at `now_ms`: a line for each
    /// temperature and fan, each zone and each disk.
    pub(crate) fn text(&self, now_ms: u64) -> String {
        let mut rows: Vec<(String, String)> = Vec::new();
        for sensor in &self.sensors {
            rows.push((format!("{}/{}", sensor.chip, sensor.label), sensor_words(sensor)));
        }
        for zone in &self.zones {
            rows.push((format!("zone/{}", zone.zone), format!("{:.1} °C", zone.value)));
        }
        for disk in &self.disks {
            rows.push((disk.disk.clone(), disk_words(disk, now_ms)));
        }
        if rows.is_empty() {
            return "No temperature or fan speed can be read on this machine.\n".to_string();
        }
        let width = rows.iter().map(|(name, _)| name.chars().count()).max().unwrap_or(0);
        rows.iter().fold(String::new(), |mut text, (name, words)| {
            let _ = writeln!(text, "{name:<width$}  {words}");
            text
        })
    }
}

/// What a sensor reads and its limits, in a line.
fn sensor_words(sensor: &Sensor) -> String {
    let unit = if sensor.kind == "fan" { "RPM" } else { "°C" };
    let mut words = match (sensor.fault, sensor.value) {
        (true, _) => "faulty".to_string(),
        (_, Some(value)) if sensor.kind == "fan" => format!("{value:.0} {unit}"),
        (_, Some(value)) => format!("{value:.1} {unit}"),
        (_, None) => "no reading".to_string(),
    };
    let limits: Vec<String> =
        [("low", sensor.min), ("high", sensor.high), ("critical", sensor.crit)]
            .into_iter()
            .filter_map(|(name, limit)| Some(format!("{name} {limit:.1} {unit}", limit = limit?)))
            .collect();
    if !limits.is_empty() {
        words.push_str(&format!(" ({})", limits.join(", ")));
    }
    if sensor.alarm {
        words.push_str(" ALARM");
    }
    words
}

/// What a disk reads and where its limits are, or that it sleeps.
fn disk_words(disk: &Disk, now_ms: u64) -> String {
    if let Some(since) = disk.asleep_ms {
        let at = i64::try_from(since / 1000).unwrap_or(0);
        let recent = now_ms.saturating_sub(since) < 20 * 3_600_000;
        let when = util::time_text(at, if recent { "%R" } else { "%F %R" }, false);
        return format!("asleep since {when}, not read");
    }
    let Some(celsius) = disk.temperature_c else { return "gives no temperature".to_string() };
    let mut words = format!("{celsius} °C");
    if let (Some(warning), Some(critical)) = (disk.warning_c, disk.critical_c) {
        let whose = match (disk.limits_from, disk.kind) {
            (Some(LimitsFrom::Kind), Some(kind)) => format!("for {}", kind.words()),
            (Some(LimitsFrom::Kind), None) => "usual for its kind".to_string(),
            _ => "the disk's own".to_string(),
        };
        words.push_str(&format!("  warning {warning} °C, critical {critical} °C ({whose})"));
    }
    words
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    /// A folder with the machine's layout, which a test fills in.
    struct Machine {
        root: PathBuf,
    }

    impl Machine {
        fn new(call: &str) -> Machine {
            let root = std::env::temp_dir()
                .join(format!("hardware-sensors-{call}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Machine { root }
        }

        fn sources(&self) -> Sources {
            Sources {
                hwmon: self.root.join("hwmon"),
                thermal: self.root.join("thermal"),
                disks: self.root.join("disks"),
            }
        }

        fn put(&self, path: &str, text: &str) {
            let file = self.root.join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, text).unwrap();
        }

        /// An Intel package's temperature with its limits, a fan with a
        /// minimum, and a fan header with nothing on it.
        fn board(&self) {
            self.put("hwmon/hwmon0/name", "coretemp\n");
            self.put("hwmon/hwmon0/temp1_input", "41000\n");
            self.put("hwmon/hwmon0/temp1_label", "Package id 0\n");
            self.put("hwmon/hwmon0/temp1_max", "84000\n");
            self.put("hwmon/hwmon0/temp1_crit", "100000\n");
            self.put("hwmon/hwmon0/temp1_crit_alarm", "0\n");
            self.put("hwmon/hwmon1/name", "nct6798\n");
            self.put("hwmon/hwmon1/fan1_input", "1190\n");
            self.put("hwmon/hwmon1/fan1_min", "300\n");
            self.put("hwmon/hwmon1/fan1_alarm", "0\n");
            self.put("hwmon/hwmon1/fan2_input", "0\n");
            self.put("hwmon/hwmon1/temp2_input", "80000\n");
            self.put("hwmon/hwmon1/temp2_fault", "1\n");
            self.put("thermal/thermal_zone0/type", "acpitz\n");
            self.put("thermal/thermal_zone0/temp", "27800\n");
        }

        fn live(&self, disk: &str, text: &str) {
            self.put(&format!("disks/{disk}.json"), text);
        }
    }

    /// A live file of disk `disk` as the service writes it.
    fn live_file(disk: &str, extra: &Value) -> String {
        let mut file = json!({
            "disk": disk, "model": "WDC WD40EFRX-68N", "serial": "WD-1", "verdict": "passed",
            "reasons": [], "hours": 100, "temperature_c": 36, "moved": 0, "waiting": 0,
            "offline": 0, "media_errors": null, "used_percent": null,
            "checked_ms": 1_794_564_000_000u64, "rose": null, "kind": "hdd", "asleep_ms": null,
            "limits": {"warning_c": 50, "critical_c": 60, "from": "kind"},
        });
        for (key, value) in extra.as_object().unwrap() {
            file[key] = value.clone();
        }
        file.to_string()
    }

    #[test]
    fn a_machine_with_no_sensor_has_empty_lists() {
        let machine = Machine::new("none");
        let found = gather(&machine.sources());
        assert_eq!(
            serde_json::from_str::<Value>(&found.json().unwrap()).unwrap(),
            json!({"format": 1, "sensors": [], "zones": [], "disks": []})
        );
        assert_eq!(found.text(0), "No temperature or fan speed can be read on this machine.\n");
    }

    #[test]
    fn the_sensors_have_their_series_and_their_limits() {
        let machine = Machine::new("board");
        machine.board();
        let value: Value =
            serde_json::from_str(&gather(&machine.sources()).json().unwrap()).unwrap();
        let sensors = value["sensors"].as_array().unwrap();
        assert_eq!(sensors.len(), 4, "{sensors:?}");
        let temp = &sensors[0];
        assert_eq!(temp["chip"], json!("coretemp-isa-0000"));
        assert_eq!(temp["label"], json!("Package id 0"));
        assert_eq!(temp["kind"], json!("temp"));
        assert_eq!(temp["series"], json!("temp{coretemp-isa-0000/Package id 0}"));
        assert_eq!((temp["value"].as_f64(), temp["high"].as_f64()), (Some(41.0), Some(84.0)));
        assert_eq!((temp["crit"].as_f64(), temp["min"].clone()), (Some(100.0), Value::Null));
        assert_eq!((temp["alarm"].clone(), temp["fault"].clone()), (json!(false), json!(false)));
        // A fan has no series and a minimum.
        let fan = &sensors[1];
        assert_eq!((fan["kind"].clone(), fan["series"].clone()), (json!("fan"), Value::Null));
        assert_eq!((fan["value"].as_f64(), fan["min"].as_f64()), (Some(1190.0), Some(300.0)));
        assert_eq!((fan["high"].clone(), fan["crit"].clone()), (Value::Null, Value::Null));
        // A fan that reads 0 with no minimum is reported as it reads.
        assert_eq!(sensors[2]["value"].as_f64(), Some(0.0));
        assert_eq!(sensors[2]["min"], Value::Null);
        // A faulty sensor has no value and says why.
        let faulty = &sensors[3];
        assert_eq!((faulty["value"].clone(), faulty["fault"].clone()), (Value::Null, json!(true)));
        // The zone is what metricsd calls it.
        assert_eq!(
            value["zones"],
            json!([{"zone": "acpitz", "series": "temp{zone/acpitz}", "value": 27.8}])
        );
    }

    #[test]
    fn the_disks_are_read_from_their_live_files_and_never_asked() {
        let machine = Machine::new("disks");
        machine.live("sdb", &live_file("sdb", &json!({"asleep_ms": 1_794_560_000_000u64})));
        machine.live(
            "nvme0n1",
            &live_file(
                "nvme0n1",
                &json!({"model": "Samsung SSD 980", "kind": "nvme", "temperature_c": 44,
                    "limits": {"warning_c": 70, "critical_c": 85, "from": "disk"}}),
            ),
        );
        // A file that is no disk's, one half written, and one that
        // does not read.
        machine.put("disks/README", "kept");
        machine.put("disks/sda.json.new", "{");
        machine.live("sdc", "not json");
        let value: Value =
            serde_json::from_str(&gather(&machine.sources()).json().unwrap()).unwrap();
        let disks = value["disks"].as_array().unwrap();
        assert_eq!(disks.len(), 2);
        // In the order of their names.
        assert_eq!(
            disks[0],
            json!({
                "disk": "nvme0n1", "model": "Samsung SSD 980", "kind": "nvme",
                "temperature_c": 44, "checked_ms": 1_794_564_000_000u64, "asleep_ms": null,
                "warning_c": 70, "critical_c": 85, "limits_from": "disk",
                "series": "temp{disk/nvme0n1}",
            })
        );
        assert_eq!(disks[1]["disk"], json!("sdb"));
        assert_eq!(disks[1]["asleep_ms"], json!(1_794_560_000_000u64));
        assert_eq!(disks[1]["limits_from"], json!("kind"));
        assert_eq!(disks[1]["series"], json!("temp{disk/sdb}"));
    }

    #[test]
    fn a_disk_with_no_limits_has_null_ones_and_an_old_file_has_no_kind() {
        let machine = Machine::new("old");
        machine.live(
            "sda",
            &live_file("sda", &json!({"limits": null, "kind": null, "temperature_c": null})),
        );
        let value: Value =
            serde_json::from_str(&gather(&machine.sources()).json().unwrap()).unwrap();
        let disk = &value["disks"][0];
        for member in ["warning_c", "critical_c", "limits_from", "kind", "temperature_c"] {
            assert_eq!(disk[member], Value::Null, "{member}");
        }
        // A file from before the members were there.
        let mut old: Value = serde_json::from_str(&live_file("sdd", &json!({}))).unwrap();
        for member in ["kind", "asleep_ms", "limits"] {
            old.as_object_mut().unwrap().remove(member);
        }
        machine.live("sdd", &old.to_string());
        let value: Value =
            serde_json::from_str(&gather(&machine.sources()).json().unwrap()).unwrap();
        assert_eq!(value["disks"][1]["disk"], json!("sdd"));
        assert_eq!(value["disks"][1]["asleep_ms"], Value::Null);
        assert_eq!(value["disks"][1]["warning_c"], Value::Null);
    }

    #[test]
    fn the_text_has_a_line_for_each_sensor_zone_and_disk() {
        let machine = Machine::new("text");
        machine.board();
        machine.live("sda", &live_file("sda", &json!({})));
        machine.live(
            "sdb",
            &live_file("sdb", &json!({"asleep_ms": 1_794_564_000_000u64 - 2 * 3_600_000})),
        );
        machine.live(
            "nvme0n1",
            &live_file(
                "nvme0n1",
                &json!({"kind": "nvme", "temperature_c": 44,
                    "limits": {"warning_c": 70, "critical_c": 85, "from": "disk"}}),
            ),
        );
        let found = gather(&machine.sources());
        let text = found.text(1_794_564_000_000);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 8, "{text}");
        assert!(
            lines[0].starts_with("coretemp-isa-0000/Package id 0")
                && lines[0].ends_with("41.0 °C (high 84.0 °C, critical 100.0 °C)"),
            "{text}"
        );
        assert!(lines[1].ends_with("1190 RPM (low 300.0 RPM)"), "{text}");
        assert!(lines[3].ends_with("faulty"), "{text}");
        assert!(lines[4].starts_with("zone/acpitz") && lines[4].ends_with("27.8 °C"), "{text}");
        assert!(
            lines[5].starts_with("nvme0n1")
                && lines[5].ends_with("44 °C  warning 70 °C, critical 85 °C (the disk's own)"),
            "{text}"
        );
        assert!(
            lines[6].starts_with("sda")
                && lines[6].ends_with("36 °C  warning 50 °C, critical 60 °C (for a hard disk)"),
            "{text}"
        );
        assert!(lines[7].starts_with("sdb") && lines[7].contains("asleep since "), "{text}");
        assert!(lines[7].ends_with(", not read"), "{text}");
    }

    #[test]
    fn a_disk_that_gives_no_temperature_says_so() {
        let machine = Machine::new("notemp");
        machine.live(
            "sda",
            &live_file("sda", &json!({"temperature_c": null, "limits": null, "kind": "ssd"})),
        );
        let text = gather(&machine.sources()).text(1_794_564_000_000);
        assert!(text.trim_end().ends_with("gives no temperature"), "{text}");
    }
}
