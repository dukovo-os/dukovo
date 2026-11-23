// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The sensor data repository: the walk of its records (IPMI 2.0 section
//! 33, commands in section 33.9 to 33.12) and the decoding of full sensor
//! records (type 01h, table 43-1) and compact ones (type 02h, table
//! 43-2).

use std::collections::HashSet;

use super::sensor::{Analog, Factors, Format, Linearization, Sensor, Thresholds, Unit};
use super::text::{self, Kind};
use super::{Error, NETFN_STORAGE, Result, Transport, command, le16};

/// Get SDR Repository Info.
const GET_SDR_REPOSITORY_INFO: u8 = 0x20;
/// Reserve SDR Repository.
const RESERVE_SDR_REPOSITORY: u8 = 0x22;
/// Get SDR.
const GET_SDR: u8 = 0x23;

/// The record ID that ends the walk (the "next record" of the last one).
const LAST_RECORD: u16 = 0xFFFF;

/// The bytes asked for in one Get SDR: the specification's usual part
/// size, which every BMC serves.
const PART: usize = 16;

/// The header of an SDR record: ID, version, type and the length of the
/// rest (table 43-?, section 43).
const HEADER: usize = 5;

/// A reservation lost to another reader is asked for again this many
/// times for one record.
const TRIES: usize = 3;

/// The most records a walk follows, so that a BMC whose "next" ids loop
/// does not hold the program for ever.
const RECORDS_MAX: usize = 8192;

/// One record of the repository.
#[derive(Debug)]
pub(super) struct Record {
    pub kind: u8,
    /// The whole record, header included.
    pub bytes: Vec<u8>,
}

/// Reads every record of the repository, in the BMC's order.
///
/// # Errors
///
/// If the BMC has no repository, does not answer or loses the
/// reservation three times in a row for one record.
pub(super) fn walk(link: &mut dyn Transport) -> Result<Vec<Record>> {
    let mut reservation = reserve(link)?;
    let mut records = Vec::new();
    let mut seen = HashSet::new();
    let mut id = 0u16;
    loop {
        let (next, record) = match read_record(link, &mut reservation, id) {
            // A repository with no records answers "not present" for the
            // first one.
            Err(Error::Completion(0xCB)) if id == 0 => return Ok(records),
            other => other?,
        };
        records.push(record);
        if next == LAST_RECORD {
            return Ok(records);
        }
        if records.len() >= RECORDS_MAX || !seen.insert(next) {
            return Err(Error::Malformed("the repository's records never end"));
        }
        id = next;
    }
}

fn reserve(link: &mut dyn Transport) -> Result<u16> {
    let reply = command(link, NETFN_STORAGE, RESERVE_SDR_REPOSITORY, &[])?;
    le16(&reply, 0)
}

/// When the repository last changed: the times of its latest addition and
/// its latest erase, as the BMC counts them (section 33.9). A program that
/// keeps the sensors it read compares these to know they are still the
/// BMC's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Changed {
    pub added: u32,
    pub erased: u32,
}

/// Get SDR Repository Info, for its two change times.
///
/// # Errors
///
/// If the BMC has no repository, does not answer or answers too little.
pub(crate) fn repository_changed(link: &mut dyn Transport) -> Result<Changed> {
    let reply = command(link, NETFN_STORAGE, GET_SDR_REPOSITORY_INFO, &[])?;
    let time = |at: usize| match reply.get(at..at + 4) {
        Some(&[a, b, c, d]) => Ok(u32::from_le_bytes([a, b, c, d])),
        _ => Err(Error::Malformed("the repository's info is cut short")),
    };
    Ok(Changed { added: time(5)?, erased: time(9)? })
}

/// One record and the ID of the next, in parts of [`PART`] bytes. The
/// reservation is asked for again when the BMC cancelled it (completion
/// code 0xC5), as it does when another program reads the repository.
fn read_record(link: &mut dyn Transport, reservation: &mut u16, id: u16) -> Result<(u16, Record)> {
    for _ in 0..TRIES {
        match read_parts(link, *reservation, id) {
            Err(Error::Completion(0xC5)) => *reservation = reserve(link)?,
            other => return other,
        }
    }
    Err(Error::Completion(0xC5))
}

