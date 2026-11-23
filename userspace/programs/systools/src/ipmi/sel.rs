// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The system event log (IPMI 2.0 section 31, commands in section 31):
//! its entries, and Clear SEL. The 16-byte records are decoded as system
//! event records (table 32-1).

use std::collections::HashSet;
use std::thread;
use std::time::Duration;

use super::events::{self, Severity};
use super::{Error, NETFN_STORAGE, Result, Transport, command, le16};

/// Get SEL Info.
const GET_SEL_INFO: u8 = 0x40;
/// Reserve SEL.
const RESERVE_SEL: u8 = 0x42;
/// Get SEL Entry.
const GET_SEL_ENTRY: u8 = 0x43;
/// Clear SEL.
const CLEAR_SEL: u8 = 0x47;

/// The "next entry" that ends the log.
const LAST_ENTRY: u16 = 0xFFFF;

/// Bytes of a record of the log.
const ENTRY_LEN: usize = 16;

/// Timestamps up to this are seconds since the BMC started, not since
/// 1970 (section 31.3: the clock has not been set).
const SINCE_START_MAX: u32 = 0x2000_0000;

/// The most entries a walk follows.
const ENTRIES_MAX: usize = 70_000;

/// Times the erase's progress is asked, 100 ms apart.
const ERASE_POLLS: usize = 100;

/// A system event of the log (record type 02h).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Event {
    /// The ID of the controller that generated it, as the record has it.
    pub generator: u16,
    pub sensor_type: u8,
    pub sensor_number: u8,
    /// The event/reading type code: 0x01 is a threshold event.
    pub event_type: u8,
    /// Whether the event is an assertion, the usual case, and not the
    /// deassertion of a condition.
    pub asserted: bool,
    /// Event data 1 to 3.
    pub data: [u8; 3],
}

/// When an entry was logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum When {
    /// Seconds since 1970.
    Clock(u32),
    /// Seconds since the BMC started: its clock was not set yet.
    SinceStart(u32),
}

/// One record of the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub id: u16,
    /// The record type: 0x02 for a system event, 0xC0 to 0xDF for OEM
    /// records with a timestamp, 0xE0 to 0xFF for those without.
    pub kind: u8,
    pub when: Option<When>,
    pub event: Option<Event>,
    /// The sixteen bytes, for a record that is not a system event.
    pub raw: [u8; ENTRY_LEN],
}

impl Event {
    /// The event's own words and how much it matters.
    pub(crate) fn describe(&self) -> (String, Severity) {
        events::describe(
            self.sensor_type,
            self.event_type,
            self.asserted,
            self.data[0],
            self.data[1],
        )
    }

    /// For a threshold event, the raw reading that crossed the threshold
    /// and the threshold, when event data 1 says that data 2 and 3 hold
    /// them (bits 7:6 and 5:4 are 01b).
    pub(crate) fn threshold_readings(&self) -> Option<(u8, u8)> {
        let data1 = self.data[0];
        (self.event_type == 0x01 && data1 >> 6 == 1 && data1 >> 4 & 3 == 1)
            .then_some((self.data[1], self.data[2]))
    }
}

impl Entry {
    /// Decodes the 16 bytes of a record (table 32-1): ID, type, a 4-byte
    /// timestamp, the generator, the event message revision, the sensor
    /// type and number, the direction and event type, and three data
    /// bytes.
    fn from_bytes(bytes: [u8; ENTRY_LEN]) -> Entry {
        let id = u16::from_le_bytes([bytes[0], bytes[1]]);
        let kind = bytes[2];
        let stamp = u32::from_le_bytes([bytes[3], bytes[4], bytes[5], bytes[6]]);
        let when = match kind {
            0x02 | 0xC0..=0xDF => Some(if stamp <= SINCE_START_MAX {
                When::SinceStart(stamp)
            } else {
                When::Clock(stamp)
            }),
            _ => None,
        };
        let event = (kind == 0x02).then(|| Event {
            generator: u16::from_le_bytes([bytes[7], bytes[8]]),
            sensor_type: bytes[10],
            sensor_number: bytes[11],
            asserted: bytes[12] & 0x80 == 0,
            event_type: bytes[12] & 0x7F,
            data: [bytes[13], bytes[14], bytes[15]],
        });
        Entry { id, kind, when, event, raw: bytes }
    }
}

