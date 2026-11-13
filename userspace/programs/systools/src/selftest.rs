// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A disk's own self-tests (ADR 0488), through radix-block(2)'s
//! SELF_TEST: started, aborted and followed for hardware(8). The disk
//! runs a test by itself while it is used, and keeps a log of the last
//! ones, which this reads in both of its forms: a SATA disk's SMART
//! self-test log (ATA/ATAPI-8, A.12) and an NVMe controller's Device
//! Self-test log (NVMe 2.0, 5.16.1.7).

use std::fs::File;
use std::os::fd::AsRawFd;

use radix_abi::errors::Kernel;
use radix_abi::{block, file};

use crate::sys::{self, SYS};
use crate::util::{self, Fail};

/// Which test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Test {
    /// A minute or two: the disk's electronics and a sample of its
    /// surface.
    Short,
    /// Every sector read: hours on a large disk.
    Extended,
}

/// How a test ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Passed,
    /// Aborted by a request, or by the disk for a reason that is not a
    /// fault: a format, a namespace taken away.
    Aborted,
    /// Cut short by a reset or a power cycle.
    Interrupted,
    /// The disk found a fault; the first sector it could not read, in its
    /// own logical sectors, where it says.
    Failed { first_bad_sector: Option<u64> },
    /// A status this does not know, as the disk gave it.
    Other(u8),
}

/// A test in the disk's log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    /// `None` for a test this did not start, such as a SATA disk's
    /// conveyance or selective test.
    pub test: Option<Test>,
    pub outcome: Outcome,
    /// The disk's power-on hours when it ran.
    pub power_on_hours: Option<u64>,
}

/// How a disk's self-tests go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct State {
    pub running: bool,
    /// How much of the test that runs is left, in percent.
    pub percent_left: u8,
    /// The newest result in the disk's log.
    pub newest: Option<Entry>,
    /// The disk's own estimate of each test's length, in minutes: the
    /// short and the extended, where it gives them.
    pub minutes: Option<(u32, u32)>,
}

/// SELF_TEST's actions.
const STATUS: u32 = block::SELF_TEST_STATUS;

/// The self-tests of disk `name` (as in `/devices/NAME`); `None` when the
/// disk runs none.
pub(crate) fn state(name: &str) -> Result<Option<State>, Fail> {
    ask(name, STATUS)
}

/// Starts test `test` on disk `name`. A SATA disk aborts a test that runs
/// to start this one; an NVMe controller refuses.
pub(crate) fn start(name: &str, test: Test) -> Result<(), Fail> {
    let action = match test {
        Test::Short => block::SELF_TEST_SHORT,
        Test::Extended => block::SELF_TEST_EXTENDED,
    };
    match ask(name, action)? {
        Some(_) => Ok(()),
        None => Err(util::fail(format!("{name}: the disk runs no self-test"))),
    }
}

/// Aborts the test that runs on disk `name`.
pub(crate) fn abort(name: &str) -> Result<(), Fail> {
    ask(name, block::SELF_TEST_ABORT).map(drop)
}

/// SELF_TEST with `action` on disk `name`, opened for writing to start or
/// abort a test.
fn ask(name: &str, action: u32) -> Result<Option<State>, Fail> {
    let path = format!("/devices/{name}");
    let device = File::options()
        .read(true)
        .write(action != STATUS)
        .open(&path)
        .map_err(|e| util::failed(&path, e))?;
    let mut value = [0u8; block::SELF_TEST_SIZE as usize];
    value[..4].copy_from_slice(&action.to_le_bytes());
    let address = value.as_mut_ptr() as u64;
    let slot = sys::slot_of(device.as_raw_fd());
    let len = value.len() as u64;
    match file::control(&SYS, slot, block::SELF_TEST, address, address, len) {
        Ok(_) => Ok(Some(parse(&value))),
        Err(status) if Kernel::from_status(status) == Some(Kernel::NotSupported) => Ok(None),
        Err(status) => Err(util::failed(&path, sys::io_error(status))),
    }
}

