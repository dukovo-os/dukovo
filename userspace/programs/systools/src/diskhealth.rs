// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! diskhealth(8): what each disk says of its health (ADR 0157), read with
//! radix-block(2)'s GET_HEALTH: a SATA disk's SMART attributes, as
//! smartctl(8) shows them with `-A`, or an NVMe controller's SMART /
//! Health Information log, and a verdict from them. With `--watch`, as
//! Spark's diskhealth service, it looks again every half hour and writes
//! a line, which Spark puts in the kernel log, whenever a disk's verdict
//! or its reasons change.
//!
//! The service also keeps alertd's alerts of the disks (ADR 0485):
//! `smart:SERIAL` while a disk's verdict is not PASSED, and the early
//! warning `smart-rising:SERIAL` for a week after a count that tells of
//! a failure to come rises (reallocated, pending and offline
//! uncorrectable sectors, an NVMe drive's media errors), or while an NVMe
//! drive has used 90% of its rated endurance. What each disk counted at
//! the last look is kept in `/system/state/diskhealth`, a file a disk,
//! so that a rise across a restart is seen too.

use std::fs::{self, File};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::time::Duration;

use radix_abi::errors::Kernel;
use radix_abi::{block, file};

use crate::alerts::protocol::{Alert, Severity, key_part};
use crate::args::{Arg, Args};
use crate::sys::{self, SYS};
use crate::util::{self, Fail};

/// Where the kernel lists the disks and partitions (radix-sysfs(5)).
const BLOCKS: &str = "/kernel/block";
/// How long `--watch` waits between looks: smartd(8)'s default.
const WATCH_EVERY: Duration = Duration::from_mins(30);
/// Where `--watch` keeps what each disk counted at its last look.
const KEPT: &str = "/system/state/diskhealth";
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

/// A disk's verdict, the worst last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Verdict {
    Passed,
    Warning,
    Failing,
}

impl Verdict {
    fn name(self) -> &'static str {
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
}

/// What a disk said of its health.
pub(crate) enum Record {
    Ata(Vec<Attribute>),
    /// A SATA disk whose SMART is off.
    AtaOff,
    Nvme(NvmeLog),
}

/// The verdict on a disk and what it rests on.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Report {
    verdict: Verdict,
    celsius: Option<i64>,
    hours: Option<u64>,
    /// An NVMe drive's share of its rated endurance used, in percent.
    used: Option<u8>,
    reasons: Vec<String>,
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

