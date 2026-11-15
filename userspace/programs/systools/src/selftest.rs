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
    Failed {
        first_bad_sector: Option<u64>,
    },
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

/// What kind of test a log's entry is, as the console names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Short,
    Extended,
    /// A SATA disk's check for damage in transport.
    Conveyance,
    /// A test this does not name, such as a selective one, or a
    /// vendor's own.
    Other,
}

impl Kind {
    /// The word the JSON gives it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Kind::Short => "short",
            Kind::Extended => "extended",
            Kind::Conveyance => "conveyance",
            Kind::Other => "other",
        }
    }
}

/// A test in the disk's log, or the one that runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Logged {
    pub kind: Kind,
    /// How it ended; `None` while it runs.
    pub outcome: Option<Outcome>,
    /// The disk's power-on hours when it ran; `None` for one that runs,
    /// whose hours the log does not give.
    pub power_on_hours: Option<u64>,
}

/// How many tests of the log `history` gives.
pub(crate) const HISTORY: usize = 5;

/// SELF_TEST's actions.
const STATUS: u32 = block::SELF_TEST_STATUS;

/// The self-tests of disk `name` (as in `/devices/NAME`); `None` when the
/// disk runs none.
pub(crate) fn state(name: &str) -> Result<Option<State>, Fail> {
    ask(name, STATUS)
}

/// The tests in the log of disk `name`, the newest first, at most
/// `HISTORY`, and the one that runs first of all; `None` when the disk
/// keeps no log.
pub(crate) fn history(name: &str) -> Result<Option<Vec<Logged>>, Fail> {
    Ok(ask_raw(name, STATUS)?.map(|value| logged(&value)))
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
    Ok(ask_raw(name, action)?.map(|value| parse(&value)))
}

/// SELF_TEST's answer to `action` on disk `name`, as the disk gave it;
/// `None` when the disk runs no self-test.
fn ask_raw(name: &str, action: u32) -> Result<Option<[u8; block::SELF_TEST_SIZE as usize]>, Fail> {
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
        Ok(_) => Ok(Some(value)),
        Err(status) if Kernel::from_status(status) == Some(Kernel::NotSupported) => Ok(None),
        Err(status) => Err(util::failed(&path, sys::io_error(status))),
    }
}

/// SELF_TEST's answer: its head of 16 bytes and the disk's log.
fn parse(value: &[u8; block::SELF_TEST_SIZE as usize]) -> State {
    let kind = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
    let minutes =
        (u16::from_le_bytes([value[6], value[7]]), u16::from_le_bytes([value[8], value[9]]));
    let log: &[u8; 512] = value[16..].try_into().expect("SELF_TEST_SIZE is 16 + 512");
    State {
        running: value[4] != 0,
        percent_left: value[5].min(100),
        newest: if kind == block::HEALTH_NVME { nvme_newest(log) } else { ata_newest(log) },
        minutes: (minutes != (0, 0)).then(|| (u32::from(minutes.0), u32::from(minutes.1))),
    }
}

/// The number of descriptors in a SATA disk's SMART self-test log.
const ATA_DESCRIPTORS: usize = 21;

/// Descriptor `number`, from 1, of a SATA disk's self-test log: 24 bytes
/// from byte 2.
fn ata_descriptor(log: &[u8; 512], number: usize) -> &[u8] {
    let at = 2 + (number - 1) * 24;
    &log[at..at + 24]
}

/// How the test in a SATA descriptor ended: the status in the high
/// nibble of its byte 1.
fn ata_outcome(entry: &[u8]) -> Outcome {
    let status = entry[1] >> 4;
    let lba = u32::from_le_bytes([entry[5], entry[6], entry[7], entry[8]]);
    match status {
        0 => Outcome::Passed,
        1 => Outcome::Aborted,
        2 => Outcome::Interrupted,
        // A fatal error, or an element that failed: electrical, servo,
        // read, or handling damage. All ones means no sector is named.
        3..=8 => Outcome::Failed {
            first_bad_sector: (lba != u32::MAX && lba != 0).then_some(u64::from(lba)),
        },
        other => Outcome::Other(other),
    }
}

