// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The disks area of `hardware check`: for each whole disk that is not
//! a drive for media that comes and goes, what the disk says of its health
//! (diskhealth's verdict and early warning, from SMART or the NVMe log),
//! its own self-test, and a scan that reads it.
//!
//! One thread takes a disk through the three in that order, so that the
//! reads of the scan do not meet a self-test under way, and the disks go
//! side by side. A line is given as soon as its check ends, and the scan
//! of a disk that failed its self-test is made all the same, since it
//! may name the sectors.

use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use super::check::{Area, Check, Problem, counted};
use super::drivetest;
use super::scan::{self, PIECE, Range, SLOW, Scan};
use crate::diskhealth::{self, Record, Verdict};
use crate::probe;
use crate::selftest::Test;
use crate::util::{self, Fail, fail};

/// A whole disk, as the kernel lists it in `/kernel/block`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Disk {
    pub(crate) name: String,
    pub(crate) model: String,
    pub(crate) bytes: u64,
    /// Its logical sector.
    pub(crate) sector: u64,
    pub(crate) rotational: bool,
}

impl Disk {
    /// What the kernel shows of disk `name`.
    fn of(name: &str) -> Disk {
        let number = |what: &str| probe::block_text(name, what)?.parse::<u64>().ok();
        Disk {
            name: name.to_string(),
            model: diskhealth::device_text(name, "model"),
            // `size` counts sectors of 512 bytes, as Linux's does.
            bytes: number("size").unwrap_or(0) * 512,
            sector: number("queue/logical_block_size")
                .filter(|sector| *sector >= 512)
                .unwrap_or(512),
            rotational: number("queue/rotational") == Some(1),
        }
    }

    /// What names the disk in a line: its model, or its name.
    fn shown(&self) -> &str {
        if self.model.is_empty() { &self.name } else { &self.model }
    }
}

/// The disks to check: `only` if it names one, else every whole disk
/// that is not a drive for media that comes and goes (a CD or floppy
/// drive).
pub(crate) fn list(only: Option<&str>) -> Result<Vec<Disk>, Fail> {
    let names = diskhealth::disks().map_err(|e| util::failed(&"/kernel/block", e))?;
    let Some(wanted) = only else {
        let fixed = |name: &&String| probe::block_text(name, "removable").as_deref() != Some("1");
        return Ok(names.iter().filter(fixed).map(|name| Disk::of(name)).collect());
    };
    let wanted = wanted.strip_prefix("/devices/").unwrap_or(wanted);
    if names.iter().any(|name| name == wanted) {
        return Ok(vec![Disk::of(wanted)]);
    }
    Err(fail(format!("no disk named {wanted}; the disks are {}", names.join(", "))))
}

/// How long the checks of `disk` take: its self-test of `test_minutes`,
/// then the scan at the speed disks of its kind read: a quarter of a
/// gigabyte and more a second for a solid state disk.
pub(crate) fn estimate(disk: &Disk, test_minutes: u32, full: bool) -> Duration {
    let speed: u64 = if disk.rotational { 100 << 20 } else { 400 << 20 };
    let bytes = if full { disk.bytes } else { disk.bytes.min(256 * PIECE) };
    Duration::from_secs(u64::from(test_minutes) * 60 + bytes / speed)
}

/// Its minutes of self-test: what the disk says, or nothing for a disk
/// that runs none.
pub(crate) fn test_minutes(disk: &Disk, full: bool) -> u32 {
    let test = if full { Test::Extended } else { Test::Short };
    match crate::selftest::state(&disk.name) {
        Ok(Some(state)) => drivetest::minutes(state.minutes, test),
        _ => 0,
    }
}

/// The check of what disk `disk` says of its health: `record` is what it
/// gave, `early` what diskhealth's early warning says of it.
pub(crate) fn health_verdict(disk: &Disk, record: &Record, early: Option<String>) -> Check {
    let name = format!("{} health", disk.name);
    let (_, report) = diskhealth::report(record);
    let model = disk.shown();
    let mut reasons = report.reasons.clone();
    reasons.extend(early);
    let reasons = reasons.join("; ");
    match report.verdict {
        Verdict::Failing => Check::failed(
            Area::Disks,
            &name,
            Problem::DiskFailing,
            format!("{model} says it is failing: {reasons}"),
        ),
        Verdict::Warning => Check::warning(
            Area::Disks,
            &name,
            Problem::DiskEarlyWarning,
            format!("{model}: {reasons}"),
        ),
        Verdict::Passed if !reasons.is_empty() => Check::warning(
            Area::Disks,
            &name,
            Problem::DiskEarlyWarning,
            format!("{model}: {reasons}"),
        ),
        Verdict::Passed => {
            let mut detail = format!("{model}: {}", report.verdict.name());
            if let Some(celsius) = report.celsius {
                detail.push_str(&format!(", {celsius} °C"));
            }
            if let Some(hours) = report.hours {
                detail.push_str(&format!(", {} hours on", diskhealth::grouped(u128::from(hours))));
            }
            Check::passed(Area::Disks, &name, detail)
        }
    }
}

