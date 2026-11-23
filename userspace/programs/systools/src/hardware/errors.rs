// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `hardware errors`: the hardware errors the kernel has heard of since
//! the start, from the processors' machine-check banks and from the
//! firmware, as `/kernel/ras` records them ([`super::ras`] reads it); and
//! the errors area of `hardware check`, which judges that record.
//!
//! The area fails when this start has an error that was not corrected
//! (recoverable or fatal) or the firmware recorded an error from before
//! it (the BERT), and warns when a memory device had [`DAILY_LINE`]
//! corrected errors or more in the last 24 hours. The times are the
//! record's own, so a record that has dropped its oldest lines cannot
//! say; the counts since the start, scaled to a day, stand in for them,
//! and the check says so.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

use serde::Serialize;

use super::check::{Area, Check, Problem, counted};
use super::devices::{self, Failed, Sources};
use super::ras::{Counts, Dimm, Error, Record, Severity};
use crate::args::{Arg, Args};
use crate::util::{self, Fail, fail};

/// How many corrected errors of one memory device in a day make a
/// warning, unless metrics.toml(5) says another number.
pub(crate) const DAILY_LINE: u64 = 24;

/// A day, in seconds.
const DAY: f64 = 86_400.0;

/// The name of the area's check.
const NAME: &str = "hardware errors";

/// The options of `hardware errors`.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Options {
    pub(crate) json: bool,
    /// Only the errors of the last this many seconds.
    pub(crate) since: Option<u64>,
}

/// The options after `errors`.
pub(crate) fn parse(args: &mut Args) -> Result<Options, Fail> {
    let mut options = Options::default();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Long(name) if name == "json" => options.json = true,
            Arg::Long(name) if name == "since" => options.since = Some(args.number()?),
            other => return Err(args.unknown(&other)),
        }
    }
    Ok(options)
}

/// What `hardware errors --json` says. With no record the lists are
/// empty and the counts are 0.
#[derive(Debug, PartialEq, Serialize)]
pub(crate) struct Report {
    /// The version of this document: 1.
    format: u32,
    errors: Vec<Error>,
    counts: Counts,
    /// How many lines of the record this program did not understand.
    skipped: usize,
}

impl Report {
    /// The report of `record`: with `since`, the errors of the last that
    /// many seconds before `uptime`, the seconds since the start.
    pub(crate) fn of(record: Record, since: Option<u64>, uptime: f64) -> Report {
        let from = since.map(|since| uptime - since as f64);
        let errors = record
            .errors
            .into_iter()
            .filter(|error| from.is_none_or(|from| error.seconds >= from))
            .collect();
        Report { format: 1, errors, counts: record.counts, skipped: record.skipped }
    }

    /// The JSON the command prints, on one line.
    ///
    /// # Errors
    ///
    /// The words of the writer's refusal, which a document of these types
    /// does not give.
    pub(crate) fn json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// The text a terminal shows: a table of the errors, oldest first,
    /// then the memory devices that have errors.
    pub(crate) fn text(&self, since: Option<u64>) -> String {
        let counts = &self.counts;
        let mut text = String::new();
        if self.errors.is_empty() {
            text.push_str(&match since {
                Some(seconds) => format!("No hardware errors in the last {seconds} seconds.\n"),
                None => "No hardware errors since this boot.\n".to_string(),
            });
        } else {
            let rows: Vec<[String; 6]> = self.errors.iter().map(row).collect();
            let head = ["TIME", "SOURCE", "SEVERITY", "KIND", "PART", "WHAT"].map(String::from);
            let mut widths = [0; 5];
            for row in rows.iter().chain([&head]) {
                for (width, cell) in widths.iter_mut().zip(row) {
                    *width = (*width).max(cell.chars().count());
                }
            }
            for row in std::iter::once(&head).chain(&rows) {
                for (at, cell) in row.iter().enumerate() {
                    // The last column is as long as it is.
                    match widths.get(at) {
                        Some(width) => {
                            let _ = write!(text, "{cell:<width$}  ");
                        }
                        None => text.push_str(cell),
                    }
                }
                text.truncate(text.trim_end().len());
                text.push('\n');
            }
        }
        let devices: Vec<&Dimm> = counts.dimms.iter().filter(|dimm| dimm.has_errors()).collect();
        let others = counts.other_dimms;
        if !devices.is_empty() || others.corrected + others.uncorrected > 0 {
            text.push_str("\nMemory devices with errors since this boot:\n");
            for dimm in devices {
                let size = dimm
                    .bytes
                    .map_or(String::new(), |bytes| format!(" ({})", util::human(bytes, false)));
                let _ = writeln!(
                    text,
                    "  {}{size}: {} corrected, {} uncorrected",
                    dimm.name(),
                    dimm.corrected,
                    dimm.uncorrected
                );
            }
            if others.corrected + others.uncorrected > 0 {
                let _ = writeln!(
                    text,
                    "  others the tables do not list: {} corrected, {} uncorrected",
                    others.corrected, others.uncorrected
                );
            }
        }
        if counts.dropped > 0 {
            let _ = writeln!(
                text,
                "\nThe record took in {} errors and holds the last {}; the counts above are of \
                 all of them.",
                counts.recorded,
                counts.recorded - counts.dropped.min(counts.recorded)
            );
        }
        if self.skipped > 0 {
            let _ = writeln!(
                text,
                "{} of the record's lines were not understood and are left out.",
                self.skipped
            );
        }
        text
    }
}

