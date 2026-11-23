// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The kernel's record of hardware errors, as `/kernel/ras/errors` and
//! `/kernel/ras/counts` show it: the parts of it that need no machine, so
//! that their tests run on the host.
//!
//! `errors` has the last 256 errors, a line each, which the kernel's
//! `write_entry` makes: the number, the time in seconds since the start,
//! the source (`mce`, `ghes<n>`, `bert`, `aer`), the severity, the kind,
//! `x` and how many errors the line stands for, `dimm=` and the memory
//! device's SMBIOS handle or `-`, then the part, a colon and a space, and
//! what it was. A part never holds a colon and a space, so the first one
//! after the `dimm` field ends the part.
//!
//! `counts` has the totals, which outlive the lines that were dropped: a
//! line each for `corrected`, `uncorrected`, `severity <name>`, `kind
//! <name>`, `recorded` and `dropped`, then a `dimm` line for each memory
//! device, `dimm <handle> [locator] [size]M corrected N uncorrected N`.
//! The locator may hold spaces, so a reader takes the last four words
//! first.
//!
//! A line that does not parse is left out and counted in `skipped`: a
//! newer kernel may say more than this reader knows, and none of it may
//! stop the rest being read. A kernel without the files has an empty
//! record, which is no error.

use std::fs;
use std::path::Path;

use serde::Serialize;

/// Where the kernel keeps the record.
pub(crate) const DIR: &str = "/kernel/ras";

/// How bad an error was, in UEFI's terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Severity {
    Informational,
    Corrected,
    Recoverable,
    Fatal,
}

impl Severity {
    pub(crate) const ALL: [Severity; 4] =
        [Severity::Informational, Severity::Corrected, Severity::Recoverable, Severity::Fatal];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Severity::Informational => "informational",
            Severity::Corrected => "corrected",
            Severity::Recoverable => "recoverable",
            Severity::Fatal => "fatal",
        }
    }

    fn parse(word: &str) -> Option<Severity> {
        Severity::ALL.into_iter().find(|severity| severity.name() == word)
    }

    /// Whether the error was not corrected and the machine had to deal
    /// with it.
    pub(crate) fn serious(self) -> bool {
        self >= Severity::Recoverable
    }
}

/// What kind of part an error was in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    Memory,
    Pcie,
    Processor,
    Other,
}

impl Kind {
    pub(crate) const ALL: [Kind; 4] = [Kind::Memory, Kind::Pcie, Kind::Processor, Kind::Other];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Kind::Memory => "memory",
            Kind::Pcie => "pcie",
            Kind::Processor => "processor",
            Kind::Other => "other",
        }
    }

    fn find(word: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.name() == word)
    }
}

/// One line of `errors`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct Error {
    /// Its number, from 1 at each start.
    pub(crate) seq: u64,
    /// When it came, in seconds since the start.
    pub(crate) seconds: f64,
    /// `mce`, `ghes<n>`, `bert` or `aer`.
    pub(crate) source: String,
    pub(crate) severity: Severity,
    pub(crate) kind: Kind,
    /// How many errors the line stands for.
    pub(crate) count: u64,
    /// The SMBIOS handle of the memory device it was in, where known.
    pub(crate) dimm: Option<u32>,
    /// The part by the name the kernel knows it by; empty if it has none.
    pub(crate) part: String,
    pub(crate) what: String,
}

impl Error {
    /// Whether it comes from the firmware's record of an error before
    /// this start (the BERT).
    pub(crate) fn before_this_boot(&self) -> bool {
        self.source == "bert"
    }
}

/// Errors counted, corrected and not.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Tally {
    pub(crate) corrected: u64,
    pub(crate) uncorrected: u64,
}

/// The errors counted for each severity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct BySeverity {
    pub(crate) informational: u64,
    pub(crate) corrected: u64,
    pub(crate) recoverable: u64,
    pub(crate) fatal: u64,
}