/// SELF_TEST's answer: its head of 16 bytes and the disk's log.
fn parse(value: &[u8; block::SELF_TEST_SIZE as usize]) -> State {
    let kind = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
    let minutes = (
        u16::from_le_bytes([value[6], value[7]]),
        u16::from_le_bytes([value[8], value[9]]),
    );
    let log: &[u8; 512] = value[16..].try_into().expect("SELF_TEST_SIZE is 16 + 512");
    State {
        running: value[4] != 0,
        percent_left: value[5].min(100),
        newest: if kind == block::HEALTH_NVME { nvme_newest(log) } else { ata_newest(log) },
        minutes: (minutes != (0, 0)).then(|| (u32::from(minutes.0), u32::from(minutes.1))),
    }
}

/// The newest test in a SATA disk's SMART self-test log: 21 descriptors
/// of 24 bytes from byte 2, used in a ring, and at byte 508 the newest's
/// number, from 1, or 0 for none yet.
fn ata_newest(log: &[u8; 512]) -> Option<Entry> {
    let index = usize::from(log[508]);
    if !(1..=21).contains(&index) {
        return None;
    }
    let at = 2 + (index - 1) * 24;
    let entry = &log[at..at + 24];
    // The routine as the LBA low register named it: 1 and 2 off-line,
    // 0x81 and 0x82 in captive mode.
    let test = match entry[0] & 0x7f {
        1 => Some(Test::Short),
        2 => Some(Test::Extended),
        _ => None,
    };
    let status = entry[1] >> 4;
    let lba = u32::from_le_bytes([entry[5], entry[6], entry[7], entry[8]]);
    let outcome = match status {
        0 => Outcome::Passed,
        1 => Outcome::Aborted,
        2 => Outcome::Interrupted,
        // A fatal error, or an element that failed: electrical, servo,
        // read, or handling damage. All ones means no sector is named.
        3..=8 => Outcome::Failed {
            first_bad_sector: (lba != u32::MAX && lba != 0).then_some(u64::from(lba)),
        },
        other => Outcome::Other(other),
    };
    let hours = u16::from_le_bytes([entry[2], entry[3]]);
    Some(Entry { test, outcome, power_on_hours: Some(u64::from(hours)) })
}

