// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! diskhealth(8): what each disk says of its health (ADR 0157), read with
//! radix-block(2)'s GET_HEALTH: a SATA disk's SMART attributes, as
//! smartctl(8) shows them with `-A`, or an NVMe controller's SMART /
//! Health Information log, and a verdict from them. With `--watch`, as
//! Spark's diskhealth service, it looks again every 10 minutes and writes
//! a line, which Spark puts in the kernel log, whenever a disk's verdict
//! or its reasons change.
//!
//! A SATA disk in standby is never woken for a look: the kernel answers
//! that it sleeps, and the service keeps what it last read of the disk and
//! since when it sleeps.
//!
//! The service also keeps alertd's alerts of the disks (ADR 0485):
//! `smart:SERIAL` while a disk's verdict is not PASSED, and the early
//! warning `smart-rising:SERIAL` for a week after a count that tells of
//! a failure to come rises (reallocated, pending and offline
//! uncorrectable sectors, an NVMe drive's media errors), or while an NVMe
//! drive has used 90% of its rated endurance. What each disk counted at
//! the last look is kept in `/system/state/diskhealth`, a file a disk,
//! so that a rise across a restart is seen too.
//!
//! For the web console, `--json` prints what the disks say as one list,
//! and the service writes each disk's summary to `/system/live/diskhealth`,
//! which anyone may read, so that `storage show` can give a disk's health
//! without opening the disk, and `hardware sensors` and metricsd its
//! temperature. The file also says what kind of disk it is, since when it
//! sleeps, and the temperatures at which it is too hot: its own where it
//! gives them, else by its kind.

use std::fs::{self, File};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use radix_abi::errors::Kernel;
use radix_abi::{block, file};
use serde::{Deserialize, Serialize};

use crate::alerts::protocol::{Alert, Severity, key_part};
use crate::args::{Arg, Args};
use crate::disk_errors;
use crate::selftest::{self, Outcome};
use crate::sys::{self, SYS};
use crate::util::{self, Fail};

/// Where the kernel lists the disks and partitions (radix-sysfs(5)).
const BLOCKS: &str = "/kernel/block";
/// How long `--watch` waits between looks. A look reads the whole record
/// and the temperature with it, so each look is the same work.
const WATCH_EVERY: Duration = Duration::from_mins(10);
/// Where `--watch` keeps what each disk counted at its last look.
const KEPT: &str = "/system/state/diskhealth";
/// Where `--watch` writes each disk's summary, a file `DISK.json` each,
/// for anyone to read.
pub(crate) const LIVE: &str = "/system/live/diskhealth";
/// How long a rise is told of after the count stopped rising.
const RISE_TOLD: u64 = 7 * 86_400;
/// The share of its rated endurance, in percent, from which an NVMe
/// drive's use is told of before it is used up.
const USED_TOLD: u8 = 90;
/// The bits of the exit status: a disk could not be read, one warns,
/// and one is failing, as smartctl's bit 3 says a disk is failing; a
/// mistake in the command line is 1.
const UNREAD: i32 = 2;
const WARNS: i32 = 4;
const FAILS: i32 = 8;

/// A disk's verdict, the worst last; in JSON, in lower case.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Verdict {
    Passed,
    Warning,
    Failing,
}

impl Verdict {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Verdict::Passed => "PASSED",
            Verdict::Warning => "WARNING",
            Verdict::Failing => "FAILING",
        }
    }
}

/// One SMART attribute of a SATA disk: its ID, whether it says the disk
/// is about to fail (pre-fail) or only that it is old, its normalized
/// value and the worst it was, the threshold at or below which it fails,
/// and its raw 48 bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Attribute {
    id: u8,
    prefail: bool,
    value: u8,
    worst: u8,
    threshold: u8,
    raw: u64,
}

/// The fields of an NVMe controller's SMART / Health Information log
/// (NVMe 2.0, 5.16.1.3) that diskhealth shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct NvmeLog {
    pub(crate) critical: u8,
    pub(crate) kelvin: u16,
    pub(crate) spare: u8,
    pub(crate) spare_threshold: u8,
    pub(crate) used: u8,
    /// Thousands of 512-byte units.
    pub(crate) units_read: u128,
    pub(crate) units_written: u128,
    pub(crate) power_cycles: u128,
    pub(crate) hours: u128,
    pub(crate) unsafe_shutdowns: u128,
    pub(crate) media_errors: u128,
    pub(crate) error_entries: u128,
    /// Minutes above the warning and the critical temperature.
    pub(crate) warning_minutes: u32,
    pub(crate) critical_minutes: u32,
    /// The controller's own warning and critical composite temperatures,
    /// in kelvin; 0 where it gives none.
    pub(crate) warning_kelvin: u16,
    pub(crate) critical_kelvin: u16,
}

/// What a disk said of its health.
#[derive(Clone)]
pub(crate) enum Record {
    Ata(Vec<Attribute>),
    /// A SATA disk whose SMART is off.
    AtaOff,
    Nvme(NvmeLog),
}

/// The verdict on a disk and what it rests on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Report {
    pub(crate) verdict: Verdict,
    pub(crate) celsius: Option<i64>,
    pub(crate) hours: Option<u64>,
    /// An NVMe drive's share of its rated endurance used, in percent.
    pub(crate) used: Option<u8>,
    pub(crate) reasons: Vec<String>,
}

impl Report {
    fn new() -> Report {
        Report { verdict: Verdict::Passed, celsius: None, hours: None, used: None, reasons: vec![] }
    }

    fn add(&mut self, verdict: Verdict, reason: String) {
        self.verdict = self.verdict.max(verdict);
        self.reasons.push(reason);
    }
}

/// A name for SMART attribute `id`, of those most disks keep; their
/// meanings are common to the makers, though each may count its own way.
fn attribute_name(id: u8) -> &'static str {
    match id {
        1 => "Raw read error rate",
        2 => "Throughput",
        3 => "Spin-up time",
        4 => "Start and stop count",
        5 => "Reallocated sectors",
        7 => "Seek error rate",
        8 => "Seek time",
        9 => "Power-on hours",
        10 => "Spin retries",
        12 => "Power cycles",
        177 => "Wear leveling count",
        183 => "Runtime bad blocks",
        184 => "End-to-end errors",
        187 => "Reported uncorrectable",
        188 => "Command timeouts",
        190 => "Airflow temperature",
        191 => "Shock count",
        192 => "Power-off retracts",
        193 => "Load cycles",
        194 => "Temperature",
        196 => "Reallocation events",
        197 => "Pending sectors",
        198 => "Offline uncorrectable",
        199 => "Interface CRC errors",
        231 => "Life left",
        233 => "Media wearout",
        241 => "Total LBAs written",
        242 => "Total LBAs read",
        _ => "Unknown",
    }
}

/// The attributes in a SATA disk's SMART data and thresholds: 30
/// entries of 12 bytes from byte 2 of each (ATA/ATAPI-8), an ID of 0
/// marking an empty one.
fn attributes(data: &[u8], thresholds: &[u8]) -> Vec<Attribute> {
    let entry = |page: &[u8], i: usize| page.get(2 + 12 * i..14 + 12 * i).unwrap_or(&[]).to_vec();
    let mut found = Vec::new();
    for i in 0..30 {
        let at = entry(data, i);
        if at.len() < 12 || at[0] == 0 {
            continue;
        }
        let threshold = entry(thresholds, i);
        // The thresholds list the same IDs in the same places.
        let threshold =
            if threshold.len() >= 2 && threshold[0] == at[0] { threshold[1] } else { 0 };
        let mut raw = [0u8; 8];
        raw[..6].copy_from_slice(&at[5..11]);
        found.push(Attribute {
            id: at[0],
            prefail: at[1] & 1 != 0,
            value: at[3],
            worst: at[4],
            threshold,
            raw: u64::from_le_bytes(raw),
        });
    }
    found
}

/// The verdict on a SATA disk from its attributes: failing when one
/// that warns of failure is at or below its threshold, as the disk's
/// own SMART RETURN STATUS would say; a warning when one that tells of
/// age is, or when sectors were reallocated or wait to be.
fn ata_report(attributes: &[Attribute]) -> Report {
    let mut report = Report::new();
    for a in attributes {
        // A threshold of 0 never fails; values past FDh are not values.
        if a.threshold != 0 && (1..=0xfd).contains(&a.value) && a.value <= a.threshold {
            let (verdict, kind) = if a.prefail {
                (Verdict::Failing, "warns of failure")
            } else {
                (Verdict::Warning, "tells of age")
            };
            report.add(
                verdict,
                format!(
                    "attribute {} ({}), which {kind}, is at {} and its threshold is {}",
                    a.id,
                    attribute_name(a.id),
                    a.value,
                    a.threshold
                ),
            );
        }
        let count = a.raw & 0xffff_ffff;
        let counted = match a.id {
            5 => Some("sectors were reallocated"),
            197 => Some("sectors wait to be reallocated"),
            198 => Some("sectors could not be read offline"),
            _ => None,
        };
        if let Some(what) = counted
            && count > 0
        {
            report.add(Verdict::Warning, format!("{count} {what}"));
        }
    }
    let raw = |id: u8| attributes.iter().find(|a| a.id == id).map(|a| a.raw);
    report.celsius = raw(194).or(raw(190)).map(|raw| i64::from(raw as u8));
    report.hours = raw(9).map(|raw| raw & 0xffff_ffff);
    report
}

/// Reads an NVMe controller's SMART / Health Information log, and the
/// temperature limits GET_HEALTH gives after it in `thresholds`.
fn nvme_log(data: &[u8], thresholds: &[u8]) -> NvmeLog {
    let wide = |at: usize| {
        let mut bytes = [0u8; 16];
        if let Some(field) = data.get(at..at + 16) {
            bytes.copy_from_slice(field);
        }
        u128::from_le_bytes(bytes)
    };
    let byte = |at: usize| data.get(at).copied().unwrap_or(0);
    let word = |at: usize| u32::from_le_bytes([byte(at), byte(at + 1), byte(at + 2), byte(at + 3)]);
    let kelvin = |at: usize| {
        let pair = |i: usize| thresholds.get(i).copied().unwrap_or(0);
        u16::from_le_bytes([pair(at), pair(at + 1)])
    };
    NvmeLog {
        critical: byte(0),
        kelvin: u16::from_le_bytes([byte(1), byte(2)]),
        spare: byte(3),
        spare_threshold: byte(4),
        used: byte(5),
        units_read: wide(32),
        units_written: wide(48),
        power_cycles: wide(112),
        hours: wide(128),
        unsafe_shutdowns: wide(144),
        media_errors: wide(160),
        error_entries: wide(176),
        warning_minutes: word(192),
        critical_minutes: word(196),
        warning_kelvin: kelvin(0),
        critical_kelvin: kelvin(2),
    }
}