fn read_parts(link: &mut dyn Transport, reservation: u16, id: u16) -> Result<(u16, Record)> {
    let (next, mut bytes) = get_part(link, reservation, id, 0, HEADER)?;
    if bytes.len() < HEADER {
        return Err(Error::Malformed("an SDR record's header is cut short"));
    }
    let total = HEADER + usize::from(bytes[4]);
    while bytes.len() < total {
        let want = PART.min(total - bytes.len());
        let (_, part) = get_part(link, reservation, id, bytes.len(), want)?;
        bytes.extend_from_slice(&part);
    }
    Ok((next, Record { kind: bytes[3], bytes }))
}

/// Get SDR for `count` bytes at `offset` of record `id`: the next record's
/// ID and the bytes.
fn get_part(
    link: &mut dyn Transport,
    reservation: u16,
    id: u16,
    offset: usize,
    count: usize,
) -> Result<(u16, Vec<u8>)> {
    let [res_low, res_high] = reservation.to_le_bytes();
    let [id_low, id_high] = id.to_le_bytes();
    let offset = u8::try_from(offset).map_err(|_| Error::Malformed("an SDR record is too long"))?;
    let count = u8::try_from(count).map_err(|_| Error::Malformed("an SDR record is too long"))?;
    let data = [res_low, res_high, id_low, id_high, offset, count];
    let reply = command(link, NETFN_STORAGE, GET_SDR, &data)?;
    let next = le16(&reply, 0)?;
    let bytes = reply[2..].to_vec();
    if bytes.len() != usize::from(count) {
        return Err(Error::Malformed("an SDR part is shorter than asked"));
    }
    Ok((next, bytes))
}

/// The sensors the repository describes, in its order.
///
/// # Errors
///
/// As [`walk`].
pub(crate) fn sensors(link: &mut dyn Transport) -> Result<Vec<Sensor>> {
    Ok(walk(link)?.iter().flat_map(decode).collect())
}

/// The sensors a record describes: one for a full record, one or more
/// for a compact one, none for other record types or a record too short
/// to be one.
fn decode(record: &Record) -> Vec<Sensor> {
    match record.kind {
        0x01 => full(&record.bytes).into_iter().collect(),
        0x02 => compact(&record.bytes),
        _ => Vec::new(),
    }
}

/// A full sensor record (table 43-1; byte numbers below count from 0 at
/// the record ID, where the table counts from 1).
fn full(bytes: &[u8]) -> Option<Sensor> {
    let name_at = 47;
    if bytes.len() < name_at + 1 {
        return None;
    }
    let units = bytes[20];
    // Table 43-1, byte 12: bits 3:2 are the threshold access support
    // (00 none), and the readable threshold mask is byte 19.
    let access = bytes[11] >> 2 & 3;
    let readable = if access == 0 { 0 } else { bytes[19] };
    let threshold = |bit: u8, at: usize| (readable >> bit & 1 == 1).then_some(bytes[at]);
    let factors = Factors::from_bytes(bytes[24..30].try_into().ok()?);
    // Sensor units 1, bits 7:6: 00 unsigned, 01 one's complement, 10 two's
    // complement, 11 no analog reading.
    let format = match units >> 6 {
        0 => Some(Format::Unsigned),
        1 => Some(Format::OnesComplement),
        2 => Some(Format::TwosComplement),
        _ => None,
    };
    let linearization = Linearization::of(bytes[23] & 0x7F);
    let analog = match (format, linearization) {
        (Some(format), Some(linearization)) => Some(Analog { format, linearization }),
        _ => None,
    };
    Some(Sensor {
        number: bytes[7],
        owner: bytes[5],
        lun: bytes[6] & 3,
        name: name(bytes, name_at),
        sensor_type: bytes[12],
        event_type: bytes[13],
        entity: bytes[8],
        entity_instance: bytes[9] & 0x7F,
        unit: unit(units, bytes[21], bytes[22]),
        analog,
        factors,
        thresholds: Thresholds {
            lower_non_recoverable: threshold(2, 39),
            lower_critical: threshold(1, 40),
            lower_non_critical: threshold(0, 41),
            upper_non_critical: threshold(3, 38),
            upper_critical: threshold(4, 37),
            upper_non_recoverable: threshold(5, 36),
        },
    })
}

