// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What bmc(8) says: one view for each command, built from what the
//! `ipmi` module read, with the text it prints and the JSON that
//! `--json` prints (ADR 0541; the members are in bmc(8)).

use std::net::Ipv4Addr;

use serde::Serialize;

use crate::ipmi::{self, Entry, Fru, Lan, Reading, Sensor};
use crate::ipmi::{DeviceId, Identify};
use crate::{json, util};

/// A command's answer.
pub(super) enum Report {
    Summary(Summary),
    Sensors(SensorList),
    Events(EventList),
    Fru(Box<FruView>),
    Identify(IdentifyView),
    Lan(LanView),
}

impl Report {
    /// The text of the answer, ending in a newline.
    pub(super) fn text(&self) -> String {
        match self {
            Report::Summary(view) => view.text(),
            Report::Sensors(view) => view.text(),
            Report::Events(view) => view.text(),
            Report::Fru(view) => view.text(),
            Report::Identify(view) => view.text(),
            Report::Lan(view) => view.text(),
        }
    }

    /// The JSON document, without its newline.
    pub(super) fn json(&self) -> Result<String, util::Fail> {
        match self {
            Report::Summary(view) => json::line(view),
            Report::Sensors(view) => json::line(view),
            Report::Events(view) => json::line(view),
            Report::Fru(view) => json::line(view),
            Report::Identify(view) => json::line(view),
            Report::Lan(view) => json::line(view),
        }
    }
}

/// Lines of `label  value`, the values in a column.
fn rows(rows: &[(&str, String)]) -> String {
    let width = rows.iter().map(|(label, _)| label.len()).max().unwrap_or(0);
    let mut text = String::new();
    for (label, value) in rows {
        text.push_str(&format!("{label:<width$}  {value}\n"));
    }
    text
}