/// The verdict on an NVMe drive from its log: failing when its critical
/// warning says its spare is short, its reliability degraded, it went
/// read-only or its backup failed; a warning when it is too hot, has used
/// its rated endurance, or has had media errors.
fn nvme_report(log: &NvmeLog) -> Report {
    let mut report = Report::new();
    let warnings = [
        (0, Verdict::Failing, "its spare capacity is below its threshold"),
        (1, Verdict::Warning, "its temperature is past a threshold"),
        (2, Verdict::Failing, "media errors have degraded its reliability"),
        (3, Verdict::Failing, "it has gone read-only"),
        (4, Verdict::Failing, "its volatile memory backup failed"),
        (5, Verdict::Failing, "its persistent memory region has gone read-only"),
    ];
    for (bit, verdict, why) in warnings {
        if log.critical & (1 << bit) != 0 {
            report.add(verdict, why.to_string());
        }
    }
    if log.used >= 100 {
        report.add(Verdict::Warning, format!("it has used {}% of its rated endurance", log.used));
    }
    if log.media_errors > 0 {
        report
            .add(Verdict::Warning, format!("{} media and data integrity errors", log.media_errors));
    }
    report.celsius = (log.kelvin != 0).then(|| i64::from(log.kelvin) - 273);
    report.hours = Some(u64::try_from(log.hours).unwrap_or(u64::MAX));
    report.used = Some(log.used);
    report
}

/// The whole disks the kernel has, by name, in order: the entries of
/// `/kernel/block` that are not partitions.
pub(crate) fn disks() -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(BLOCKS)? {
        let entry = entry?;
        if !entry.path().join("partition").exists() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// Every name `/kernel/block` has, the partitions too, as a name asked
/// for must be one of these: a partition answers for its disk.
fn block_names() -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(BLOCKS)? {
        names.push(entry?.file_name().to_string_lossy().into_owned());
    }
    Ok(names)
}

/// A line of what the kernel shows of disk `name`'s device.
pub(crate) fn device_text(name: &str, what: &str) -> String {
    let path = format!("{BLOCKS}/{name}/device/{what}");
    fs::read_to_string(path).map_or_default(|text| text.trim().to_string())
}

/// What a look at a disk found.
pub(crate) enum Reading {
    /// What the disk said of its health.
    Record(Record),
    /// The disk keeps no record: a USB or virtual disk.
    None,
    /// A SATA disk in standby, or one that did not say: the kernel did
    /// not read it, so as not to wake it.
    Asleep,
}

/// Asks disk `name` of its health.
pub(crate) fn read(name: &str) -> Result<Reading, Fail> {
    let path = format!("/devices/{name}");
    let device = File::open(&path).map_err(|e| util::failed(&path, e))?;
    let mut value = [0u8; block::HEALTH_SIZE as usize];
    let address = value.as_mut_ptr() as u64;
    let slot = sys::slot_of(device.as_raw_fd());
    let len = value.len() as u64;
    match file::control(&SYS, slot, block::GET_HEALTH, address, address, len) {
        Ok(_) => {}
        Err(status) if Kernel::from_status(status) == Some(Kernel::NotSupported) => {
            return Ok(Reading::None);
        }
        Err(status) if Kernel::from_status(status) == Some(Kernel::WouldBlock) => {
            return Ok(Reading::Asleep);
        }
        Err(status) => return Err(util::failed(&path, sys::io_error(status))),
    }
    let word =
        |at: usize| u32::from_le_bytes([value[at], value[at + 1], value[at + 2], value[at + 3]]);
    let (data, thresholds) = (&value[8..520], &value[520..1032]);
    Ok(Reading::Record(match word(0) {
        block::HEALTH_NVME => Record::Nvme(nvme_log(data, thresholds)),
        _ if word(4) & block::HEALTH_SMART_OFF != 0 => Record::AtaOff,
        _ => Record::Ata(attributes(data, thresholds)),
    }))
}

/// The words that say why a disk was not read.
pub(crate) fn asleep_words(name: &str) -> String {
    format!("{name} is asleep (standby): it is not read, so as not to wake it")
}

/// The report on `record`, and its kind's name.
pub(crate) fn report(record: &Record) -> (&'static str, Report) {
    match record {
        Record::Ata(attributes) => ("SATA", ata_report(attributes)),
        Record::AtaOff => {
            let mut report = Report::new();
            report.add(Verdict::Warning, "its SMART is off".to_string());
            ("SATA", report)
        }
        Record::Nvme(log) => ("NVMe", nvme_report(log)),
    }
}

/// What kind of disk it is, which decides the temperatures it is judged
/// by when it gives none of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DiskKind {
    /// A SATA disk that spins.
    Hdd,
    /// A SATA disk of flash.
    Ssd,
    Nvme,
}

impl DiskKind {
    /// The kind in the words of the limit's explanation: "for a hard
    /// disk".
    pub(crate) fn words(self) -> &'static str {
        match self {
            DiskKind::Hdd => "a hard disk",
            DiskKind::Ssd => "a solid-state disk",
            DiskKind::Nvme => "an NVMe drive",
        }
    }
}

/// The warning and critical temperatures, in degrees Celsius, of each
/// kind of disk, for a disk that gives none of its own: most hard disks
/// are rated to 55 or 60 and fail more above 50, flash lasts to 70.
const LIMITS_BY_KIND: [(DiskKind, i64, i64); 3] =
    [(DiskKind::Hdd, 50, 60), (DiskKind::Ssd, 65, 70), (DiskKind::Nvme, 70, 80)];

/// Where a disk's limits come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum LimitsFrom {
    /// What the disk itself says.
    Disk,
    /// The usual for its kind.
    Kind,
}

/// The temperatures at which a disk is too hot, and where they come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Limits {
    pub(crate) warning_c: i64,
    pub(crate) critical_c: i64,
    pub(crate) from: LimitsFrom,
}

/// The kind of disk that said `record`: an NVMe controller is NVMe, and a
/// SATA disk spins unless the kernel says `rotational` is false.
pub(crate) fn kind_of(record: &Record, rotational: Option<bool>) -> DiskKind {
    match record {
        Record::Nvme(_) => DiskKind::Nvme,
        _ if rotational == Some(false) => DiskKind::Ssd,
        _ => DiskKind::Hdd,
    }
}

/// Whether the kernel says disk `name` spins.
fn rotational(name: &str) -> Option<bool> {
    let text = fs::read_to_string(format!("{BLOCKS}/{name}/queue/rotational")).ok()?;
    match text.trim() {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

/// The temperatures at which `record`'s disk, of `kind`, is too hot; none
/// when it gives no temperature.
///
/// An NVMe controller says its own warning and critical temperatures; one
/// it leaves out is the kind's. A SATA disk may say its critical one as
/// the threshold of its airflow temperature (attribute 190), which counts
/// down from 100 as the disk warms, so the threshold is a temperature
/// only where the value follows 100 less the temperature. The threshold of
/// attribute 194 is 0 on nearly every disk and is no limit.
pub(crate) fn limits_of(record: &Record, kind: DiskKind) -> Option<Limits> {
    let celsius = report(record).1.celsius?;
    let (kind_warning, kind_critical) = LIMITS_BY_KIND
        .iter()
        .find(|(of, ..)| *of == kind)
        .map_or((50, 60), |&(_, warning, critical)| (warning, critical));
    let by_kind =
        Limits { warning_c: kind_warning, critical_c: kind_critical, from: LimitsFrom::Kind };
    match record {
        Record::Nvme(log) => {
            let own = |kelvin: u16| (kelvin != 0).then(|| i64::from(kelvin) - 273);
            let (warning, critical) = (own(log.warning_kelvin), own(log.critical_kelvin));
            if warning.is_none() && critical.is_none() {
                return Some(by_kind);
            }
            let (mut warning_c, mut critical_c) =
                (warning.unwrap_or(kind_warning), critical.unwrap_or(kind_critical));
            if warning_c >= critical_c {
                // One of them is the kind's: move that one, never the disk's.
                if warning.is_some() {
                    critical_c = warning_c + 5;
                } else {
                    warning_c = critical_c - 5;
                }
            }
            Some(Limits { warning_c, critical_c, from: LimitsFrom::Disk })
        }
        Record::Ata(attributes) => {
            let own = attributes.iter().find(|a| {
                a.id == 190
                    && (1..=99).contains(&a.threshold)
                    && (i64::from(a.value) - (100 - celsius)).abs() <= 2
            });
            Some(own.map_or(by_kind, |a| {
                let critical_c = 100 - i64::from(a.threshold);
                Limits { warning_c: critical_c - 5, critical_c, from: LimitsFrom::Disk }
            }))
        }
        Record::AtaOff => None,
    }
}

/// A count with commas between its thousands.
pub(crate) fn grouped(n: u128) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A disk's summary, which `--json` and the live file both give: its
/// verdict and what the verdict rests on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Summary {
    /// The disk's name, as in `/devices/NAME`.
    pub(crate) disk: String,
    /// What the driver says of the disk; null where it says nothing.
    pub(crate) model: Option<String>,
    pub(crate) serial: Option<String>,
    pub(crate) verdict: Verdict,
    pub(crate) reasons: Vec<String>,
    /// Hours powered on and the temperature, null where the disk does
    /// not say.
    pub(crate) hours: Option<u64>,
    pub(crate) temperature_c: Option<i64>,
    /// A SATA disk's sectors moved to spares, waiting to be moved, and
    /// not readable offline; null for an NVMe drive, for a disk whose
    /// SMART is off, and for one that does not list the attribute.
    pub(crate) moved: Option<u64>,
    pub(crate) waiting: Option<u64>,
    pub(crate) offline: Option<u64>,
    /// An NVMe drive's media errors and its share of its rated
    /// endurance used, in percent; null for a SATA disk.
    pub(crate) media_errors: Option<u64>,
    pub(crate) used_percent: Option<u8>,
}

/// The summary of disk `disk`, whose driver gave `model` and `serial`
/// (empty where it gave none), from what it said of its health.
fn summary(disk: &str, model: &str, serial: &str, record: &Record) -> Summary {
    let judged = report(record).1;
    let said = |text: &str| (!text.is_empty()).then(|| text.to_string());
    let count = |id: u8| match record {
        Record::Ata(attributes) => {
            attributes.iter().find(|a| a.id == id).map(|a| a.raw & 0xffff_ffff)
        }
        _ => None,
    };
    let media_errors = match record {
        Record::Nvme(log) => Some(u64::try_from(log.media_errors).unwrap_or(u64::MAX)),
        _ => None,
    };
    Summary {
        disk: disk.to_string(),
        model: said(model),
        serial: said(serial),
        verdict: judged.verdict,
        reasons: judged.reasons,
        hours: judged.hours,
        temperature_c: judged.celsius,
        moved: count(5),
        waiting: count(197),
        offline: count(198),
        media_errors,
        used_percent: judged.used,
    }
}

/// A SATA attribute in `--json`.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct AttributeJson {
    id: u8,
    name: &'static str,
    prefail: bool,
    value: u8,
    worst: u8,
    threshold: u8,
    raw: u64,
}

