// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The BMC's numeric sensors sorted by what they measure, for `hardware
//! sensors` and metricsd(8), which both show or keep a temperature, a
//! fan's speed, a voltage, a current or a power under the kind's own
//! name and do not want the discrete sensors of a power supply or a
//! drive slot.
//!
//! A sensor's kind comes from its unit first and from its sensor type
//! (table 42-3) when the unit says nothing, because BMCs file a power
//! sensor under "current" or "other units-based sensor". A temperature is
//! always given in degrees Celsius.

use super::sensor::{Reading, Sensor};
use super::{Result, Transport, read_sensor};

/// What a numeric sensor measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Temperature,
    Fan,
    Voltage,
    Current,
    Power,
}

/// Where a sensor stands, as the worst threshold it has crossed says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Ok,
    Warning,
    Critical,
    NonRecoverable,
    /// The BMC has no reading, or scanning of the sensor is off.
    Unavailable,
    /// The sensor belongs to another controller, which this program
    /// cannot ask.
    NotReachable,
}

impl Status {
    /// The word for the status, as the JSON has it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Warning => "warning",
            Status::Critical => "critical",
            Status::NonRecoverable => "non-recoverable",
            Status::Unavailable => "unavailable",
            Status::NotReachable => "not-reachable",
        }
    }

    /// The status of a threshold reading whose comparison bits are
    /// `crossed`: the worst of the three levels, on whichever side.
    fn of_crossed(crossed: u8) -> Status {
        if crossed & 0b100_100 != 0 {
            Status::NonRecoverable
        } else if crossed & 0b010_010 != 0 {
            Status::Critical
        } else if crossed & 0b001_001 != 0 {
            Status::Warning
        } else {
            Status::Ok
        }
    }
}

/// A numeric sensor of the BMC and what it reads.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Measured {
    /// The sensor's number, which tells two sensors of one name apart.
    pub number: u8,
    pub name: String,
    pub kind: Kind,
    /// The unit as written next to the value; `°C` for each temperature.
    pub unit: String,
    pub value: Option<f64>,
    pub status: Status,
}

impl Sensor {
    /// What the sensor measures, for a sensor with a numeric reading;
    /// `None` for a discrete one and for one whose record gives no
    /// conversion.
    pub(crate) fn kind(&self) -> Option<Kind> {
        if !self.is_threshold() || self.analog.is_none() {
            return None;
        }
        match (self.unit.base, self.sensor_type) {
            (1..=3, _) | (0, 0x01) => Some(Kind::Temperature),
            (4, _) | (0, 0x02) => Some(Kind::Voltage),
            (5, _) | (0, 0x03) => Some(Kind::Current),
            (6, _) => Some(Kind::Power),
            (18, _) | (0, 0x04) => Some(Kind::Fan),
            _ => None,
        }
    }
}

/// `value` in degrees Celsius, from the unit code of table 43-15: 1 is
/// Celsius, 2 Fahrenheit and 3 Kelvin.
fn celsius(value: f64, unit: u8) -> f64 {
    match unit {
        2 => (value - 32.0) / 1.8,
        3 => value - 273.15,
        _ => value,
    }
}

/// Reads the numeric sensors among `sensors`, in their order. A sensor
/// that another controller owns is there with no value and the status
/// [`Status::NotReachable`].
///
/// # Errors
///
/// If the transport fails, or a non-linear sensor's constants cannot be
/// had.
pub(crate) fn measure(link: &mut dyn Transport, sensors: &[Sensor]) -> Result<Vec<Measured>> {
    let mut measured = Vec::new();
    for sensor in sensors {
        let Some(kind) = sensor.kind() else { continue };
        let (value, status) = if sensor.readable_here() {
            match read_sensor(link, sensor)? {
                Reading::Threshold { value, crossed, .. } => (value, Status::of_crossed(crossed)),
                _ => (None, Status::Unavailable),
            }
        } else {
            (None, Status::NotReachable)
        };
        let (value, unit) = if kind == Kind::Temperature {
            (value.map(|reading| celsius(reading, sensor.unit.base)), "°C".to_string())
        } else {
            (value, sensor.unit.text())
        };
        measured.push(Measured {
            number: sensor.number,
            name: sensor.name.clone(),
            kind,
            unit,
            value,
            status,
        });
    }
    Ok(measured)
}

#[cfg(test)]
mod tests {
    use super::super::sdr::sensors;
    use super::super::sim::{self, Sim};
    use super::*;

