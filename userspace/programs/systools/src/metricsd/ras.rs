// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The hardware errors in metricsd(8): the history's series from the
//! kernel's record (`/kernel/ras`, which `hardware errors` shows) and the
//! alerts about it.
//!
//! The series are `ras.corrected` and `ras.uncorrected`, the errors the
//! record counted in the step, and `ras.dimm{NAME}` for each memory
//! device, its corrected errors in the step. The kernel's counts only
//! grow, so each is the difference between two readings; the first step,
//! and one across a restart of the machine, has none.
//!
//! The alerts, which [`Watch::alerts`] works out after each step, are:
//!
//! - a critical alert for each error that was not corrected (recoverable
//!   or fatal) and each error of the firmware's record from before this
//!   start (BERT), once, with the key `ras:<number>`;
//! - a warning for a memory device that has as many corrected errors in a
//!   day as `dimm_corrected_per_day` says, from the record's times (or
//!   its counts, scaled, when the record dropped older lines), cleared
//!   when it falls under three quarters of that.
//!
//! The number of the last error that was alerted is kept in `ras-seen`
//! in metricsd's state folder, with the start it belongs to, so that a
//! restart of metricsd does not alert again for what it had. A new start
//! numbers the errors from 1 again, which the start time tells.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::sample::{Snapshot, named};
use crate::alerts::protocol::{About, Alert, Severity, key_part};
use crate::hardware::errors::{Busy, busy_dimms};
use crate::hardware::ras::{Error, Record};

/// The file in the state folder that holds the last error alerted.
const SEEN: &str = "ras-seen";

/// Two start times this far apart, in seconds, are the same start: the
/// time is worked out from the clock, which may be set.
const SAME_START: i64 = 120;

/// How many error alerts are kept in the set, the newest.
const KEPT: usize = 20;

const ERROR: &str = "MET-109";
const DIMM: &str = "MET-110";

/// The series the record gives, added to `row`: the step's corrected and
/// uncorrected errors and each memory device's corrected. A kernel
/// with no record gives none.
pub fn levels(before: Option<&Snapshot>, now: &Snapshot, row: &mut Vec<(String, f64)>) {
    if !now.ras.counted {
        return;
    }
    // A reading before this one is one of the same record, and of the
    // same start, or it has nothing to take away.
    let before = before.map(|b| &b.ras).filter(|b| b.counted);
    let grew = |now: u64, before: Option<u64>| {
        before.and_then(|before| now.checked_sub(before)).map_or(f64::NAN, |d| d as f64)
    };
    let counts = &now.ras.counts;
    row.push(("ras.corrected".into(), grew(counts.corrected, before.map(|b| b.counts.corrected))));
    row.push((
        "ras.uncorrected".into(),
        grew(counts.uncorrected, before.map(|b| b.counts.uncorrected)),
    ));
    for dimm in &counts.dimms {
        let earlier = before
            .and_then(|b| b.counts.dimms.iter().find(|d| d.handle == dimm.handle))
            .map(|d| d.corrected);
        row.push((named("ras.dimm", &dimm.name()), grew(dimm.corrected, earlier)));
    }
}

/// The last error alerted, and the start it was in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Seen {
    /// The start time, in seconds since 1970.
    start: i64,
    seq: u64,
}

impl Seen {
    fn parse(text: &str) -> Option<Seen> {
        let mut words = text.split_whitespace();
        let start = words.next()?.parse().ok()?;
        let seq = words.next()?.parse().ok()?;
        Some(Seen { start, seq })
    }
}

/// What metricsd keeps of the hardware errors from one step to the next.
#[derive(Debug, Default)]
pub struct Watch {
    /// Where the last error alerted is kept; none keeps it nowhere.
    file: Option<PathBuf>,
    seen: Option<Seen>,
    loaded: bool,
    /// The alerts of the errors this service saw, the oldest first.
    errors: Vec<Alert>,
    /// The memory devices that have a warning now.
    warned: HashSet<String>,
}