/// A test of the disk's own log in `--json`.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct SelfTest {
    /// `short`, `extended`, `conveyance` or `other`.
    kind: &'static str,
    /// `passed`, `failed`, `aborted` or `running`.
    result: &'static str,
    /// The disk's power-on hours when it ran; null while it runs.
    at_hours: Option<u64>,
}

/// One disk in `--json`: its summary, the attributes of a SATA disk and
/// its own log of tests.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Detail {
    #[serde(flatten)]
    summary: Summary,
    /// Null for an NVMe drive, and for a SATA disk whose SMART is off.
    attributes: Option<Vec<AttributeJson>>,
    /// The disk's last tests, at most five, the newest first; null where
    /// the disk keeps no log or it could not be read.
    self_tests: Option<Vec<SelfTest>>,
}

/// The test `logged` as the JSON gives it. A test that ended with a
/// status the JSON has no word for is left out rather than named wrongly.
fn self_test(logged: &selftest::Logged) -> Option<SelfTest> {
    let result = match logged.outcome {
        None => "running",
        Some(Outcome::Passed) => "passed",
        Some(Outcome::Failed { .. }) => "failed",
        Some(Outcome::Aborted | Outcome::Interrupted) => "aborted",
        Some(Outcome::Other(_)) => return None,
    };
    Some(SelfTest { kind: logged.kind.name(), result, at_hours: logged.power_on_hours })
}

/// The disk in `--json`, from what it said of its health and its log of
/// tests.
fn detail(
    disk: &str,
    model: &str,
    serial: &str,
    record: &Record,
    tests: Option<&[selftest::Logged]>,
) -> Detail {
    let attributes = match record {
        Record::Ata(attributes) => Some(
            attributes
                .iter()
                .map(|a| AttributeJson {
                    id: a.id,
                    name: attribute_name(a.id),
                    prefail: a.prefail,
                    value: a.value,
                    worst: a.worst,
                    threshold: a.threshold,
                    raw: a.raw,
                })
                .collect(),
        ),
        Record::AtaOff | Record::Nvme(_) => None,
    };
    Detail {
        summary: summary(disk, model, serial, record),
        attributes,
        self_tests: tests.map(|tests| tests.iter().filter_map(self_test).collect()),
    }
}

/// The early warning that stands, in the live file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Rose {
    pub(crate) at_ms: u64,
    pub(crate) title: String,
}

/// What `--watch` writes of a disk, in `LIVE`: its summary, when it was
/// looked at, and the early warning that stands, null when none does; what
/// kind of disk it is; since when it sleeps, null while it does not; and
/// the temperatures at which it is too hot, null when it gives none.
///
/// The members after the early warning are missing from a file an older
/// service wrote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Live {
    #[serde(flatten)]
    pub(crate) summary: Summary,
    pub(crate) checked_ms: u64,
    pub(crate) rose: Option<Rose>,
    #[serde(default)]
    pub(crate) kind: Option<DiskKind>,
    #[serde(default)]
    pub(crate) asleep_ms: Option<u64>,
    #[serde(default)]
    pub(crate) limits: Option<Limits>,
}

/// The live file's content for disk `disk` at a look at `now`, in
/// seconds, whose count of rises left `rose`.
fn live(disk: &str, record: &Record, now: u64, rose: Option<&(u64, String)>) -> Live {
    let kind = kind_of(record, rotational(disk));
    Live {
        summary: summary(disk, &device_text(disk, "model"), &device_text(disk, "serial"), record),
        checked_ms: now.saturating_mul(1000),
        rose: rose.map(|(at, title)| Rose { at_ms: at.saturating_mul(1000), title: title.clone() }),
        kind: Some(kind),
        asleep_ms: None,
        limits: limits_of(record, kind),
    }
}

/// What the live file in `dir` says of `disk`, if it is there and reads.
fn read_live(dir: &Path, disk: &str) -> Option<Live> {
    let text = fs::read_to_string(dir.join(format!("{disk}.json"))).ok()?;
    serde_json::from_str(&text).ok()
}

/// Writes the live file of a disk into `dir`, whole or not at all: to a
/// file beside it, renamed over it. Anyone may read it.
fn write_live(dir: &Path, live: &Live) -> io::Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o755).create(dir)?;
    let text = serde_json::to_string(live).map_err(io::Error::other)?;
    let path = dir.join(format!("{}.json", live.summary.disk));
    let new = dir.join(format!("{}.json.new", live.summary.disk));
    let mut file =
        fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o644).open(&new)?;
    writeln!(file, "{text}")?;
    file.sync_all()?;
    drop(file);
    fs::rename(new, path)
}

/// Removes the live files of the disks that are not in `disks` (gone, or
/// giving no record), and the files a crash left half written.
fn clear_live(dir: &Path, disks: &[String]) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let stale = match name.strip_suffix(".json") {
            Some(disk) => !disks.iter().any(|known| known == disk),
            None => name.ends_with(".json.new"),
        };
        if stale && let Err(err) = fs::remove_file(entry.path()) {
            eprintln!("diskhealth: {}", util::failed(&entry.path().display(), err).message);
        }
    }
}

/// The details of one disk: its verdict and reasons, then its attributes
/// or its log.
fn details(out: &mut impl Write, name: &str, record: Option<&Record>) -> io::Result<()> {
    let model = device_text(name, "model");
    let serial = device_text(name, "serial");
    let Some(record) = record else {
        return writeln!(out, "{name}: {model}: keeps no record of its health");
    };
    let (kind, report) = report(record);
    writeln!(out, "{name}: {kind}, {model}, serial {serial}")?;
    writeln!(out, "Health: {}", report.verdict.name())?;
    for reason in &report.reasons {
        writeln!(out, "  {reason}")?;
    }
    match record {
        Record::Ata(attributes) => {
            writeln!(out, "ID   ATTRIBUTE               TYPE      VALUE WORST THRESH RAW")?;
            for a in attributes {
                let kind = if a.prefail { "pre-fail" } else { "old-age" };
                writeln!(
                    out,
                    "{:>3}  {:<22}  {kind:<8}  {:>5} {:>5} {:>6} {}",
                    a.id,
                    attribute_name(a.id),
                    a.value,
                    a.worst,
                    a.threshold,
                    a.raw
                )?;
            }
        }
        Record::AtaOff => writeln!(out, "Its SMART is off; the disk's setup can turn it on.")?,
        Record::Nvme(log) => {
            let celsius = report.celsius.map_or("-".to_string(), |c| format!("{c} °C"));
            let bytes =
                |units: u128| util::human((units * 512_000).min(u128::from(u64::MAX)) as u64, true);
            let lines = [
                ("Critical warning", format!("{:#04x}", log.critical)),
                ("Temperature", celsius),
                ("Available spare", format!("{}%", log.spare)),
                ("Available spare threshold", format!("{}%", log.spare_threshold)),
                ("Percentage used", format!("{}%", log.used)),
                (
                    "Data read",
                    format!("{} units, {}", grouped(log.units_read), bytes(log.units_read)),
                ),
                (
                    "Data written",
                    format!("{} units, {}", grouped(log.units_written), bytes(log.units_written)),
                ),
                ("Power cycles", grouped(log.power_cycles)),
                ("Power-on hours", grouped(log.hours)),
                ("Unsafe shutdowns", grouped(log.unsafe_shutdowns)),
                ("Media and data integrity errors", grouped(log.media_errors)),
                ("Error log entries", grouped(log.error_entries)),
                ("Minutes past warning temperature", log.warning_minutes.to_string()),
                ("Minutes past critical temperature", log.critical_minutes.to_string()),
            ];
            for (label, value) in lines {
                writeln!(out, "{:<34}{value}", format!("{label}:"))?;
            }
        }
    }
    Ok(())
}

/// One line of the table for each disk.
fn table(out: &mut impl Write, rows: &[(String, Option<Record>)]) -> io::Result<()> {
    writeln!(
        out,
        "{:<10} {:<5} {:<8} {:>6} {:>8} {:>5}  MODEL",
        "DISK", "TYPE", "HEALTH", "TEMP", "HOURS", "USED"
    )?;
    for (name, record) in rows {
        let model = device_text(name, "model");
        let Some(record) = record else {
            writeln!(
                out,
                "{name:<10} {:<5} {:<8} {:>6} {:>8} {:>5}  {model}",
                "-", "-", "-", "-", "-"
            )?;
            continue;
        };
        let (kind, report) = report(record);
        let celsius = report.celsius.map_or("-".to_string(), |c| format!("{c} °C"));
        let hours = report.hours.map_or("-".to_string(), |h| grouped(u128::from(h)));
        let used = report.used.map_or("-".to_string(), |u| format!("{u}%"));
        let verdict = report.verdict.name();
        writeln!(
            out,
            "{name:<10} {kind:<5} {verdict:<8} {celsius:>6} {hours:>8} {used:>5}  {model}"
        )?;
    }
    Ok(())
}

/// The counts of a disk whose rise tells of a failure to come, and an
/// NVMe drive's share of its rated endurance used.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Counts {
    reallocated: u64,
    pending: u64,
    offline: u64,
    media_errors: u64,
    used: Option<u8>,
}