/// Reads an NVMe controller's SMART / Health Information log.
fn nvme_log(data: &[u8]) -> NvmeLog {
    let wide = |at: usize| {
        let mut bytes = [0u8; 16];
        if let Some(field) = data.get(at..at + 16) {
            bytes.copy_from_slice(field);
        }
        u128::from_le_bytes(bytes)
    };
    let byte = |at: usize| data.get(at).copied().unwrap_or(0);
    let word = |at: usize| u32::from_le_bytes([byte(at), byte(at + 1), byte(at + 2), byte(at + 3)]);
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

/// A line of what the kernel shows of disk `name`'s device.
pub(crate) fn device_text(name: &str, what: &str) -> String {
    let path = format!("{BLOCKS}/{name}/device/{what}");
    fs::read_to_string(path).map_or_default(|text| text.trim().to_string())
}

/// Asks disk `name` of its health; `None` if it keeps no record.
pub(crate) fn read(name: &str) -> Result<Option<Record>, Fail> {
    let path = format!("/devices/{name}");
    let device = File::open(&path).map_err(|e| util::failed(&path, e))?;
    let mut value = [0u8; block::HEALTH_SIZE as usize];
    let address = value.as_mut_ptr() as u64;
    let slot = sys::slot_of(device.as_raw_fd());
    let len = value.len() as u64;
    match file::control(&SYS, slot, block::GET_HEALTH, address, address, len) {
        Ok(_) => {}
        Err(status) if Kernel::from_status(status) == Some(Kernel::NotSupported) => {
            return Ok(None);
        }
        Err(status) => return Err(util::failed(&path, sys::io_error(status))),
    }
    let word =
        |at: usize| u32::from_le_bytes([value[at], value[at + 1], value[at + 2], value[at + 3]]);
    let (data, thresholds) = (&value[8..520], &value[520..1032]);
    Ok(Some(match word(0) {
        block::HEALTH_NVME => Record::Nvme(nvme_log(data)),
        _ if word(4) & block::HEALTH_SMART_OFF != 0 => Record::AtaOff,
        _ => Record::Ata(attributes(data, thresholds)),
    }))
}

/// The report on `record`, and its kind's name.
fn report(record: &Record) -> (&'static str, Report) {
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
    let rising = rose.as_ref().filter(|(at, _)| now.saturating_sub(*at) < RISE_TOLD);
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

/// The kept file of the disk `id` stands for.
fn kept_path(id: &str) -> String {
    format!("{KEPT}/{}", id.replace(['/', '.'], "-"))
}

/// Writes `kept` to its file, by a new file renamed over it.
fn keep(path: &str, kept: &Kept) -> io::Result<()> {
    fs::create_dir_all(KEPT)?;
    let new = format!("{path}.new");
    fs::write(&new, kept.text())?;
    fs::rename(&new, path)
}

/// The alerts of a disk at a look at `now`, keeping its counts.
fn disk_alerts(name: &str, record: &Record, report: &Report, now: u64) -> Vec<Alert> {
    let (disk, id) = identity(name);
    let mut alerts: Vec<Alert> = verdict_alert(report, &disk, name, &id).into_iter().collect();
    if let Some(counts) = Counts::of(record) {
        let path = kept_path(&id);
        let before = fs::read_to_string(&path).ok().and_then(|text| Kept::parse(&text));
        let (kept, rising) = early_warning(before.as_ref(), counts, now, &disk, &id);
        if let Err(err) = keep(&path, &kept) {
            eprintln!("diskhealth: {}", util::failed(&Path::new(&path).display(), err).message);
        }
        alerts.extend(rising);
    }
    alerts
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

/// `--watch`: looks at every disk now and every half hour, writes a
/// line of each whose verdict or reasons changed, and sets the disks'
/// alerts. Runs until stopped.
fn watch() -> Result<i32, Fail> {
    let mut last: Vec<(String, Report)> = Vec::new();
    // Each disk's alerts at its last look, kept for a look that cannot
    // read it, so that they are not cleared for that.
    let mut alerts: Vec<(String, Vec<Alert>)> = Vec::new();
    let mut unreached = false;
    loop {
        let names = disks().map_err(|e| util::failed(&BLOCKS, e))?;
        let now = util::now_ns() / 1_000_000_000;
        alerts.retain(|(disk, _)| names.contains(disk));
        let mut out = io::stdout().lock();
        for name in names {
            let report = match read(&name) {
                Ok(Some(record)) => {
                    let report = report(&record).1;
                    let found = disk_alerts(&name, &record, &report, now);
                    match alerts.iter_mut().find(|(disk, _)| *disk == name) {
                        Some((_, held)) => *held = found,
                        None => alerts.push((name.clone(), found)),
                    }
                    report
                }
                Ok(None) => continue,
                Err(fail) => {
                    eprintln!("diskhealth: {}", fail.message);
                    continue;
                }
            };
            let seen = last.iter_mut().find(|(seen, _)| *seen == name);
            match seen {
                Some((_, before)) if same_but_numbers(before, &report) => {}
                Some((_, before)) => {
                    writeln!(out, "{}", watch_line(&name, &report))?;
                    *before = report;
                }
                None => {
                    writeln!(out, "{}", watch_line(&name, &report))?;
                    last.push((name, report));
                }
            }
        }
        out.flush()?;
        drop(out);
        let all: Vec<Alert> = alerts.iter().flat_map(|(_, held)| held.iter().cloned()).collect();
        match crate::alerts::client::set(&all) {
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

/// diskhealth(8).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let (mut all, mut names) = (false, Vec::new());
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('a') => all = true,
            Arg::Long(long) if long == "all" => all = true,
            Arg::Long(long) if long == "watch" => return watch(),
            Arg::Operand(name) => {
                let name = name.to_string_lossy();
                names.push(name.strip_prefix("/devices/").unwrap_or(&name).to_string());
            }
            other => return Err(args.unknown(&other)),
        }
    }
    let details_wanted = all || !names.is_empty();
    if names.is_empty() {
        names = disks().map_err(|e| util::failed(&BLOCKS, e))?;
    }
    let mut rows = Vec::new();
    let mut status = 0;
    for name in names {
        match read(&name) {
            Ok(record) => {
                if let Some(record) = &record {
                    status |= match report(record).1.verdict {
                        Verdict::Passed => 0,
                        Verdict::Warning => WARNS,
                        Verdict::Failing => FAILS,
                    };
                }
                rows.push((name, record));
            }
            Err(fail) => {
                eprintln!("diskhealth: {}", fail.message);
                status |= UNREAD;
            }
        }
    }
    let mut out = io::stdout().lock();
    if details_wanted {
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
        let log = nvme_log(&data);
        assert_eq!(
            (log.kelvin, log.spare, log.used, log.units_written, log.hours),
            (323, 100, 3, 7, 500)
        );
        let report = nvme_report(&log);
        assert_eq!(report.verdict, Verdict::Passed);
        assert_eq!((report.celsius, report.hours, report.used), (Some(50), Some(500), Some(3)));
        data[0] = 1 << 1;
        assert_eq!(nvme_report(&nvme_log(&data)).verdict, Verdict::Warning);
        data[0] |= 1 << 2;
        let report = nvme_report(&nvme_log(&data));
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
        assert_eq!(kept_path("../x/y"), format!("{KEPT}/---x-y"));
    }
}