/// The newest test in the head of an NVMe controller's Device Self-test
/// log: after four bytes of the test that runs, results of 28 bytes,
/// newest first, a result of 0xF marking an entry not used.
fn nvme_newest(log: &[u8; 512]) -> Option<Entry> {
    let entry = &log[4..32];
    let result = entry[0] & 0xf;
    if result == 0xf {
        return None;
    }
    let test = match entry[0] >> 4 {
        1 => Some(Test::Short),
        2 => Some(Test::Extended),
        _ => None,
    };
    // Byte 2 says which of the diagnostic fields hold something; bit 1,
    // the failing LBA.
    let lba_valid = entry[2] & 0b10 != 0;
    let lba = u64::from_le_bytes(entry[16..24].try_into().expect("8 bytes"));
    let outcome = match result {
        0 => Outcome::Passed,
        // By Device Self-test, a reset, a namespace removed, a format, an
        // unknown reason, a sanitize: none a fault of the disk.
        1 | 3 | 4 | 8 | 9 => Outcome::Aborted,
        2 => Outcome::Interrupted,
        // A fatal error, a segment that failed for an unknown reason, or
        // segments that failed.
        5..=7 => Outcome::Failed { first_bad_sector: lba_valid.then_some(lba) },
        other => Outcome::Other(other),
    };
    let hours = u64::from_le_bytes(entry[4..12].try_into().expect("8 bytes"));
    Some(Entry { test, outcome, power_on_hours: Some(hours) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(kind: u32, running: bool, left: u8, minutes: (u16, u16)) -> [u8; 528] {
        let mut value = [0u8; 528];
        value[..4].copy_from_slice(&kind.to_le_bytes());
        (value[4], value[5]) = (u8::from(running), left);
        value[6..8].copy_from_slice(&minutes.0.to_le_bytes());
        value[8..10].copy_from_slice(&minutes.1.to_le_bytes());
        value
    }

    #[test]
    fn a_sata_log_gives_its_newest_descriptor() {
        let mut value = answer(block::HEALTH_ATA, false, 0, (2, 54));
        let log = &mut value[16..];
        // Descriptor 1, an old short test that passed; descriptor 2, the
        // newest, an extended test whose read element failed at 0x1234.
        (log[2], log[3], log[4]) = (0x01, 0x00, 10);
        let at = 2 + 24;
        (log[at], log[at + 1], log[at + 2], log[at + 3]) = (0x02, 0x70, 0x34, 0x12);
        log[at + 5..at + 9].copy_from_slice(&0x1234u32.to_le_bytes());
        log[508] = 2;
        let state = parse(&value);
        assert!(!state.running);
        assert_eq!(state.minutes, Some((2, 54)));
        let newest = state.newest.unwrap();
        assert_eq!(newest.test, Some(Test::Extended));
        assert_eq!(newest.outcome, Outcome::Failed { first_bad_sector: Some(0x1234) });
        assert_eq!(newest.power_on_hours, Some(0x1234));
    }

    #[test]
    fn a_sata_log_with_no_tests_has_no_newest() {
        let state = parse(&answer(block::HEALTH_ATA, true, 70, (0, 0)));
        assert!(state.running);
        assert_eq!(state.percent_left, 70);
        assert_eq!((state.newest, state.minutes), (None, None));
    }

    #[test]
    fn sata_statuses_read_as_outcomes() {
        for (status, outcome) in [
            (0x00, Outcome::Passed),
            (0x10, Outcome::Aborted),
            (0x20, Outcome::Interrupted),
            (0x30, Outcome::Failed { first_bad_sector: None }),
            (0x90, Outcome::Other(9)),
        ] {
            let mut value = answer(block::HEALTH_ATA, false, 0, (1, 1));
            let log = &mut value[16..];
            (log[2], log[3]) = (0x81, status);
            log[7..11].copy_from_slice(&u32::MAX.to_le_bytes());
            log[508] = 1;
            let newest = parse(&value).newest.unwrap();
            assert_eq!((newest.test, newest.outcome), (Some(Test::Short), outcome));
        }
    }

    #[test]
    fn an_nvme_log_gives_its_first_result() {
        let mut value = answer(block::HEALTH_NVME, true, 60, (2, 25));
        let log = &mut value[16..];
        // A short test runs, 40% done; the newest result, an extended
        // test that failed segments at block 0x1234.
        (log[0], log[1]) = (0x1, 40);
        (log[4], log[6]) = (0x27, 0b10);
        log[8..16].copy_from_slice(&1234u64.to_le_bytes());
        log[20..28].copy_from_slice(&0x1234u64.to_le_bytes());
        let state = parse(&value);
        assert!(state.running);
        assert_eq!(state.percent_left, 60);
        let newest = state.newest.unwrap();
        assert_eq!(newest.test, Some(Test::Extended));
        assert_eq!(newest.outcome, Outcome::Failed { first_bad_sector: Some(0x1234) });
        assert_eq!(newest.power_on_hours, Some(1234));
    }

    #[test]
    fn an_nvme_log_names_no_sector_it_does_not_mark_valid() {
        let mut value = answer(block::HEALTH_NVME, false, 0, (2, 25));
        let log = &mut value[16..];
        log[4] = 0x15;
        log[20..28].copy_from_slice(&0x1234u64.to_le_bytes());
        let newest = parse(&value).newest.unwrap();
        assert_eq!(newest.outcome, Outcome::Failed { first_bad_sector: None });
    }

    #[test]
    fn an_empty_nvme_log_has_no_newest() {
        let mut value = answer(block::HEALTH_NVME, false, 0, (2, 0));
        value[16 + 4] = 0x0f;
        assert_eq!(parse(&value).newest, None);
        value[16 + 4] = 0x11;
        assert_eq!(parse(&value).newest.unwrap().outcome, Outcome::Aborted);
    }
}