impl Counts {
    /// The counts `record` gives; none from a SATA disk whose SMART is
    /// off.
    fn of(record: &Record) -> Option<Counts> {
        match record {
            Record::Ata(attributes) => {
                let count = |id: u8| {
                    attributes.iter().find(|a| a.id == id).map_or(0, |a| a.raw & 0xffff_ffff)
                };
                Some(Counts {
                    reallocated: count(5),
                    pending: count(197),
                    offline: count(198),
                    ..Counts::default()
                })
            }
            Record::AtaOff => None,
            Record::Nvme(log) => Some(Counts {
                media_errors: u64::try_from(log.media_errors).unwrap_or(u64::MAX),
                used: Some(log.used),
                ..Counts::default()
            }),
        }
    }

    /// What rose from `before` to these, as "8 more reallocated sectors".
    fn rises_from(&self, before: &Counts) -> Vec<String> {
        let counted = [
            (before.reallocated, self.reallocated, "reallocated sector", "reallocated sectors"),
            (
                before.pending,
                self.pending,
                "sector waiting to be reallocated",
                "sectors waiting to be reallocated",
            ),
            (
                before.offline,
                self.offline,
                "sector that could not be read offline",
                "sectors that could not be read offline",
            ),
            (before.media_errors, self.media_errors, "media error", "media errors"),
        ];
        counted
            .into_iter()
            .filter(|(was, now, ..)| now > was)
            .map(|(was, now, one, many)| {
                let more = now - was;
                format!("{more} more {}", if more == 1 { one } else { many })
            })
            .collect()
    }
}

/// What `--watch` keeps of a disk: its counts at the last look and when
/// that was, in seconds since 1970, and the newest rise, when it was seen
/// and the title that tells of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Kept {
    counts: Counts,
    checked: u64,
    rose: Option<(u64, String)>,
}

impl Kept {
    /// The file's text: a `name value` line each.
    fn text(&self) -> String {
        let c = &self.counts;
        let mut text = format!(
            "reallocated {}\npending {}\noffline {}\nmedia-errors {}\nchecked {}\n",
            c.reallocated, c.pending, c.offline, c.media_errors, self.checked
        );
        if let Some(used) = c.used {
            text += &format!("used {used}\n");
        }
        if let Some((at, title)) = &self.rose {
            text += &format!("rose {at}\nrise {title}\n");
        }
        text
    }

    /// What a file's `text` keeps, if it is whole.
    fn parse(text: &str) -> Option<Kept> {
        let value =
            |name: &str| text.lines().find_map(|line| line.strip_prefix(name)?.strip_prefix(' '));
        let number = |name: &str| value(name)?.parse::<u64>().ok();
        let counts = Counts {
            reallocated: number("reallocated")?,
            pending: number("pending")?,
            offline: number("offline")?,
            media_errors: number("media-errors")?,
            used: value("used").and_then(|used| used.parse().ok()),
        };
        let rose = number("rose").zip(value("rise").map(String::from));
        Some(Kept { counts, checked: number("checked")?, rose })
    }
}

/// The kept rise at `now`, while it is still told of.
fn standing(rose: &Option<(u64, String)>, now: u64) -> Option<&(u64, String)> {
    rose.as_ref().filter(|(at, _)| now.saturating_sub(*at) < RISE_TOLD)
}

/// The early warning of a disk at a look at `now` that found `counts`,
/// after the look that left `before`: what to keep, and the alert to
/// raise, if any. `disk` names the disk for people, `id` for the key.
fn early_warning(
    before: Option<&Kept>,
    counts: Counts,
    now: u64,
    disk: &str,
    id: &str,
) -> (Kept, Option<Alert>) {
    let rises = before.map_or_default(|before| counts.rises_from(&before.counts));
    let rose = if rises.is_empty() {
        before.and_then(|before| before.rose.clone())
    } else {
        let since = before.map_or(now, |before| before.checked);
        let since = util::time_text(i64::try_from(since).unwrap_or(0), "%F %R", false);
        Some((now, format!("Disk {disk} has {} since {since}", rises.join(", "))))
    };
    let rising = standing(&rose, now);
    let worn = counts.used.filter(|&used| used >= USED_TOLD);
    let worn_line = worn.map(|used| format!("Disk {disk} has used {used}% of its rated endurance"));
    let title = rising.map(|(_, title)| title.clone()).or_else(|| worn_line.clone());
    let alert = title.map(|title| {
        let mut detail = String::new();
        if let (Some(_), Some(line)) = (rising, &worn_line) {
            detail = format!("{line}.\n");
        }
        detail.push_str(
            "Disks often fail within weeks of these counts starting to rise. Make sure what is \
             on it is backed up (backup list), and plan to replace it.",
        );
        Alert::new(format!("smart-rising:{id}"), Severity::Warning, &title, &detail)
    });
    (Kept { counts, checked: now, rose }, alert)
}

/// The alert of a disk whose verdict is not PASSED, if it has one.
fn verdict_alert(report: &Report, disk: &str, name: &str, id: &str) -> Option<Alert> {
    let (severity, title) = match report.verdict {
        Verdict::Passed => return None,
        Verdict::Warning => (Severity::Warning, format!("Disk {disk} needs a look")),
        Verdict::Failing => (Severity::Critical, format!("Disk {disk} is failing")),
    };
    let detail = format!("{}\n`diskhealth {name}` shows its record.", report.reasons.join("\n"));
    Some(Alert::new(format!("smart:{id}"), severity, &title, &detail))
}

/// How a disk is named in alerts, and the part of their keys and of the
/// kept file's name that stands for it: its model and serial number,
/// else its name.
fn identity(name: &str) -> (String, String) {
    let (model, serial) = (device_text(name, "model"), device_text(name, "serial"));
    let disk = match (model.is_empty(), serial.is_empty()) {
        (false, false) => format!("{model} ({name}, serial {serial})"),
        (false, true) => format!("{model} ({name})"),
        _ => name.to_string(),
    };
    let id = key_part(if serial.is_empty() { name } else { &serial });
    (disk, id)
}

/// The kept file in `dir` of the disk `id` stands for.
fn kept_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(id.replace(['/', '.'], "-"))
}

/// What the early warning of `--watch` says of disk `name` now, as the
/// title of its `smart-rising` alert, if it has one: a count that tells
/// of a failure to come rose since the service's last look, or the
/// disk's NVMe endurance is nearly used up. Reading it changes nothing
/// the service keeps.
pub(crate) fn early_warning_title(name: &str, record: &Record) -> Option<String> {
    let counts = Counts::of(record)?;
    let (disk, id) = identity(name);
    let before = fs::read_to_string(kept_path(Path::new(KEPT), &id))
        .ok()
        .and_then(|text| Kept::parse(&text));
    let now = util::now_ns() / 1_000_000_000;
    early_warning(before.as_ref(), counts, now, &disk, &id).1.map(|alert| alert.title)
}

/// Writes `kept` to its file, by a new file renamed over it.
fn keep(dir: &Path, path: &Path, kept: &Kept) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let mut new = path.as_os_str().to_owned();
    new.push(".new");
    fs::write(&new, kept.text())?;
    fs::rename(&new, path)
}

/// The alerts of a disk at a look at `now`, keeping its counts, and the
/// early warning that stands for the live file. `looked` is when the
/// service last read the disk, which the rise is told since, and `kept`
/// the folder of the counts. A file whose counts are as they were is not
/// written again, so that the state disk is not written at every look.
fn disk_alerts(
    kept_dir: &Path,
    name: &str,
    record: &Record,
    report: &Report,
    now: u64,
    looked: Option<u64>,
) -> (Vec<Alert>, Option<(u64, String)>) {
    let (disk, id) = identity(name);
    let mut alerts: Vec<Alert> = verdict_alert(report, &disk, name, &id).into_iter().collect();
    let mut standing_rise = None;
    if let Some(counts) = Counts::of(record) {
        let path = kept_path(kept_dir, &id);
        let before = fs::read_to_string(&path).ok().and_then(|text| Kept::parse(&text));
        let seen = before.as_ref().map(|before| Kept {
            checked: before.checked.max(looked.unwrap_or(0)),
            ..before.clone()
        });
        let (kept, rising) = early_warning(seen.as_ref(), counts, now, &disk, &id);
        standing_rise = standing(&kept.rose, now).cloned();
        let changed =
            before.as_ref().is_none_or(|b| b.counts != kept.counts || b.rose != kept.rose);
        if changed && let Err(err) = keep(kept_dir, &path, &kept) {
            eprintln!("diskhealth: {}", util::failed(&path.display(), err).message);
        }
        alerts.extend(rising);
    }
    (alerts, standing_rise)
}

/// The line `--watch` writes of a disk: its verdict and reasons, or what
/// it rests on when it passed.
fn watch_line(name: &str, report: &Report) -> String {
    let mut line = format!("{name}: {}", report.verdict.name());
    if report.reasons.is_empty() {
        if let Some(c) = report.celsius {
            line.push_str(&format!(", {c} °C"));
        }
        if let Some(h) = report.hours {
            let unit = if h == 1 { "hour" } else { "hours" };
            line.push_str(&format!(", {} {unit} on", grouped(u128::from(h))));
        }
    } else {
        line.push_str(": ");
        line.push_str(&report.reasons.join("; "));
    }
    line
}

/// What `--watch` keeps from one look to the next, and where it writes.
struct Watch {
    /// Where the disks' live files are, and where their counts are kept.
    live: PathBuf,
    kept: PathBuf,
    /// Each disk's report at the look that last told of it.
    last: Vec<(String, Report)>,
    /// Each disk's alerts at its last look, kept for a look that cannot
    /// read it, so that they are not cleared for that.
    alerts: Vec<(String, Vec<Alert>)>,
    /// When each disk now asleep was first found so, in seconds.
    asleep: Vec<(String, u64)>,
    /// When each disk was last read, in seconds.
    looked: Vec<(String, u64)>,
}

impl Watch {
    /// A watch that writes where the system keeps its files.
    fn new() -> Watch {
        Watch {
            live: PathBuf::from(LIVE),
            kept: PathBuf::from(KEPT),
            last: Vec::new(),
            alerts: Vec::new(),
            asleep: Vec::new(),
            looked: Vec::new(),
        }
    }