/// What Get SEL Info says (table 31-?).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Info {
    pub entries: u16,
    /// The times of the latest addition and the latest erase, as the BMC
    /// counts them: a log whose entries, addition and erase times are the
    /// same as at the last look has nothing new.
    pub added: u32,
    pub erased: u32,
    /// Free bytes, 0xFFFF for 65535 and more.
    pub free: u16,
    /// Whether events were lost because the log was full.
    pub overflow: bool,
    /// Whether Reserve SEL is supported, which Clear SEL needs.
    pub reserves: bool,
}

/// Get SEL Info.
///
/// # Errors
///
/// If the BMC has no event log, or does not answer.
pub(crate) fn info(link: &mut dyn Transport) -> Result<Info> {
    let reply = command(link, NETFN_STORAGE, GET_SEL_INFO, &[])?;
    let support = *reply.get(13).ok_or(Error::Malformed("the SEL info is cut short"))?;
    // The reply is at least 14 bytes, as `support` was found.
    let time =
        |at: usize| u32::from_le_bytes([reply[at], reply[at + 1], reply[at + 2], reply[at + 3]]);
    Ok(Info {
        entries: le16(&reply, 1)?,
        added: time(5),
        erased: time(9),
        free: le16(&reply, 3)?,
        overflow: support & 0x80 != 0,
        reserves: support & 0x02 != 0,
    })
}

fn reserve(link: &mut dyn Transport) -> Result<u16> {
    let reply = command(link, NETFN_STORAGE, RESERVE_SEL, &[])?;
    le16(&reply, 0)
}

/// Reads the log, oldest first.
///
/// # Errors
///
/// If the BMC does not answer, or has no event log.
pub(crate) fn read_log(link: &mut dyn Transport) -> Result<Vec<Entry>> {
    let found = info(link)?;
    if found.entries == 0 {
        return Ok(Vec::new());
    }
    // A reservation is only needed for partial reads; entries are read
    // whole, so a BMC that cannot reserve is read with reservation 0.
    let reservation = if found.reserves { reserve(link)? } else { 0 };
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    let mut id = 0u16;
    loop {
        let (next, entry) = get_entry(link, reservation, id)?;
        entries.push(entry);
        if next == LAST_ENTRY {
            return Ok(entries);
        }
        if entries.len() >= ENTRIES_MAX || !seen.insert(next) {
            return Err(Error::Malformed("the log's entries never end"));
        }
        id = next;
    }
}

/// Get SEL Entry for the whole record `id` (0 is the first).
fn get_entry(link: &mut dyn Transport, reservation: u16, id: u16) -> Result<(u16, Entry)> {
    let [res_low, res_high] = reservation.to_le_bytes();
    let [id_low, id_high] = id.to_le_bytes();
    let data = [res_low, res_high, id_low, id_high, 0, 0xFF];
    let reply = command(link, NETFN_STORAGE, GET_SEL_ENTRY, &data)?;
    let next = le16(&reply, 0)?;
    let Some(bytes) = reply.get(2..2 + ENTRY_LEN).and_then(|b| <[u8; ENTRY_LEN]>::try_from(b).ok())
    else {
        return Err(Error::Malformed("a log entry is cut short"));
    };
    Ok((next, Entry::from_bytes(bytes)))
}

