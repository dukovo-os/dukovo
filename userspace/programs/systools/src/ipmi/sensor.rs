// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A sensor as its SDR record describes it, its reading (Get Sensor
//! Reading, IPMI 2.0 section 35.14), and the conversion of a raw reading to
//! a value (section 36.3).

use super::events;
use super::{Error, NETFN_SENSOR, Result, Transport, command};

/// Get Sensor Reading.
const GET_SENSOR_READING: u8 = 0x2D;
/// Get Sensor Reading Factors, for a non-linear sensor's constants at a
/// reading (section 35.5).
const GET_SENSOR_READING_FACTORS: u8 = 0x23;

/// The event/reading type code of a threshold-based sensor (table 42-1).
pub(super) const THRESHOLD_EVENT_TYPE: u8 = 0x01;

/// How the raw reading is a number (table 43-1, sensor units 1, bits 7:6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Format {
    Unsigned,
    OnesComplement,
    TwosComplement,
}

/// The function the converted reading goes through, the linearization
/// byte of a full record (table 43-1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Linearization {
    Linear,
    Ln,
    Log10,
    Log2,
    Exp,
    Exp10,
    Exp2,
    Reciprocal,
    Square,
    Cube,
    SquareRoot,
    CubeReciprocal,
    /// 0x70: the constants depend on the reading, and the BMC gives them
    /// for each one (Get Sensor Reading Factors).
    NonLinear,
}

impl Linearization {
    /// The function a linearization byte names; `None` for a number the
    /// specification does not define.
    pub(super) fn of(code: u8) -> Option<Linearization> {
        Some(match code {
            0x00 => Linearization::Linear,
            0x01 => Linearization::Ln,
            0x02 => Linearization::Log10,
            0x03 => Linearization::Log2,
            0x04 => Linearization::Exp,
            0x05 => Linearization::Exp10,
            0x06 => Linearization::Exp2,
            0x07 => Linearization::Reciprocal,
            0x08 => Linearization::Square,
            0x09 => Linearization::Cube,
            0x0A => Linearization::SquareRoot,
            0x0B => Linearization::CubeReciprocal,
            0x70 => Linearization::NonLinear,
            _ => return None,
        })
    }

    fn apply(self, y: f64) -> f64 {
        match self {
            Linearization::Linear | Linearization::NonLinear => y,
            Linearization::Ln => y.ln(),
            Linearization::Log10 => y.log10(),
            Linearization::Log2 => y.log2(),
            Linearization::Exp => y.exp(),
            Linearization::Exp10 => 10f64.powf(y),
            Linearization::Exp2 => 2f64.powf(y),
            Linearization::Reciprocal => 1.0 / y,
            Linearization::Square => y * y,
            Linearization::Cube => y * y * y,
            Linearization::SquareRoot => y.sqrt(),
            Linearization::CubeReciprocal => 1.0 / (y * y * y),
        }
    }
}

/// The constants of `y = L[(M x + B * 10^Bexp) * 10^Rexp]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Factors {
    /// M, 10 bits, two's complement.
    pub m: i16,
    /// B, 10 bits, two's complement.
    pub b: i16,
    /// The exponent of B, 4 bits, two's complement.
    pub b_exp: i8,
    /// The result exponent, 4 bits, two's complement.
    pub r_exp: i8,
}

impl Factors {
    /// The factors from the six bytes a full record has at 24 to 29 (after
    /// the linearization byte) and Get Sensor Reading Factors gives from
    /// its fourth byte: M's low 8 bits; M's high 2 bits above the 6-bit
    /// tolerance; B's low 8 bits; B's high 2 bits above the accuracy's low
    /// 6; the rest of the accuracy and its exponent; and the two
    /// exponents, R in the high nibble.
    pub(super) fn from_bytes(bytes: [u8; 6]) -> Factors {
        let m = u16::from(bytes[0]) | (u16::from(bytes[1] & 0xC0) << 2);
        let b = u16::from(bytes[2]) | (u16::from(bytes[3] & 0xC0) << 2);
        Factors {
            m: sign_extend(m, 10),
            b: sign_extend(b, 10),
            b_exp: i8::try_from(sign_extend(u16::from(bytes[5] & 0x0F), 4)).unwrap_or(0),
            r_exp: i8::try_from(sign_extend(u16::from(bytes[5] >> 4), 4)).unwrap_or(0),
        }
    }
}

