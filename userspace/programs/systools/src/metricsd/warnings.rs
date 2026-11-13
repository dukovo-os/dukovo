// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The early warnings of metricsd(8): what the owner is told before a
//! disk is full, a volume has lost its second copy, a certificate has
//! ended or a sensor has overheated (ADR 0484).
//!
//! After each step [`Warnings::check`] works out the whole set of problems
//! the reading shows, and [`Warnings::report`] sends it to alertd(8) as
//! the service's alerts, when the set has changed and in any case every
//! 10 minutes, so that alertd's view catches up after a restart of either
//! side. What alertd then does with it, telling the owner once for each
//! new alert, is alertd's.
//!
//! Every check has a hysteresis, so that a value that sits on its line
//! does not raise and clear its alert again and again: a filesystem that
//! is 90% full is a warning from 90 and not one below 87, and memory is
//! low after five steps in a row, not one. Each alert's title names the
//! thing and the number, and its detail what to do and an error ID, such
//! as `MET-101`, whose page in the support center says more.
//!
//! A mistake of the sender's own, a key or title that alertd would refuse,
//! cannot be made: keys are cut down to what alertd takes and titles to
//! one line. When a warning is missing, `log metricsd` says whether
//! alertd took the last set.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;

use super::sample::{Filesystem, Snapshot, Temperature, Volume};
use super::settings::Thresholds;
use super::store::{self, Selector};
use crate::alerts::protocol::{Alert, DETAIL_MAX, KEY_MAX, Severity, TITLE_MAX};
use crate::{logd, util};

/// The folder of the system's certificates, where `*.pem` and `*.crt`
/// files are checked.
pub const SSL: &str = "/system/settings/ssl";

/// The percent of a filesystem in use that is critical.
const FULL_CRITICAL: f64 = 97.0;
/// How far a value falls below a line before its alert is cleared, in
/// percent of a filesystem and degrees of a sensor.
const DROP: f64 = 3.0;

/// A filesystem less full than this, in percent, is not filling.
const FILLING_FLOOR: f64 = 50.0;
/// A filling warning is cleared when the days to full are more than the
/// setting times this.
const FILLING_CLEAR: f64 = 1.5;
/// The rate a filesystem fills at is the line through the last 6 hours of
/// its use, one reading at least a minute after the one before, and not
/// less than 30 of them.
const FILL_WINDOW: i64 = 6 * 3600;
const FILL_SPACING: i64 = 60;
const FILL_POINTS: usize = 30;

/// Memory is low when less than 5% of it is available for 5 steps in a
/// row, until 10% is.
const MEMORY_LOW: f64 = 5.0;
const MEMORY_OK: f64 = 10.0;
const STEPS_IN_A_ROW: u32 = 5;

/// A certificate is critical when it ends within this many days.
const CERTIFICATE_CRITICAL_DAYS: i64 = 3;
/// How often the certificates are read.
const CERTIFICATES_EVERY: Duration = Duration::from_secs(3600);

/// The alerts are sent again after this long, whether they changed or
/// not, and a send that failed is tried again after the shorter one.
const REFRESH: Duration = Duration::from_secs(600);
const RETRY: Duration = Duration::from_secs(10);

const FULL: &str = "MET-101";
const FILLING: &str = "MET-102";
const MISSING: &str = "MET-103";
const BEHIND: &str = "MET-104";
const MEMORY: &str = "MET-105";
const HOT: &str = "MET-106";
const CERTIFICATE: &str = "MET-107";

/// How bad a check finds a thing, which it keeps from one step to the
/// next for its hysteresis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    #[default]
    Clear,
    Warning,
    Critical,
}

impl Level {
    fn severity(self) -> Option<Severity> {
        match self {
            Level::Clear => None,
            Level::Warning => Some(Severity::Warning),
            Level::Critical => Some(Severity::Critical),
        }
    }
}

/// The level of a value, a percentage or a temperature, that was at level
/// `before` the step before: a warning from `warn` up, critical from
/// `critical` up if there is one. A warning is cleared only `DROP` below
/// its line, and a critical one is a warning again only `DROP` below
/// its own.
fn level_after(before: Level, value: f64, warn: f64, critical: Option<f64>) -> Level {
    if critical.is_some_and(|critical| value >= critical) {
        return Level::Critical;
    }
    match before {
        Level::Critical if critical.is_some_and(|critical| value >= critical - DROP) => {
            Level::Critical
        }
        Level::Critical | Level::Warning if value >= warn - DROP => Level::Warning,
        Level::Clear if value >= warn => Level::Warning,
        _ => Level::Clear,
    }
}

/// A key alertd takes, made of a kind and the name of the thing: lower
/// case letters, digits and `._:/-`, the rest of the name's characters
/// turned into `-`, and not longer than a key may be.
fn key(kind: &str, name: &str) -> String {
    let clean = |c: char| {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() || "._:/-".contains(c) { c } else { '-' }
    };
    let name: String = name.chars().map(clean).collect();
    let mut key = format!("{kind}:{name}");
    key.truncate(KEY_MAX);
    key
}

/// `text` as a title alertd takes: one line, not longer than a title may
/// be, ending in an ellipsis if it was cut.
fn one_line(text: &str) -> String {
    let flat: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    if flat.chars().count() <= TITLE_MAX {
        return flat;
    }
    let mut cut: String = flat.chars().take(TITLE_MAX - 1).collect();
    cut.push('…');
    cut
}

/// An alert, its detail ended with the error ID of its page.
fn alert(key: String, level: Level, title: &str, advice: &str, id: &str) -> Option<Alert> {
    let mut detail = format!("{advice} ({id})");
    detail.truncate(DETAIL_MAX);
    Some(Alert { key, severity: level.severity()?, title: one_line(title), detail })
}

/// `bytes` as `1.2 TB`, in powers of 1000.
fn size_text(bytes: u64) -> String {
    let text = util::human(bytes, true);
    match text.strip_suffix(|c: char| c.is_ascii_alphabetic()) {
        Some(number) => format!("{number} {}B", &text[number.len()..]),
        None => format!("{text} B"),
    }
}