    /// A full record of sensor `number` with the sensor type and base
    /// unit given, which `sim::full_record` makes a temperature in °C.
    fn record(id: u16, number: u8, name: &str, sensor_type: u8, unit: u8) -> Vec<u8> {
        let mut bytes = sim::full_record(id, number, name);
        bytes[12] = sensor_type;
        bytes[21] = unit;
        bytes
    }

    fn bmc() -> Sim {
        let mut bmc = Sim::new();
        bmc.sdr = vec![
            record(1, 1, "Inlet Temp", 0x01, 1),
            record(2, 2, "Fan1", 0x04, 18),
            record(3, 3, "12V", 0x02, 4),
            record(4, 4, "Current 1", 0x03, 5),
            // Filed under "other units-based", as a power sensor often is.
            record(5, 5, "Pwr Consumption", 0x0B, 6),
            sim::compact_record(6, 9, "PS Status", 1),
        ];
        bmc
    }

    #[test]
    fn a_sensor_is_sorted_by_its_unit_and_then_by_its_type() {
        let mut bmc = bmc();
        let found = sensors(&mut bmc).unwrap();
        let kinds: Vec<Option<Kind>> = found.iter().map(Sensor::kind).collect();
        assert_eq!(
            kinds,
            [
                Some(Kind::Temperature),
                Some(Kind::Fan),
                Some(Kind::Voltage),
                Some(Kind::Current),
                Some(Kind::Power),
                None
            ]
        );
        // No unit and a temperature type is a temperature; no unit and an
        // unknown type is no kind.
        let mut unitless = found[0].clone();
        unitless.unit.base = 0;
        assert_eq!(unitless.kind(), Some(Kind::Temperature));
        unitless.sensor_type = 0x0B;
        assert_eq!(unitless.kind(), None);
    }

    #[test]
    fn readings_come_with_their_value_unit_and_the_worst_threshold_crossed() {
        let mut bmc = bmc();
        let found = sensors(&mut bmc).unwrap();
        // Raw 100 is 30 degrees. The upper critical bit (4) and the upper
        // non-critical (3) are both set, and critical is the worse.
        bmc.readings.insert(1, vec![100, 0x40, 0b01_1000]);
        bmc.readings.insert(2, vec![100, 0x40, 0]);
        bmc.readings.insert(3, vec![100, 0x40, 0b00_0100]);
        bmc.readings.insert(4, vec![100, 0x60, 0]);
        let measured = measure(&mut bmc, &found).unwrap();
        assert_eq!(measured.len(), 5);
        assert_eq!(measured[0].name, "Inlet Temp");
        assert_eq!(measured[0].unit, "°C");
        assert!((measured[0].value.unwrap() - 30.0).abs() < 1e-9);
        assert_eq!(measured[0].status, Status::Critical);
        assert_eq!(measured[1].status, Status::Ok);
        assert_eq!(measured[1].kind, Kind::Fan);
        // Bit 2 is the lower non-recoverable threshold.
        assert_eq!(measured[2].status, Status::NonRecoverable);
        // The BMC says the reading is unavailable; a sensor it does not
        // know is the same.
        assert_eq!((measured[3].value, measured[3].status), (None, Status::Unavailable));
        assert_eq!((measured[4].value, measured[4].status), (None, Status::Unavailable));
    }

    #[test]
    fn a_sensor_of_another_controller_is_listed_and_not_asked() {
        let mut bmc = Sim::new();
        let mut other = record(1, 7, "PSU Temp", 0x01, 1);
        // The owner is the controller at 0x30, not the BMC.
        other[5] = 0x30;
        bmc.sdr = vec![other];
        let found = sensors(&mut bmc).unwrap();
        let measured = measure(&mut bmc, &found).unwrap();
        assert_eq!(measured.len(), 1);
        assert_eq!((measured[0].value, measured[0].status), (None, Status::NotReachable));
    }

    #[test]
    fn a_temperature_in_fahrenheit_or_kelvin_is_given_in_celsius() {
        assert!((celsius(212.0, 2) - 100.0).abs() < 1e-9);
        assert!((celsius(373.15, 3) - 100.0).abs() < 1e-9);
        assert!((celsius(30.0, 1) - 30.0).abs() < 1e-9);
        let mut bmc = Sim::new();
        bmc.sdr = vec![record(1, 1, "Board", 0x01, 2)];
        // Raw 100 is 30 in the sensor's own unit, here degrees Fahrenheit.
        bmc.readings.insert(1, vec![100, 0x40, 0]);
        let found = sensors(&mut bmc).unwrap();
        let measured = measure(&mut bmc, &found).unwrap();
        assert!((measured[0].value.unwrap() - (30.0 - 32.0) / 1.8).abs() < 1e-9);
        assert_eq!(measured[0].unit, "°C");
    }
}