/// `value`, which has `bits` bits, as a signed number.
fn sign_extend(value: u16, bits: u32) -> i16 {
    let shift = 16 - bits;
    // The shifts move the sign bit to the top and back, which copies it.
    ((value << shift) as i16) >> shift
}

/// `value` times ten to the power `exponent`; a division for a negative
/// one, which keeps 300 * 10^-1 at exactly 30.
fn scale(value: f64, exponent: i8) -> f64 {
    let power = 10f64.powi(i32::from(exponent.abs()));
    if exponent < 0 { value / power } else { value * power }
}

/// A raw reading as a number, by the sensor's data format (section 36.1).
fn raw_number(format: Format, raw: u8) -> f64 {
    match format {
        Format::Unsigned => f64::from(raw),
        Format::TwosComplement => f64::from(raw as i8),
        // One's complement: a set top bit makes it negative, by the
        // complement of the other bits.
        Format::OnesComplement => {
            if raw & 0x80 == 0 {
                f64::from(raw)
            } else {
                -f64::from(!raw)
            }
        }
    }
}

/// How a full record's sensor is converted.
#[derive(Clone, Copy, Debug)]
pub(super) struct Analog {
    pub format: Format,
    pub linearization: Linearization,
}

impl Analog {
    /// `raw` as a value in the sensor's unit, with the record's constants.
    /// `None` when the function has no value there, as the logarithm of 0.
    pub(super) fn value(self, raw: u8, factors: Factors) -> Option<f64> {
        let x = raw_number(self.format, raw);
        let linear = scale(
            f64::from(factors.m) * x + scale(f64::from(factors.b), factors.b_exp),
            factors.r_exp,
        );
        Some(self.linearization.apply(linear)).filter(|value| value.is_finite())
    }
}

/// The six thresholds of a full record, as raw readings, for those the
/// record says can be read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Thresholds {
    pub lower_non_recoverable: Option<u8>,
    pub lower_critical: Option<u8>,
    pub lower_non_critical: Option<u8>,
    pub upper_non_critical: Option<u8>,
    pub upper_critical: Option<u8>,
    pub upper_non_recoverable: Option<u8>,
}

/// The unit of a sensor (table 43-1 units bytes, table 43-15).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Unit {
    pub base: u8,
    /// The modifier unit's code, used when `modifier` is not 0.
    pub modifier_unit: u8,
    /// 0 for none, 1 for the base unit divided by the modifier unit, 2
    /// for the two multiplied.
    pub modifier: u8,
    /// The rate unit: 0 none, then per microsecond, millisecond, second,
    /// minute, hour and day.
    pub rate: u8,
    pub percentage: bool,
}

impl Unit {
    /// The unit as written next to a value: `°C`, `RPM`, `W`.
    pub(crate) fn text(self) -> String {
        // A percentage is the whole unit, whatever the base unit says.
        if self.percentage {
            return "%".to_string();
        }
        let mut text = unit_name(self.base);
        match self.modifier {
            1 => text.push_str(&format!("/{}", unit_name(self.modifier_unit))),
            2 => text.push_str(&format!("*{}", unit_name(self.modifier_unit))),
            _ => {}
        }
        text.push_str(match self.rate {
            1 => "/us",
            2 => "/ms",
            3 => "/s",
            4 => "/min",
            5 => "/h",
            6 => "/day",
            _ => "",
        });
        text
    }
}

/// The name of a base unit, for the units a BMC's sensors mostly have, the
/// first of table 43-15; a number for the others.
fn unit_name(code: u8) -> String {
    match code {
        0 => "",
        1 => "°C",
        2 => "°F",
        3 => "K",
        4 => "V",
        5 => "A",
        6 => "W",
        7 => "J",
        8 => "C",
        9 => "VA",
        14 => "kPa",
        15 => "PSI",
        17 => "CFM",
        18 => "RPM",
        19 => "Hz",
        20 => "us",
        21 => "ms",
        22 => "s",
        23 => "min",
        24 => "h",
        25 => "day",
        58 => "ppm",
        60 => "dB",
        66 => "bit",
        70 => "B",
        88 => "errors",
        89 => "correctable errors",
        90 => "uncorrectable errors",
        other => return format!("unit {other}"),
    }
    .to_string()
}