/// The status of a SATA test that runs.
const ATA_RUNNING: u8 = 0xf;

/// The routine of a SATA descriptor, as the LBA low register named it:
/// 1 and 2 off-line, 0x81 and 0x82 in captive mode, and so on for the
/// conveyance (3) and selective (4) tests.
fn ata_routine(entry: &[u8]) -> u8 {
    entry[0] & 0x7f
}

/// The newest test in a SATA disk's SMART self-test log: 21 descriptors
/// of 24 bytes from byte 2, used in a ring, and at byte 508 the newest's
/// number, from 1, or 0 for none yet.
fn ata_newest(log: &[u8; 512]) -> Option<Entry> {
    let index = usize::from(log[508]);
    if !(1..=ATA_DESCRIPTORS).contains(&index) {
        return None;
    }
    let entry = ata_descriptor(log, index);
    let test = match ata_routine(entry) {
        1 => Some(Test::Short),
        2 => Some(Test::Extended),
        _ => None,
    };
    let hours = u16::from_le_bytes([entry[2], entry[3]]);
    Some(Entry { test, outcome: ata_outcome(entry), power_on_hours: Some(u64::from(hours)) })
}

/// The tests in a SATA disk's log, the newest first: the ring read back
/// from its newest descriptor to one never written, which is all zeros
/// (a test always has a routine number).
fn ata_logged(log: &[u8; 512]) -> Vec<Logged> {
    let newest = usize::from(log[508]);
    if !(1..=ATA_DESCRIPTORS).contains(&newest) {
        return Vec::new();
    }
    (0..ATA_DESCRIPTORS)
        .map(|back| {
            ata_descriptor(log, (newest + ATA_DESCRIPTORS - 1 - back) % ATA_DESCRIPTORS + 1)
        })
        .take_while(|entry| entry.iter().any(|&byte| byte != 0))
        .take(HISTORY)
        .map(|entry| {
            let outcome = ata_outcome(entry);
            let running = outcome == Outcome::Other(ATA_RUNNING);
            Logged {
                kind: match ata_routine(entry) {
                    1 => Kind::Short,
                    2 => Kind::Extended,
                    3 => Kind::Conveyance,
                    _ => Kind::Other,
                },
                outcome: (!running).then_some(outcome),
                power_on_hours: (!running)
                    .then(|| u64::from(u16::from_le_bytes([entry[2], entry[3]]))),
            }
        })
        .collect()
}

/// The number of results that fit in the 512 bytes of an NVMe
/// controller's Device Self-test log we read: after four bytes of the
/// test that runs, results of 28 bytes.
const NVME_RESULTS: usize = (512 - 4) / 28;

/// Result `number`, from 0, in the head of an NVMe controller's Device
/// Self-test log.
fn nvme_result(log: &[u8; 512], number: usize) -> &[u8] {
    let at = 4 + number * 28;
    &log[at..at + 28]
}

/// How the test in an NVMe result ended; `None` for an entry not used,
/// marked by a result of 0xF.
fn nvme_outcome(entry: &[u8]) -> Option<Outcome> {
    let result = entry[0] & 0xf;
    if result == 0xf {
        return None;
    }
    // Byte 2 says which of the diagnostic fields hold something; bit 1,
    // the failing LBA.
    let lba_valid = entry[2] & 0b10 != 0;
    let lba = u64::from_le_bytes(entry[16..24].try_into().expect("8 bytes"));
    Some(match result {
        0 => Outcome::Passed,
        // By Device Self-test, a reset, a namespace removed, a format, an
        // unknown reason, a sanitize: none a fault of the disk.
        1 | 3 | 4 | 8 | 9 => Outcome::Aborted,
        2 => Outcome::Interrupted,
        // A fatal error, a segment that failed for an unknown reason, or
        // segments that failed.
        5..=7 => Outcome::Failed { first_bad_sector: lba_valid.then_some(lba) },
        other => Outcome::Other(other),
    })
}