/// The percent of `whole` that `part` is.
fn percent(part: u64, whole: u64) -> f64 {
    part as f64 * 100.0 / whole as f64
}

/// The readings of the last 6 hours of one filesystem's use, which the
/// line of its filling is drawn through.
#[derive(Default)]
struct Window {
    /// A time in seconds since 1970 and the bytes in use then.
    points: VecDeque<(i64, f64)>,
}

impl Window {
    /// Takes in a reading, unless it is less than a minute after the
    /// last. A clock that went back starts the window again.
    fn add(&mut self, at: i64, used: f64) {
        match self.points.back() {
            Some(&(last, _)) if at < last => self.points.clear(),
            Some(&(last, _)) if at - last < FILL_SPACING => return,
            _ => {}
        }
        self.points.push_back((at, used));
        while self.points.front().is_some_and(|&(first, _)| at - first > FILL_WINDOW) {
            self.points.pop_front();
        }
    }

    /// The days until the line through the readings reaches `size`
    /// bytes, if they are enough and the line rises.
    fn days_to_full(&self, size: f64) -> Option<f64> {
        if self.points.len() < FILL_POINTS {
            return None;
        }
        let (start, last) = (self.points.front()?.0, self.points.back()?.0);
        let count = self.points.len() as f64;
        // About the means, so that 1.3 TB and a clock in seconds do not
        // cost the slope its digits.
        let mean_time = self.points.iter().map(|&(at, _)| (at - start) as f64).sum::<f64>() / count;
        let mean_used = self.points.iter().map(|&(_, used)| used).sum::<f64>() / count;
        let (mut spread, mut together) = (0.0, 0.0);
        for &(at, used) in &self.points {
            let time = (at - start) as f64 - mean_time;
            spread += time * time;
            together += time * (used - mean_used);
        }
        let slope = together / spread;
        if !slope.is_finite() || slope <= 0.0 {
            return None;
        }
        let used_now = mean_used + slope * ((last - start) as f64 - mean_time);
        Some(((size - used_now) / slope).max(0.0) / 86_400.0)
    }
}

/// What a filesystem's checks keep from step to step.
#[derive(Default)]
struct FilesystemState {
    full: Level,
    filling: bool,
    window: Window,
}

/// What a sensor's check keeps.
#[derive(Default)]
struct SensorState {
    level: Level,
    /// The steps in a row it was on or over its line, while it is clear.
    over: u32,
}

/// Whether a filesystem that is `used_percent` full and fills in `days`
/// is a warning, that was one or not the step before: from `limit` days,
/// and not until more than `limit` times 1.5.
fn filling_after(before: bool, used_percent: f64, days: Option<f64>, limit: f64) -> bool {
    let Some(days) = days.filter(|_| used_percent >= FILLING_FLOOR) else { return false };
    if before { days <= limit * FILLING_CLEAR } else { days <= limit }
}

/// Memory's check: whether it is low, and the steps in a row it has been
/// below the line that raises it.
#[derive(Default)]
struct MemoryState {
    low: bool,
    below: u32,
}

impl MemoryState {
    /// Takes in a step's `available` percent of memory.
    fn step(&mut self, available: f64) {
        if self.low {
            self.low = available < MEMORY_OK;
            self.below = 0;
        } else if available < MEMORY_LOW {
            self.below += 1;
            self.low = self.below >= STEPS_IN_A_ROW;
        } else {
            self.below = 0;
        }
    }
}

/// When the alerts were sent, to send them only when they changed and
/// every [`REFRESH`], and a failed send again after [`RETRY`].
#[derive(Default)]
struct Sender {
    /// What alertd took last, and when.
    sent: Option<(Vec<Alert>, Instant)>,
    /// When a send last failed, until one succeeds.
    failed: Option<Instant>,
}

/// What [`Warnings::report`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Sent {
    /// Nothing was due.
    Not,
    /// alertd took the set.
    Yes,
    /// alertd did not, for the reason; a try again follows.
    Failed(String),
}

/// The early warnings: what each check keeps, and the sender.
pub struct Warnings {
    thresholds: Thresholds,
    ssl: PathBuf,
    filesystems: HashMap<String, FilesystemState>,
    memory: MemoryState,
    sensors: HashMap<String, SensorState>,
    /// The certificates' alerts, and when they were read.
    certificates: Option<(Vec<Alert>, Instant)>,
    sender: Sender,
}

impl Warnings {
    /// The warnings for `thresholds`, which read the certificates in `ssl`.
    pub fn new(thresholds: Thresholds, ssl: &Path) -> Warnings {
        Warnings {
            thresholds,
            ssl: ssl.to_path_buf(),
            filesystems: HashMap::new(),
            memory: MemoryState::default(),
            sensors: HashMap::new(),
            certificates: None,
            sender: Sender::default(),
        }
    }

    /// Takes new thresholds, as after a reload. The certificates are read
    /// again at the next step, since their line moved.
    pub fn set_thresholds(&mut self, thresholds: Thresholds) {
        self.thresholds = thresholds;
        self.certificates = None;
    }

    /// Takes in the filesystems' use of the last 6 hours from the history
    /// in `dir`, which `now`, seconds since 1970, is the end of, so that
    /// a restart does not start the line from nothing.
    pub fn backfill(&mut self, dir: &Path, now: i64) {
        let select = [Selector::parse("fs.used")];
        let Ok(series) = store::read(dir, now - FILL_WINDOW, now, &select, None) else { return };
        for series in series {
            let mount = series.name.strip_prefix("fs.used{").and_then(|m| m.strip_suffix('}'));
            let Some(mount) = mount else { continue };
            let state = self.filesystems.entry(mount.to_string()).or_default();
            for (at, used) in series.points.into_iter().filter(|(_, used)| used.is_finite()) {
                state.window.add(at, f64::from(used));
            }
        }
    }