impl Watch {
    /// Keeps the last error alerted in the state folder `dir`.
    pub fn keep_in(&mut self, dir: &Path) {
        self.file = Some(dir.join(SEEN));
        self.loaded = false;
    }

    fn load(&mut self) {
        if !std::mem::replace(&mut self.loaded, true) {
            self.seen = self
                .file
                .as_ref()
                .and_then(|file| fs::read_to_string(file).ok())
                .and_then(|text| Seen::parse(&text));
        }
    }

    /// Keeps `seen`. A folder that does not take it costs only a repeat
    /// of the alerts after a restart.
    fn keep(&self, seen: Seen) {
        let Some(file) = &self.file else { return };
        if let Some(dir) = file.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let temporary = file.with_extension("new");
        if fs::write(&temporary, format!("{} {}\n", seen.start, seen.seq)).is_ok() {
            let _ = fs::rename(temporary, file);
        }
    }

    /// The alerts the reading shows, with `line` corrected errors of a
    /// memory device in a day a warning.
    pub fn alerts(&mut self, snapshot: &Snapshot, line: u32) -> Vec<Alert> {
        let record = &snapshot.ras;
        if !record.counted && record.errors.is_empty() {
            return self.errors.clone();
        }
        self.load();
        let start = i64::try_from(snapshot.boot_time_ns / 1_000_000_000).unwrap_or(0);
        let highest = record.errors.iter().map(|error| error.seq).max().unwrap_or(0);
        // Where the errors were numbered from: the last alerted of this
        // start, or none; and a number above all there are is another
        // start's.
        let last = match self.seen {
            Some(seen) if (seen.start - start).abs() <= SAME_START && seen.seq <= highest => {
                seen.seq
            }
            _ => 0,
        };
        for error in record.errors.iter().filter(|error| error.seq > last) {
            if error.severity.serious() || error.before_this_boot() {
                self.errors.push(error_alert(error));
            }
        }
        let excess = self.errors.len().saturating_sub(KEPT);
        self.errors.drain(..excess);
        if highest > last {
            let seen = Seen { start, seq: highest };
            self.seen = Some(seen);
            self.keep(seen);
        }
        let uptime = snapshot.monotonic_ns as f64 / 1e9;
        let mut alerts = self.errors.clone();
        alerts.extend(self.dimm_alerts(record, uptime, line));
        alerts
    }

    /// The warnings of the memory devices.
    fn dimm_alerts(&mut self, record: &Record, uptime: f64, line: u32) -> Vec<Alert> {
        let line = u64::from(line);
        // A device that has a warning keeps it down to three quarters of
        // the line, so that a count at the line does not raise and clear
        // it as the day slides.
        let low = (line * 3 / 4).max(1);
        let busy: Vec<Busy> = busy_dimms(record, uptime, low)
            .into_iter()
            .filter(|busy| busy.errors >= line || self.warned.contains(&busy.name))
            .collect();
        self.warned = busy.iter().map(|busy| busy.name.clone()).collect();
        busy.iter().map(dimm_alert).collect()
    }
}

/// The critical alert of an error that was not corrected, or came before
/// this start.
fn error_alert(error: &Error) -> Alert {
    let part = if error.part.is_empty() { error.source.as_str() } else { error.part.as_str() };
    let title = if error.before_this_boot() {
        format!(
            "The firmware recorded a {} error from before this start in {part}: {}",
            error.severity.name(),
            error.what
        )
    } else {
        format!("A {} hardware error in {part}: {}", error.severity.name(), error.what)
    };
    let detail = format!(
        "`hardware errors` lists it with the others the kernel heard of. Back up what matters; \
         replace the part if it reports again. ({ERROR})"
    );
    Alert::new(format!("ras:{}", error.seq), Severity::Critical, &title, &detail)
        .about(About::Hardware)
}