impl BySeverity {
    fn slot(&mut self, severity: Severity) -> &mut u64 {
        match severity {
            Severity::Informational => &mut self.informational,
            Severity::Corrected => &mut self.corrected,
            Severity::Recoverable => &mut self.recoverable,
            Severity::Fatal => &mut self.fatal,
        }
    }
}

/// The errors counted for each kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct ByKind {
    pub(crate) memory: u64,
    pub(crate) pcie: u64,
    pub(crate) processor: u64,
    pub(crate) other: u64,
}

impl ByKind {
    fn slot(&mut self, kind: Kind) -> &mut u64 {
        match kind {
            Kind::Memory => &mut self.memory,
            Kind::Pcie => &mut self.pcie,
            Kind::Processor => &mut self.processor,
            Kind::Other => &mut self.other,
        }
    }
}

/// A memory device and the errors counted for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Dimm {
    /// Its SMBIOS handle.
    pub(crate) handle: u32,
    /// Its place on the board, where the tables say it.
    pub(crate) locator: Option<String>,
    /// Its size in bytes, where the tables say it.
    pub(crate) bytes: Option<u64>,
    pub(crate) corrected: u64,
    pub(crate) uncorrected: u64,
}

impl Dimm {
    /// What to call it: its locator, else its handle as the kernel
    /// writes it.
    pub(crate) fn name(&self) -> String {
        self.locator.clone().unwrap_or_else(|| format!("{:#06x}", self.handle))
    }

    /// Whether it has any error.
    pub(crate) fn has_errors(&self) -> bool {
        self.corrected > 0 || self.uncorrected > 0
    }
}

/// What `counts` says.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Counts {
    pub(crate) corrected: u64,
    pub(crate) uncorrected: u64,
    pub(crate) by_severity: BySeverity,
    pub(crate) by_kind: ByKind,
    /// How many errors the record took in, and how many of its lines
    /// were dropped to make room.
    pub(crate) recorded: u64,
    pub(crate) dropped: u64,
    pub(crate) dimms: Vec<Dimm>,
    /// Errors in memory devices the tables do not list.
    pub(crate) other_dimms: Tally,
}

/// The record: its lines, its counts, and how many lines were left out
/// because they did not parse.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Record {
    pub(crate) errors: Vec<Error>,
    pub(crate) counts: Counts,
    /// Whether the kernel has a counts file at all.
    pub(crate) counted: bool,
    pub(crate) skipped: usize,
}

impl Record {
    /// The record in `dir`; a file that is not there is an empty one.
    pub(crate) fn read(dir: &Path) -> Record {
        let errors = fs::read_to_string(dir.join("errors")).ok();
        let counts = fs::read_to_string(dir.join("counts")).ok();
        Record::parse(errors.as_deref(), counts.as_deref())
    }

    /// The record the texts of its two files make.
    pub(crate) fn parse(errors: Option<&str>, counts: Option<&str>) -> Record {
        let (errors, lost) = errors.map_or_else(Default::default, parse_errors);
        let (counted, counts, unknown) = match counts {
            Some(text) => {
                let (counts, unknown) = parse_counts(text);
                (true, counts, unknown)
            }
            None => (false, Counts::default(), 0),
        };
        Record { errors, counts, counted, skipped: lost + unknown }
    }

    /// The memory devices that have an error, in the order the kernel
    /// lists them.
    pub(crate) fn dimms_with_errors(&self) -> impl Iterator<Item = &Dimm> {
        self.counts.dimms.iter().filter(|dimm| dimm.has_errors())
    }
}

/// The lines of `errors`, and how many did not parse.
pub(crate) fn parse_errors(text: &str) -> (Vec<Error>, usize) {
    let mut errors = Vec::new();
    let mut skipped = 0;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        match parse_line(line) {
            Some(error) => errors.push(error),
            None => skipped += 1,
        }
    }
    (errors, skipped)
}