/// A sensor of the BMC, from a full or compact record.
#[derive(Clone, Debug)]
pub(crate) struct Sensor {
    /// The sensor's number, which Get Sensor Reading takes.
    pub number: u8,
    /// The controller that owns the sensor: 0x20 is the BMC itself.
    pub owner: u8,
    /// The owner's LUN, which the transport cannot address but 0.
    pub lun: u8,
    pub name: String,
    pub sensor_type: u8,
    /// The event/reading type code: 0x01 threshold, 0x02 to 0x0C generic
    /// discrete, 0x6F sensor-specific.
    pub event_type: u8,
    pub entity: u8,
    pub entity_instance: u8,
    pub unit: Unit,
    /// How a raw reading is converted: only a full record of a sensor
    /// with an analog reading has it.
    pub(super) analog: Option<Analog>,
    /// The constants of the record, which a non-linear sensor replaces
    /// with the BMC's for each reading.
    pub(super) factors: Factors,
    pub thresholds: Thresholds,
}

impl Sensor {
    /// Whether the sensor is a threshold sensor, whose reading is a
    /// number, and not one of discrete states.
    pub(crate) fn is_threshold(&self) -> bool {
        self.event_type == THRESHOLD_EVENT_TYPE
    }

    /// Whether this program can read the sensor: it is the BMC's own, on
    /// LUN 0.
    pub(crate) fn readable_here(&self) -> bool {
        self.owner == 0x20 && self.lun == 0
    }

    /// The value of `raw` in the sensor's unit. `None` for a sensor with
    /// no analog reading or a value the function does not have.
    ///
    /// # Errors
    ///
    /// For a non-linear sensor, if the BMC refuses to give its constants.
    pub(crate) fn value(&self, link: &mut dyn Transport, raw: u8) -> Result<Option<f64>> {
        let Some(analog) = &self.analog else { return Ok(None) };
        let factors = if analog.linearization == Linearization::NonLinear {
            reading_factors(link, self.number, raw)?
        } else {
            self.factors
        };
        Ok(analog.value(raw, factors))
    }
}

/// The constants for a reading of a non-linear sensor.
fn reading_factors(link: &mut dyn Transport, number: u8, raw: u8) -> Result<Factors> {
    let reply = command(link, NETFN_SENSOR, GET_SENSOR_READING_FACTORS, &[number, raw])?;
    // The first byte is the next reading the same constants hold for.
    let Some(bytes) = reply.get(1..7).and_then(|bytes| <[u8; 6]>::try_from(bytes).ok()) else {
        return Err(Error::Malformed("the sensor reading factors are cut short"));
    };
    Ok(Factors::from_bytes(bytes))
}

/// What a sensor says now.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Reading {
    /// The BMC has no reading: bit 5 of the status says it is
    /// unavailable, or the sensor is not there.
    Unavailable,
    /// Scanning of the sensor is turned off (bit 6 of the status clear).
    ScanningDisabled,
    /// A threshold sensor's reading.
    Threshold {
        raw: u8,
        /// The converted reading, when the record says how.
        value: Option<f64>,
        /// The threshold comparison bits: 0 to 2 are below the lower
        /// non-critical, critical and non-recoverable thresholds, 3 to 5
        /// above the upper ones.
        crossed: u8,
    },
    /// The states of a discrete sensor, bit n set for offset n, from
    /// bytes 4 and 5 of the response.
    Discrete { asserted: u16 },
}

impl Reading {
    /// The worst threshold crossed, as words: `ok` for none.
    pub(crate) fn state(crossed: u8) -> &'static str {
        for (bit, text) in [
            (2, "lower non-recoverable"),
            (5, "upper non-recoverable"),
            (1, "lower critical"),
            (4, "upper critical"),
            (0, "lower non-critical"),
            (3, "upper non-critical"),
        ] {
            if crossed >> bit & 1 == 1 {
                return text;
            }
        }
        "ok"
    }

    /// The names of the states a discrete sensor is in.
    pub(crate) fn state_names(sensor: &Sensor, asserted: u16) -> Vec<String> {
        (0..15u8)
            .filter(|offset| asserted >> offset & 1 == 1)
            .map(|offset| {
                events::offset_name(sensor.sensor_type, sensor.event_type, offset)
                    .unwrap_or_else(|| format!("state {offset}"))
            })
            .collect()
    }
}