    /// One look at the disks `names`, at `now` in seconds, each asked of
    /// its health by `read`: writes a line of each whose verdict or reasons
    /// changed to `out`, keeps each live file, and removes those of disks
    /// that are gone or keep no record. A disk that sleeps keeps the file
    /// and the alerts it had, with the time it was found asleep; one that
    /// cannot be read keeps them as they were.
    fn look(
        &mut self,
        names: Vec<String>,
        read: &dyn Fn(&str) -> Result<Reading, Fail>,
        now: u64,
        out: &mut dyn Write,
    ) -> io::Result<()> {
        self.alerts.retain(|(disk, _)| names.contains(disk));
        self.asleep.retain(|(disk, _)| names.contains(disk));
        // The disks whose live files stay: those that gave a record, and
        // those that could not be read this time, whose file keeps the
        // time of the look that did.
        let mut present: Vec<String> = Vec::new();
        for name in names {
            let report = match read(&name) {
                Ok(Reading::Record(record)) => {
                    let report = report(&record).1;
                    let looked =
                        self.looked.iter().find(|(disk, _)| *disk == name).map(|(_, at)| *at);
                    let (found, rose) =
                        disk_alerts(&self.kept, &name, &record, &report, now, looked);
                    let file = live(&name, &record, now, rose.as_ref());
                    if let Err(err) = write_live(&self.live, &file) {
                        eprintln!(
                            "diskhealth: {}: {}",
                            self.live.display(),
                            util::strip_os_error(&err)
                        );
                    }
                    self.asleep.retain(|(disk, _)| *disk != name);
                    set(&mut self.looked, &name, now);
                    present.push(name.clone());
                    match self.alerts.iter_mut().find(|(disk, _)| *disk == name) {
                        Some((_, held)) => *held = found,
                        None => self.alerts.push((name.clone(), found)),
                    }
                    report
                }
                Ok(Reading::None) => continue,
                Ok(Reading::Asleep) => {
                    self.keep_asleep(&name, now);
                    present.push(name);
                    continue;
                }
                Err(fail) => {
                    eprintln!("diskhealth: {}", fail.message);
                    present.push(name);
                    continue;
                }
            };
            let seen = self.last.iter_mut().find(|(seen, _)| *seen == name);
            match seen {
                Some((_, before)) if same_but_numbers(before, &report) => {}
                Some((_, before)) => {
                    writeln!(out, "{}", watch_line(&name, &report))?;
                    *before = report;
                }
                None => {
                    writeln!(out, "{}", watch_line(&name, &report))?;
                    self.last.push((name, report));
                }
            }
        }
        out.flush()?;
        clear_live(&self.live, &present);
        Ok(())
    }

    /// Says in the live file of the sleeping disk `name` since when it
    /// sleeps, which is when this service found it so, or, after a restart
    /// of it, what the file already said. The rest of the file is what the
    /// last look that read the disk left, and is none when there is no
    /// such look.
    fn keep_asleep(&mut self, name: &str, now: u64) {
        let Some(mut file) = read_live(&self.live, name) else { return };
        let since = match self.asleep.iter().find(|(disk, _)| disk == name) {
            Some((_, since)) => *since,
            None => file.asleep_ms.map_or(now, |ms| ms / 1000),
        };
        set(&mut self.asleep, name, since);
        file.asleep_ms = Some(since.saturating_mul(1000));
        if let Err(err) = write_live(&self.live, &file) {
            eprintln!("diskhealth: {}: {}", self.live.display(), util::strip_os_error(&err));
        }
    }

    /// Every alert the disks have now.
    fn all_alerts(&self) -> Vec<Alert> {
        self.alerts.iter().flat_map(|(_, held)| held.iter().cloned()).collect()
    }
}

/// Sets what `list` holds for `name` to `value`.
fn set(list: &mut Vec<(String, u64)>, name: &str, value: u64) {
    match list.iter_mut().find(|(disk, _)| disk == name) {
        Some((_, held)) => *held = value,
        None => list.push((name.to_string(), value)),
    }
}

/// `--watch`: looks at every disk now and every 10 minutes, writes a
/// line of each whose verdict or reasons changed, and sets the disks'
/// alerts. Runs until stopped.
fn watch() -> Result<i32, Fail> {
    let mut watch = Watch::new();
    let mut unreached = false;
    loop {
        let names = disks().map_err(|e| util::failed(&BLOCKS, e))?;
        let now = util::now_ns() / 1_000_000_000;
        watch.look(names, &read, now, &mut io::stdout().lock())?;
        match crate::alerts::client::set(&watch.all_alerts()) {
            Ok(()) => unreached = false,
            Err(why) if !unreached => {
                unreached = true;
                eprintln!("diskhealth: cannot tell alertd: {why}");
            }
            Err(_) => {}
        }
        std::thread::sleep(WATCH_EVERY);
    }
}

/// Whether two reports differ only in temperature and hours, which
/// `--watch` does not tell of.
fn same_but_numbers(a: &Report, b: &Report) -> bool {
    a.verdict == b.verdict && a.reasons == b.reasons
}

/// The first of `names` that is not one of the kernel's `known` names.
fn unknown<'a>(names: &'a [String], known: &[String]) -> Option<&'a String> {
    names.iter().find(|name| !known.contains(name))
}

/// `--json`: the disks that keep a record, as a list of objects on one
/// line. A disk that keeps none is left out.
fn json(out: &mut impl Write, rows: &[(String, Option<Record>)]) -> Result<(), Fail> {
    let mut found = Vec::new();
    for (name, record) in rows {
        let Some(record) = record else { continue };
        let tests = match selftest::history(name) {
            Ok(tests) => tests,
            Err(fail) => {
                eprintln!("diskhealth: {}", fail.message);
                None
            }
        };
        found.push(detail(
            name,
            &device_text(name, "model"),
            &device_text(name, "serial"),
            record,
            tests.as_deref(),
        ));
    }
    let line = serde_json::to_string(&found).map_err(|error| util::fail(error.to_string()))?;
    writeln!(out, "{line}")?;
    Ok(())
}