fn health(disk: &Disk) -> Check {
    let name = format!("{} health", disk.name);
    match diskhealth::read(&disk.name) {
        Err(fail) => Check::skipped(Area::Disks, &name, fail.message),
        Ok(None) => Check::skipped(Area::Disks, &name, "it keeps no record of its health".into()),
        Ok(Some(record)) => {
            let early = diskhealth::early_warning_title(&disk.name, &record);
            health_verdict(disk, &record, early)
        }
    }
}

/// The ranges as a list: `20000, 30000-30003`.
fn ranges_text(ranges: &[Range]) -> String {
    let named: Vec<String> = ranges
        .iter()
        .map(|range| {
            if range.first == range.last {
                range.first.to_string()
            } else {
                format!("{}-{}", range.first, range.last)
            }
        })
        .collect();
    named.join(", ")
}

/// The check of what the scan of `disk` found.
pub(crate) fn scan_verdict(disk: &Disk, found: &Scan) -> Check {
    let name = format!("{} read scan", disk.name);
    let read = util::human(found.bytes, false);
    if !found.unreadable.is_empty() {
        let more = if found.unnarrowed > 0 {
            format!(" and more in {} other places", found.unnarrowed)
        } else {
            String::new()
        };
        let detail =
            format!("unreadable sectors: {}{more}; {read} read", ranges_text(&found.unreadable));
        return Check::failed(Area::Disks, &name, Problem::DiskUnreadable, detail);
    }
    let mut trouble = Vec::new();
    if found.slow > 0 {
        trouble.push(format!(
            "{} of {} took over {} seconds, the slowest {:.1}",
            counted(found.slow, "read"),
            util::human(PIECE, false),
            SLOW.as_secs(),
            found.slowest.as_secs_f64()
        ));
    }
    if found.retried > 0 {
        let reads = counted(found.retried, "read");
        trouble.push(format!("{reads} failed and worked when tried again"));
    }
    if !trouble.is_empty() {
        return Check::warning(Area::Disks, &name, Problem::DiskSlow, trouble.join("; "));
    }
    if found.stopped {
        return Check::skipped(Area::Disks, &name, format!("stopped after reading {read}"));
    }
    let total = util::human(disk.bytes, false);
    Check::passed(
        Area::Disks,
        &name,
        format!("read {read} of the disk's {total}; none unreadable, none slow"),
    )
}

/// Reads disk `disk` and gives the check of the scan.
fn read_scan(disk: &Disk, full: bool, stop: &AtomicBool) -> Check {
    let name = format!("{} read scan", disk.name);
    let path = format!("/devices/{}", disk.name);
    if disk.bytes == 0 {
        return Check::skipped(Area::Disks, &name, "the kernel shows no size for the disk".into());
    }
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) => {
            return Check::skipped(Area::Disks, &name, util::failed(&path, error).message);
        }
    };
    // A piece is at most PIECE bytes long, which fits the buffer.
    let mut buffer = vec![0u8; PIECE as usize];
    let mut read = |offset: u64, length: u64| -> io::Result<Duration> {
        let started = Instant::now();
        file.read_exact_at(&mut buffer[..length as usize], offset)?;
        Ok(started.elapsed())
    };
    let found =
        scan::scan(scan::pieces(disk.bytes, disk.sector, full), disk.sector, &mut read, stop);
    scan_verdict(disk, &found)
}

/// Takes `disk` through its checks, giving each line to `emit` when it
/// is done.
pub(crate) fn check(disk: &Disk, full: bool, stop: &AtomicBool, emit: &(dyn Fn(Check) + Sync)) {
    emit(health(disk));
    emit(drivetest::run(&disk.name, full, stop));
    emit(read_scan(disk, full, stop));
}

#[cfg(test)]
mod tests {
    use super::super::check::Outcome;
    use super::*;
    use crate::diskhealth::NvmeLog;

    fn disk() -> Disk {
        Disk {
            name: "nvme0n1".into(),
            model: "QEMU NVMe Ctrl".into(),
            bytes: 64 << 20,
            sector: 512,
            rotational: false,
        }
    }

    fn healthy() -> NvmeLog {
        NvmeLog { kelvin: 323, hours: 1234, ..NvmeLog::default() }
    }

    #[test]
    fn a_disk_that_passes_says_its_temperature_and_hours() {
        let check = health_verdict(&disk(), &Record::Nvme(healthy()), None);
        assert_eq!(check.outcome, Outcome::Passed);
        assert_eq!(check.name, "nvme0n1 health");
        assert_eq!(check.detail, "QEMU NVMe Ctrl: PASSED, 50 °C, 1,234 hours on");
    }

    #[test]
    fn a_disk_that_says_it_is_failing_fails_with_its_reasons() {
        let log = NvmeLog { critical: 1 << 2, ..healthy() };
        let check = health_verdict(&disk(), &Record::Nvme(log), None);
        assert_eq!(check.outcome, Outcome::Failed);
        assert_eq!(check.id.as_deref(), Some("HW-111"));
        assert_eq!(
            check.detail,
            "QEMU NVMe Ctrl says it is failing: media errors have degraded its reliability"
        );
    }