/// Reads a sensor of the BMC.
///
/// A refusal of the BMC for this one sensor reads as unavailable, so that
/// one sensor that cannot be read does not end a listing.
///
/// # Errors
///
/// If the transport fails, or a non-linear sensor's constants cannot be
/// had.
pub(crate) fn read_sensor(link: &mut dyn Transport, sensor: &Sensor) -> Result<Reading> {
    let reply = match command(link, NETFN_SENSOR, GET_SENSOR_READING, &[sensor.number]) {
        Ok(reply) => reply,
        Err(Error::Completion(_)) => return Ok(Reading::Unavailable),
        Err(error) => return Err(error),
    };
    let [raw, status, rest @ ..] = &reply[..] else {
        return Err(Error::Malformed("the sensor reading is cut short"));
    };
    // Section 35.14: bit 5 of the second byte says the reading is
    // unavailable, and bit 6 that scanning is enabled.
    if status & 0x20 != 0 {
        return Ok(Reading::Unavailable);
    }
    if status & 0x40 == 0 {
        return Ok(Reading::ScanningDisabled);
    }
    if sensor.is_threshold() {
        let crossed = rest.first().copied().unwrap_or(0) & 0x3F;
        let value = sensor.value(link, *raw)?;
        return Ok(Reading::Threshold { raw: *raw, value, crossed });
    }
    let low = u16::from(rest.first().copied().unwrap_or(0));
    let high = u16::from(rest.get(1).copied().unwrap_or(0) & 0x7F);
    Ok(Reading::Discrete { asserted: low | high << 8 })
}