/// The power-on hours an NVMe result holds.
fn nvme_hours(entry: &[u8]) -> u64 {
    u64::from_le_bytes(entry[4..12].try_into().expect("8 bytes"))
}

/// The newest test in the head of an NVMe controller's Device Self-test
/// log: after four bytes of the test that runs, results of 28 bytes,
/// newest first, a result of 0xF marking an entry not used.
fn nvme_newest(log: &[u8; 512]) -> Option<Entry> {
    let entry = nvme_result(log, 0);
    let outcome = nvme_outcome(entry)?;
    let test = match entry[0] >> 4 {
        1 => Some(Test::Short),
        2 => Some(Test::Extended),
        _ => None,
    };
    Some(Entry { test, outcome, power_on_hours: Some(nvme_hours(entry)) })
}

/// The tests in an NVMe controller's log: the one that runs, named by
/// the log's first byte, then the results, newest first, up to an entry
/// not used.
fn nvme_logged(log: &[u8; 512]) -> Vec<Logged> {
    let running = match log[0] & 0xf {
        0 => None,
        1 => Some(Kind::Short),
        2 => Some(Kind::Extended),
        _ => Some(Kind::Other),
    };
    let finished = (0..NVME_RESULTS).map_while(|number| {
        let entry = nvme_result(log, number);
        let outcome = nvme_outcome(entry)?;
        let kind = match entry[0] >> 4 {
            1 => Kind::Short,
            2 => Kind::Extended,
            _ => Kind::Other,
        };
        Some(Logged { kind, outcome: Some(outcome), power_on_hours: Some(nvme_hours(entry)) })
    });
    running
        .map(|kind| Logged { kind, outcome: None, power_on_hours: None })
        .into_iter()
        .chain(finished)
        .take(HISTORY)
        .collect()
}