/// The warning of a memory device with many corrected errors in a day.
fn dimm_alert(busy: &Busy) -> Alert {
    let how = if busy.estimated {
        format!("about {} corrected errors a day", busy.errors)
    } else {
        format!("{} corrected errors in the last 24 hours", busy.errors)
    };
    let title = format!("Memory device {} had {how}", busy.name);
    let detail = format!(
        "Memory that needs correcting is often starting to fail. Run Test the memory from the \
         start menu, and replace the module if the errors keep coming; `hardware errors` shows \
         them. ({DIMM})"
    );
    Alert::new(format!("ras-dimm:{}", key_part(&busy.name)), Severity::Warning, &title, &detail)
        .about(About::Hardware)
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    const COUNTS: &str = "corrected 5\nuncorrected 0\nrecorded 2\ndropped 0\ndimm 0x1100 DIMM_A1 \
                          16384M corrected 5 uncorrected 0\ndimm 0x1101 DIMM_B1 16384M corrected \
                          0 uncorrected 0\n";

    fn snapshot(errors: &str, counts: Option<&str>, start: u64, uptime: u64) -> Snapshot {
        Snapshot {
            ras: Record::parse(Some(errors), counts),
            boot_time_ns: start * 1_000_000_000,
            monotonic_ns: uptime * 1_000_000_000,
            ..Snapshot::default()
        }
    }

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("metricsd-ras-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn keys(alerts: &[Alert]) -> Vec<&str> {
        alerts.iter().map(|alert| alert.key.as_str()).collect()
    }

    const FATAL: &str = "1 12.345 bert fatal pcie x1 - PCIe 0000:3b:00.0: PCIe error: bad\n";
    const RECOVERABLE: &str = "2 40.000 aer recoverable pcie x1 - 0000:00:1c.0: bad TLP\n";
    const CORRECTED: &str = "3 41.000 mce corrected memory x1 - CPU 3 bank 7: fixed\n";

    #[test]
    fn the_series_are_the_growth_of_the_counts_in_the_step() {
        let before = snapshot("", Some(COUNTS), 1000, 60);
        let more = COUNTS
            .replace("corrected 5\nuncorrected 0", "corrected 9\nuncorrected 2")
            .replace("corrected 5 uncorrected 0", "corrected 9 uncorrected 0");
        let now = snapshot("", Some(&more), 1000, 120);
        let mut row = Vec::new();
        levels(Some(&before), &now, &mut row);
        let names: Vec<&str> = row.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            ["ras.corrected", "ras.uncorrected", "ras.dimm{DIMM_A1}", "ras.dimm{DIMM_B1}"]
        );
        let values: Vec<f64> = row.iter().map(|(_, value)| *value).collect();
        assert_eq!(values, [4.0, 2.0, 4.0, 0.0]);
    }

    #[test]
    fn the_first_step_and_a_counter_that_went_back_have_no_value() {
        let now = snapshot("", Some(COUNTS), 1000, 60);
        let mut row = Vec::new();
        levels(None, &now, &mut row);
        assert_eq!(row.len(), 4);
        assert!(row.iter().all(|(_, value)| value.is_nan()));
        // A machine that started again has counts from 0.
        let after_restart = snapshot("", Some("corrected 1\nuncorrected 0\n"), 2000, 5);
        let mut row = Vec::new();
        levels(Some(&now), &after_restart, &mut row);
        assert!(row[0].1.is_nan(), "{row:?}");
    }

    #[test]
    fn a_device_that_appears_has_no_value_for_its_first_step() {
        let before = snapshot("", Some("corrected 0\nuncorrected 0\n"), 1000, 60);
        let now = snapshot("", Some(COUNTS), 1000, 120);
        let mut row = Vec::new();
        levels(Some(&before), &now, &mut row);
        assert_eq!(row[0].1, 5.0);
        assert!(row[2].1.is_nan());
    }

    #[test]
    fn a_kernel_without_the_record_has_no_series_and_no_alerts() {
        let none = Snapshot::default();
        let mut row = Vec::new();
        levels(None, &none, &mut row);
        assert!(row.is_empty());
        assert!(Watch::default().alerts(&none, 24).is_empty());
    }

    #[test]
    fn a_dimm_without_a_locator_is_named_by_its_handle() {
        let text = "corrected 1\nuncorrected 0\ndimm 0x1102 corrected 1 uncorrected 0\n";
        let now = snapshot("", Some(text), 1, 1);
        let mut row = Vec::new();
        levels(None, &now, &mut row);
        assert_eq!(row[2].0, "ras.dimm{0x1102}");
    }

    #[test]
    fn an_uncorrected_error_and_a_bert_error_are_critical_alerts_about_hardware_once() {
        let dir = folder("once");
        let mut watch = Watch::default();
        watch.keep_in(&dir);
        let text = format!("{FATAL}{RECOVERABLE}{CORRECTED}");
        let alerts = watch.alerts(&snapshot(&text, Some(COUNTS), 1000, 100), 24);
        assert_eq!(keys(&alerts), ["ras:1", "ras:2"]);
        for alert in &alerts {
            assert_eq!(alert.severity, Severity::Critical);
            assert_eq!(alert.kind(), Some(About::Hardware));
            assert!(alert.invalid().is_none(), "{:?}", alert.invalid());
            assert!(alert.detail.ends_with("(MET-109)"));
        }
        assert_eq!(
            alerts[0].title,
            "The firmware recorded a fatal error from before this start in PCIe \
             0000:3b:00.0: PCIe error: bad"
        );
        assert_eq!(alerts[1].title, "A recoverable hardware error in 0000:00:1c.0: bad TLP");
        // The next step has the same alerts, and no new one.
        let again = watch.alerts(&snapshot(&text, Some(COUNTS), 1000, 160), 24);
        assert_eq!(again, alerts);
        // A new error is one more.
        let more = format!("{text}4 170.000 mce fatal processor x1 - CPU 1 bank 2: dead\n");
        let third = watch.alerts(&snapshot(&more, Some(COUNTS), 1000, 200), 24);
        assert_eq!(keys(&third), ["ras:1", "ras:2", "ras:4"]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_corrected_error_is_no_alert_by_itself() {
        let mut watch = Watch::default();
        let alerts = watch.alerts(&snapshot(CORRECTED, Some(COUNTS), 1000, 100), 24);
        assert!(alerts.is_empty());
    }

    #[test]
    fn a_restart_does_not_alert_again_for_what_it_had_but_for_what_is_new() {
        let dir = folder("restart");
        let text = format!("{FATAL}{RECOVERABLE}");
        let mut first = Watch::default();
        first.keep_in(&dir);
        assert_eq!(first.alerts(&snapshot(&text, Some(COUNTS), 1000, 100), 24).len(), 2);
        assert_eq!(fs::read_to_string(dir.join(SEEN)).unwrap(), "1000 2\n");
        // metricsd starts again in the same start of the machine; the
        // start time is read a few seconds differently.
        let mut second = Watch::default();
        second.keep_in(&dir);
        let more = format!("{text}5 300.000 mce fatal processor x1 - CPU 1 bank 2: dead\n");
        let alerts = second.alerts(&snapshot(&more, Some(COUNTS), 1003, 400), 24);
        assert_eq!(keys(&alerts), ["ras:5"]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_new_start_of_the_machine_numbers_the_errors_from_1_again() {
        let dir = folder("boot");
        let text = format!("{FATAL}{RECOVERABLE}");
        let mut first = Watch::default();
        first.keep_in(&dir);
        first.alerts(&snapshot(&text, Some(COUNTS), 1000, 100), 24);
        // The machine started an hour later and has one error, number 1.
        let mut second = Watch::default();
        second.keep_in(&dir);
        let alerts = second.alerts(
            &snapshot(RECOVERABLE.replace("2 ", "1 ").as_str(), Some(COUNTS), 4600, 30),
            24,
        );
        assert_eq!(keys(&alerts), ["ras:1"]);
        assert_eq!(fs::read_to_string(dir.join(SEEN)).unwrap(), "4600 1\n");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_state_file_that_does_not_read_or_write_is_no_trouble() {
        let dir = folder("broken");
        fs::write(dir.join(SEEN), "garbage").unwrap();
        let mut watch = Watch::default();
        watch.keep_in(&dir);
        assert_eq!(watch.alerts(&snapshot(FATAL, Some(COUNTS), 1000, 100), 24).len(), 1);
        // A folder that cannot be made.
        let mut nowhere = Watch::default();
        nowhere.keep_in(&dir.join(SEEN).join("below"));
        assert_eq!(nowhere.alerts(&snapshot(FATAL, Some(COUNTS), 1000, 100), 24).len(), 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn only_the_newest_twenty_error_alerts_are_kept() {
        let mut watch = Watch::default();
        let text = (1..=30).fold(String::new(), |mut text, n| {
            let _ = writeln!(text, "{n} {n}.000 aer recoverable pcie x1 - dev{n}: bad");
            text
        });
        let alerts = watch.alerts(&snapshot(&text, Some(COUNTS), 1000, 100), 24);
        assert_eq!(alerts.len(), 20);
        assert_eq!(alerts[0].key, "ras:11");
        assert_eq!(alerts[19].key, "ras:30");
    }

    fn corrected_lines(handle: &str, times: u64, start: f64) -> String {
        (1..=times).fold(String::new(), |mut text, n| {
            let _ = writeln!(
                text,
                "{n} {:.3} ghes1 corrected memory x1 dimm={handle} DIMM A: memory error",
                start + n as f64
            );
            text
        })
    }

    #[test]
    fn a_device_over_the_daily_line_is_a_warning_until_it_is_under_three_quarters() {
        let mut watch = Watch::default();
        let counts = COUNTS.replace("recorded 2", "recorded 24");
        let at = |watch: &mut Watch, times: u64| {
            watch.alerts(
                &snapshot(&corrected_lines("0x1100", times, 100.0), Some(&counts), 1000, 500),
                24,
            )
        };
        assert!(at(&mut watch, 23).is_empty());
        let alerts = at(&mut watch, 24);
        assert_eq!(keys(&alerts), ["ras-dimm:dimm_a1"]);
        assert_eq!(alerts[0].severity, Severity::Warning);
        assert_eq!(alerts[0].kind(), Some(About::Hardware));
        assert_eq!(
            alerts[0].title,
            "Memory device DIMM_A1 had 24 corrected errors in the last 24 hours"
        );
        assert!(alerts[0].detail.ends_with("(MET-110)"));
        assert!(alerts[0].invalid().is_none());
        // 20 is under the line but not under three quarters of it (18).
        assert_eq!(keys(&at(&mut watch, 20)), ["ras-dimm:dimm_a1"]);
        assert!(at(&mut watch, 17).is_empty());
        // And it needs the whole line to be raised again.
        assert!(at(&mut watch, 20).is_empty());
    }

    #[test]
    fn a_setting_moves_the_line_and_an_estimate_says_it_is_one() {
        let mut watch = Watch::default();
        let counts = "corrected 480\nuncorrected 0\nrecorded 480\ndropped 224\ndimm 0x1100 \
                      DIMM_A1 16384M corrected 480 uncorrected 0\n";
        let two_days = 2 * 86_400;
        let alerts = watch.alerts(&snapshot("", Some(counts), 1000, two_days), 24);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].title, "Memory device DIMM_A1 had about 240 corrected errors a day");
        let mut lax = Watch::default();
        assert!(lax.alerts(&snapshot("", Some(counts), 1000, two_days), 500).is_empty());
    }

    #[test]
    fn the_key_of_a_device_is_one_alertd_takes() {
        let busy =
            Busy { name: "Bank 0 DIMM 3/A+".into(), handle: Some(1), errors: 30, estimated: false };
        let alert = dimm_alert(&busy);
        assert_eq!(alert.key, "ras-dimm:bank-0-dimm-3/a-");
        assert!(alert.invalid().is_none());
    }
}