/// A number as the kernel writes a handle: `0x1100`, or plain digits.
fn parse_handle(word: &str) -> Option<u32> {
    match word.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => word.parse().ok(),
    }
}

/// The seconds of `12.345`.
fn parse_seconds(word: &str) -> Option<f64> {
    let seconds: f64 = word.parse().ok()?;
    (seconds.is_finite() && seconds >= 0.0).then_some(seconds)
}

/// A source's name: letters and digits, as `mce` and `ghes3` are.
fn plain(word: &str) -> bool {
    !word.is_empty() && word.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// The error a line of `errors` stands for, if it parses. A kind that
/// this reader does not know is `other`; a severity it does not know
/// leaves the line out, since what to do about it is not known.
pub(crate) fn parse_line(line: &str) -> Option<Error> {
    let mut fields = line.splitn(8, ' ');
    let seq = fields.next()?.parse().ok()?;
    let seconds = parse_seconds(fields.next()?)?;
    let source = fields.next().filter(|source| plain(source))?.to_string();
    let severity = Severity::parse(fields.next()?)?;
    let kind = Kind::find(fields.next()?).unwrap_or(Kind::Other);
    let count = fields.next()?.strip_prefix('x')?.parse().ok()?;
    let dimm = match fields.next()? {
        "-" => None,
        word => Some(parse_handle(word.strip_prefix("dimm=")?)?),
    };
    let rest = fields.next()?;
    // The part ends at the first colon and space; with nothing after the
    // colon the space was left off.
    let (part, what) = match rest.split_once(": ") {
        Some(split) => split,
        None => (rest.strip_suffix(':')?, ""),
    };
    let part = if part == "-" { "" } else { part };
    Some(Error {
        seq,
        seconds,
        source,
        severity,
        kind,
        count,
        dimm,
        part: part.to_string(),
        what: what.trim_end().to_string(),
    })
}

/// The counts in `text`, and how many of its lines this reader does not
/// know.
pub(crate) fn parse_counts(text: &str) -> (Counts, usize) {
    let mut counts = Counts::default();
    let mut skipped = 0;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        if !count_line(&mut counts, line) {
            skipped += 1;
        }
    }
    (counts, skipped)
}