/// One error as the cells of a row.
fn row(error: &Error) -> [String; 6] {
    let times = if error.count > 1 { format!(" x{}", error.count) } else { String::new() };
    [
        format!("{:.3}", error.seconds),
        error.source.clone(),
        error.severity.name().to_string(),
        error.kind.name().to_string(),
        if error.part.is_empty() { "-".to_string() } else { error.part.clone() },
        format!("{}{times}", error.what),
    ]
}

/// `hardware errors`: the record of this machine, which is empty on a
/// kernel that keeps none.
pub(crate) fn command(args: &mut Args) -> Result<i32, Fail> {
    let options = parse(args)?;
    let record = Record::read(Path::new(super::ras::DIR));
    let report = Report::of(record, options.since, uptime());
    let mut out = std::io::stdout().lock();
    if options.json {
        let text = report.json().map_err(|error| fail(format!("cannot write JSON: {error}")))?;
        writeln!(out, "{text}")?;
    } else {
        write!(out, "{}", report.text(options.since))?;
    }
    Ok(0)
}

/// The seconds since the start, on the clock the record's times are on.
pub(crate) fn uptime() -> f64 {
    util::monotonic_ns() as f64 / 1e9
}

/// A memory device that had a day's errors over the line.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Busy {
    /// Its locator, or its handle.
    pub(crate) name: String,
    pub(crate) handle: Option<u32>,
    /// Its corrected errors in the last 24 hours, or an estimate.
    pub(crate) errors: u64,
    /// Whether `errors` is the counts since the start scaled to a day,
    /// because the record had dropped older lines.
    pub(crate) estimated: bool,
}

/// The name of memory device `handle`: from the counts if they list it.
fn dimm_name(counts: &Counts, handle: u32) -> String {
    counts
        .dimms
        .iter()
        .find(|dimm| dimm.handle == handle)
        .map_or_else(|| format!("{handle:#06x}"), Dimm::name)
}

/// The memory devices with `line` corrected errors or more in the 24
/// hours before `uptime`, the worst first.
///
/// The record's lines say when each error came. A record that dropped
/// lines cannot say for the older ones, and the counts since the start
/// take their place: as they are when the start is less than a day ago,
/// and scaled down to a day when it is more.
pub(crate) fn busy_dimms(record: &Record, uptime: f64, line: u64) -> Vec<Busy> {
    let mut found: Vec<Busy> = Vec::new();
    if record.counts.dropped > 0 {
        let scale = if uptime > DAY { DAY / uptime } else { 1.0 };
        for dimm in &record.counts.dimms {
            let errors = (dimm.corrected as f64 * scale).round() as u64;
            found.push(Busy {
                name: dimm.name(),
                handle: Some(dimm.handle),
                errors,
                estimated: uptime > DAY,
            });
        }
    } else {
        let from = uptime - DAY;
        for error in record.errors.iter().filter(|error| {
            error.severity == Severity::Corrected && error.seconds >= from && error.dimm.is_some()
        }) {
            let handle = error.dimm;
            match found.iter_mut().find(|busy| busy.handle == handle) {
                Some(busy) => busy.errors += error.count,
                None => found.push(Busy {
                    name: handle.map_or_else(String::new, |h| dimm_name(&record.counts, h)),
                    handle,
                    errors: error.count,
                    estimated: false,
                }),
            }
        }
    }
    found.retain(|busy| busy.errors >= line && busy.errors > 0);
    found.sort_by(|a, b| b.errors.cmp(&a.errors).then_with(|| a.name.cmp(&b.name)));
    found
}