/// A compact sensor record (table 43-2): no analog reading, thresholds
/// or conversion. Its share count (byte 23, bits 3:0) is how many
/// sensors with consecutive numbers it stands for, named by the
/// identification string and an instance modifier: a number, or a letter
/// when bits 5:4 are 01b, counting up from the offset of byte 24.
fn compact(bytes: &[u8]) -> Vec<Sensor> {
    let name_at = 31;
    if bytes.len() < name_at + 1 {
        return Vec::new();
    }
    let share = bytes[23] & 0x0F;
    let count = share.max(1);
    let alphabetic = bytes[23] >> 4 & 3 == 1;
    let first = bytes[24] & 0x7F;
    let instances_differ = bytes[24] & 0x80 != 0;
    let base_name = name(bytes, name_at);
    (0..count)
        .map(|index| {
            let name = if share > 1 {
                let step = first.saturating_add(index);
                if alphabetic {
                    format!("{base_name} {}", char::from(b'A' + step.min(25)))
                } else {
                    format!("{base_name} {step}")
                }
            } else {
                base_name.clone()
            };
            Sensor {
                number: bytes[7].wrapping_add(index),
                owner: bytes[5],
                lun: bytes[6] & 3,
                name,
                sensor_type: bytes[12],
                event_type: bytes[13],
                entity: bytes[8],
                entity_instance: (bytes[9] & 0x7F).wrapping_add(if instances_differ {
                    index
                } else {
                    0
                }),
                unit: unit(bytes[20], bytes[21], bytes[22]),
                analog: None,
                factors: Factors { m: 0, b: 0, b_exp: 0, r_exp: 0 },
                thresholds: Thresholds::default(),
            }
        })
        .collect()
}

/// The unit from the units bytes of table 43-1: bits 5:3 of byte 1 are the
/// rate unit, bits 2:1 the modifier relation and bit 0 the percentage.
fn unit(units1: u8, base: u8, modifier_unit: u8) -> Unit {
    Unit {
        base,
        modifier_unit,
        modifier: units1 >> 1 & 3,
        rate: units1 >> 3 & 7,
        percentage: units1 & 1 == 1,
    }
}

/// The identification string at `at`: a type/length byte (section 43.15)
/// and up to 16 bytes. A string with no length is the sensor's number.
fn name(bytes: &[u8], at: usize) -> String {
    let type_length = bytes[at];
    let length = usize::from(type_length & 0x1F);
    let string = bytes.get(at + 1..at + 1 + length).unwrap_or(&[]);
    let kind = Kind::of(type_length);
    // A name in an SDR is text: a record that claims binary or Unicode
    // has bytes nothing here can show as text.
    let text = text::decode(if kind == Kind::Binary { Kind::Latin1 } else { kind }, string);
    if text.is_empty() { format!("sensor 0x{:02X}", bytes[7]) } else { text }
}