/// The name of a sensor type (table 42-3).
pub(crate) fn type_name(code: u8) -> String {
    match code {
        0x01 => "temperature",
        0x02 => "voltage",
        0x03 => "current",
        0x04 => "fan",
        0x05 => "physical security",
        0x06 => "platform security",
        0x07 => "processor",
        0x08 => "power supply",
        0x09 => "power unit",
        0x0A => "cooling device",
        0x0B => "other units-based sensor",
        0x0C => "memory",
        0x0D => "drive slot",
        0x0E => "POST memory resize",
        0x0F => "system firmware progress",
        0x10 => "event logging disabled",
        0x11 => "watchdog 1",
        0x12 => "system event",
        0x13 => "critical interrupt",
        0x14 => "button or switch",
        0x15 => "module or board",
        0x16 => "microcontroller",
        0x17 => "add-in card",
        0x18 => "chassis",
        0x19 => "chip set",
        0x1A => "other FRU",
        0x1B => "cable or interconnect",
        0x1C => "terminator",
        0x1D => "system boot or restart",
        0x1E => "boot error",
        0x1F => "OS boot",
        0x20 => "OS stop",
        0x21 => "slot or connector",
        0x22 => "system ACPI power state",
        0x23 => "watchdog 2",
        0x24 => "platform alert",
        0x25 => "entity presence",
        0x26 => "monitor ASIC",
        0x27 => "LAN",
        0x28 => "management subsystem health",
        0x29 => "battery",
        0x2A => "session audit",
        0x2B => "version change",
        0x2C => "FRU state",
        other => return format!("type 0x{other:02X}"),
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analog(format: Format, linearization: Linearization) -> Analog {
        Analog { format, linearization }
    }

    const fn factors(m: i16, b: i16, b_exp: i8, r_exp: i8) -> Factors {
        Factors { m, b, b_exp, r_exp }
    }

    #[test]
    fn a_reading_is_m_x_plus_b_times_ten_to_the_exponents() {
        // (5 * 100 + (-20) * 10^1) * 10^-1 = (500 - 200) / 10 = 30.
        let a = analog(Format::Unsigned, Linearization::Linear);
        let value = a.value(100, factors(5, -20, 1, -1)).unwrap();
        assert!((value - 30.0).abs() < 1e-9, "{value}");
    }

    #[test]
    fn the_data_format_says_what_the_raw_byte_is() {
        let f = factors(1, 0, 0, 0);
        let twos = analog(Format::TwosComplement, Linearization::Linear);
        assert_eq!(twos.value(0xFB, f), Some(-5.0));
        assert_eq!(twos.value(0x7F, f), Some(127.0));
        let ones = analog(Format::OnesComplement, Linearization::Linear);
        assert_eq!(ones.value(0xFA, f), Some(-5.0));
        assert_eq!(ones.value(0x05, f), Some(5.0));
        let unsigned = analog(Format::Unsigned, Linearization::Linear);
        assert_eq!(unsigned.value(0xFB, f), Some(251.0));
    }

    #[test]
    fn each_linearization_applies_its_function() {
        let f = factors(1, 0, 0, 0);
        let check = |linearization, raw, expected: f64| {
            let value = analog(Format::Unsigned, linearization).value(raw, f).unwrap();
            assert!((value - expected).abs() < 1e-9, "{linearization:?}: {value}");
        };
        check(Linearization::Linear, 4, 4.0);
        check(Linearization::Ln, 1, 0.0);
        check(Linearization::Log10, 100, 2.0);
        check(Linearization::Log2, 8, 3.0);
        check(Linearization::Exp, 0, 1.0);
        check(Linearization::Exp10, 2, 100.0);
        check(Linearization::Exp2, 5, 32.0);
        check(Linearization::Reciprocal, 4, 0.25);
        check(Linearization::Square, 5, 25.0);
        check(Linearization::Cube, 3, 27.0);
        check(Linearization::SquareRoot, 49, 7.0);
        check(Linearization::CubeReciprocal, 2, 0.125);
        // The logarithm of 0 and the reciprocal of 0 have no value.
        let log = analog(Format::Unsigned, Linearization::Log10);
        assert_eq!(log.value(0, f), None);
        let recip = analog(Format::Unsigned, Linearization::Reciprocal);
        assert_eq!(recip.value(0, f), None);
    }

    #[test]
    fn linearization_numbers_are_those_of_the_table() {
        assert_eq!(Linearization::of(0), Some(Linearization::Linear));
        assert_eq!(Linearization::of(0x0B), Some(Linearization::CubeReciprocal));
        assert_eq!(Linearization::of(0x70), Some(Linearization::NonLinear));
        assert_eq!(Linearization::of(0x0C), None);
    }

    #[test]
    fn factors_are_cut_from_their_bytes_with_their_signs() {
        // M = 0x3FF (two's complement 10 bits: -1), B = 0x200 (-512),
        // B exponent 0xF (-1), R exponent 0xE (-2).
        let f = Factors::from_bytes([0xFF, 0xC5, 0x00, 0x80, 0x00, 0xEF]);
        assert_eq!(f, factors(-1, -512, -1, -2));
        let f = Factors::from_bytes([0x02, 0x00, 0x0A, 0x00, 0x00, 0x01]);
        assert_eq!(f, factors(2, 10, 1, 0));
    }

    #[test]
    fn units_are_written_with_their_modifier_rate_and_percentage() {
        let unit = |base, modifier_unit, modifier, rate, percentage| Unit {
            base,
            modifier_unit,
            modifier,
            rate,
            percentage,
        };
        assert_eq!(unit(1, 0, 0, 0, false).text(), "°C");
        assert_eq!(unit(18, 0, 0, 0, false).text(), "RPM");
        assert_eq!(unit(6, 0, 0, 0, true).text(), "%");
        assert_eq!(unit(70, 0, 0, 3, false).text(), "B/s");
        assert_eq!(unit(5, 4, 2, 0, false).text(), "A*V");
        assert_eq!(unit(0, 0, 0, 0, false).text(), "");
        assert_eq!(unit(99, 0, 0, 0, false).text(), "unit 99");
    }

    #[test]
    fn the_worst_threshold_crossed_is_the_state() {
        assert_eq!(Reading::state(0), "ok");
        assert_eq!(Reading::state(0b001000), "upper non-critical");
        assert_eq!(Reading::state(0b011000), "upper critical");
        assert_eq!(Reading::state(0b000011), "lower critical");
        assert_eq!(Reading::state(0b100100), "lower non-recoverable");
    }

    fn inlet(bmc: &mut super::super::sim::Sim) -> Sensor {
        bmc.sdr = vec![super::super::sim::full_record(1, 4, "Inlet Temp")];
        super::super::sdr::sensors(bmc).unwrap().remove(0)
    }

    #[test]
    fn a_reading_is_converted_with_the_records_constants() {
        let mut bmc = super::super::sim::Sim::new();
        let sensor = inlet(&mut bmc);
        // Raw 100 is 30 degrees, with the status "scanning enabled" and
        // the upper non-critical threshold crossed (bit 3).
        bmc.readings.insert(4, vec![100, 0x40, 0x08]);
        let Reading::Threshold { raw, value, crossed } = read_sensor(&mut bmc, &sensor).unwrap()
        else {
            panic!("a threshold reading");
        };
        assert_eq!((raw, crossed), (100, 0x08));
        assert!((value.unwrap() - 30.0).abs() < 1e-9, "{value:?}");
        assert_eq!(Reading::state(crossed), "upper non-critical");
    }

    #[test]
    fn the_unavailable_and_scanning_disabled_bits_hide_the_value() {
        let mut bmc = super::super::sim::Sim::new();
        let sensor = inlet(&mut bmc);
        bmc.readings.insert(4, vec![100, 0x60, 0]);
        assert_eq!(read_sensor(&mut bmc, &sensor).unwrap(), Reading::Unavailable);
        bmc.readings.insert(4, vec![100, 0x00, 0]);
        assert_eq!(read_sensor(&mut bmc, &sensor).unwrap(), Reading::ScanningDisabled);
        // A sensor the BMC does not know is unavailable, not an error.
        bmc.readings.remove(&4);
        assert_eq!(read_sensor(&mut bmc, &sensor).unwrap(), Reading::Unavailable);
    }

    #[test]
    fn a_discrete_sensor_reads_its_states() {
        let mut bmc = super::super::sim::Sim::new();
        bmc.sdr = vec![super::super::sim::compact_record(2, 9, "PS Status", 1)];
        let sensor = super::super::sdr::sensors(&mut bmc).unwrap().remove(0);
        // Offsets 0 (presence) and 1 (failure) of a power supply.
        bmc.readings.insert(9, vec![0, 0xC0, 0x03, 0x00]);
        let reading = read_sensor(&mut bmc, &sensor).unwrap();
        assert_eq!(reading, Reading::Discrete { asserted: 3 });
        assert_eq!(Reading::state_names(&sensor, 3), ["presence detected", "failure detected"]);
        assert_eq!(Reading::state_names(&sensor, 1 << 12), ["state 12"]);
    }

    #[test]
    fn a_non_linear_sensor_asks_the_bmc_for_the_constants_at_its_reading() {
        let mut bmc = super::super::sim::Sim::new();
        let mut record = super::super::sim::full_record(1, 4, "Curve");
        record[23] = 0x70;
        bmc.sdr = vec![record];
        let sensor = super::super::sdr::sensors(&mut bmc).unwrap().remove(0);
        // At raw 50 the BMC's constants are M = 2, B = 10, B exponent 1 and
        // R exponent 0: 2 * 50 + 10 * 10 = 200. At raw 60 they are M = 3,
        // B = 0, no exponents: 180.
        bmc.factors.insert((4, 50), [0x02, 0x00, 0x0A, 0x00, 0x00, 0x01]);
        bmc.factors.insert((4, 60), [0x03, 0x00, 0x00, 0x00, 0x00, 0x00]);
        bmc.readings.insert(4, vec![50, 0x40, 0]);
        let Reading::Threshold { value, .. } = read_sensor(&mut bmc, &sensor).unwrap() else {
            panic!("a threshold reading");
        };
        assert_eq!(value, Some(200.0));
        assert_eq!(sensor.value(&mut bmc, 60).unwrap(), Some(180.0));
        // No constants at a reading is the BMC's refusal.
        assert!(sensor.value(&mut bmc, 70).is_err());
    }

    #[test]
    fn only_the_bmcs_own_sensors_on_lun_0_are_read_here() {
        let mut bmc = super::super::sim::Sim::new();
        let mut sensor = inlet(&mut bmc);
        assert!(sensor.readable_here());
        sensor.owner = 0x30;
        assert!(!sensor.readable_here());
        sensor.owner = 0x20;
        sensor.lun = 1;
        assert!(!sensor.readable_here());
    }
}