    /// The set of problems `snapshot` shows, by key: each check takes in
    /// the reading, and what a thing that is gone had is forgotten, so
    /// that its alert is gone from the set. `now` is when it was read.
    pub fn check(&mut self, snapshot: &Snapshot, now: Instant) -> Vec<Alert> {
        let mut alerts = self.filesystem_alerts(snapshot);
        alerts.extend(volume_alerts(&snapshot.volumes));
        alerts.extend(self.memory_alerts(snapshot));
        alerts.extend(self.temperature_alerts(&snapshot.temperatures));
        alerts.extend(self.certificate_alerts(snapshot.at, now));
        // Two things whose names are one key once cut down are one alert,
        // the worse of them.
        alerts.sort_by(|a, b| a.key.cmp(&b.key).then(b.severity.cmp(&a.severity)));
        alerts.dedup_by(|later, first| later.key == first.key);
        alerts
    }

    /// Sends `alerts` to alertd with `set` if they differ from what it
    /// was last given, or it was last given them 10 minutes before, or a
    /// send failed and 10 seconds have passed since.
    pub fn report(
        &mut self,
        alerts: &[Alert],
        now: Instant,
        set: impl FnOnce(&[Alert]) -> Result<(), String>,
    ) -> Sent {
        let sender = &mut self.sender;
        let due = match &sender.sent {
            Some((sent, at)) => sent != alerts || now.duration_since(*at) >= REFRESH,
            None => true,
        };
        let waiting = sender.failed.is_some_and(|at| now.duration_since(at) < RETRY);
        if !due || waiting {
            return Sent::Not;
        }
        match set(alerts) {
            Ok(()) => {
                sender.sent = Some((alerts.to_vec(), now));
                sender.failed = None;
                Sent::Yes
            }
            Err(why) => {
                sender.failed = Some(now);
                Sent::Failed(why)
            }
        }
    }

    fn filesystem_alerts(&mut self, snapshot: &Snapshot) -> Vec<Alert> {
        let mut alerts = Vec::new();
        let mut states = HashMap::new();
        for filesystem in snapshot.filesystems.iter().filter(|f| f.size > 0) {
            let mut state = self.filesystems.remove(&filesystem.mount).unwrap_or_default();
            // Nothing to be done about the space of what cannot be
            // written.
            if !filesystem.read_only {
                alerts.extend(self.check_filesystem(filesystem, snapshot.at, &mut state));
            }
            states.insert(filesystem.mount.clone(), state);
        }
        self.filesystems = states;
        alerts
    }

    /// One filesystem's checks, `at` being the time of the reading.
    fn check_filesystem(
        &self,
        filesystem: &Filesystem,
        at: i64,
        state: &mut FilesystemState,
    ) -> Vec<Alert> {
        let used = percent(filesystem.used(), filesystem.size);
        let warn = f64::from(self.thresholds.filesystem);
        state.full = level_after(state.full, used, warn, Some(FULL_CRITICAL));
        state.window.add(at, filesystem.used() as f64);
        let days = state.window.days_to_full(filesystem.size as f64);
        let limit = f64::from(self.thresholds.filling_days);
        state.filling = filling_after(state.filling, used, days, limit);
        let mount = &filesystem.mount;
        let mut alerts = Vec::new();
        let (in_use, whole) = (size_text(filesystem.used()), size_text(filesystem.size));
        alerts.extend(alert(
            key("fs-full", mount),
            state.full,
            &format!("{mount} is {}% full ({in_use} of {whole})", used.floor()),
            &format!(
                "Free space on {mount} or add a disk before it is full: `df -h {mount}` shows \
                 what is in use."
            ),
            FULL,
        ));
        if let (true, Some(days)) = (state.filling, days) {
            alerts.extend(alert(
                key("fs-filling", mount),
                Level::Warning,
                &format!("{mount} will be full in {} at the last 6 hours' rate", about(days)),
                &format!(
                    "{mount} holds {in_use} of {whole} and has been filling at this \
                     rate for 6 hours. Free space or add a disk before then: `df -h {mount}` \
                     shows what is in use."
                ),
                FILLING,
            ));
        }
        alerts
    }

    fn memory_alerts(&mut self, snapshot: &Snapshot) -> Option<Alert> {
        let memory = &snapshot.memory;
        if memory.total == 0 {
            return None;
        }
        let available = percent(memory.available(), memory.total);
        self.memory.step(available);
        alert(
            "memory:low".into(),
            if self.memory.low { Level::Warning } else { Level::Clear },
            &format!(
                "Memory is nearly used up: {} of {} is available",
                size_text(memory.available() * 1024),
                size_text(memory.total * 1024)
            ),
            "`top` shows which programs hold the memory and `free` how it is used; stop what is \
             not needed, or add memory.",
            MEMORY,
        )
    }

    fn temperature_alerts(&mut self, temperatures: &[Temperature]) -> Vec<Alert> {
        let margin = f64::from(self.thresholds.temperature_margin);
        let mut alerts = Vec::new();
        let mut states = HashMap::new();
        for temperature in temperatures {
            let Some((warn, critical)) = lines(temperature, margin) else { continue };
            let instance = temperature.instance();
            let mut state = self.sensors.remove(&instance).unwrap_or_default();
            let level = level_after(state.level, temperature.celsius, warn, critical);
            // A sensor is raised only after it has stayed over its line.
            if state.level == Level::Clear && level != Level::Clear {
                state.over += 1;
                state.level = if state.over >= STEPS_IN_A_ROW { level } else { Level::Clear };
            } else {
                state.over = 0;
                state.level = level;
            }
            alerts.extend(alert(
                key("temp", &instance),
                state.level,
                &format!(
                    "{instance} is at {} °C; its limit is {} °C",
                    temperature.celsius.round(),
                    critical.unwrap_or(warn).round()
                ),
                "`sensors` shows every sensor. Check that the fans turn and the air can reach \
                 them.",
                HOT,
            ));
            states.insert(instance, state);
        }
        self.sensors = states;
        alerts
    }

    /// The certificates' alerts, read again once an hour.
    fn certificate_alerts(&mut self, at: i64, now: Instant) -> Vec<Alert> {
        let stale =
            |(_, read): &(Vec<Alert>, Instant)| now.duration_since(*read) >= CERTIFICATES_EVERY;
        if self.certificates.as_ref().is_none_or(stale) {
            let alerts = certificates(&self.ssl, at, i64::from(self.thresholds.certificate_days));
            self.certificates = Some((alerts, now));
        }
        self.certificates.as_ref().map_or_default(|(alerts, _)| alerts.clone())
    }
}