/// Erases the log: Clear SEL with its reservation (section 31.9), asked
/// for progress until the erase ends.
///
/// # Errors
///
/// If the BMC refuses, does not answer, or the erase does not end.
pub(crate) fn clear_log(link: &mut dyn Transport) -> Result<()> {
    let reservation = reserve(link)?;
    let [low, high] = reservation.to_le_bytes();
    // "CLR" and 0xAA starts the erase; 0x00 asks how far it is.
    command(link, NETFN_STORAGE, CLEAR_SEL, &[low, high, b'C', b'L', b'R', 0xAA])?;
    for _ in 0..ERASE_POLLS {
        let reply = command(link, NETFN_STORAGE, CLEAR_SEL, &[low, high, b'C', b'L', b'R', 0x00])?;
        if reply.first().is_some_and(|progress| progress & 0x0F == 0x01) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(Error::Refused("the BMC did not finish erasing the event log".into()))
}

#[cfg(test)]
mod tests {
    use super::super::sim::{self, Sim};
    use super::*;

    #[test]
    fn the_log_is_read_entry_by_entry_to_its_end() {
        let mut bmc = Sim::new();
        bmc.sel = vec![
            sim::sel_entry(1, 100, 0x01, 4, 0x01, [0x59, 0x5A, 0x55]),
            sim::sel_entry(2, 1_794_000_000, 0x08, 7, 0x6F, [0x01, 0, 0]),
            sim::sel_entry(3, 1_794_000_100, 0x23, 0, 0xEF, [0xC1, 0x04, 0]),
        ];
        let entries = read_log(&mut bmc).unwrap();
        assert_eq!(entries.len(), 3);
        let first = &entries[0];
        assert_eq!(first.id, 1);
        assert_eq!(first.when, Some(When::SinceStart(100)));
        let event = first.event.as_ref().unwrap();
        assert_eq!((event.sensor_type, event.sensor_number, event.event_type), (0x01, 4, 0x01));
        assert!(event.asserted);
        assert_eq!(event.describe().0, "upper critical going high");
        assert_eq!(event.threshold_readings(), Some((0x5A, 0x55)));
        let second = &entries[1];
        assert_eq!(second.when, Some(When::Clock(1_794_000_000)));
        assert_eq!(second.event.as_ref().unwrap().describe().0, "failure detected");
        assert_eq!(second.event.as_ref().unwrap().threshold_readings(), None);
        let third = entries[2].event.as_ref().unwrap();
        assert!(!third.asserted);
        assert_eq!(third.describe(), ("hard reset, SMS or OS timer".to_string(), Severity::Info));
    }

    #[test]
    fn an_empty_log_has_no_entries() {
        let mut bmc = Sim::new();
        assert!(read_log(&mut bmc).unwrap().is_empty());
    }

    #[test]
    fn oem_records_keep_their_bytes() {
        let mut bmc = Sim::new();
        let mut oem = sim::sel_entry(5, 1_794_000_000, 0, 0, 0, [0; 3]);
        oem[2] = 0xC5;
        let mut untimed = sim::sel_entry(6, 0, 0, 0, 0, [0; 3]);
        untimed[2] = 0xF0;
        bmc.sel = vec![oem, untimed];
        let entries = read_log(&mut bmc).unwrap();
        assert_eq!(entries[0].kind, 0xC5);
        assert_eq!(entries[0].when, Some(When::Clock(1_794_000_000)));
        assert!(entries[0].event.is_none());
        assert_eq!(entries[1].when, None);
        assert_eq!(entries[1].raw[2], 0xF0);
    }

    #[test]
    fn clearing_reserves_sends_clr_and_waits_for_the_erase() {
        let mut bmc = Sim::new();
        bmc.sel = vec![sim::sel_entry(1, 100, 0x01, 4, 0x01, [0x59, 0x5A, 0x55])];
        clear_log(&mut bmc).unwrap();
        assert!(bmc.sel.is_empty());
        assert!(read_log(&mut bmc).unwrap().is_empty());
        assert_eq!(bmc.erase_requests, 1);
    }

    #[test]
    fn a_clear_without_the_reservation_is_refused_by_the_bmc() {
        let mut bmc = Sim::new();
        bmc.sel = vec![sim::sel_entry(1, 100, 0x01, 4, 0x01, [0x59, 0x5A, 0x55])];
        let reply = bmc.request(NETFN_STORAGE, CLEAR_SEL, &[0, 0, b'C', b'L', b'R', 0xAA]).unwrap();
        assert_eq!(reply, [0xC5]);
        assert_eq!(bmc.sel.len(), 1);
    }

    #[test]
    fn the_info_says_how_full_the_log_is() {
        let mut bmc = Sim::new();
        bmc.sel = vec![sim::sel_entry(1, 100, 0x01, 4, 0x01, [0, 0, 0])];
        let found = info(&mut bmc).unwrap();
        assert_eq!(found.entries, 1);
        assert!(found.reserves);
        assert!(!found.overflow);
        assert_eq!(found.free, 0x0F00 - 16);
        assert_eq!((found.added, found.erased), (0, 0));
        bmc.sel_added = 0x0A0B_0C0D;
        bmc.sel_erased = 9;
        let found = info(&mut bmc).unwrap();
        assert_eq!((found.added, found.erased), (0x0A0B_0C0D, 9));
    }
}