/// diskhealth(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let (mut all, mut as_json, mut names) = (false, false, Vec::new());
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('a') => all = true,
            Arg::Long(long) if long == "all" => all = true,
            Arg::Long(long) if long == "json" => as_json = true,
            Arg::Long(long) if long == "watch" => return watch(),
            Arg::Operand(name) => {
                let name = name.to_string_lossy();
                names.push(name.strip_prefix("/devices/").unwrap_or(&name).to_string());
            }
            other => return Err(args.unknown(&other)),
        }
    }
    if all && as_json {
        return Err(util::usage(
            "-a does not go with --json: the JSON always has the attributes".into(),
        ));
    }
    let details_wanted = all || !names.is_empty();
    if names.is_empty() {
        names = disks().map_err(|e| util::failed(&BLOCKS, e))?;
    } else {
        let known = block_names().map_err(|e| util::failed(&BLOCKS, e))?;
        if let Some(name) = unknown(&names, &known) {
            return Err(disk_errors::no_such_disk(name));
        }
    }
    let mut rows = Vec::new();
    let mut status = 0;
    for name in names {
        match read(&name) {
            Ok(Reading::Record(record)) => {
                status |= match report(&record).1.verdict {
                    Verdict::Passed => 0,
                    Verdict::Warning => WARNS,
                    Verdict::Failing => FAILS,
                };
                rows.push((name, Some(record)));
            }
            Ok(Reading::None) => rows.push((name, None)),
            Ok(Reading::Asleep) => {
                eprintln!("diskhealth: {}", asleep_words(&name));
                status |= UNREAD;
            }
            Err(fail) => {
                eprintln!("diskhealth: {}", fail.message);
                status |= UNREAD;
            }
        }
    }
    let mut out = io::stdout().lock();
    if as_json {
        json(&mut out, &rows)?;
    } else if details_wanted {
        for (i, (name, record)) in rows.iter().enumerate() {
            if i > 0 {
                writeln!(out)?;
            }
            details(&mut out, name, record.as_ref())?;
        }
    } else {
        table(&mut out, &rows)?;
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    /// A SMART data page and thresholds page with `entries` (id, flags,
    /// value, worst, raw, threshold).
    fn pages(entries: &[(u8, u16, u8, u8, u64, u8)]) -> (Vec<u8>, Vec<u8>) {
        let (mut data, mut thresholds) = (vec![0u8; 512], vec![0u8; 512]);
        for (i, &(id, flags, value, worst, raw, threshold)) in entries.iter().enumerate() {
            let at = 2 + 12 * i;
            data[at] = id;
            data[at + 1..at + 3].copy_from_slice(&flags.to_le_bytes());
            (data[at + 3], data[at + 4]) = (value, worst);
            data[at + 5..at + 11].copy_from_slice(&raw.to_le_bytes()[..6]);
            (thresholds[at], thresholds[at + 1]) = (id, threshold);
        }
        (data, thresholds)
    }

    #[test]
    fn a_healthy_sata_disk_passes() {
        // As QEMU's IDE disk answers.
        let (data, thresholds) = pages(&[
            (1, 3, 100, 100, 0, 6),
            (5, 3, 100, 100, 0, 36),
            (9, 3, 100, 100, 1234, 0),
            (190, 3, 69, 69, 0x1f1f_001f, 50),
        ]);
        let found = attributes(&data, &thresholds);
        assert_eq!(found.len(), 4);
        assert_eq!(
            found[1],
            Attribute { id: 5, prefail: true, value: 100, worst: 100, threshold: 36, raw: 0 }
        );
        let report = ata_report(&found);
        assert_eq!(report.verdict, Verdict::Passed);
        assert_eq!((report.celsius, report.hours), (Some(31), Some(1234)));
    }

    #[test]
    fn sata_attributes_at_their_thresholds_warn_or_fail() {
        let (data, thresholds) = pages(&[(10, 2, 30, 30, 0, 30), (194, 2, 60, 50, 40, 0)]);
        let report = ata_report(&attributes(&data, &thresholds));
        assert_eq!(report.verdict, Verdict::Warning);
        assert_eq!(report.celsius, Some(40));
        let (data, thresholds) = pages(&[(10, 2, 30, 30, 0, 30), (1, 3, 5, 5, 0, 6)]);
        let report = ata_report(&attributes(&data, &thresholds));
        assert_eq!(report.verdict, Verdict::Failing);
        assert_eq!(report.reasons.len(), 2);
        // A threshold of 0 never fails.
        let (data, thresholds) = pages(&[(1, 3, 1, 1, 0, 0)]);
        assert_eq!(ata_report(&attributes(&data, &thresholds)).verdict, Verdict::Passed);
    }

    #[test]
    fn reallocated_and_pending_sectors_warn() {
        let (data, thresholds) = pages(&[(5, 3, 100, 100, 8, 36), (197, 2, 100, 100, 2, 0)]);
        let report = ata_report(&attributes(&data, &thresholds));
        assert_eq!(report.verdict, Verdict::Warning);
        assert_eq!(
            report.reasons,
            ["8 sectors were reallocated", "2 sectors wait to be reallocated"]
        );
    }

    #[test]
    fn the_nvme_log_reads_and_judges() {
        let mut data = vec![0u8; 512];
        data[1..3].copy_from_slice(&323u16.to_le_bytes());
        (data[3], data[4], data[5]) = (100, 10, 3);
        data[48] = 7;
        data[128..130].copy_from_slice(&500u16.to_le_bytes());
        let log = nvme_log(&data, &[]);
        assert_eq!(
            (log.kelvin, log.spare, log.used, log.units_written, log.hours),
            (323, 100, 3, 7, 500)
        );
        let report = nvme_report(&log);
        assert_eq!(report.verdict, Verdict::Passed);
        assert_eq!((report.celsius, report.hours, report.used), (Some(50), Some(500), Some(3)));
        data[0] = 1 << 1;
        assert_eq!(nvme_report(&nvme_log(&data, &[])).verdict, Verdict::Warning);
        data[0] |= 1 << 2;
        let report = nvme_report(&nvme_log(&data, &[]));
        assert_eq!(report.verdict, Verdict::Failing);
        assert_eq!(report.reasons.len(), 2);
    }

    #[test]
    fn counts_have_commas() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }

    #[test]
    fn a_disk_that_is_not_passed_has_an_alert_of_its_verdict() {
        let mut report = Report::new();
        assert_eq!(verdict_alert(&report, "WDC (sda)", "sda", "wd-1"), None);
        report.add(Verdict::Warning, "8 sectors were reallocated".into());
        let alert = verdict_alert(&report, "WDC (sda)", "sda", "wd-1").unwrap();
        assert_eq!((alert.key.as_str(), alert.severity), ("smart:wd-1", Severity::Warning));
        assert_eq!(alert.title, "Disk WDC (sda) needs a look");
        assert_eq!(alert.detail, "8 sectors were reallocated\n`diskhealth sda` shows its record.");
        report.add(Verdict::Failing, "it has gone read-only".into());
        let alert = verdict_alert(&report, "WDC (sda)", "sda", "wd-1").unwrap();
        assert_eq!(
            (alert.severity, alert.title.as_str()),
            (Severity::Critical, "Disk WDC (sda) is failing")
        );
    }

    #[test]
    fn the_counts_come_from_either_kind_of_disk() {
        let (data, thresholds) = pages(&[
            (5, 3, 100, 100, 8, 36),
            (197, 2, 100, 100, 2, 0),
            (198, 2, 100, 100, 0x1_0000_0001, 0),
        ]);
        let counts = Counts::of(&Record::Ata(attributes(&data, &thresholds))).unwrap();
        assert_eq!(counts, Counts { reallocated: 8, pending: 2, offline: 1, ..Counts::default() });
        assert_eq!(Counts::of(&Record::AtaOff), None);
        let log = NvmeLog { media_errors: 3, used: 91, ..NvmeLog::default() };
        assert_eq!(
            Counts::of(&Record::Nvme(log)),
            Some(Counts { media_errors: 3, used: Some(91), ..Counts::default() })
        );
    }

    #[test]
    fn a_rise_is_an_early_warning_for_a_week() {
        const DAY: u64 = 86_400;
        let start = 1_800_000_000;
        let counts = |reallocated, pending| Counts { reallocated, pending, ..Counts::default() };
        // The first look keeps the counts and says nothing.
        let (kept, alert) = early_warning(None, counts(8, 0), start, "WDC (sda)", "wd-1");
        assert_eq!((kept.checked, &kept.rose, alert), (start, &None, None));
        // The same counts later: still nothing.
        let (kept, alert) =
            early_warning(Some(&kept), counts(8, 0), start + 1800, "WDC (sda)", "wd-1");
        assert_eq!(alert, None);
        // A rise is told of, from the look before.
        let now = start + 3600;
        let (kept, alert) = early_warning(Some(&kept), counts(16, 2), now, "WDC (sda)", "wd-1");
        let alert = alert.unwrap();
        let since = util::time_text(i64::try_from(start + 1800).unwrap(), "%F %R", false);
        assert_eq!((alert.key.as_str(), alert.severity), ("smart-rising:wd-1", Severity::Warning));
        assert_eq!(
            alert.title,
            format!(
                "Disk WDC (sda) has 8 more reallocated sectors, 2 more sectors waiting to be reallocated since {since}"
            )
        );
        assert_eq!(kept.rose.as_ref().map(|(at, _)| *at), Some(now));
        // It stays, with the same title, through a restart, for a week.
        let kept = Kept::parse(&kept.text()).unwrap();
        let (kept, again) =
            early_warning(Some(&kept), counts(16, 2), now + 6 * DAY, "WDC (sda)", "wd-1");
        assert_eq!(again.unwrap().title, alert.title);
        let (_, gone) =
            early_warning(Some(&kept), counts(16, 2), now + 7 * DAY, "WDC (sda)", "wd-1");
        assert_eq!(gone, None);
        // A count that falls, as after a disk is wiped, is no rise.
        let (_, fell) =
            early_warning(Some(&kept), counts(0, 0), now + 8 * DAY, "WDC (sda)", "wd-1");
        assert_eq!(fell, None);
    }

    #[test]
    fn an_nvme_drive_near_its_rated_endurance_is_told_of() {
        let worn = |used| Counts { used: Some(used), ..Counts::default() };
        assert_eq!(early_warning(None, worn(89), 100, "nvme0", "s1").1, None);
        let alert = early_warning(None, worn(90), 100, "nvme0", "s1").1.unwrap();
        assert_eq!(alert.title, "Disk nvme0 has used 90% of its rated endurance");
        // With a rise as well, the rise is the title and the wear is said.
        let kept = Kept { counts: worn(95), checked: 50, rose: None };
        let both = Counts { media_errors: 1, used: Some(95), ..Counts::default() };
        let alert = early_warning(Some(&kept), both, 100, "nvme0", "s1").1.unwrap();
        assert!(alert.title.starts_with("Disk nvme0 has 1 more media error since"));
        assert!(alert.detail.starts_with("Disk nvme0 has used 95% of its rated endurance.\n"));
    }

    #[test]
    fn what_is_kept_reads_back_and_a_torn_file_is_none() {
        let kept = Kept {
            counts: Counts {
                reallocated: 1,
                pending: 2,
                offline: 3,
                media_errors: 4,
                used: Some(5),
            },
            checked: 6,
            rose: Some((
                7,
                "Disk sda has 1 more reallocated sectors since 2026-11-13 10:00".into(),
            )),
        };
        assert_eq!(Kept::parse(&kept.text()), Some(kept.clone()));
        let plain = Kept { counts: Counts::default(), checked: 9, rose: None };
        assert_eq!(Kept::parse(&plain.text()), Some(plain));
        assert_eq!(Kept::parse("reallocated 1\npending 2\n"), None);
        assert_eq!(kept_path(Path::new(KEPT), "../x/y"), Path::new(KEPT).join("---x-y"));
    }

    /// A SATA disk as QEMU's IDE disk answers, with five reallocated
    /// sectors, two waiting and none offline.
    fn worn_sata() -> Record {
        let (data, thresholds) = pages(&[
            (1, 3, 100, 100, 0, 6),
            (5, 3, 100, 100, 5, 36),
            (9, 3, 100, 100, 21804, 0),
            (194, 2, 62, 50, 38, 0),
            (197, 2, 100, 100, 2, 0),
        ]);
        Record::Ata(attributes(&data, &thresholds))
    }

    fn healthy_nvme() -> Record {
        Record::Nvme(NvmeLog {
            kelvin: 313,
            used: 3,
            hours: 500,
            media_errors: 0,
            ..NvmeLog::default()
        })
    }

    #[test]
    fn a_sata_disk_has_its_counts_and_no_nvme_fields() {
        let found = summary("sdb", "WD20EFZX", "WD-WX12A3B4C588", &worn_sata());
        assert_eq!(
            serde_json::to_value(&found).unwrap(),
            json!({
                "disk": "sdb", "model": "WD20EFZX", "serial": "WD-WX12A3B4C588",
                "verdict": "warning",
                "reasons": ["5 sectors were reallocated", "2 sectors wait to be reallocated"],
                "hours": 21804, "temperature_c": 38,
                "moved": 5, "waiting": 2, "offline": null,
                "media_errors": null, "used_percent": null,
            })
        );
    }

    #[test]
    fn an_nvme_drive_has_its_log_and_no_sata_fields() {
        let found = summary("nvme0n1", "", "", &healthy_nvme());
        assert_eq!(
            serde_json::to_value(&found).unwrap(),
            json!({
                "disk": "nvme0n1", "model": null, "serial": null, "verdict": "passed",
                "reasons": [], "hours": 500, "temperature_c": 40,
                "moved": null, "waiting": null, "offline": null,
                "media_errors": 0, "used_percent": 3,
            })
        );
    }

    #[test]
    fn a_disk_whose_smart_is_off_has_no_counts_and_no_attributes() {
        let off = detail("sda", "M", "S", &Record::AtaOff, Some(&[]));
        let off = serde_json::to_value(&off).unwrap();
        assert_eq!(off["verdict"], json!("warning"));
        assert_eq!(off["reasons"], json!(["its SMART is off"]));
        for field in ["hours", "temperature_c", "moved", "waiting", "offline", "attributes"] {
            assert_eq!(off[field], Value::Null, "{field}");
        }
        assert_eq!(off["self_tests"], json!([]));
    }

    #[test]
    fn the_json_of_a_sata_disk_lists_its_attributes_and_tests() {
        let tests = [
            selftest::Logged {
                kind: selftest::Kind::Extended,
                outcome: Some(Outcome::Passed),
                power_on_hours: Some(21700),
            },
            selftest::Logged {
                kind: selftest::Kind::Short,
                outcome: Some(Outcome::Failed { first_bad_sector: Some(7) }),
                power_on_hours: Some(21000),
            },
            selftest::Logged {
                kind: selftest::Kind::Conveyance,
                outcome: Some(Outcome::Interrupted),
                power_on_hours: Some(20000),
            },
            selftest::Logged {
                kind: selftest::Kind::Other,
                outcome: Some(Outcome::Aborted),
                power_on_hours: Some(100),
            },
            selftest::Logged { kind: selftest::Kind::Short, outcome: None, power_on_hours: None },
            // A status the JSON has no word for is left out.
            selftest::Logged {
                kind: selftest::Kind::Short,
                outcome: Some(Outcome::Other(0x25)),
                power_on_hours: Some(1),
            },
        ];
        let found = detail("sdb", "WD20EFZX", "WD-1", &worn_sata(), Some(&tests));
        let found = serde_json::to_value(&found).unwrap();
        assert_eq!(
            found["attributes"][1],
            json!({"id": 5, "name": "Reallocated sectors", "prefail": true, "value": 100,
                "worst": 100, "threshold": 36, "raw": 5})
        );
        assert_eq!(found["attributes"].as_array().unwrap().len(), 5);
        assert_eq!(
            found["self_tests"],
            json!([
                {"kind": "extended", "result": "passed", "at_hours": 21700},
                {"kind": "short", "result": "failed", "at_hours": 21000},
                {"kind": "conveyance", "result": "aborted", "at_hours": 20000},
                {"kind": "other", "result": "aborted", "at_hours": 100},
                {"kind": "short", "result": "running", "at_hours": null},
            ])
        );
    }

    #[test]
    fn an_nvme_drive_has_no_attributes_and_a_disk_with_no_log_has_no_tests() {
        let found = detail("nvme0n1", "M", "S", &healthy_nvme(), None);
        let found = serde_json::to_value(&found).unwrap();
        assert_eq!((&found["attributes"], &found["self_tests"]), (&Value::Null, &Value::Null));
        // The summary's fields come first, in the order the design names.
        let line = serde_json::to_string(&detail("sda", "M", "S", &healthy_nvme(), None)).unwrap();
        let keys = [
            "disk",
            "model",
            "serial",
            "verdict",
            "reasons",
            "hours",
            "temperature_c",
            "moved",
            "waiting",
            "offline",
            "media_errors",
            "used_percent",
            "attributes",
            "self_tests",
        ];
        let places: Vec<_> =
            keys.iter().map(|key| line.find(&format!("\"{key}\":")).unwrap()).collect();
        assert!(places.is_sorted(), "{line}");
    }

    #[test]
    fn the_live_file_has_the_summary_and_the_early_warning() {
        let rose = (1_763_080_000, "sdb moved 8 worn-out sectors to spares".to_string());
        let file = Live {
            summary: summary("sdb", "WD20EFZX", "WD-1", &worn_sata()),
            checked_ms: 1_763_120_000_000,
            rose: Some(Rose { at_ms: rose.0 * 1000, title: rose.1 }),
            kind: Some(DiskKind::Hdd),
            asleep_ms: Some(1_763_119_000_000),
            limits: Some(Limits { warning_c: 50, critical_c: 60, from: LimitsFrom::Kind }),
        };
        let value = serde_json::to_value(&file).unwrap();
        assert_eq!(value["checked_ms"], json!(1_763_120_000_000u64));
        assert_eq!(
            value["rose"],
            json!({"at_ms": 1_763_080_000_000u64, "title": "sdb moved 8 worn-out sectors to spares"})
        );
        assert_eq!(value["verdict"], json!("warning"));
        assert_eq!(value["kind"], json!("hdd"));
        assert_eq!(value["asleep_ms"], json!(1_763_119_000_000u64));
        assert_eq!(value["limits"], json!({"warning_c": 50, "critical_c": 60, "from": "kind"}));
        assert!(value.get("attributes").is_none() && value.get("self_tests").is_none());
        // What is written reads back as it was.
        let back: Live = serde_json::from_value(value).unwrap();
        assert_eq!(back, file);
        let calm = Live { rose: None, ..file };
        let value = serde_json::to_value(&calm).unwrap();
        assert_eq!(value["rose"], Value::Null);
        assert_eq!(serde_json::from_value::<Live>(value).unwrap(), calm);
    }

    #[test]
    fn a_rise_is_in_the_live_file_for_a_week_only() {
        let rose = Some((1_000, "title".to_string()));
        assert!(standing(&rose, 1_000 + RISE_TOLD - 1).is_some());
        assert_eq!(standing(&rose, 1_000 + RISE_TOLD), None);
        assert_eq!(standing(&None, 5), None);
        let file = live("sdb", &worn_sata(), 2_000, rose.as_ref());
        assert_eq!(file.checked_ms, 2_000_000);
        assert_eq!(file.rose, Some(Rose { at_ms: 1_000_000, title: "title".into() }));
    }

    /// A folder of its own for each test, as the tests run at once.
    fn folder(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("diskhealth-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn a_live_file_is_written_whole_and_readable_by_anyone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = folder("write").join("live");
        let file = live("sdb", &worn_sata(), 2_000, None);
        write_live(&dir, &file).unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!((mode(&dir), mode(&dir.join("sdb.json"))), (0o755, 0o644));
        let text = fs::read_to_string(dir.join("sdb.json")).unwrap();
        assert!(text.ends_with("}\n"), "{text}");
        assert_eq!(serde_json::from_str::<Live>(&text).unwrap(), file);
        assert!(!dir.join("sdb.json.new").exists());
        // The next look replaces it.
        let later = live("sdb", &worn_sata(), 4_000, None);
        write_live(&dir, &later).unwrap();
        let text = fs::read_to_string(dir.join("sdb.json")).unwrap();
        assert_eq!(serde_json::from_str::<Live>(&text).unwrap().checked_ms, 4_000_000);
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn the_live_file_of_a_disk_that_goes_is_removed() {
        let dir = folder("clear").join("live");
        for disk in ["sda", "sdb", "nvme0n1"] {
            write_live(&dir, &live(disk, &worn_sata(), 2_000, None)).unwrap();
        }
        // A half-written file of a crash, and a file that is not a disk's.
        fs::write(dir.join("sda.json.new"), "{").unwrap();
        fs::write(dir.join("README"), "kept").unwrap();
        clear_live(&dir, &["sda".to_string(), "nvme0n1".to_string()]);
        let mut left: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["README", "nvme0n1.json", "sda.json"]);
        // Nothing to clear in a folder that is not there.
        clear_live(&dir.join("none"), &[]);
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_name_the_kernel_does_not_list_is_found_out() {
        let disks = ["nvme0n1".to_string(), "sda".to_string(), "sda1".to_string()];
        let asked =
            |names: &[&str]| -> Vec<String> { names.iter().map(ToString::to_string).collect() };
        assert_eq!(unknown(&asked(&["sda"]), &disks), None);
        assert_eq!(unknown(&asked(&[]), &disks), None);
        // A partition answers for its disk, and a path is not a name.
        assert_eq!(unknown(&asked(&["sda1"]), &disks), None);
        assert_eq!(unknown(&asked(&["sda", "sdb"]), &disks).map(String::as_str), Some("sdb"));
        assert_eq!(unknown(&asked(&["../x"]), &disks).map(String::as_str), Some("../x"));
    }

    #[test]
    fn the_json_of_no_disks_is_an_empty_list() {
        let mut out = Vec::new();
        json(&mut out, &[]).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "[]\n");
        // A disk that keeps no record is left out.
        let mut out = Vec::new();
        json(&mut out, &[("vda".to_string(), None)]).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "[]\n");
    }

    /// A SATA disk at `celsius` whose airflow attribute (190) has the
    /// value `value` and the threshold `threshold`.
    fn airflow(celsius: u64, value: u8, threshold: u8) -> Record {
        let (data, thresholds) = pages(&[(190, 3, value, value, celsius, threshold)]);
        Record::Ata(attributes(&data, &thresholds))
    }

    fn limits(warning_c: i64, critical_c: i64, from: LimitsFrom) -> Option<Limits> {
        Some(Limits { warning_c, critical_c, from })
    }

    #[test]
    fn a_sata_disk_that_says_its_limit_in_its_airflow_attribute_has_it() {
        // 62 is 100 less 38, and 45 is where the disk fails: 55, and 50 as
        // the warning.
        assert_eq!(
            limits_of(&airflow(38, 62, 45), DiskKind::Hdd),
            limits(50, 55, LimitsFrom::Disk)
        );
        // Within 2 of what 100 less the temperature is still tracks it.
        assert_eq!(
            limits_of(&airflow(38, 64, 45), DiskKind::Hdd),
            limits(50, 55, LimitsFrom::Disk)
        );
        // A value that does not follow the temperature, or a threshold of
        // 0 or past 99, is not a limit: the kind's is used.
        assert_eq!(
            limits_of(&airflow(38, 69, 45), DiskKind::Hdd),
            limits(50, 60, LimitsFrom::Kind)
        );
        assert_eq!(limits_of(&airflow(38, 62, 0), DiskKind::Ssd), limits(65, 70, LimitsFrom::Kind));
        assert_eq!(
            limits_of(&airflow(38, 62, 100), DiskKind::Hdd),
            limits(50, 60, LimitsFrom::Kind)
        );
        // The threshold of attribute 194 says nothing of heat.
        let (data, thresholds) = pages(&[(194, 2, 62, 50, 38, 45)]);
        let record = Record::Ata(attributes(&data, &thresholds));
        assert_eq!(limits_of(&record, DiskKind::Hdd), limits(50, 60, LimitsFrom::Kind));
    }

    #[test]
    fn a_disk_that_gives_no_temperature_has_no_limits() {
        assert_eq!(limits_of(&Record::AtaOff, DiskKind::Hdd), None);
        let (data, thresholds) = pages(&[(9, 3, 100, 100, 5, 0)]);
        assert_eq!(limits_of(&Record::Ata(attributes(&data, &thresholds)), DiskKind::Ssd), None);
        assert_eq!(limits_of(&Record::Nvme(NvmeLog::default()), DiskKind::Nvme), None);
    }

    #[test]
    fn an_nvme_drive_says_its_limits_in_kelvin() {
        let nvme = |warning_kelvin, critical_kelvin| {
            Record::Nvme(NvmeLog {
                kelvin: 320,
                warning_kelvin,
                critical_kelvin,
                ..NvmeLog::default()
            })
        };
        // 343 K and 358 K.
        assert_eq!(limits_of(&nvme(343, 358), DiskKind::Nvme), limits(70, 85, LimitsFrom::Disk));
        // Neither: the kind's.
        assert_eq!(limits_of(&nvme(0, 0), DiskKind::Nvme), limits(70, 80, LimitsFrom::Kind));
        // One of them: the other is the kind's, and still from the disk.
        assert_eq!(limits_of(&nvme(333, 0), DiskKind::Nvme), limits(60, 80, LimitsFrom::Disk));
        assert_eq!(limits_of(&nvme(0, 358), DiskKind::Nvme), limits(70, 85, LimitsFrom::Disk));
        // A warning that is not below the critical limit: the critical is
        // 5 above it, where the kind gave the critical; where the kind gave
        // the warning, the warning is 5 below the disk's critical.
        assert_eq!(limits_of(&nvme(363, 0), DiskKind::Nvme), limits(90, 95, LimitsFrom::Disk));
        assert_eq!(limits_of(&nvme(0, 338), DiskKind::Nvme), limits(60, 65, LimitsFrom::Disk));
        assert_eq!(limits_of(&nvme(358, 343), DiskKind::Nvme), limits(85, 90, LimitsFrom::Disk));
    }

    #[test]
    fn the_nvme_limits_are_read_from_the_thresholds_page() {
        let mut thresholds = vec![0u8; 512];
        thresholds[0..2].copy_from_slice(&343u16.to_le_bytes());
        thresholds[2..4].copy_from_slice(&373u16.to_le_bytes());
        let log = nvme_log(&[0; 512], &thresholds);
        assert_eq!((log.warning_kelvin, log.critical_kelvin), (343, 373));
        let none = nvme_log(&[0; 512], &[0; 512]);
        assert_eq!((none.warning_kelvin, none.critical_kelvin), (0, 0));
    }

    #[test]
    fn the_kind_is_nvme_hard_disk_or_flash() {
        assert_eq!(kind_of(&healthy_nvme(), None), DiskKind::Nvme);
        assert_eq!(kind_of(&worn_sata(), Some(true)), DiskKind::Hdd);
        assert_eq!(kind_of(&worn_sata(), Some(false)), DiskKind::Ssd);
        // A kernel that does not say: the stricter limits.
        assert_eq!(kind_of(&worn_sata(), None), DiskKind::Hdd);
    }

    #[test]
    fn a_live_file_an_older_service_wrote_still_reads() {
        let file = live("sdb", &worn_sata(), 2_000, None);
        let mut value = serde_json::to_value(&file).unwrap();
        for member in ["kind", "asleep_ms", "limits"] {
            value.as_object_mut().unwrap().remove(member);
        }
        let old: Live = serde_json::from_value(value).unwrap();
        assert_eq!((old.kind, old.asleep_ms, old.limits), (None, None, None));
        assert_eq!(old.summary, file.summary);
    }

    #[test]
    fn a_live_file_has_the_kind_the_limits_and_no_sleep() {
        let file = live("sdb", &worn_sata(), 2_000, None);
        assert_eq!(file.kind, Some(DiskKind::Hdd));
        assert_eq!(file.asleep_ms, None);
        assert_eq!(file.limits, limits(50, 60, LimitsFrom::Kind));
        let nvme = live("nvme0n1", &healthy_nvme(), 2_000, None);
        assert_eq!(
            (nvme.kind, nvme.limits),
            (Some(DiskKind::Nvme), limits(70, 80, LimitsFrom::Kind))
        );
        // A disk whose SMART is off has no temperature, so no limits.
        let off = live("sdc", &Record::AtaOff, 2_000, None);
        assert_eq!(off.limits, None);
    }

    /// A watch in folders of its own.
    fn watch_in(name: &str) -> Watch {
        let base = folder(name);
        Watch { live: base.join("live"), kept: base.join("kept"), ..Watch::new() }
    }

    /// What a look asks of each disk: `found` for each name, and the
    /// disks it does not name keep no record.
    fn reader(found: Vec<(&'static str, Reading)>) -> impl Fn(&str) -> Result<Reading, Fail> {
        let found: Vec<(&'static str, bool, Option<Record>)> = found
            .into_iter()
            .map(|(name, reading)| match reading {
                Reading::Record(record) => (name, false, Some(record)),
                Reading::Asleep => (name, true, None),
                Reading::None => (name, false, None),
            })
            .collect();
        move |name: &str| {
            let asked = found.iter().find(|(disk, ..)| *disk == name);
            Ok(match asked {
                Some((_, _, Some(record))) => Reading::Record(record.clone()),
                Some((_, true, None)) => Reading::Asleep,
                _ => Reading::None,
            })
        }
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn a_sleeping_disk_keeps_its_file_and_its_alerts_and_says_since_when() {
        let mut watch = watch_in("asleep");
        let mut out = Vec::new();
        // The first look reads the disk, which has sectors reallocated.
        let awake = reader(vec![("sdz", Reading::Record(worn_sata()))]);
        watch.look(names(&["sdz"]), &awake, 1_000, &mut out).unwrap();
        let first = read_live(&watch.live, "sdz").unwrap();
        assert_eq!((first.asleep_ms, first.checked_ms), (None, 1_000_000));
        let alerts = watch.all_alerts();
        assert!(!alerts.is_empty());
        // It falls asleep: the file keeps what was read, and says since
        // when.
        let asleep = reader(vec![("sdz", Reading::Asleep)]);
        watch.look(names(&["sdz"]), &asleep, 1_600, &mut out).unwrap();
        let sleeping = read_live(&watch.live, "sdz").unwrap();
        assert_eq!((sleeping.asleep_ms, sleeping.checked_ms), (Some(1_600_000), 1_000_000));
        assert_eq!(sleeping.summary, first.summary);
        assert_eq!(watch.all_alerts(), alerts);
        // Still asleep at the next look: the same since.
        watch.look(names(&["sdz"]), &asleep, 2_200, &mut out).unwrap();
        assert_eq!(read_live(&watch.live, "sdz").unwrap().asleep_ms, Some(1_600_000));
        // A service that began again finds the file's time.
        let mut again =
            Watch { live: watch.live.clone(), kept: watch.kept.clone(), ..Watch::new() };
        again.look(names(&["sdz"]), &asleep, 2_800, &mut out).unwrap();
        assert_eq!(read_live(&watch.live, "sdz").unwrap().asleep_ms, Some(1_600_000));
        // Awake again: read, and the sleep is gone.
        watch.look(names(&["sdz"]), &awake, 3_400, &mut out).unwrap();
        let woke = read_live(&watch.live, "sdz").unwrap();
        assert_eq!((woke.asleep_ms, woke.checked_ms), (None, 3_400_000));
        // Asleep once more: since the new time.
        watch.look(names(&["sdz"]), &asleep, 4_000, &mut out).unwrap();
        assert_eq!(read_live(&watch.live, "sdz").unwrap().asleep_ms, Some(4_000_000));
        fs::remove_dir_all(watch.live.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_disk_found_asleep_with_no_file_gets_none_and_one_that_goes_loses_its_file() {
        let mut watch = watch_in("goes");
        let mut out = Vec::new();
        let asleep = reader(vec![("sdz", Reading::Asleep)]);
        watch.look(names(&["sdz"]), &asleep, 1_000, &mut out).unwrap();
        assert!(read_live(&watch.live, "sdz").is_none());
        let awake = reader(vec![("sdz", Reading::Record(worn_sata()))]);
        watch.look(names(&["sdz"]), &awake, 1_600, &mut out).unwrap();
        assert!(read_live(&watch.live, "sdz").is_some());
        // The disk is gone: its file and its alerts go.
        watch.look(Vec::new(), &awake, 2_200, &mut out).unwrap();
        assert!(read_live(&watch.live, "sdz").is_none());
        assert!(watch.all_alerts().is_empty());
        fs::remove_dir_all(watch.live.parent().unwrap()).unwrap();
    }

    #[test]
    fn the_counts_are_written_only_when_one_changed() {
        let mut watch = watch_in("counts");
        let mut out = Vec::new();
        let awake = reader(vec![("sdz", Reading::Record(worn_sata()))]);
        watch.look(names(&["sdz"]), &awake, 1_000, &mut out).unwrap();
        let file = kept_path(&watch.kept, &identity("sdz").1);
        let written = fs::metadata(&file).unwrap().modified().unwrap();
        let text = fs::read_to_string(&file).unwrap();
        // The same counts at the next look: the file is not touched.
        std::thread::sleep(Duration::from_millis(20));
        watch.look(names(&["sdz"]), &awake, 1_600, &mut out).unwrap();
        assert_eq!(fs::metadata(&file).unwrap().modified().unwrap(), written);
        assert_eq!(fs::read_to_string(&file).unwrap(), text);
        // A count that rose is written, with what the rise says since
        // the last look and not since the file was written.
        let (data, thresholds) = pages(&[(5, 3, 100, 100, 9, 36), (194, 2, 62, 50, 38, 0)]);
        let worse =
            reader(vec![("sdz", Reading::Record(Record::Ata(attributes(&data, &thresholds))))]);
        watch.look(names(&["sdz"]), &worse, 2_200, &mut out).unwrap();
        let kept = Kept::parse(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!((kept.counts.reallocated, kept.checked), (9, 2_200));
        let rose = kept.rose.unwrap().1;
        let since = util::time_text(1_600, "%F %R", false);
        assert!(rose.contains("4 more reallocated sectors") && rose.ends_with(&since), "{rose}");
        fs::remove_dir_all(watch.live.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_line_is_written_only_when_a_verdict_changes() {
        let mut watch = watch_in("lines");
        let awake = reader(vec![("sdz", Reading::Record(worn_sata()))]);
        let mut out = Vec::new();
        watch.look(names(&["sdz"]), &awake, 1_000, &mut out).unwrap();
        let first = String::from_utf8(out).unwrap();
        assert!(first.starts_with("sdz: WARNING: 5 sectors were reallocated"), "{first}");
        let mut out = Vec::new();
        watch.look(names(&["sdz"]), &awake, 1_600, &mut out).unwrap();
        assert!(out.is_empty());
        // Asleep says nothing either.
        let asleep = reader(vec![("sdz", Reading::Asleep)]);
        watch.look(names(&["sdz"]), &asleep, 2_200, &mut out).unwrap();
        assert!(out.is_empty());
        fs::remove_dir_all(watch.live.parent().unwrap()).unwrap();
    }

    #[test]
    fn the_words_for_a_disk_that_sleeps_say_it_is_not_woken() {
        assert_eq!(
            asleep_words("sda"),
            "sda is asleep (standby): it is not read, so as not to wake it"
        );
    }
}