/// The temperature at which `temperature` is a warning and, if it has
/// one, a critical limit: `margin` degrees below its critical
/// temperature, or its maximum where it has no critical one. A sensor with
/// neither, or with one that is no temperature, is not watched.
fn lines(temperature: &Temperature, margin: f64) -> Option<(f64, Option<f64>)> {
    let positive = |limit: Option<f64>| limit.filter(|limit| *limit > 0.0);
    match (positive(temperature.crit), positive(temperature.max)) {
        (Some(critical), _) => Some((critical - margin, Some(critical))),
        (None, Some(max)) => Some((max, None)),
        (None, None) => None,
    }
}

/// How long `days` is, said roughly: "about 2 days", "about 5 hours".
fn about(days: f64) -> String {
    let hours = (days * 24.0).round().max(1.0);
    if hours >= 36.0 {
        format!("about {} days", days.round())
    } else if hours >= 24.0 {
        "about a day".into()
    } else if hours == 1.0 {
        "about an hour".into()
    } else {
        format!("about {hours} hours")
    }
}

/// The alerts of the Btrfs volumes: a critical one for each that is
/// missing a disk, a warning for each with a disk that is behind.
fn volume_alerts(volumes: &[Volume]) -> Vec<Alert> {
    let mut alerts = Vec::new();
    for volume in volumes {
        let name = volume.name();
        let level = |count: u64, level| if count > 0 { level } else { Level::Clear };
        alerts.extend(alert(
            key("btrfs-missing", name),
            level(volume.missing, Level::Critical),
            &format!(
                "Btrfs volume {name} is missing {} of its {} disks; its data has no second \
                 copy until it is replaced",
                volume.missing, volume.disks
            ),
            "Put a new disk in its place with `replace start` (replace(8)); a second failure \
             before then loses data.",
            MISSING,
        ));
        alerts.extend(alert(
            key("btrfs-behind", name),
            level(volume.behind, Level::Warning),
            &format!(
                "Btrfs volume {name} has {} of its {} disks behind: they missed changes",
                volume.behind, volume.disks
            ),
            "A scrub brings them up to date from the other copies: run `scrub start` on a \
             folder of the volume (scrub(8)).",
            BEHIND,
        ));
    }
    alerts
}

/// The alerts of the certificates in the PEM files `*.pem` and `*.crt`
/// of `dir`: the first certificate of each file, a warning when it ends
/// within `days` days of `now`, seconds since 1970, and critical when it
/// ends within 3 days or has ended. A file with no certificate in it, a
/// key's, is left out, and so is a certificate that does not read.
fn certificates(dir: &Path, now: i64, days: i64) -> Vec<Alert> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("pem") || e.eq_ignore_ascii_case("crt"))
        })
        .collect();
    files.sort();
    files.iter().filter_map(|file| certificate_alert(file, now, days)).collect()
}