/// Takes in one line of `counts`; false if it is not one this reader
/// knows.
fn count_line(counts: &mut Counts, line: &str) -> bool {
    let words: Vec<&str> = line.split_whitespace().collect();
    let number = |word: &str| word.parse::<u64>().ok();
    match words.as_slice() {
        ["corrected", n] => number(n).map(|n| counts.corrected = n).is_some(),
        ["uncorrected", n] => number(n).map(|n| counts.uncorrected = n).is_some(),
        ["recorded", n] => number(n).map(|n| counts.recorded = n).is_some(),
        ["dropped", n] => number(n).map(|n| counts.dropped = n).is_some(),
        ["severity", name, n] => match (Severity::parse(name), number(n)) {
            (Some(severity), Some(n)) => {
                *counts.by_severity.slot(severity) = n;
                true
            }
            _ => false,
        },
        ["kind", name, n] => match (Kind::find(name), number(n)) {
            (Some(kind), Some(n)) => {
                *counts.by_kind.slot(kind) = n;
                true
            }
            _ => false,
        },
        ["dimm", "other", "corrected", c, "uncorrected", u] => match (number(c), number(u)) {
            (Some(corrected), Some(uncorrected)) => {
                counts.other_dimms = Tally { corrected, uncorrected };
                true
            }
            _ => false,
        },
        ["dimm", handle, middle @ .., "corrected", c, "uncorrected", u] => {
            let (Some(handle), Some(corrected), Some(uncorrected)) =
                (parse_handle(handle), number(c), number(u))
            else {
                return false;
            };
            // A size, `16384M`, ends what is between; the rest is the
            // locator, which may hold spaces.
            let (size, locator) = match middle.split_last() {
                Some((last, rest)) => match last.strip_suffix('M').and_then(number) {
                    Some(mib) => (mib.checked_mul(1 << 20), rest),
                    None => (None, middle),
                },
                None => (None, middle),
            };
            let locator = (!locator.is_empty()).then(|| locator.join(" "));
            counts.dimms.push(Dimm { handle, locator, bytes: size, corrected, uncorrected });
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BERT: &str = "1 12.345 bert fatal pcie x1 - PCIe 0000:3b:00.0: PCIe error: device \
                        0000:3b:00.0 (8086:1533), port root port";
    const GHES: &str = "2 40.002 ghes3 corrected memory x1 dimm=0x1100 DIMM 0: memory error: \
                        type single-bit ECC, address 0x12345000, module handle 0x1100";
    const MCE: &str = "3 61.500 mce corrected memory x4 - CPU 3 bank 7 channel 1: corrected error";

    fn line(text: &str) -> Error {
        parse_line(text).unwrap_or_else(|| panic!("does not parse: {text}"))
    }

    #[test]
    fn a_firmware_line_with_a_part_that_is_cut_at_the_first_colon_and_space() {
        let error = line(BERT);
        assert_eq!(error.seq, 1);
        assert_eq!(error.seconds, 12.345);
        assert_eq!(error.source, "bert");
        assert_eq!((error.severity, error.kind), (Severity::Fatal, Kind::Pcie));
        assert_eq!((error.count, error.dimm), (1, None));
        assert_eq!(error.part, "PCIe 0000:3b:00.0");
        // The colons inside the words stay with what it was.
        assert_eq!(error.what, "PCIe error: device 0000:3b:00.0 (8086:1533), port root port");
        assert!(error.before_this_boot());
    }

    #[test]
    fn a_line_with_a_memory_device_has_its_handle() {
        let error = line(GHES);
        assert_eq!(error.source, "ghes3");
        assert_eq!(error.severity, Severity::Corrected);
        assert_eq!(error.kind, Kind::Memory);
        assert_eq!(error.dimm, Some(0x1100));
        assert_eq!(error.part, "DIMM 0");
        assert!(error.what.starts_with("memory error: type single-bit ECC"));
        assert!(!error.before_this_boot());
    }

    #[test]
    fn a_machine_check_line_has_a_count_above_one() {
        let error = line(MCE);
        assert_eq!((error.seq, error.seconds, error.count), (3, 61.5, 4));
        assert_eq!((error.source.as_str(), error.part.as_str()), ("mce", "CPU 3 bank 7 channel 1"));
        assert_eq!(error.what, "corrected error");
    }

    #[test]
    fn a_pcie_aer_line_and_every_severity_and_kind_are_known() {
        let error = line("9 1.000 aer recoverable pcie x2 - 0000:00:1c.0: bad TLP");
        assert_eq!((error.source.as_str(), error.severity), ("aer", Severity::Recoverable));
        let error = line("10 2.000 ghes0 informational other x1 - cache: note");
        assert_eq!((error.severity, error.kind), (Severity::Informational, Kind::Other));
        let error = line("11 3.000 mce corrected processor x1 - CPU 0 bank 1: x");
        assert_eq!(error.kind, Kind::Processor);
        assert!(Severity::Fatal.serious() && Severity::Recoverable.serious());
        assert!(!Severity::Corrected.serious() && !Severity::Informational.serious());
    }

    #[test]
    fn a_part_with_no_name_is_empty_and_what_may_be_empty() {
        assert_eq!(line("4 5.000 mce corrected memory x1 - -: text").part, "");
        let bare = line("4 5.000 mce corrected memory x1 - CPU 1:");
        assert_eq!((bare.part.as_str(), bare.what.as_str()), ("CPU 1", ""));
        let spaced = line("4 5.000 mce corrected memory x1 - CPU 1: ");
        assert_eq!((spaced.part.as_str(), spaced.what.as_str()), ("CPU 1", ""));
    }

    #[test]
    fn a_part_that_held_a_colon_and_a_space_was_written_with_a_space() {
        // write_entry turns the pair into one space, so the part ends at
        // the next colon and space.
        let error = line("5 6.000 ghes1 corrected memory x1 dimm=0x2 Node 1 slot A: memory: x");
        assert_eq!(error.part, "Node 1 slot A");
        assert_eq!(error.what, "memory: x");
        // A colon with no space after it does not end the part.
        let error = line("5 6.000 aer corrected pcie x1 - 0000:3b:00.0: link");
        assert_eq!(error.part, "0000:3b:00.0");
    }

    #[test]
    fn a_kind_that_is_not_known_is_other_and_a_severity_that_is_not_known_skips_the_line() {
        assert_eq!(line("6 7.000 mce corrected video x1 - GPU: x").kind, Kind::Other);
        assert_eq!(parse_line("6 7.000 mce dire memory x1 - GPU: x"), None);
    }

    #[test]
    fn a_handle_may_be_written_in_decimal() {
        assert_eq!(line("7 8.000 ghes1 corrected memory x1 dimm=4352 A: x").dimm, Some(4352));
        assert_eq!(parse_line("7 8.000 ghes1 corrected memory x1 dimm=zz A: x"), None);
        assert_eq!(parse_line("7 8.000 ghes1 corrected memory x1 0x1100 A: x"), None);
    }

    #[test]
    fn a_line_that_does_not_parse_is_skipped_and_counted() {
        let text = format!(
            "{BERT}\nnot a line\n\n{GHES}\n1 2 3\n-1 5.0 mce corrected memory x1 - a: b\n\
             8 -5.0 mce corrected memory x1 - a: b\n8 5.0 MCE corrected memory x1 - a: b\n\
             8 5.0 mce corrected memory 1 - a: b\n8 5.0 mce corrected memory x1 - no colon\n\
             8 5.0 mce corrected memory x1 -\n{MCE}\n"
        );
        let (errors, skipped) = parse_errors(&text);
        let seqs: Vec<u64> = errors.iter().map(|error| error.seq).collect();
        assert_eq!(seqs, [1, 2, 3]);
        // The blank line is not a line; the other eight are.
        assert_eq!(skipped, 8);
    }

    #[test]
    fn a_line_cut_short_does_not_panic() {
        let whole = GHES;
        for end in 0..whole.len() {
            if whole.is_char_boundary(end) {
                let _ = parse_line(&whole[..end]);
            }
        }
        assert_eq!(parse_line(""), None);
        assert_eq!(parse_line("   "), None);
    }

    #[test]
    fn what_is_not_ascii_is_kept_whole() {
        let error = line("1 1.000 mce corrected memory x1 - DIMM Ä1: temperature 85 °C");
        assert_eq!((error.part.as_str(), error.what.as_str()), ("DIMM Ä1", "temperature 85 °C"));
    }

    const COUNTS: &str = "corrected 17\nuncorrected 2\nseverity informational 0\nseverity \
                          corrected 17\nseverity recoverable 1\nseverity fatal 1\nkind memory \
                          15\nkind pcie 3\nkind processor 1\nkind other 0\nrecorded 300\n\
                          dropped 44\ndimm 0x1100 DIMM_A1 16384M corrected 16 uncorrected 0\n\
                          dimm 0x1101 DIMM_B1 16384M corrected 0 uncorrected 0\ndimm 0x1102 \
                          corrected 1 uncorrected 2\ndimm other corrected 0 uncorrected 0\n";

    #[test]
    fn the_counts_are_read_with_each_total_and_each_memory_device() {
        let (counts, skipped) = parse_counts(COUNTS);
        assert_eq!(skipped, 0);
        assert_eq!((counts.corrected, counts.uncorrected), (17, 2));
        assert_eq!(
            counts.by_severity,
            BySeverity { informational: 0, corrected: 17, recoverable: 1, fatal: 1 }
        );
        assert_eq!(counts.by_kind, ByKind { memory: 15, pcie: 3, processor: 1, other: 0 });
        assert_eq!((counts.recorded, counts.dropped), (300, 44));
        assert_eq!(counts.dimms.len(), 3);
        assert_eq!(
            counts.dimms[0],
            Dimm {
                handle: 0x1100,
                locator: Some("DIMM_A1".into()),
                bytes: Some(16384 << 20),
                corrected: 16,
                uncorrected: 0
            }
        );
        // A device with neither locator nor size is named by its handle.
        assert_eq!(counts.dimms[2].locator, None);
        assert_eq!(counts.dimms[2].bytes, None);
        assert_eq!(counts.dimms[2].name(), "0x1102");
        assert_eq!(counts.dimms[0].name(), "DIMM_A1");
        assert_eq!(counts.other_dimms, Tally::default());
    }

    #[test]
    fn a_locator_with_spaces_and_a_size_that_is_missing_are_both_read() {
        let (counts, skipped) = parse_counts(
            "dimm 0x20 Bank 0 DIMM 3 8192M corrected 1 uncorrected 0\n\
             dimm 0x21 Channel A Slot 1 corrected 0 uncorrected 4\n\
             dimm 0x22 2048M corrected 5 uncorrected 0\n\
             dimm other corrected 3 uncorrected 1\n",
        );
        assert_eq!(skipped, 0);
        assert_eq!(counts.dimms[0].locator.as_deref(), Some("Bank 0 DIMM 3"));
        assert_eq!(counts.dimms[0].bytes, Some(8192 << 20));
        assert_eq!(counts.dimms[1].locator.as_deref(), Some("Channel A Slot 1"));
        assert_eq!(counts.dimms[1].bytes, None);
        assert_eq!(
            (counts.dimms[2].locator.clone(), counts.dimms[2].bytes),
            (None, Some(2048 << 20))
        );
        assert_eq!(counts.other_dimms, Tally { corrected: 3, uncorrected: 1 });
    }

    #[test]
    fn counts_lines_that_are_not_known_are_skipped_and_counted() {
        let (counts, skipped) = parse_counts(
            "corrected 1\nfuture 9\nseverity dire 2\nkind video 3\ncorrected many\ndimm zz \
             corrected 1 uncorrected 1\ndimm 0x1 corrected 1\n\n",
        );
        assert_eq!(counts.corrected, 1);
        assert_eq!(skipped, 6);
        assert!(counts.dimms.is_empty());
    }

    #[test]
    fn only_a_device_with_an_error_has_errors() {
        let record = Record::parse(None, Some(COUNTS));
        let names: Vec<String> = record.dimms_with_errors().map(Dimm::name).collect();
        assert_eq!(names, ["DIMM_A1", "0x1102"]);
    }

    #[test]
    fn a_kernel_without_the_files_has_an_empty_record() {
        let dir = std::env::temp_dir().join(format!("hardware-ras-{}-none", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let record = Record::read(&dir);
        assert_eq!(record, Record::default());
        assert!(!record.counted);
        assert!(record.errors.is_empty());
    }

    #[test]
    fn the_record_is_read_from_its_two_files_and_skipped_lines_are_added() {
        let dir = std::env::temp_dir().join(format!("hardware-ras-{}-files", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("errors"), format!("{BERT}\n{GHES}\ngarbage\n")).unwrap();
        fs::write(dir.join("counts"), format!("{COUNTS}more 1\n")).unwrap();
        let record = Record::read(&dir);
        assert_eq!(record.errors.len(), 2);
        assert!(record.counted);
        assert_eq!(record.skipped, 2);
        assert_eq!(record.counts.recorded, 300);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_errors_alone_make_a_record_too() {
        let record = Record::parse(Some(MCE), None);
        assert_eq!(record.errors.len(), 1);
        assert!(!record.counted);
    }
}