    #[test]
    fn counts_that_come_before_a_failure_warn_and_an_early_warning_is_added() {
        let log = NvmeLog { media_errors: 3, ..healthy() };
        let check = health_verdict(&disk(), &Record::Nvme(log), None);
        assert_eq!((check.outcome, check.id.as_deref()), (Outcome::Warning, Some("HW-115")));
        assert_eq!(check.detail, "QEMU NVMe Ctrl: 3 media and data integrity errors");
        let early = Some("Disk X has 1 more media error since 2026-11-13 10:00".to_string());
        let check = health_verdict(&disk(), &Record::Nvme(log), early.clone());
        assert!(check.detail.ends_with("; Disk X has 1 more media error since 2026-11-13 10:00"));
        // A disk that passes may still have the early warning.
        let check = health_verdict(&disk(), &Record::Nvme(healthy()), early);
        assert_eq!((check.outcome, check.id.as_deref()), (Outcome::Warning, Some("HW-115")));
        let off = health_verdict(&disk(), &Record::AtaOff, None);
        assert_eq!(off.outcome, Outcome::Warning);
        assert_eq!(off.detail, "QEMU NVMe Ctrl: its SMART is off");
    }

    #[test]
    fn a_disk_without_a_model_is_named_by_its_name() {
        let nameless = Disk { model: String::new(), ..disk() };
        let check = health_verdict(&nameless, &Record::Nvme(healthy()), None);
        assert!(check.detail.starts_with("nvme0n1: PASSED"), "{}", check.detail);
    }

    #[test]
    fn ranges_are_listed_with_dashes_for_the_longer_ones() {
        let ranges = [Range { first: 20000, last: 20000 }, Range { first: 30000, last: 30003 }];
        assert_eq!(ranges_text(&ranges), "20000, 30000-30003");
    }

    #[test]
    fn unreadable_sectors_fail_the_scan_and_are_named() {
        let found = Scan {
            bytes: 64 << 20,
            unreadable: vec![
                Range { first: 40000, last: 40000 },
                Range { first: 70000, last: 70001 },
            ],
            sectors: 3,
            ..Scan::default()
        };
        let check = scan_verdict(&disk(), &found);
        assert_eq!((check.outcome, check.id.as_deref()), (Outcome::Failed, Some("HW-113")));
        assert_eq!(check.name, "nvme0n1 read scan");
        assert_eq!(check.detail, "unreadable sectors: 40000, 70000-70001; 64M read");
        let more = Scan { unnarrowed: 4, ..found };
        assert!(scan_verdict(&disk(), &more).detail.contains("70001 and more in 4 other places"));
    }

    #[test]
    fn slow_reads_and_reads_that_had_to_be_repeated_warn() {
        let slow = Scan {
            bytes: 64 << 20,
            slow: 2,
            slowest: Duration::from_millis(3400),
            ..Scan::default()
        };
        let check = scan_verdict(&disk(), &slow);
        assert_eq!((check.outcome, check.id.as_deref()), (Outcome::Warning, Some("HW-114")));
        assert_eq!(check.detail, "2 reads of 4.0M took over 2 seconds, the slowest 3.4");
        let both = Scan { retried: 1, ..slow };
        assert!(
            scan_verdict(&disk(), &both)
                .detail
                .ends_with("; 1 read failed and worked when tried again")
        );
    }

    #[test]
    fn a_scan_with_nothing_wrong_passes_and_a_stopped_one_says_so() {
        let found = Scan { bytes: 64 << 20, ..Scan::default() };
        let check = scan_verdict(&disk(), &found);
        assert_eq!(check.outcome, Outcome::Passed);
        assert_eq!(check.detail, "read 64M of the disk's 64M; none unreadable, none slow");
        let stopped = Scan { bytes: 8 << 20, stopped: true, ..Scan::default() };
        let check = scan_verdict(&disk(), &stopped);
        assert_eq!(check.outcome, Outcome::Skipped);
        assert_eq!(check.detail, "stopped after reading 8.0M");
        // What was found before the stop still counts.
        let bad = Scan { stopped: true, unreadable: vec![Range { first: 1, last: 1 }], ..stopped };
        assert_eq!(scan_verdict(&disk(), &bad).outcome, Outcome::Failed);
    }

    #[test]
    fn the_time_a_disk_takes_is_its_test_and_its_scan() {
        let solid = Disk { bytes: 2 << 40, ..disk() };
        // A quick scan reads 1 GiB at 400 MiB a second, after a 2-minute
        // test.
        assert_eq!(estimate(&solid, 2, false), Duration::from_secs(120 + 2));
        let spinning = Disk { rotational: true, ..solid.clone() };
        assert_eq!(estimate(&spinning, 2, false), Duration::from_secs(120 + 10));
        // A full scan reads all of 2 TiB.
        assert_eq!(
            estimate(&spinning, 180, true),
            Duration::from_secs(180 * 60 + (2 << 40) / (100 << 20))
        );
        assert_eq!(estimate(&disk(), 0, false), Duration::ZERO);
    }
}