/// The name of an entity ID (table 43-13), for the entities a BMC's
/// sensors are mostly on.
pub(crate) fn entity_name(code: u8) -> String {
    match code {
        0x01 => "other",
        0x03 => "processor",
        0x04 => "disk or disk bay",
        0x05 => "peripheral bay",
        0x06 => "system management module",
        0x07 => "system board",
        0x08 => "memory module",
        0x09 => "processor module",
        0x0A => "power supply",
        0x0B => "add-in card",
        0x0C => "front panel board",
        0x0D => "back panel board",
        0x0E => "power system board",
        0x0F => "drive backplane",
        0x10 => "system internal expansion board",
        0x11 => "other system board",
        0x12 => "processor board",
        0x13 => "power unit",
        0x14 => "power module",
        0x15 => "power management board",
        0x17 => "system chassis",
        0x1A => "disk drive bay",
        0x1D => "fan or cooling device",
        0x1E => "cooling unit",
        0x1F => "cable or interconnect",
        0x20 => "memory device",
        0x22 => "system firmware",
        0x23 => "operating system",
        0x24 => "system bus",
        0x26 => "remote management",
        0x27 => "external environment",
        0x28 => "battery",
        0x2E => "management controller firmware",
        0x30 => "PCI bus",
        0x31 => "PCI Express bus",
        0x35 => "real-time clock",
        0x37 | 0x40 => "air inlet",
        0x41 => "processor",
        0x42 => "baseboard",
        other => return format!("entity 0x{other:02X}"),
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::super::sim::{self, Sim};
    use super::*;

    #[test]
    fn the_walk_reads_each_record_in_parts_and_follows_the_next_ids() {
        let mut bmc = Sim::new();
        bmc.sdr = vec![
            sim::full_record(1, 4, "Inlet Temp"),
            sim::compact_record(2, 9, "PS Status", 1),
            sim::full_record(3, 5, "Fan1"),
        ];
        let records = walk(&mut bmc).unwrap();
        let ids: Vec<(u16, u8)> = records
            .iter()
            .map(|record| (u16::from_le_bytes([record.bytes[0], record.bytes[1]]), record.kind))
            .collect();
        assert_eq!(ids, [(1, 1), (2, 2), (3, 1)]);
        assert_eq!(records[0].bytes.len(), 48 + "Inlet Temp".len());
        assert!(bmc.largest_sdr_read <= PART);
    }

    #[test]
    fn the_repository_says_when_it_was_added_to_and_erased() {
        let mut bmc = Sim::new();
        bmc.sdr_added = 0x0102_0304;
        bmc.sdr_erased = 7;
        assert_eq!(
            repository_changed(&mut bmc).unwrap(),
            Changed { added: 0x0102_0304, erased: 7 }
        );
    }

    #[test]
    fn a_cancelled_reservation_is_asked_for_again() {
        let mut bmc = Sim::new();
        bmc.sdr = vec![sim::full_record(1, 4, "Inlet Temp"), sim::full_record(2, 5, "Fan1")];
        bmc.cancel_reservation_after = Some(2);
        let records = walk(&mut bmc).unwrap();
        assert_eq!(records.len(), 2);
        assert!(bmc.reservations >= 2);
    }

    #[test]
    fn a_repository_that_never_ends_is_refused() {
        let mut bmc = Sim::new();
        bmc.sdr = vec![sim::full_record(1, 4, "A"), sim::full_record(2, 5, "B")];
        bmc.sdr_loops = true;
        assert!(matches!(walk(&mut bmc), Err(Error::Malformed(_))));
    }

    #[test]
    fn a_full_record_gives_the_sensor_its_name_type_unit_and_constants() {
        let mut bmc = Sim::new();
        bmc.sdr = vec![sim::full_record(1, 4, "Inlet Temp")];
        let found = sensors(&mut bmc).unwrap();
        let [sensor] = &found[..] else { panic!("one sensor") };
        assert_eq!(sensor.name, "Inlet Temp");
        assert_eq!(sensor.number, 4);
        assert_eq!(sensor.owner, 0x20);
        assert_eq!(sensor.sensor_type, 0x01);
        assert_eq!(sensor.event_type, 0x01);
        assert_eq!(sensor.entity, 0x37);
        assert_eq!(sensor.entity_instance, 1);
        assert_eq!(sensor.unit.text(), "°C");
        assert_eq!(sensor.factors, Factors { m: 5, b: -20, b_exp: 1, r_exp: -1 });
        let analog = sensor.analog.unwrap();
        assert_eq!(analog.format, Format::Unsigned);
        assert_eq!(analog.linearization, Linearization::Linear);
        // Thresholds the record marks readable: lower critical (bit 1) and
        // upper critical (bit 4) and upper non-critical (bit 3).
        assert_eq!(sensor.thresholds.upper_critical, Some(120));
        assert_eq!(sensor.thresholds.upper_non_critical, Some(110));
        assert_eq!(sensor.thresholds.lower_critical, Some(40));
        assert_eq!(sensor.thresholds.lower_non_critical, None);
        assert_eq!(sensor.thresholds.upper_non_recoverable, None);
    }

    #[test]
    fn a_compact_record_has_no_conversion() {
        let mut bmc = Sim::new();
        bmc.sdr = vec![sim::compact_record(2, 9, "PS Status", 1)];
        let found = sensors(&mut bmc).unwrap();
        let [sensor] = &found[..] else { panic!("one sensor") };
        assert_eq!(sensor.name, "PS Status");
        assert_eq!(sensor.sensor_type, 0x08);
        assert_eq!(sensor.event_type, 0x6F);
        assert!(sensor.analog.is_none());
        assert!(!sensor.is_threshold());
    }

    #[test]
    fn a_shared_compact_record_is_several_sensors() {
        let mut bmc = Sim::new();
        let mut record = sim::compact_record(2, 9, "PSU", 3);
        record[24] = 1;
        bmc.sdr = vec![record];
        let found = sensors(&mut bmc).unwrap();
        let named: Vec<(u8, &str)> = found.iter().map(|s| (s.number, s.name.as_str())).collect();
        assert_eq!(named, [(9, "PSU 1"), (10, "PSU 2"), (11, "PSU 3")]);
        let mut record = sim::compact_record(2, 9, "PSU", 2);
        record[23] |= 0x10;
        bmc.sdr = vec![record];
        let found = sensors(&mut bmc).unwrap();
        assert_eq!(found[1].name, "PSU B");
    }

    #[test]
    fn a_record_with_no_name_is_called_by_its_number() {
        let mut bmc = Sim::new();
        bmc.sdr = vec![sim::compact_record(2, 0x2A, "", 1)];
        let found = sensors(&mut bmc).unwrap();
        assert_eq!(found[0].name, "sensor 0x2A");
    }

    #[test]
    fn a_name_may_be_packed_six_bit_ascii() {
        let mut bmc = Sim::new();
        let mut record = sim::compact_record(2, 9, "", 1);
        // "FAN" in 6-bit ASCII: F = 0x26, A = 0x21, N = 0x2E; 3 characters take 3
        // bytes' worth of bits, 18 bits, so 3 bytes, trailing bits 0.
        let value = 0x26 | (0x21 << 6) | (0x2E << 12);
        record.truncate(31);
        record.push(0x80 | 3);
        record.extend_from_slice(&[value as u8, (value >> 8) as u8, (value >> 16) as u8]);
        record[4] = u8::try_from(record.len() - 5).unwrap();
        bmc.sdr = vec![record];
        let found = sensors(&mut bmc).unwrap();
        assert_eq!(found[0].name, "FAN");
    }

    #[test]
    fn other_record_types_and_short_records_give_no_sensors() {
        let mut bmc = Sim::new();
        let mut locator = sim::full_record(1, 1, "x");
        locator[3] = 0x12;
        let mut short = sim::full_record(2, 1, "x");
        short.truncate(20);
        short[4] = 15;
        bmc.sdr = vec![locator, short];
        assert!(sensors(&mut bmc).unwrap().is_empty());
    }

    #[test]
    fn entities_are_named_by_their_numbers() {
        assert_eq!(entity_name(0x37), "air inlet");
        assert_eq!(entity_name(0x0A), "power supply");
        assert_eq!(entity_name(0xEE), "entity 0xEE");
    }
}