/// `value` in at most three decimals, without trailing zeros.
fn number(value: f64) -> String {
    let text = format!("{value:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A value rounded to the three decimals a reading has, for the JSON.
fn rounded(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// `value` with its unit.
fn with_unit(value: f64, unit: &str) -> String {
    format!("{} {unit}", number(value)).trim().to_string()
}

#[derive(Serialize)]
pub(super) struct Summary {
    pub manufacturer_id: u32,
    pub manufacturer: String,
    pub product_id: u16,
    pub device_id: u8,
    pub firmware: String,
    pub ipmi_version: String,
    pub functions: Vec<&'static str>,
    pub guid: Option<String>,
    pub lan: Option<LanView>,
}

impl Summary {
    pub(super) fn new(id: &DeviceId, guid: Option<String>, lan: Option<&Lan>) -> Summary {
        Summary {
            manufacturer_id: id.manufacturer,
            manufacturer: id.manufacturer_name(),
            product_id: id.product,
            device_id: id.device_id,
            firmware: id.firmware.clone(),
            ipmi_version: id.ipmi_version.clone(),
            functions: id.functions.clone(),
            guid,
            lan: lan.map(LanView::new),
        }
    }

    fn text(&self) -> String {
        let lan = match &self.lan {
            Some(lan) => lan.summary(),
            None => "none".to_string(),
        };
        rows(&[
            ("Maker", format!("{} ({})", self.manufacturer, self.manufacturer_id)),
            ("Product", format!("0x{:04X}", self.product_id)),
            ("Firmware", self.firmware.clone()),
            ("IPMI", self.ipmi_version.clone()),
            ("GUID", self.guid.clone().unwrap_or_else(|| "none".into())),
            ("Functions", self.functions.join(", ")),
            ("LAN", lan),
        ])
    }
}

#[derive(Serialize)]
pub(super) struct LanView {
    pub channel: u8,
    pub source: String,
    pub address: Option<String>,
    pub mask: Option<String>,
    pub prefix: Option<u8>,
    pub gateway: Option<String>,
    pub mac: Option<String>,
    pub vlan: Option<u16>,
}

/// The prefix length of a netmask whose ones are all at the start.
pub(super) fn prefix_of(mask: Ipv4Addr) -> Option<u8> {
    let bits = u32::from(mask);
    let ones = bits.leading_ones();
    (ones + bits.trailing_zeros() == 32).then(|| u8::try_from(ones).unwrap_or(32))
}

fn mac_text(mac: [u8; 6]) -> String {
    mac.iter().map(|byte| format!("{byte:02x}")).collect::<Vec<_>>().join(":")
}

impl LanView {
    pub(super) fn new(lan: &Lan) -> LanView {
        LanView {
            channel: lan.channel,
            source: lan.source.name(),
            address: lan.address.map(|address| address.to_string()),
            mask: lan.mask.map(|mask| mask.to_string()),
            prefix: lan.mask.and_then(prefix_of),
            gateway: lan.gateway.map(|gateway| gateway.to_string()),
            mac: lan.mac.map(mac_text),
            vlan: lan.vlan,
        }
    }

    /// The address with its prefix and source, on one line.
    fn summary(&self) -> String {
        let Some(address) = &self.address else {
            return format!("no address ({})", self.source);
        };
        let prefix = self.prefix.map_or(String::new(), |prefix| format!("/{prefix}"));
        let mac = self.mac.as_ref().map_or(String::new(), |mac| format!(", MAC {mac}"));
        format!("{address}{prefix} ({}){mac}", self.source)
    }

    fn text(&self) -> String {
        let none = || "none".to_string();
        let mask = match (&self.mask, self.prefix) {
            (Some(mask), Some(prefix)) => format!("{mask} (/{prefix})"),
            (Some(mask), None) => mask.clone(),
            (None, _) => none(),
        };
        rows(&[
            ("Channel", self.channel.to_string()),
            ("Source", self.source.clone()),
            ("Address", self.address.clone().unwrap_or_else(none)),
            ("Netmask", mask),
            ("Gateway", self.gateway.clone().unwrap_or_else(none)),
            ("MAC", self.mac.clone().unwrap_or_else(none)),
            ("VLAN", self.vlan.map_or_else(none, |vlan| vlan.to_string())),
        ])
    }
}

#[derive(Serialize)]
pub(super) struct SensorList {
    pub sensors: Vec<SensorView>,
}

#[derive(Serialize)]
pub(super) struct SensorView {
    pub number: u8,
    pub name: String,
    #[serde(rename = "type")]
    pub sensor_type: String,
    pub entity: String,
    pub entity_instance: u8,
    pub unit: String,
    /// `ok`, `unavailable`, `scanning_disabled` or `not_reachable`.
    pub status: &'static str,
    pub value: Option<f64>,
    pub raw: Option<u8>,
    /// A threshold sensor's worst threshold crossed, `ok` for none.
    pub state: Option<&'static str>,
    /// A discrete sensor's states.
    pub asserted: Vec<String>,
    pub thresholds: Thresholds,
}

#[derive(Serialize, Default)]
pub(super) struct Thresholds {
    pub lower_non_recoverable: Option<f64>,
    pub lower_critical: Option<f64>,
    pub lower_non_critical: Option<f64>,
    pub upper_non_critical: Option<f64>,
    pub upper_critical: Option<f64>,
    pub upper_non_recoverable: Option<f64>,
}

impl SensorView {
    /// The view of `sensor` with `reading`, or with none for a sensor
    /// that this program cannot ask.
    pub(super) fn new(
        link: &mut dyn ipmi::Transport,
        sensor: &Sensor,
        reading: Option<&Reading>,
    ) -> Result<SensorView, ipmi::Error> {
        let mut convert = |raw: Option<u8>| -> Result<Option<f64>, ipmi::Error> {
            match raw {
                // A non-linear sensor's constants may be unavailable at a
                // threshold's reading; that threshold is not shown.
                Some(raw) => match sensor.value(link, raw) {
                    Ok(value) => Ok(value.map(rounded)),
                    Err(ipmi::Error::Completion(_)) => Ok(None),
                    Err(error) => Err(error),
                },
                None => Ok(None),
            }
        };
        let limits = &sensor.thresholds;
        let thresholds = Thresholds {
            lower_non_recoverable: convert(limits.lower_non_recoverable)?,
            lower_critical: convert(limits.lower_critical)?,
            lower_non_critical: convert(limits.lower_non_critical)?,
            upper_non_critical: convert(limits.upper_non_critical)?,
            upper_critical: convert(limits.upper_critical)?,
            upper_non_recoverable: convert(limits.upper_non_recoverable)?,
        };
        let mut view = SensorView {
            number: sensor.number,
            name: sensor.name.clone(),
            sensor_type: ipmi::sensor_type_name(sensor.sensor_type),
            entity: ipmi::entity_name(sensor.entity),
            entity_instance: sensor.entity_instance,
            unit: sensor.unit.text(),
            status: "ok",
            value: None,
            raw: None,
            state: None,
            asserted: Vec::new(),
            thresholds,
        };
        match reading {
            None => view.status = "not_reachable",
            Some(Reading::Unavailable) => view.status = "unavailable",
            Some(Reading::ScanningDisabled) => view.status = "scanning_disabled",
            Some(Reading::Threshold { raw, value, crossed }) => {
                view.raw = Some(*raw);
                view.value = value.map(rounded);
                view.state = Some(Reading::state(*crossed));
            }
            Some(Reading::Discrete { asserted }) => {
                view.asserted = Reading::state_names(sensor, *asserted);
            }
        }
        Ok(view)
    }

    /// The reading column: the value, the raw byte of a sensor with no
    /// conversion, or a dash.
    fn reading_text(&self) -> String {
        match (self.value, self.raw) {
            (Some(value), _) => with_unit(value, &self.unit),
            (None, Some(raw)) => format!("0x{raw:02X}"),
            (None, None) => "-".into(),
        }
    }

    fn state_text(&self) -> String {
        match self.status {
            "unavailable" => "unavailable".into(),
            "scanning_disabled" => "scanning disabled".into(),
            "not_reachable" => "not reachable".into(),
            _ => match self.state {
                Some(state) => state.to_string(),
                None if self.asserted.is_empty() => "no state set".into(),
                None => self.asserted.join(", "),
            },
        }
    }
}

impl SensorList {
    fn text(&self) -> String {
        if self.sensors.is_empty() {
            return "The BMC has no sensors.\n".into();
        }
        let table: Vec<[String; 4]> = self
            .sensors
            .iter()
            .map(|sensor| {
                [
                    sensor.name.clone(),
                    sensor.sensor_type.clone(),
                    sensor.reading_text(),
                    sensor.state_text(),
                ]
            })
            .collect();
        columns(&["NAME", "TYPE", "READING", "STATE"], &table)
    }
}

/// A table with `heads`, each column as wide as its widest cell and the
/// last one not padded.
fn columns<const N: usize>(heads: &[&str; N], table: &[[String; N]]) -> String {
    let mut widths = heads.map(str::len);
    for row in table {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(util::width(cell));
        }
    }
    let line = |cells: Vec<&str>| {
        let mut text = String::new();
        for (index, cell) in cells.iter().enumerate() {
            if index + 1 == N {
                text.push_str(cell);
            } else {
                let pad = widths[index].saturating_sub(util::width(cell));
                text.push_str(cell);
                text.push_str(&" ".repeat(pad + 2));
            }
        }
        text.push('\n');
        text
    };
    let mut text = line(heads.to_vec());
    for row in table {
        text.push_str(&line(row.iter().map(String::as_str).collect()));
    }
    text
}

#[derive(Serialize)]
pub(super) struct EventList {
    pub events: Vec<EventView>,
    /// Whether the log was erased after it was read.
    pub cleared: bool,
}

#[derive(Serialize)]
pub(super) struct EventView {
    pub id: u16,
    /// The record type: 2 is a system event.
    pub record_type: u8,
    /// RFC 3339 in UTC, when the BMC's clock was set.
    #[serde(serialize_with = "serialize_option_time")]
    pub time: Option<i64>,
    /// Seconds since the BMC started, when its clock was not set.
    pub since_start: Option<u32>,
    pub sensor_type: Option<String>,
    pub sensor_number: Option<u8>,
    /// The sensor's name, when the repository has it.
    pub sensor: Option<String>,
    /// `asserted` or `deasserted`.
    pub direction: Option<&'static str>,
    pub event: String,
    pub severity: Option<&'static str>,
    /// For a threshold event, the reading that crossed the threshold and
    /// the threshold, in the sensor's unit when the repository tells how.
    pub reading: Option<f64>,
    pub threshold: Option<f64>,
    pub reading_raw: Option<u8>,
    pub threshold_raw: Option<u8>,
    #[serde(skip)]
    pub unit: String,
}

#[allow(clippy::ref_option)]
fn serialize_option_time<S: serde::Serializer>(
    seconds: &Option<i64>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match seconds {
        Some(seconds) => serializer.serialize_str(&json::time(*seconds)),
        None => serializer.serialize_none(),
    }
}

impl EventView {
    /// The view of `entry`, with the sensor of the repository it names
    /// when there is one.
    pub(super) fn new(
        link: &mut dyn ipmi::Transport,
        entry: &Entry,
        sensors: &[Sensor],
    ) -> Result<EventView, ipmi::Error> {
        let (time, since_start) = match entry.when {
            Some(ipmi::When::Clock(seconds)) => (Some(i64::from(seconds)), None),
            Some(ipmi::When::SinceStart(seconds)) => (None, Some(seconds)),
            None => (None, None),
        };
        let mut view = EventView {
            id: entry.id,
            record_type: entry.kind,
            time,
            since_start,
            sensor_type: None,
            sensor_number: None,
            sensor: None,
            direction: None,
            event: format!("OEM record, type 0x{:02X}", entry.kind),
            severity: None,
            reading: None,
            threshold: None,
            reading_raw: None,
            threshold_raw: None,
            unit: String::new(),
        };
        let Some(event) = &entry.event else { return Ok(view) };
        let (text, severity) = event.describe();
        let sensor = sensors.iter().find(|sensor| {
            sensor.number == event.sensor_number && sensor.sensor_type == event.sensor_type
        });
        view.sensor_type = Some(ipmi::sensor_type_name(event.sensor_type));
        view.sensor_number = Some(event.sensor_number);
        view.sensor = sensor.map(|sensor| sensor.name.clone());
        view.direction = Some(if event.asserted { "asserted" } else { "deasserted" });
        view.event = text;
        view.severity = Some(severity.name());
        if let Some((reading, threshold)) = event.threshold_readings() {
            view.reading_raw = Some(reading);
            view.threshold_raw = Some(threshold);
            if let Some(sensor) = sensor {
                view.unit = sensor.unit.text();
                view.reading = sensor.value(link, reading)?.map(rounded);
                view.threshold = sensor.value(link, threshold)?.map(rounded);
            }
        }
        Ok(view)
    }

    fn when_text(&self) -> String {
        match (self.time, self.since_start) {
            (Some(seconds), _) => util::local_time(seconds, "%Y-%m-%d %H:%M:%S"),
            (None, Some(seconds)) => format!("start +{}", util::clock_time(u64::from(seconds))),
            (None, None) => "-".into(),
        }
    }

    fn sensor_text(&self) -> String {
        match (&self.sensor, &self.sensor_type, self.sensor_number) {
            (Some(name), _, Some(number)) => format!("{name} (#{number})"),
            (None, Some(kind), Some(number)) => format!("{kind} #{number}"),
            _ => "-".into(),
        }
    }

    fn event_text(&self) -> String {
        let mut text = self.event.clone();
        if let Some(direction) = self.direction {
            text.push_str(&format!(", {direction}"));
        }
        if let Some(reading) = self.reading {
            text.push_str(&format!("; reading {}", with_unit(reading, &self.unit)));
        } else if let Some(raw) = self.reading_raw {
            text.push_str(&format!("; reading 0x{raw:02X}"));
        }
        if let Some(threshold) = self.threshold {
            text.push_str(&format!(", threshold {}", with_unit(threshold, &self.unit)));
        } else if let Some(raw) = self.threshold_raw {
            text.push_str(&format!(", threshold 0x{raw:02X}"));
        }
        text
    }
}

impl EventList {
    fn text(&self) -> String {
        let mut text = if self.events.is_empty() {
            "The event log is empty.\n".to_string()
        } else {
            let table: Vec<[String; 4]> = self
                .events
                .iter()
                .map(|event| {
                    [
                        event.id.to_string(),
                        event.when_text(),
                        event.sensor_text(),
                        event.event_text(),
                    ]
                })
                .collect();
            columns(&["ID", "TIME", "SENSOR", "EVENT"], &table)
        };
        if self.cleared {
            text.push_str("The event log is cleared.\n");
        }
        text
    }
}

#[derive(Serialize)]
pub(super) struct FruView {
    pub chassis: Option<ChassisView>,
    pub board: Option<BoardView>,
    pub product: Option<ProductView>,
    pub notes: Vec<String>,
}

#[derive(Serialize)]
pub(super) struct ChassisView {
    #[serde(rename = "type")]
    pub kind: String,
    pub part_number: String,
    pub serial_number: String,
    pub custom: Vec<String>,
}

#[derive(Serialize)]
pub(super) struct BoardView {
    #[serde(serialize_with = "serialize_option_time")]
    pub manufactured: Option<i64>,
    pub manufacturer: String,
    pub product_name: String,
    pub serial_number: String,
    pub part_number: String,
    pub file_id: String,
    pub custom: Vec<String>,
}

#[derive(Serialize)]
pub(super) struct ProductView {
    pub manufacturer: String,
    pub name: String,
    pub part_number: String,
    pub version: String,
    pub serial_number: String,
    pub asset_tag: String,
    pub file_id: String,
    pub custom: Vec<String>,
}

impl FruView {
    pub(super) fn new(fru: Fru) -> FruView {
        FruView {
            chassis: fru.chassis.map(|c| ChassisView {
                kind: c.kind,
                part_number: c.part_number,
                serial_number: c.serial_number,
                custom: c.custom,
            }),
            board: fru.board.map(|b| BoardView {
                manufactured: b.manufactured,
                manufacturer: b.manufacturer,
                product_name: b.product_name,
                serial_number: b.serial_number,
                part_number: b.part_number,
                file_id: b.file_id,
                custom: b.custom,
            }),
            product: fru.product.map(|p| ProductView {
                manufacturer: p.manufacturer,
                name: p.name,
                part_number: p.part_number,
                version: p.version,
                serial_number: p.serial_number,
                asset_tag: p.asset_tag,
                file_id: p.file_id,
                custom: p.custom,
            }),
            notes: fru.notes,
        }
    }

    fn text(&self) -> String {
        let mut sections = Vec::new();
        if let Some(chassis) = &self.chassis {
            let mut lines = vec![
                ("Type", chassis.kind.clone()),
                ("Part number", chassis.part_number.clone()),
                ("Serial number", chassis.serial_number.clone()),
            ];
            lines.extend(chassis.custom.iter().map(|field| ("Custom", field.clone())));
            sections.push(section("Chassis", &lines));
        }
        if let Some(board) = &self.board {
            let made = board.manufactured.map_or_else(String::new, |seconds| {
                util::time_text(seconds, "%Y-%m-%d %H:%M", true)
            });
            let mut lines = vec![
                ("Manufactured", made),
                ("Maker", board.manufacturer.clone()),
                ("Product", board.product_name.clone()),
                ("Serial number", board.serial_number.clone()),
                ("Part number", board.part_number.clone()),
                ("FRU file ID", board.file_id.clone()),
            ];
            lines.extend(board.custom.iter().map(|field| ("Custom", field.clone())));
            sections.push(section("Board", &lines));
        }
        if let Some(product) = &self.product {
            let mut lines = vec![
                ("Maker", product.manufacturer.clone()),
                ("Name", product.name.clone()),
                ("Part number", product.part_number.clone()),
                ("Version", product.version.clone()),
                ("Serial number", product.serial_number.clone()),
                ("Asset tag", product.asset_tag.clone()),
                ("FRU file ID", product.file_id.clone()),
            ];
            lines.extend(product.custom.iter().map(|field| ("Custom", field.clone())));
            sections.push(section("Product", &lines));
        }
        for note in &self.notes {
            sections.push(format!("Note: {note}\n"));
        }
        if sections.is_empty() {
            return "The FRU has no chassis, board or product area.\n".into();
        }
        sections.join("\n")
    }
}

/// A heading, and the lines of the section below it without the empty
/// values.
fn section(heading: &str, lines: &[(&str, String)]) -> String {
    let shown: Vec<(&str, String)> =
        lines.iter().filter(|(_, value)| !value.is_empty()).cloned().collect();
    let body = rows(&shown);
    let mut text = format!("{heading}\n");
    for line in body.lines() {
        text.push_str(&format!("  {line}\n"));
    }
    text
}

#[derive(Serialize)]
pub(super) struct IdentifyView {
    /// `seconds`, `on` or `off`.
    pub mode: &'static str,
    pub seconds: Option<u8>,
}

impl IdentifyView {
    pub(super) fn new(what: Identify) -> IdentifyView {
        match what {
            Identify::Seconds(seconds) => IdentifyView { mode: "seconds", seconds: Some(seconds) },
            Identify::ForceOn => IdentifyView { mode: "on", seconds: None },
            Identify::Off => IdentifyView { mode: "off", seconds: None },
        }
    }

    fn text(&self) -> String {
        match (self.mode, self.seconds) {
            ("seconds", Some(seconds)) => {
                let plural = if seconds == 1 { "" } else { "s" };
                format!("The identify light is on for {seconds} second{plural}.\n")
            }
            ("on", _) => "The identify light is on until it is turned off.\n".into(),
            _ => "The identify light is off.\n".into(),
        }
    }
}