/// The tests in SELF_TEST's answer.
fn logged(value: &[u8; block::SELF_TEST_SIZE as usize]) -> Vec<Logged> {
    let kind = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);
    let log: &[u8; 512] = value[16..].try_into().expect("SELF_TEST_SIZE is 16 + 512");
    if kind == block::HEALTH_NVME { nvme_logged(log) } else { ata_logged(log) }
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

    /// Puts a SATA descriptor `number` (from 1) in `log`: routine, status
    /// in the high nibble, hours.
    fn descriptor(log: &mut [u8], number: usize, routine: u8, status: u8, hours: u16) {
        let at = 2 + (number - 1) * 24;
        (log[at], log[at + 1]) = (routine, status << 4);
        log[at + 2..at + 4].copy_from_slice(&hours.to_le_bytes());
    }

    #[test]
    fn a_sata_history_reads_back_to_the_first_descriptor_never_written() {
        let mut value = answer(block::HEALTH_ATA, false, 0, (2, 54));
        let log = &mut value[16..];
        descriptor(log, 1, 0x01, 0, 100);
        descriptor(log, 2, 0x82, 3, 200);
        descriptor(log, 3, 0x03, 0, 300);
        log[508] = 3;
        assert_eq!(
            logged(&value),
            [
                Logged {
                    kind: Kind::Conveyance,
                    outcome: Some(Outcome::Passed),
                    power_on_hours: Some(300)
                },
                Logged {
                    kind: Kind::Extended,
                    outcome: Some(Outcome::Failed { first_bad_sector: None }),
                    power_on_hours: Some(200),
                },
                Logged {
                    kind: Kind::Short,
                    outcome: Some(Outcome::Passed),
                    power_on_hours: Some(100)
                },
            ]
        );
    }

    #[test]
    fn a_sata_history_wraps_the_ring_and_holds_five() {
        let mut value = answer(block::HEALTH_ATA, false, 0, (2, 54));
        let log = &mut value[16..];
        // A full ring whose newest is 2: then 1, 21, 20, 19 and no more.
        for number in 1..=ATA_DESCRIPTORS {
            descriptor(log, number, 0x01, 0, number as u16);
        }
        log[508] = 2;
        let hours: Vec<_> = logged(&value).iter().map(|test| test.power_on_hours).collect();
        assert_eq!(hours, [Some(2), Some(1), Some(21), Some(20), Some(19)]);
    }

    #[test]
    fn a_sata_test_that_runs_has_no_outcome_or_hours() {
        let mut value = answer(block::HEALTH_ATA, true, 50, (2, 54));
        let log = &mut value[16..];
        descriptor(log, 1, 0x01, 0, 100);
        // A selective test that runs.
        descriptor(log, 2, 0x04, ATA_RUNNING, 0);
        log[508] = 2;
        let tests = logged(&value);
        assert_eq!(tests[0], Logged { kind: Kind::Other, outcome: None, power_on_hours: None });
        assert_eq!(tests[1].outcome, Some(Outcome::Passed));
        assert_eq!(tests.len(), 2);
    }

    #[test]
    fn a_sata_log_with_no_tests_has_no_history() {
        assert_eq!(logged(&answer(block::HEALTH_ATA, false, 0, (2, 54))), []);
    }

    /// Puts an NVMe result `number` (from 0) in `log`: test code in the
    /// high nibble, result in the low, hours.
    fn result(log: &mut [u8], number: usize, test: u8, result: u8, hours: u64) {
        let at = 4 + number * 28;
        log[at] = test << 4 | result;
        log[at + 4..at + 12].copy_from_slice(&hours.to_le_bytes());
    }

    #[test]
    fn an_nvme_history_has_the_test_that_runs_then_the_results_to_an_unused_entry() {
        let mut value = answer(block::HEALTH_NVME, true, 60, (2, 25));
        let log = &mut value[16..];
        log[0] = 0x2;
        result(log, 0, 1, 0, 900);
        result(log, 1, 2, 7, 800);
        result(log, 2, 0xe, 1, 700);
        result(log, 3, 1, 0xf, 0);
        result(log, 4, 1, 0, 500);
        let tests = logged(&value);
        let kinds: Vec<_> = tests.iter().map(|test| test.kind).collect();
        assert_eq!(kinds, [Kind::Extended, Kind::Short, Kind::Extended, Kind::Other]);
        assert_eq!(tests[0].outcome, None);
        assert_eq!(tests[1].outcome, Some(Outcome::Passed));
        assert_eq!(tests[2].outcome, Some(Outcome::Failed { first_bad_sector: None }));
        assert_eq!(tests[3].outcome, Some(Outcome::Aborted));
        assert_eq!(tests[3].power_on_hours, Some(700));
    }

    #[test]
    fn an_nvme_history_holds_five_and_an_empty_log_none() {
        let mut value = answer(block::HEALTH_NVME, false, 0, (2, 25));
        // A controller marks every result it has not used with 0xF.
        value[16 + 4] = 0x0f;
        assert_eq!(logged(&value), []);
        let log = &mut value[16..];
        for number in 0..NVME_RESULTS {
            result(log, number, 1, 0, number as u64);
        }
        let hours: Vec<_> = logged(&value).iter().map(|test| test.power_on_hours).collect();
        assert_eq!(hours, [Some(0), Some(1), Some(2), Some(3), Some(4)]);
    }

    #[test]
    fn kinds_have_the_words_of_the_json() {
        let words: Vec<_> =
            [Kind::Short, Kind::Extended, Kind::Conveyance, Kind::Other].map(Kind::name).to_vec();
        assert_eq!(words, ["short", "extended", "conveyance", "other"]);
    }
}