/// The number of errors the lines of `record` stand for at `severity`.
fn in_lines(record: &Record, severity: Severity) -> u64 {
    record.errors.iter().filter(|e| e.severity == severity).map(|e| e.count).sum()
}

/// `n` of what `words` say, as `1 fatal error` and `2 recoverable errors`.
fn serious_words(fatal: u64, recoverable: u64) -> String {
    let parts: Vec<String> = [(fatal, "fatal"), (recoverable, "recoverable")]
        .into_iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, word)| format!("{n} {word}"))
        .collect();
    let noun = if fatal + recoverable == 1 { "error" } else { "errors" };
    format!("{} {noun}", parts.join(" and "))
}

/// The check of the area for `record`, `uptime` seconds after the start,
/// with `line` corrected errors of one memory device in a day a warning.
pub(crate) fn check_of(record: &Record, uptime: f64, line: u64) -> Check {
    if !record.counted && record.errors.is_empty() {
        return Check::skipped(
            Area::Errors,
            NAME,
            "the kernel keeps no record of hardware errors on this machine".into(),
        );
    }
    // The counts cover the lines that were dropped; the lines are what a
    // kernel gives when it has no counts.
    let total = |severity: Severity, counted: u64| counted.max(in_lines(record, severity));
    let fatal = total(Severity::Fatal, record.counts.by_severity.fatal);
    let recoverable = total(Severity::Recoverable, record.counts.by_severity.recoverable);
    let earlier: u64 =
        record.errors.iter().filter(|error| error.before_this_boot()).map(|e| e.count).sum();
    let busy = busy_dimms(record, uptime, line);
    if fatal + recoverable > 0 || earlier > 0 {
        let mut said = Vec::new();
        if fatal + recoverable > 0 {
            said.push(format!("{} counted since the start", serious_words(fatal, recoverable)));
        }
        if earlier > 0 {
            said.push(format!(
                "the firmware recorded {} from before this start",
                counted(earlier, "error")
            ));
        }
        let mut detail = said.join("; ");
        if let Some(last) = record
            .errors
            .iter()
            .rev()
            .find(|error| error.severity.serious() || error.before_this_boot())
            .filter(|error| !error.part.is_empty())
        {
            let _ = write!(detail, "; the latest is in {}", last.part);
        }
        return Check::failed(Area::Errors, NAME, Problem::HardwareError, detail);
    }
    if !busy.is_empty() {
        let each: Vec<String> = busy
            .iter()
            .map(|busy| format!("{} had {}", busy.name, counted(busy.errors, "corrected error")))
            .collect();
        let estimated = busy.iter().any(|busy| busy.estimated);
        let note = if estimated {
            "; the record lost its older lines, so these are the counts since the start scaled \
             to a day"
        } else {
            " in the last 24 hours"
        };
        let detail = if estimated {
            format!("{}{note}", each.join(", "))
        } else {
            format!("{}{note}", each.join(" and "))
        };
        return Check::warning(Area::Errors, NAME, Problem::DimmCorrectedRate, detail);
    }
    let corrected = record.counts.corrected.max(in_lines(record, Severity::Corrected));
    let detail = if corrected == 0 {
        "none heard of since the start".to_string()
    } else {
        format!(
            "{} heard of since the start, none a memory device had {line} of in a day",
            counted(corrected, "corrected error")
        )
    };
    Check::passed(Area::Errors, NAME, detail)
}