/// The alert for the certificate in `file`, if it is due.
fn certificate_alert(file: &Path, now: i64, days: i64) -> Option<Alert> {
    let first = CertificateDer::pem_file_iter(file).ok()?.next()?.ok()?;
    let (_, end) = crate::logforward::validity(&first)?;
    let end = i64::try_from(end).ok()?;
    let left = end - now;
    if left > days * 86_400 {
        return None;
    }
    let level =
        if left <= CERTIFICATE_CRITICAL_DAYS * 86_400 { Level::Critical } else { Level::Warning };
    let (year, month, day) = logd::civil(end.div_euclid(86_400));
    let name = file.file_name()?.to_string_lossy();
    let verb = if left <= 0 { "ended" } else { "ends" };
    alert(
        key("certificate", &name),
        level,
        &format!("The certificate in {} {verb} on {year:04}-{month:02}-{day:02}", file.display()),
        if left <= 0 {
            "It is no longer valid, and programs that check it refuse it: replace it with a \
             renewed certificate from its issuer."
        } else {
            "Replace it with a renewed certificate from its issuer before then, and have the \
             services that use it read it again."
        },
        CERTIFICATE,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1_000_000_000;

    fn filesystem(mount: &str, used_percent: u64) -> Filesystem {
        Filesystem {
            device: "vda1".into(),
            fstype: "ext4".into(),
            mount: mount.into(),
            read_only: false,
            size: 100 * GB,
            free: (100 - used_percent) * GB,
            available: (100 - used_percent) * GB,
        }
    }

    fn snapshot(filesystems: Vec<Filesystem>) -> Snapshot {
        Snapshot { at: 1_800_000_000, filesystems, ..Snapshot::default() }
    }

    fn warnings() -> Warnings {
        Warnings::new(Thresholds::default(), Path::new("/nonexistent-ssl"))
    }

    /// The alerts `snapshot` gives, as `key severity` words.
    fn said(warnings: &mut Warnings, snapshot: &Snapshot) -> Vec<String> {
        warnings
            .check(snapshot, Instant::now())
            .iter()
            .map(|a| format!("{} {}", a.key, a.severity.name()))
            .collect()
    }

    fn at_used(warnings: &mut Warnings, mount: &str, percent: u64) -> Vec<String> {
        said(warnings, &snapshot(vec![filesystem(mount, percent)]))
    }

    #[test]
    fn a_filesystem_is_a_warning_from_the_setting_and_critical_from_97() {
        let mut w = warnings();
        assert!(at_used(&mut w, "/data", 89).is_empty());
        assert_eq!(at_used(&mut w, "/data", 90), ["fs-full:/data warning"]);
        assert_eq!(at_used(&mut w, "/data", 96), ["fs-full:/data warning"]);
        assert_eq!(at_used(&mut w, "/data", 97), ["fs-full:/data critical"]);
    }

    #[test]
    fn the_filesystem_hysteresis_is_held_on_the_way_down() {
        let mut w = warnings();
        at_used(&mut w, "/data", 98);
        // Critical until below 94, then a warning until below 87.
        assert_eq!(at_used(&mut w, "/data", 94), ["fs-full:/data critical"]);
        assert_eq!(at_used(&mut w, "/data", 93), ["fs-full:/data warning"]);
        assert_eq!(at_used(&mut w, "/data", 87), ["fs-full:/data warning"]);
        assert!(at_used(&mut w, "/data", 86).is_empty());
        // And once clear it comes back at the line, not before.
        assert!(at_used(&mut w, "/data", 89).is_empty());
        assert_eq!(at_used(&mut w, "/data", 90), ["fs-full:/data warning"]);
        // A setting of 80 moves the lines with it.
        let mut w =
            Warnings::new(Thresholds { filesystem: 80, ..Thresholds::default() }, Path::new("/x"));
        assert_eq!(at_used(&mut w, "/data", 80), ["fs-full:/data warning"]);
        assert_eq!(at_used(&mut w, "/data", 77), ["fs-full:/data warning"]);
        assert!(at_used(&mut w, "/data", 76).is_empty());
    }

    #[test]
    fn the_title_names_the_mount_and_the_numbers() {
        let mut w = warnings();
        let alerts = w.check(&snapshot(vec![filesystem("/data", 93)]), Instant::now());
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].title, "/data is 93% full (93 GB of 100 GB)");
        assert!(alerts[0].detail.contains("`df -h /data`"));
        assert!(alerts[0].detail.ends_with("(MET-101)"), "{}", alerts[0].detail);
        assert_eq!(size_text(1_300_000_000_000), "1.3 TB");
        assert_eq!(size_text(512), "512 B");
    }

    #[test]
    fn a_read_only_mount_and_one_that_went_away_have_no_alert() {
        let mut w = warnings();
        let mut disc = filesystem("/media/disc", 100);
        disc.read_only = true;
        assert!(said(&mut w, &snapshot(vec![disc])).is_empty());
        assert_eq!(at_used(&mut w, "/stick", 95), ["fs-full:/stick warning"]);
        // Its alert is gone from the set with it, and its hysteresis.
        assert!(said(&mut w, &snapshot(Vec::new())).is_empty());
        assert!(at_used(&mut w, "/stick", 89).is_empty());
    }

    /// A window of readings a minute apart ending at `end`, used rising
    /// by `per_hour` bytes an hour from `start_used`.
    fn rising(count: i64, start_used: f64, per_hour: f64) -> Window {
        let mut window = Window::default();
        for i in 0..count {
            window
                .add(1_800_000_000 - (count - 1 - i) * 60, start_used + per_hour * i as f64 / 60.0);
        }
        window
    }

    #[test]
    fn the_fill_line_gives_the_days_to_full() {
        // 100 GB, 60 GB used and 1 GB an hour for the 6 hours: 40 GB to
        // go at 24 GB a day.
        let window = rising(360, 54e9, 1e9);
        let days = window.days_to_full(100e9).unwrap();
        assert!((days - 40.0 / 24.0).abs() < 0.01, "{days}");
        // Falling or flat, there is no line to the top.
        assert!(rising(360, 54e9, -1e9).days_to_full(100e9).is_none());
        assert!(rising(360, 54e9, 0.0).days_to_full(100e9).is_none());
        // Once past it the days are none.
        assert_eq!(rising(360, 54e9, 1e9).days_to_full(50e9), Some(0.0));
    }

    #[test]
    fn readings_less_than_a_minute_apart_and_older_than_6_hours_are_left_out() {
        let mut window = Window::default();
        window.add(1000, 1.0);
        window.add(1030, 2.0);
        window.add(1060, 3.0);
        assert_eq!(window.points.len(), 2);
        window.add(1060 + FILL_WINDOW, 4.0);
        assert_eq!(window.points.len(), 2, "the first is over 6 hours back");
        // A clock that went back starts it again.
        window.add(500, 5.0);
        assert_eq!(window.points.len(), 1);
    }

    #[test]
    fn filling_is_raised_cleared_and_held() {
        // From 3 days, not until more than 4.5 days, and not below half
        // full.
        assert!(!filling_after(false, 60.0, Some(3.1), 3.0));
        assert!(filling_after(false, 60.0, Some(3.0), 3.0));
        assert!(filling_after(true, 60.0, Some(4.4), 3.0));
        assert!(!filling_after(true, 60.0, Some(4.6), 3.0));
        assert!(!filling_after(true, 49.9, Some(1.0), 3.0));
        assert!(!filling_after(false, 90.0, None, 3.0));
        assert!(!filling_after(true, 90.0, None, 3.0));
    }

    /// Steps of `check` a minute apart, `used` percent more each, the first
    /// at `used_start`.
    fn fill(w: &mut Warnings, steps: i64, used_start: f64, per_step: f64) -> Vec<Alert> {
        let mut last = Vec::new();
        for i in 0..steps {
            let used = (used_start + per_step * i as f64) * 1e9;
            let filesystem = Filesystem { free: 100 * GB - used as u64, ..filesystem("/data", 0) };
            let snapshot = Snapshot {
                at: 1_800_000_000 + i * 60,
                filesystems: vec![filesystem],
                ..Snapshot::default()
            };
            last = w.check(&snapshot, Instant::now());
        }
        last
    }

    #[test]
    fn filling_needs_30_readings_and_a_line_that_reaches_the_top() {
        let mut w = warnings();
        // 29 readings of a fast rise give nothing, the 30th a warning.
        let alerts = fill(&mut w, 29, 60.0, 0.1);
        assert!(alerts.iter().all(|a| a.key != "fs-filling:/data"), "{alerts:?}");
        let alerts = fill(&mut w, 31, 60.0, 0.1);
        let found = alerts.iter().find(|a| a.key == "fs-filling:/data").expect("filling");
        assert_eq!(found.severity, Severity::Warning);
        assert!(found.title.starts_with("/data will be full in about "), "{}", found.title);
        assert!(found.title.ends_with(" at the last 6 hours' rate"), "{}", found.title);
        assert!(found.detail.ends_with("(MET-102)"));
        // A filesystem under half full does not warn, whatever its rate.
        let mut w = warnings();
        let alerts = fill(&mut w, 60, 1.0, 0.1);
        assert!(alerts.iter().all(|a| a.key != "fs-filling:/data"), "{alerts:?}");
    }

    #[test]
    fn filling_is_held_until_the_days_pass_one_and_a_half_times_the_setting() {
        let mut w = warnings();
        // 0.1 GB a minute: 40 GB left is under 7 hours.
        let alerts = fill(&mut w, 40, 60.0, 0.1);
        assert!(alerts.iter().any(|a| a.key == "fs-filling:/data"));
        // The same state slows to 0.002 GB a minute (about 14 days): once
        // enough of the window is that slow, it clears.
        let mut cleared = false;
        let mut at = 1_800_000_000 + 40 * 60;
        let mut used = 64.0e9;
        for _ in 0..400 {
            used += 0.002e9;
            at += 60;
            let filesystem = Filesystem { free: 100 * GB - used as u64, ..filesystem("/data", 0) };
            let snapshot = Snapshot { at, filesystems: vec![filesystem], ..Snapshot::default() };
            let alerts = w.check(&snapshot, Instant::now());
            if alerts.iter().all(|a| a.key != "fs-filling:/data") {
                cleared = true;
                break;
            }
        }
        assert!(cleared);
    }

    fn volume(missing: u64, behind: u64) -> Volume {
        Volume { uuid: "b7c1".into(), label: "data".into(), disks: 2, missing, behind }
    }

    #[test]
    fn a_missing_disk_is_critical_and_a_disk_behind_a_warning_until_they_are_not() {
        let alerts = volume_alerts(&[volume(1, 0)]);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].key, "btrfs-missing:data");
        assert_eq!(alerts[0].severity, Severity::Critical);
        assert_eq!(
            alerts[0].title,
            "Btrfs volume data is missing 1 of its 2 disks; its data has no second copy until it is replaced"
        );
        assert!(
            alerts[0].detail.contains("`replace start`") && alerts[0].detail.ends_with("(MET-103)")
        );
        let alerts = volume_alerts(&[volume(0, 1)]);
        assert_eq!(alerts[0].key, "btrfs-behind:data");
        assert_eq!(alerts[0].severity, Severity::Warning);
        assert!(alerts[0].detail.ends_with("(MET-104)"));
        assert_eq!(volume_alerts(&[volume(1, 1)]).len(), 2);
        assert!(volume_alerts(&[volume(0, 0)]).is_empty());
        // Without a label, the volume is its UUID.
        let unlabelled = Volume { label: String::new(), ..volume(1, 0) };
        assert_eq!(volume_alerts(&[unlabelled])[0].key, "btrfs-missing:b7c1");
    }

    fn memory_step(w: &mut Warnings, available_kib: u64) -> Vec<Alert> {
        let memory = crate::proc::Memory {
            total: 1000,
            free: available_kib,
            cache: 0,
            ..Default::default()
        };
        w.check(&Snapshot { memory, ..Snapshot::default() }, Instant::now())
    }

    #[test]
    fn memory_is_low_after_five_steps_below_5_percent_and_until_10() {
        let mut w = warnings();
        for _ in 0..4 {
            assert!(memory_step(&mut w, 40).is_empty());
        }
        // One step above breaks the run.
        assert!(memory_step(&mut w, 60).is_empty());
        for _ in 0..4 {
            assert!(memory_step(&mut w, 40).is_empty());
        }
        let alerts = memory_step(&mut w, 40);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].key, "memory:low");
        assert_eq!(alerts[0].severity, Severity::Warning);
        assert_eq!(alerts[0].title, "Memory is nearly used up: 41 KB of 1.1 MB is available");
        assert!(alerts[0].detail.ends_with("(MET-105)"));
        // Held between 5 and 10 percent, cleared from 10.
        assert_eq!(memory_step(&mut w, 70).len(), 1);
        assert_eq!(memory_step(&mut w, 99).len(), 1);
        assert!(memory_step(&mut w, 100).is_empty());
        // And a machine whose memory was not read has no alert.
        assert!(w.check(&Snapshot::default(), Instant::now()).is_empty());
    }

    fn sensor(celsius: f64, max: Option<f64>, crit: Option<f64>) -> Temperature {
        Temperature {
            chip: "coretemp-isa-0000".into(),
            sensor: "temp1".into(),
            label: "Package id 0".into(),
            celsius,
            max,
            crit,
        }
    }

    fn temperature_step(w: &mut Warnings, sensors: Vec<Temperature>) -> Vec<Alert> {
        w.check(&Snapshot { temperatures: sensors, ..Snapshot::default() }, Instant::now())
    }

    #[test]
    fn a_sensor_near_its_critical_temperature_is_raised_after_five_steps() {
        let mut w = warnings();
        // The line is 5 degrees below the critical 100.
        for _ in 0..4 {
            assert!(temperature_step(&mut w, vec![sensor(95.0, None, Some(100.0))]).is_empty());
        }
        let alerts = temperature_step(&mut w, vec![sensor(95.0, None, Some(100.0))]);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].key, "temp:coretemp-isa-0000/package-id-0");
        assert_eq!(alerts[0].severity, Severity::Warning);
        assert_eq!(
            alerts[0].title,
            "coretemp-isa-0000/Package id 0 is at 95 °C; its limit is 100 °C"
        );
        assert!(alerts[0].detail.contains("`sensors`") && alerts[0].detail.ends_with("(MET-106)"));
        // Critical at the limit, at once, held until 3 degrees below it.
        assert_eq!(
            temperature_step(&mut w, vec![sensor(100.0, None, Some(100.0))])[0].severity,
            Severity::Critical
        );
        assert_eq!(
            temperature_step(&mut w, vec![sensor(97.0, None, Some(100.0))])[0].severity,
            Severity::Critical
        );
        assert_eq!(
            temperature_step(&mut w, vec![sensor(96.0, None, Some(100.0))])[0].severity,
            Severity::Warning
        );
        // Held down to 3 degrees under the line, 92, and clear below.
        assert_eq!(temperature_step(&mut w, vec![sensor(92.0, None, Some(100.0))]).len(), 1);
        assert!(temperature_step(&mut w, vec![sensor(91.9, None, Some(100.0))]).is_empty());
    }

    #[test]
    fn a_sensor_that_does_not_stay_over_its_line_is_not_raised() {
        let mut w = warnings();
        for _ in 0..3 {
            temperature_step(&mut w, vec![sensor(96.0, None, Some(100.0))]);
        }
        temperature_step(&mut w, vec![sensor(80.0, None, Some(100.0))]);
        for _ in 0..4 {
            assert!(temperature_step(&mut w, vec![sensor(96.0, None, Some(100.0))]).is_empty());
        }
    }

    #[test]
    fn a_sensor_without_a_critical_temperature_uses_its_maximum_and_one_without_either_is_not_watched()
     {
        let mut w = warnings();
        let hot = sensor(81.0, Some(80.0), None);
        for _ in 0..4 {
            temperature_step(&mut w, vec![hot.clone()]);
        }
        let alerts = temperature_step(&mut w, vec![hot.clone()]);
        assert_eq!(alerts[0].severity, Severity::Warning);
        assert!(alerts[0].title.ends_with("its limit is 80 °C"), "{}", alerts[0].title);
        // With a margin of 10 a critical of 100 is a line of 90.
        let mut w = Warnings::new(
            Thresholds { temperature_margin: 10, ..Thresholds::default() },
            Path::new("/x"),
        );
        for _ in 0..5 {
            assert!(temperature_step(&mut w, vec![sensor(89.0, None, Some(100.0))]).is_empty());
        }
        for _ in 0..4 {
            temperature_step(&mut w, vec![sensor(90.0, None, Some(100.0))]);
        }
        assert_eq!(temperature_step(&mut w, vec![sensor(90.0, None, Some(100.0))]).len(), 1);
        // Neither limit, or a limit of 0 that some sensors give.
        let mut w = warnings();
        for _ in 0..6 {
            assert!(
                temperature_step(
                    &mut w,
                    vec![sensor(200.0, None, None), sensor(200.0, Some(0.0), Some(0.0))]
                )
                .is_empty()
            );
        }
    }

    fn testdata(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../tls-test/testdata").join(name)
    }

    /// A folder of certificates of the test's own, holding the files of
    /// tls-test's fixtures called `files`, as `(from, to)`.
    fn ssl(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("metricsd-warnings-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for (from, to) in files {
            fs::copy(testdata(from), dir.join(to)).unwrap();
        }
        dir
    }

    fn ends(name: &str) -> i64 {
        let der = CertificateDer::pem_file_iter(testdata(name)).unwrap().next().unwrap().unwrap();
        i64::try_from(crate::logforward::validity(&der).unwrap().1).unwrap()
    }

    #[test]
    fn a_certificate_that_has_ended_is_critical_and_says_when() {
        let dir = ssl("expired", &[("expired.pem", "www.pem")]);
        let alerts = certificates(&dir, 1_800_000_000, 14);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].key, "certificate:www.pem");
        assert_eq!(alerts[0].severity, Severity::Critical);
        assert_eq!(
            alerts[0].title,
            format!("The certificate in {}/www.pem ended on 2021-01-01", dir.display())
        );
        assert!(alerts[0].detail.ends_with("(MET-107)"));
    }

    #[test]
    fn a_certificate_is_a_warning_within_the_setting_and_critical_within_three_days() {
        let dir = ssl("lines", &[("rsa-2048.pem", "a.crt")]);
        let end = ends("rsa-2048.pem");
        let day = 86_400;
        assert!(certificates(&dir, end - 15 * day, 14).is_empty());
        let alerts = certificates(&dir, end - 14 * day, 14);
        assert_eq!(alerts[0].severity, Severity::Warning);
        assert!(alerts[0].title.contains(" ends on "), "{}", alerts[0].title);
        assert_eq!(certificates(&dir, end - 4 * day, 14)[0].severity, Severity::Warning);
        assert_eq!(certificates(&dir, end - 3 * day, 14)[0].severity, Severity::Critical);
        assert_eq!(certificates(&dir, end + day, 14)[0].severity, Severity::Critical);
        // A setting of 30 days warns earlier.
        assert_eq!(certificates(&dir, end - 30 * day, 30).len(), 1);
    }

    #[test]
    fn only_the_first_certificate_of_a_pem_file_counts_and_keys_and_other_files_are_skipped() {
        let dir = ssl(
            "skips",
            &[
                ("expired.pem", "chain.pem"),
                ("expired.key", "key.pem"),
                ("client.key", "client.crt"),
                ("message.txt", "notes.txt"),
            ],
        );
        // A chain: the expired leaf, then the authority's.
        let mut chain = fs::read(dir.join("chain.pem")).unwrap();
        chain.extend(fs::read(testdata("ca.pem")).unwrap());
        fs::write(dir.join("chain.pem"), chain).unwrap();
        let alerts = certificates(&dir, 1_800_000_000, 14);
        assert_eq!(alerts.len(), 1, "{alerts:?}");
        assert_eq!(alerts[0].key, "certificate:chain.pem");
        assert!(certificates(&dir.join("not-there"), 1_800_000_000, 14).is_empty());
    }

    #[test]
    fn a_renewed_certificate_clears_its_alert_and_the_certificates_are_read_once_an_hour() {
        let dir = ssl("hourly", &[("expired.pem", "www.pem")]);
        let mut w = Warnings::new(Thresholds::default(), &dir);
        let start = Instant::now();
        let snapshot = snapshot(Vec::new());
        assert_eq!(w.check(&snapshot, start).len(), 1);
        // Renewed (a file that ends far away) and removed: the old result
        // stands until the hour is out.
        fs::copy(testdata("rsa-2048.pem"), dir.join("www.pem")).unwrap();
        let far_future =
            Snapshot { at: ends("rsa-2048.pem") - 365 * 86_400, ..Snapshot::default() };
        assert_eq!(w.check(&far_future, start + Duration::from_secs(3599)).len(), 1);
        assert!(w.check(&far_future, start + Duration::from_secs(3600)).is_empty());
        // The file gone clears it as well, at the next reading.
        fs::copy(testdata("expired.pem"), dir.join("www.pem")).unwrap();
        assert_eq!(w.check(&snapshot, start + Duration::from_secs(7200)).len(), 1);
        fs::remove_file(dir.join("www.pem")).unwrap();
        assert!(w.check(&snapshot, start + Duration::from_secs(10_800)).is_empty());
        // New thresholds read them again at once.
        fs::copy(testdata("expired.pem"), dir.join("www.pem")).unwrap();
        w.set_thresholds(Thresholds { certificate_days: 30, ..Thresholds::default() });
        assert_eq!(w.check(&snapshot, start + Duration::from_secs(10_801)).len(), 1);
    }

    fn some_alert(title: &str) -> Alert {
        alert(key("fs-full", "/data"), Level::Warning, title, "do it", FULL).unwrap()
    }

    #[test]
    fn the_set_is_sent_when_it_changed_and_every_ten_minutes() {
        let mut w = warnings();
        let start = Instant::now();
        let (first, second) = (vec![some_alert("one")], vec![some_alert("two")]);
        let mut calls = Vec::new();
        let mut send = |w: &mut Warnings, alerts: &[Alert], seconds: u64| {
            w.report(alerts, start + Duration::from_secs(seconds), |set| {
                calls.push(set.len());
                Ok(())
            })
        };
        // At the first step, even an empty set is sent, so that what
        // alertd had from before a restart is cleared.
        assert_eq!(send(&mut w, &[], 0), Sent::Yes);
        assert_eq!(send(&mut w, &[], 60), Sent::Not);
        assert_eq!(send(&mut w, &first, 120), Sent::Yes);
        assert_eq!(send(&mut w, &first, 180), Sent::Not);
        assert_eq!(send(&mut w, &second, 240), Sent::Yes);
        assert_eq!(send(&mut w, &second, 839), Sent::Not);
        assert_eq!(send(&mut w, &second, 840), Sent::Yes);
        assert_eq!(calls, [0, 1, 1, 1]);
    }

    #[test]
    fn a_send_that_failed_is_tried_again_after_ten_seconds() {
        let mut w = warnings();
        let start = Instant::now();
        let alerts = vec![some_alert("one")];
        let later = |seconds| start + Duration::from_secs(seconds);
        let refuse = |_: &[Alert]| Err("alertd is not running".to_string());
        assert_eq!(
            w.report(&alerts, later(0), refuse),
            Sent::Failed("alertd is not running".into())
        );
        assert_eq!(w.report(&alerts, later(9), |_| Ok(())), Sent::Not);
        assert_eq!(w.report(&alerts, later(10), |_| Ok(())), Sent::Yes);
        // It is then as after any send.
        assert_eq!(w.report(&alerts, later(11), |_| panic!("not due")), Sent::Not);
    }

    #[test]
    fn what_alertd_would_refuse_is_never_sent() {
        let mut w = warnings();
        let mount = format!("/volumes/{}\nMy Disk — \"é\" {}", "X".repeat(20), "y".repeat(300));
        let mut disk = filesystem(&mount, 99);
        disk.mount = mount.clone();
        let sensors =
            vec![Temperature { label: "Tctl\nCPU 0".into(), ..sensor(99.0, None, Some(100.0)) }];
        let volumes = vec![Volume { label: "Ünï\ncode".into(), ..volume(1, 1) }];
        let mut found = Vec::new();
        for _ in 0..6 {
            let snapshot = Snapshot {
                filesystems: vec![disk.clone()],
                temperatures: sensors.clone(),
                volumes: volumes.clone(),
                ..Snapshot::default()
            };
            found = w.check(&snapshot, Instant::now());
        }
        assert!(found.len() >= 4, "{found:?}");
        for alert in &found {
            assert_eq!(alert.invalid(), None, "{alert:?}");
        }
        // Two names that are one key once cut down are one alert.
        let (a, b) = (filesystem("/Data", 99), filesystem("/data", 91));
        let alerts = w.check(&snapshot(vec![a, b]), Instant::now());
        assert_eq!(alerts.iter().filter(|alert| alert.key == "fs-full:/data").count(), 1);
        assert_eq!(alerts[0].severity, Severity::Critical);
    }

    #[test]
    fn the_history_gives_the_line_its_first_points_after_a_restart() {
        let dir = std::env::temp_dir().join(format!("metricsd-backfill-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let now = i64::try_from(
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
        )
        .unwrap();
        let mut history = store::Store::open(&dir, now - 3600).unwrap();
        for i in 0..60 {
            let used = (60.0e9 + 1e9 * f64::from(i) / 60.0) as f32;
            history.push(now - 3600 + i64::from(i) * 60, &[("fs.used{/data}".to_string(), used)]);
        }
        history.flush().unwrap();
        let mut w = warnings();
        w.backfill(&dir, now);
        let window = &w.filesystems["/data"].window;
        assert_eq!(window.points.len(), 60);
        // About 1 GB an hour into 100 GB: 40 GB left is 40 hours.
        let days = window.days_to_full(100e9).unwrap();
        assert!((days - 40.0 / 24.0).abs() < 0.1, "{days}");
    }

    #[test]
    fn levels_are_held_between_the_lines() {
        use Level::{Clear, Critical, Warning};
        assert_eq!(level_after(Clear, 89.9, 90.0, Some(97.0)), Clear);
        assert_eq!(level_after(Clear, 90.0, 90.0, Some(97.0)), Warning);
        assert_eq!(level_after(Warning, 87.0, 90.0, Some(97.0)), Warning);
        assert_eq!(level_after(Warning, 86.9, 90.0, Some(97.0)), Clear);
        assert_eq!(level_after(Warning, 97.0, 90.0, Some(97.0)), Critical);
        assert_eq!(level_after(Critical, 94.0, 90.0, Some(97.0)), Critical);
        assert_eq!(level_after(Critical, 93.9, 90.0, Some(97.0)), Warning);
        assert_eq!(level_after(Critical, 86.0, 90.0, Some(97.0)), Clear);
        // Without a critical line, only the warning's.
        assert_eq!(level_after(Clear, 500.0, 90.0, None), Warning);
    }

    #[test]
    fn about_says_the_time_roughly() {
        assert_eq!(about(2.2), "about 2 days");
        assert_eq!(about(1.6), "about 2 days");
        assert_eq!(about(1.2), "about a day");
        assert_eq!(about(0.2), "about 5 hours");
        assert_eq!(about(0.001), "about an hour");
        assert_eq!(about(0.0), "about an hour");
    }
}