/// The check for each PCIe device the kernel marked failed after a fatal
/// error.
pub(crate) fn pcie_checks(failed: &[Failed]) -> Vec<Check> {
    failed
        .iter()
        .map(|device| {
            let detail = format!(
                "PCIe device {} ({}) stopped after a fatal error; its driver cannot use it \
                 until the machine restarts",
                device.address, device.name
            );
            Check::failed(
                Area::Errors,
                &format!("pcie {}", device.address),
                Problem::PcieDeviceFailed,
                detail,
            )
        })
        .collect()
}

/// The checks of the area, read from the kernel's record in `dir` and the
/// state of the PCI devices in `pci`.
pub(crate) fn run(dir: &Path, pci: &Sources) -> Vec<Check> {
    let mut checks = vec![check_of(&Record::read(dir), uptime(), DAILY_LINE)];
    checks.extend(pcie_checks(&devices::failed(pci)));
    checks
}

/// The memory devices that have errors, as the memory area of the check
/// names them: `DIMM_A1 3 corrected, DIMM_B1 1 uncorrected`; empty when
/// none has.
pub(crate) fn dimms_with_errors(record: &Record) -> String {
    let named: Vec<String> = record
        .dimms_with_errors()
        .map(|dimm| {
            let mut counts = Vec::new();
            if dimm.uncorrected > 0 {
                counts.push(format!("{} uncorrected", dimm.uncorrected));
            }
            if dimm.corrected > 0 {
                counts.push(format!("{} corrected", dimm.corrected));
            }
            format!("{} {}", dimm.name(), counts.join(", "))
        })
        .collect();
    named.join("; ")
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::super::check::Outcome;
    use super::*;

    fn record(errors: &str, counts: Option<&str>) -> Record {
        Record::parse(Some(errors), counts)
    }

    fn memory_lines(handle: &str, from: u64, times: u64, start: f64, step: f64) -> String {
        (0..times).fold(String::new(), |mut text, n| {
            let _ = writeln!(
                text,
                "{} {:.3} ghes1 corrected memory x1 dimm={handle} DIMM A1: memory error",
                from + n,
                start + n as f64 * step
            );
            text
        })
    }

    const COUNTS: &str = "corrected 30\nuncorrected 0\nseverity informational 0\nseverity \
                          corrected 30\nseverity recoverable 0\nseverity fatal 0\nkind memory \
                          30\nkind pcie 0\nkind processor 0\nkind other 0\nrecorded 30\n\
                          dropped 0\ndimm 0x1100 DIMM_A1 16384M corrected 30 uncorrected 0\n\
                          dimm 0x1101 DIMM_B1 16384M corrected 0 uncorrected 0\n";

    fn options(items: &[&str]) -> Result<Options, Fail> {
        parse(&mut Args::new("hardware", items.iter().map(OsString::from).collect()))
    }

    #[test]
    fn a_failed_pcie_device_is_a_failure_of_the_errors_area() {
        let none = pcie_checks(&[]);
        assert!(none.is_empty());
        let failed = [Failed { address: "0000:03:00.0".into(), name: "Intel e1000e".into() }];
        let checks = pcie_checks(&failed);
        assert_eq!(checks.len(), 1);
        let check = &checks[0];
        assert_eq!((check.area, check.outcome), (Area::Errors, Outcome::Failed));
        assert_eq!(check.id.as_deref(), Some("HW-163"));
        assert_eq!(
            check.detail,
            "PCIe device 0000:03:00.0 (Intel e1000e) stopped after a fatal error; its driver \
             cannot use it until the machine restarts"
        );
    }

    #[test]
    fn the_options_are_json_and_since_and_nothing_else() {
        assert_eq!(options(&[]).unwrap(), Options::default());
        let both = options(&["--json", "--since", "3600"]).unwrap();
        assert_eq!(both, Options { json: true, since: Some(3600) });
        assert_eq!(
            options(&["--since", "soon"]).unwrap_err().message,
            "'soon' is not a valid number"
        );
        assert!(options(&["--since"]).is_err());
        assert!(options(&["memory"]).is_err());
        assert!(options(&["--all"]).is_err());
    }

    #[test]
    fn an_empty_record_is_an_empty_report_with_one_calm_line() {
        let report = Report::of(Record::default(), None, 100.0);
        assert_eq!(report.text(None), "No hardware errors since this boot.\n");
        let value: serde_json::Value = serde_json::from_str(&report.json().unwrap()).unwrap();
        assert_eq!(value["format"], 1);
        assert_eq!(value["errors"], serde_json::json!([]));
        assert_eq!(value["counts"]["corrected"], 0);
        assert_eq!(value["counts"]["dimms"], serde_json::json!([]));
        assert_eq!(value["counts"]["by_severity"]["fatal"], 0);
        assert_eq!(value["counts"]["by_kind"]["memory"], 0);
        assert_eq!(value["skipped"], 0);
    }

    #[test]
    fn the_json_gives_each_member_of_an_error_and_of_the_counts() {
        let text = "1 12.345 bert fatal pcie x1 - PCIe 0000:3b:00.0: PCIe error: x\n\
                    2 40.002 ghes3 corrected memory x2 dimm=0x1100 DIMM 0: memory error\n";
        let report = Report::of(record(text, Some(COUNTS)), None, 100.0);
        let value: serde_json::Value = serde_json::from_str(&report.json().unwrap()).unwrap();
        assert_eq!(
            value["errors"][0],
            serde_json::json!({
                "seq": 1, "seconds": 12.345, "source": "bert", "severity": "fatal",
                "kind": "pcie", "count": 1, "dimm": null, "part": "PCIe 0000:3b:00.0",
                "what": "PCIe error: x"
            })
        );
        assert_eq!(value["errors"][1]["dimm"], 0x1100);
        assert_eq!(value["errors"][1]["count"], 2);
        let counts = &value["counts"];
        assert_eq!((&counts["corrected"], &counts["uncorrected"]), (&30.into(), &0.into()));
        assert_eq!(counts["by_severity"]["corrected"], 30);
        assert_eq!(counts["by_kind"]["memory"], 30);
        assert_eq!((&counts["recorded"], &counts["dropped"]), (&30.into(), &0.into()));
        assert_eq!(
            counts["dimms"][0],
            serde_json::json!({
                "handle": 0x1100, "locator": "DIMM_A1", "bytes": 16384u64 << 20,
                "corrected": 30, "uncorrected": 0
            })
        );
        assert_eq!(counts["other_dimms"], serde_json::json!({"corrected": 0, "uncorrected": 0}));
    }

    #[test]
    fn since_keeps_the_errors_of_the_last_seconds() {
        let text = memory_lines("0x1100", 1, 5, 100.0, 100.0);
        // Times 100, 200, 300, 400, 500; now is 520 s after the start.
        let report = Report::of(record(&text, None), Some(250), 520.0);
        let seqs: Vec<u64> = report.errors.iter().map(|error| error.seq).collect();
        assert_eq!(seqs, [3, 4, 5]);
        let none = Report::of(record(&text, None), Some(10), 900.0);
        assert_eq!(none.text(Some(10)), "No hardware errors in the last 10 seconds.\n");
    }

    #[test]
    fn the_text_is_a_table_and_the_memory_devices_with_errors() {
        let text = "1 12.345 bert fatal pcie x1 - PCIe 0000:3b:00.0: PCIe error\n\
                    2 40.002 ghes3 corrected memory x3 dimm=0x1100 DIMM 0: memory error\n\
                    3 61.500 mce corrected processor x1 - -: cache\n";
        let counts = "corrected 4\nuncorrected 0\nrecorded 3\ndropped 0\ndimm 0x1100 DIMM_A1 \
                      16384M corrected 3 uncorrected 0\ndimm 0x1101 DIMM_B1 16384M corrected 0 \
                      uncorrected 0\ndimm other corrected 1 uncorrected 0\n";
        let out = Report::of(record(text, Some(counts)), None, 100.0).text(None);
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].starts_with("TIME    SOURCE  SEVERITY   KIND       "), "{out}");
        assert!(lines[0].ends_with("PART               WHAT"), "{out}");
        assert!(
            lines[1]
                .starts_with("12.345  bert    fatal      pcie       PCIe 0000:3b:00.0  PCIe error"),
            "{out}"
        );
        assert!(lines[2].ends_with("memory error x3"), "{out}");
        assert!(lines[3].contains("  -  "), "{out}");
        assert!(out.contains("\nMemory devices with errors since this boot:\n"), "{out}");
        assert!(out.contains("  DIMM_A1 (16G): 3 corrected, 0 uncorrected\n"), "{out}");
        assert!(!out.contains("DIMM_B1"), "{out}");
        assert!(out.contains("others the tables do not list: 1 corrected, 0 uncorrected"), "{out}");
        assert!(!out.contains("record took in"), "{out}");
        assert!(lines.iter().all(|line| line.trim_end() == *line));
    }

    #[test]
    fn a_record_that_dropped_lines_and_lines_that_did_not_parse_say_so() {
        let counts = "corrected 300\nrecorded 300\ndropped 44\n";
        let out = Report::of(record("bad\n", Some(counts)), None, 1.0).text(None);
        assert!(out.contains("The record took in 300 errors and holds the last 256;"), "{out}");
        assert!(out.contains("1 of the record's lines were not understood"), "{out}");
    }

    #[test]
    fn no_record_at_all_skips_the_check_and_none_heard_of_passes_it() {
        let check = check_of(&Record::default(), 100.0, 24);
        assert_eq!(check.outcome, Outcome::Skipped);
        let quiet = check_of(&record("", Some("corrected 0\nuncorrected 0\n")), 100.0, 24);
        assert_eq!(quiet.outcome, Outcome::Passed);
        assert_eq!(quiet.detail, "none heard of since the start");
        assert_eq!(quiet.area, Area::Errors);
        assert_eq!(quiet.id, None);
    }

    #[test]
    fn a_fatal_or_recoverable_error_fails_the_check_and_names_the_part() {
        let text = "7 50.000 aer recoverable pcie x1 - 0000:00:1c.0: bad TLP\n";
        let check = check_of(&record(text, None), 100.0, 24);
        assert_eq!(check.outcome, Outcome::Failed);
        assert_eq!(check.id.as_deref(), Some("HW-161"));
        assert_eq!(
            check.detail,
            "1 recoverable error counted since the start; the latest is in 0000:00:1c.0"
        );
        // The counts say how many there were when lines were dropped.
        let counts = "severity recoverable 3\nseverity fatal 1\nrecorded 9\ndropped 0\n";
        let check = check_of(&record(text, Some(counts)), 100.0, 24);
        assert!(
            check.detail.starts_with("1 fatal and 3 recoverable errors counted since the start"),
            "{}",
            check.detail
        );
    }

    #[test]
    fn any_bert_error_fails_the_check_even_a_corrected_one() {
        let text = "1 3.000 bert corrected memory x2 dimm=0x1100 DIMM 0: memory error\n";
        let check = check_of(&record(text, None), 100.0, 24);
        assert_eq!(check.outcome, Outcome::Failed);
        assert_eq!(
            check.detail,
            "the firmware recorded 2 errors from before this start; the latest is in DIMM 0"
        );
    }

    #[test]
    fn a_device_with_24_corrected_errors_in_a_day_warns_and_23_passes() {
        let day = memory_lines("0x1100", 1, 24, 100.0, 10.0);
        let check = check_of(&record(&day, Some(COUNTS)), 500.0, 24);
        assert_eq!(check.outcome, Outcome::Warning);
        assert_eq!(check.id.as_deref(), Some("HW-162"));
        assert_eq!(check.detail, "DIMM_A1 had 24 corrected errors in the last 24 hours");
        let few = memory_lines("0x1100", 1, 23, 100.0, 10.0);
        let check = check_of(&record(&few, None), 500.0, 24);
        assert_eq!(check.outcome, Outcome::Passed);
        assert!(check.detail.starts_with("23 corrected errors heard of since the start"));
        // The line is the setting's.
        assert_eq!(check_of(&record(&few, None), 500.0, 20).outcome, Outcome::Warning);
    }

    #[test]
    fn errors_older_than_a_day_and_errors_of_two_devices_are_not_added_up() {
        // 12 errors a day ago and 12 now: 12 in the last day.
        let mut text = memory_lines("0x1100", 1, 12, 100.0, 1.0);
        text.push_str(&memory_lines("0x1100", 13, 12, 300_000.0, 1.0));
        let check = check_of(&record(&text, None), 300_100.0, 24);
        assert_eq!(check.outcome, Outcome::Passed);
        // Two devices with 12 each are two quiet devices.
        let mut two = memory_lines("0x1100", 1, 12, 100.0, 1.0);
        two.push_str(&memory_lines("0x1101", 13, 12, 100.0, 1.0));
        assert_eq!(check_of(&record(&two, None), 200.0, 24).outcome, Outcome::Passed);
    }

    #[test]
    fn errors_without_a_device_are_not_a_devices_and_informational_ones_do_not_count() {
        let many = (1..=30).fold(String::new(), |mut text, n| {
            let _ =
                writeln!(text, "{n} {n}.000 mce corrected memory x1 - CPU 0 bank 7 channel 1: x");
            text
        });
        assert_eq!(check_of(&record(&many, None), 100.0, 24).outcome, Outcome::Passed);
        let info =
            memory_lines("0x1100", 1, 30, 100.0, 1.0).replace(" corrected ", " informational ");
        assert_eq!(check_of(&record(&info, None), 200.0, 24).outcome, Outcome::Passed);
    }

    #[test]
    fn a_record_that_dropped_lines_uses_the_counts_scaled_to_a_day_and_says_so() {
        let counts = "corrected 480\nuncorrected 0\nrecorded 480\ndropped 224\ndimm 0x1100 \
                      DIMM_A1 16384M corrected 480 uncorrected 0\ndimm 0x1101 DIMM_B1 8192M \
                      corrected 5 uncorrected 0\n";
        // Two days of uptime: 480 since the start is 240 a day.
        let check = check_of(&record("", Some(counts)), 2.0 * DAY, 24);
        assert_eq!(check.outcome, Outcome::Warning);
        assert_eq!(
            check.detail,
            "DIMM_A1 had 240 corrected errors; the record lost its older lines, so these are \
             the counts since the start scaled to a day"
        );
        // Less than a day since the start: the counts are the day's.
        let busy = busy_dimms(&record("", Some(counts)), 3600.0, 24);
        assert_eq!(busy.len(), 1);
        assert_eq!((busy[0].errors, busy[0].estimated), (480, false));
        // Over five days, 480 is 96 a day, and 5 is nothing.
        let busy = busy_dimms(&record("", Some(counts)), 5.0 * DAY, 24);
        assert_eq!(
            (busy[0].name.as_str(), busy[0].errors, busy[0].estimated),
            ("DIMM_A1", 96, true)
        );
        assert!(busy_dimms(&record("", Some(counts)), 5.0 * DAY, 100).is_empty());
    }

    #[test]
    fn the_worst_device_comes_first() {
        let mut text = memory_lines("0x1101", 1, 30, 100.0, 1.0);
        text.push_str(&memory_lines("0x1100", 31, 40, 100.0, 1.0));
        let busy = busy_dimms(&record(&text, Some(COUNTS)), 500.0, 24);
        let names: Vec<(&str, u64)> =
            busy.iter().map(|busy| (busy.name.as_str(), busy.errors)).collect();
        assert_eq!(names, [("DIMM_A1", 40), ("DIMM_B1", 30)]);
        // A handle the counts do not list is named by the handle.
        let unlisted =
            busy_dimms(&record(&memory_lines("0x9", 1, 30, 100.0, 1.0), None), 100.0, 24);
        assert_eq!(unlisted[0].name, "0x0009");
    }

    #[test]
    fn the_memory_area_names_the_devices_that_have_errors() {
        let counts = "dimm 0x1 DIMM_A1 8192M corrected 3 uncorrected 0\ndimm 0x2 DIMM_B1 8192M \
                      corrected 0 uncorrected 0\ndimm 0x3 DIMM_C1 corrected 1 uncorrected 2\n";
        assert_eq!(
            dimms_with_errors(&Record::parse(None, Some(counts))),
            "DIMM_A1 3 corrected; DIMM_C1 2 uncorrected, 1 corrected"
        );
        assert_eq!(dimms_with_errors(&Record::default()), "");
    }
}
