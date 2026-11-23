// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The record of hardware errors the kernel keeps for `/kernel/ras`: the
//! parts of it that need no hardware, so that their tests run on the host.
//!
//! - [`Ring`]: the last [`RING_ENTRIES`] errors, each a fixed-size
//!   [`Entry`], the oldest dropped first. Nothing is allocated. An entry
//!   says where the report came from ([`Source`]: a machine-check bank,
//!   one of the firmware's error sources, the boot error record, PCIe
//!   AER), how bad it was ([`Severity`]), what kind of part ([`Kind`]),
//!   and which part, by the name on the board where it is known.
//! - [`RateLimit`]: how many log lines a part (a bank, a DIMM, a device)
//!   may have in a minute, so that a failing DIMM does not fill the log.
//!   The record and the counts take every error; only the log is
//!   limited, and the first line after a quiet minute says how many were
//!   left out.
//! - [`Counts`] and [`Dimms`]: errors counted, in all, by severity and
//!   kind, and by the memory device (SMBIOS type 17 handle) they came
//!   from.
//! - [`write_entry`]: the line the record's file shows for an entry.

use core::fmt::{self, Write};

/// How many errors the record keeps.
pub const RING_ENTRIES: usize = 256;

/// The bytes of an entry's text; what does not fit is cut at a character.
pub const TEXT_BYTES: usize = 160;

/// The bytes of an entry's part: "DIMM A3", "0000:3b:00.0".
pub const PART_BYTES: usize = 48;

/// How many log lines a part may have in one window.
pub const LOGGED_PER_WINDOW: u32 = 10;

/// The window the limit counts in: a minute.
pub const WINDOW_NS: u64 = 60_000_000_000;

/// How many parts are limited apart; errors from more are never limited,
/// as a table that is full cannot say more is wrong.
pub const LIMITED_PARTS: usize = 32;

/// Where a report came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Source {
    /// A CPU's machine-check bank, read by the kernel.
    #[default]
    MachineCheck,
    /// One of the firmware's error sources (ACPI's GHES), by its id.
    Firmware(u16),
    /// The firmware's record of an error that ended the last boot (BERT).
    LastBoot,
    /// A PCIe device's Advanced Error Reporting.
    Pcie,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::MachineCheck => f.write_str("mce"),
            Source::Firmware(id) => write!(f, "ghes{id}"),
            Source::LastBoot => f.write_str("bert"),
            Source::Pcie => f.write_str("aer"),
        }
    }
}

/// How bad an error was, in UEFI's terms (appendix N), which the
/// machine checks' map onto.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Only for information.
    Informational,
    /// The hardware corrected it.
    #[default]
    Corrected,
    /// Not corrected, and the machine went on.
    Recoverable,
    /// Not corrected, and the machine could not go on.
    Fatal,
}

impl Severity {
    /// Whether the hardware corrected the error (or it was only for
    /// information).
    pub fn corrected(self) -> bool {
        self <= Severity::Corrected
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Severity::Informational => "informational",
            Severity::Corrected => "corrected",
            Severity::Recoverable => "recoverable",
            Severity::Fatal => "fatal",
        })
    }
}

/// What kind of part an error was in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    /// Memory: a DIMM, a memory controller's channel.
    Memory,
    /// A PCIe device or port.
    Pcie,
    /// A processor: its caches, its buses, its other blocks.
    #[default]
    Processor,
    /// Anything else the firmware reports.
    Other,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Kind::Memory => "memory",
            Kind::Pcie => "pcie",
            Kind::Processor => "processor",
            Kind::Other => "other",
        })
    }
}

/// How many memory devices are counted apart.
pub const COUNTED_DIMMS: usize = 64;

/// A fixed-size text of at most `N` bytes (below 256), cut where it is
/// full.
#[derive(Clone, Copy)]
pub struct Text<const N: usize = TEXT_BYTES> {
    bytes: [u8; N],
    len: u8,
}

impl<const N: usize> Text<N> {
    /// No text.
    pub const fn new() -> Text<N> {
        const { assert!(N < 256) };
        Text { bytes: [0; N], len: 0 }
    }

    /// The text.
    pub fn as_str(&self) -> &str {
        // The bytes were written whole characters at a time.
        core::str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or("")
    }
}

impl<const N: usize> Default for Text<N> {
    fn default() -> Text<N> {
        Text::new()
    }
}

impl<const N: usize> fmt::Debug for Text<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl<const N: usize> Write for Text<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let used = usize::from(self.len);
        let mut take = s.len().min(N - used);
        while !s.is_char_boundary(take) {
            take -= 1;
        }
        self.bytes[used..used + take].copy_from_slice(&s.as_bytes()[..take]);
        // At most N, which is below 256.
        self.len = (used + take) as u8;
        // Text that does not fit is cut, which is not an error.
        Ok(())
    }
}

/// One error of the record.
#[derive(Clone, Copy, Debug, Default)]
pub struct Entry {
    /// Its number: the first error of the boot is 1.
    pub seq: u64,
    /// When it was read, in nanoseconds since boot.
    pub time_ns: u64,
    /// Where the report came from.
    pub source: Source,
    /// How bad it was.
    pub severity: Severity,
    /// What kind of part it was in.
    pub kind: Kind,
    /// How many errors it stands for.
    pub count: u64,
    /// The SMBIOS handle of the memory device it came from, if known.
    pub dimm: Option<u16>,
    /// The part, by the name on the board where it is known: "DIMM A3",
    /// "0000:3b:00.0", "CPU 3 bank 7".
    pub part: Text<PART_BYTES>,
    /// What it was.
    pub text: Text,
}

/// The last [`RING_ENTRIES`] errors.
pub struct Ring {
    entries: [Entry; RING_ENTRIES],
    /// Errors pushed since boot.
    total: u64,
}

impl Ring {
    /// An empty record.
    // Only ever made in a static's initializer, where the array is not on a
    // stack; the record is too big to be made on one at run time.
    // It has no `Default`, which would make it on a stack too.
    #[allow(clippy::large_stack_arrays, clippy::new_without_default)]
    pub const fn new() -> Ring {
        Ring {
            entries: [Entry {
                seq: 0,
                time_ns: 0,
                source: Source::MachineCheck,
                severity: Severity::Corrected,
                kind: Kind::Processor,
                count: 0,
                dimm: None,
                part: Text::new(),
                text: Text::new(),
            }; RING_ENTRIES],
            total: 0,
        }
    }

    /// Adds an error, numbering it (`entry.seq` is set), and drops the
    /// oldest if the record is full. Returns the number.
    pub fn push(&mut self, mut entry: Entry) -> u64 {
        self.total += 1;
        entry.seq = self.total;
        self.entries[(self.total as usize - 1) % RING_ENTRIES] = entry;
        self.total
    }

    /// How many errors the record holds.
    pub fn len(&self) -> usize {
        (self.total as usize).min(RING_ENTRIES)
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// How many errors came since boot, those dropped included.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// The number of the oldest error kept; 1 until the record is full.
    pub fn first_seq(&self) -> u64 {
        self.total - self.len() as u64 + 1
    }

    /// The error numbered `seq`, if the record still holds it.
    pub fn get(&self, seq: u64) -> Option<&Entry> {
        (seq >= self.first_seq() && seq <= self.total)
            .then(|| &self.entries[(seq as usize - 1) % RING_ENTRIES])
    }

    /// The first error numbered at least `seq` (the oldest kept if the
    /// record dropped it), for a reader that goes on from where it was.
    pub fn from(&self, seq: u64) -> Option<&Entry> {
        self.get(seq.max(self.first_seq()))
    }
}

/// What the log does with an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Log {
    /// Log it; `left_out` errors of the part's last window were not.
    Line {
        /// How many errors of the part's last window were left out.
        left_out: u32,
    },
    /// Do not.
    Skip,
}

#[derive(Clone, Copy)]
struct Window {
    part: u64,
    start_ns: u64,
    logged: u32,
    left_out: u32,
}

/// How many log lines each part may have per [`WINDOW_NS`].
pub struct RateLimit {
    windows: [Window; LIMITED_PARTS],
    len: usize,
}

impl RateLimit {
    /// No part has logged.
    pub const fn new() -> RateLimit {
        RateLimit {
            windows: [Window { part: 0, start_ns: 0, logged: 0, left_out: 0 }; LIMITED_PARTS],
            len: 0,
        }
    }

    /// Whether an error of `part` at `now_ns` is logged. The first
    /// [`LOGGED_PER_WINDOW`] errors of a window are; the rest are counted,
    /// and the first of the next window is told how many.
    pub fn check(&mut self, part: u64, now_ns: u64) -> Log {
        let index = match self.windows[..self.len].iter().position(|w| w.part == part) {
            Some(index) => index,
            None if self.len < LIMITED_PARTS => {
                self.windows[self.len] = Window { part, start_ns: now_ns, logged: 0, left_out: 0 };
                self.len += 1;
                self.len - 1
            }
            None => return Log::Line { left_out: 0 },
        };
        let window = &mut self.windows[index];
        let mut left_out = 0;
        if now_ns.saturating_sub(window.start_ns) >= WINDOW_NS {
            left_out = window.left_out;
            *window = Window { part, start_ns: now_ns, logged: 0, left_out: 0 };
        }
        if window.logged < LOGGED_PER_WINDOW {
            window.logged += 1;
            Log::Line { left_out }
        } else {
            window.left_out = window.left_out.saturating_add(1);
            Log::Skip
        }
    }
}

impl Default for RateLimit {
    fn default() -> RateLimit {
        RateLimit::new()
    }
}

/// Corrected and uncorrected errors counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// The errors the hardware corrected.
    pub corrected: u64,
    /// Those it did not.
    pub uncorrected: u64,
}

impl Counts {
    /// Adds `count` errors.
    pub fn add(&mut self, corrected: bool, count: u64) {
        let which = if corrected { &mut self.corrected } else { &mut self.uncorrected };
        *which = which.saturating_add(count);
    }
}

/// Errors counted by severity and by kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    /// By [`Severity`], in its order: informational, corrected,
    /// recoverable, fatal.
    pub by_severity: [u64; 4],
    /// By [`Kind`], in its order: memory, PCIe, processor, other.
    pub by_kind: [u64; 4],
}

impl Totals {
    /// None counted.
    pub const fn new() -> Totals {
        Totals { by_severity: [0; 4], by_kind: [0; 4] }
    }

    /// Adds `count` errors of `severity` in a part of `kind`.
    pub fn add(&mut self, severity: Severity, kind: Kind, count: u64) {
        let s = &mut self.by_severity[severity as usize];
        *s = s.saturating_add(count);
        let k = &mut self.by_kind[kind as usize];
        *k = k.saturating_add(count);
    }

    /// The errors corrected, or only for information.
    pub fn corrected(&self) -> u64 {
        self.by_severity[Severity::Informational as usize]
            .saturating_add(self.by_severity[Severity::Corrected as usize])
    }

    /// The errors not corrected.
    pub fn uncorrected(&self) -> u64 {
        self.by_severity[Severity::Recoverable as usize]
            .saturating_add(self.by_severity[Severity::Fatal as usize])
    }
}

/// Errors counted by memory device, in the order each first had one.
pub struct Dimms {
    handles: [u16; COUNTED_DIMMS],
    counts: [Counts; COUNTED_DIMMS],
    len: usize,
    /// Errors of devices past [`COUNTED_DIMMS`].
    pub other: Counts,
}

impl Dimms {
    /// None counted.
    pub const fn new() -> Dimms {
        Dimms {
            handles: [0; COUNTED_DIMMS],
            counts: [Counts { corrected: 0, uncorrected: 0 }; COUNTED_DIMMS],
            len: 0,
            other: Counts { corrected: 0, uncorrected: 0 },
        }
    }

    /// Counts `count` errors of the device with SMBIOS handle `handle`.
    pub fn count(&mut self, handle: u16, corrected: bool, count: u64) {
        let index = match self.handles[..self.len].iter().position(|&h| h == handle) {
            Some(index) => index,
            None if self.len < COUNTED_DIMMS => {
                self.handles[self.len] = handle;
                self.len += 1;
                self.len - 1
            }
            None => return self.other.add(corrected, count),
        };
        self.counts[index].add(corrected, count);
    }

    /// The counts of the device with SMBIOS handle `handle`, if it had an
    /// error.
    pub fn get(&self, handle: u16) -> Option<Counts> {
        let index = self.handles[..self.len].iter().position(|&h| h == handle)?;
        Some(self.counts[index])
    }

    /// The devices counted: handle and counts.
    pub fn iter(&self) -> impl Iterator<Item = (u16, Counts)> + '_ {
        self.handles[..self.len].iter().copied().zip(self.counts[..self.len].iter().copied())
    }
}

impl Default for Dimms {
    fn default() -> Dimms {
        Dimms::new()
    }
}

/// Writes the line the record shows for `entry`, its fields apart by a
/// space up to the part: its number, its time in seconds since boot to
/// the millisecond, the source, the severity, the kind, `x` and how many
/// errors it stands for, `dimm=` and the memory device's SMBIOS handle
/// (or `-`), then the part, a colon and a space, and what it was. A part
/// never holds a colon and a space, so a reader splits the line there.
pub fn write_entry(out: &mut dyn Write, entry: &Entry) -> fmt::Result {
    let ms = entry.time_ns / 1_000_000;
    write!(
        out,
        "{} {}.{:03} {} {} {} x{}",
        entry.seq,
        ms / 1000,
        ms % 1000,
        entry.source,
        entry.severity,
        entry.kind,
        entry.count
    )?;
    match entry.dimm {
        Some(handle) => write!(out, " dimm={handle:#06x}")?,
        None => out.write_str(" -")?,
    }
    let part = entry.part.as_str();
    let part = if part.is_empty() { "-" } else { part };
    // A colon and a space inside the part become a space.
    out.write_str(" ")?;
    for (index, piece) in part.split(": ").enumerate() {
        if index > 0 {
            out.write_str(" ")?;
        }
        out.write_str(piece)?;
    }
    writeln!(out, ": {}", entry.text.as_str())
}

#[cfg(test)]
mod tests {
    use std::string::String;

    use super::*;

    fn entry(text: &str) -> Entry {
        let mut entry = Entry { count: 1, ..Entry::default() };
        let _ = entry.part.write_str("CPU 3 bank 5");
        let _ = entry.text.write_str(text);
        entry
    }

    #[test]
    fn the_ring_keeps_the_last_entries_and_numbers_them_from_one() {
        let mut ring = Ring::new();
        assert!(ring.is_empty());
        assert_eq!(ring.first_seq(), 1);
        for i in 0..RING_ENTRIES + 10 {
            let seq = ring.push(Entry { time_ns: i as u64, ..entry("x") });
            assert_eq!(seq, i as u64 + 1);
        }
        assert_eq!(ring.len(), RING_ENTRIES);
        assert_eq!(ring.total(), RING_ENTRIES as u64 + 10);
        assert_eq!(ring.first_seq(), 11);
        assert!(ring.get(10).is_none());
        assert_eq!(ring.get(11).map(|e| e.time_ns), Some(10));
        assert_eq!(ring.get(ring.total()).map(|e| e.seq), Some(ring.total()));
        assert!(ring.get(ring.total() + 1).is_none());
        // A reader that was at 4 goes on from the oldest kept.
        assert_eq!(ring.from(4).map(|e| e.seq), Some(11));
    }

    #[test]
    fn text_is_cut_at_a_character() {
        let mut text = Text::<TEXT_BYTES>::new();
        for _ in 0..TEXT_BYTES {
            // Two bytes each; 160 is even, so the 80th fits and no more.
            let _ = text.write_str("é");
        }
        assert_eq!(text.as_str().len(), TEXT_BYTES);
        let mut odd = Text::<TEXT_BYTES>::new();
        let _ = odd.write_str("a");
        for _ in 0..TEXT_BYTES {
            let _ = odd.write_str("é");
        }
        assert_eq!(odd.as_str().len(), TEXT_BYTES - 1);
        assert!(odd.as_str().starts_with("aé"));
    }

    #[test]
    fn a_part_logs_ten_lines_a_minute_and_says_how_many_it_left_out() {
        let mut limit = RateLimit::new();
        let s = 1_000_000_000;
        for i in 0..LOGGED_PER_WINDOW {
            assert_eq!(limit.check(7, u64::from(i) * s), Log::Line { left_out: 0 });
        }
        for i in 0..25 {
            assert_eq!(limit.check(7, 20 * s + i), Log::Skip);
        }
        // Another part has its own window.
        assert_eq!(limit.check(8, 21 * s), Log::Line { left_out: 0 });
        // A minute after the first error, the window starts again.
        assert_eq!(limit.check(7, 60 * s), Log::Line { left_out: 25 });
        assert_eq!(limit.check(7, 61 * s), Log::Line { left_out: 0 });
    }

    #[test]
    fn a_full_table_never_limits() {
        let mut limit = RateLimit::new();
        for part in 0..LIMITED_PARTS as u64 {
            assert_eq!(limit.check(part, 0), Log::Line { left_out: 0 });
        }
        for _ in 0..50 {
            assert_eq!(limit.check(1000, 0), Log::Line { left_out: 0 });
        }
    }

    #[test]
    fn dimms_are_counted_by_handle() {
        let mut dimms = Dimms::new();
        dimms.count(0x1a, true, 3);
        dimms.count(0x1b, false, 1);
        dimms.count(0x1a, true, 2);
        dimms.count(0x1a, false, 1);
        let all: std::vec::Vec<_> = dimms.iter().collect();
        assert_eq!(
            all,
            [
                (0x1a, Counts { corrected: 5, uncorrected: 1 }),
                (0x1b, Counts { corrected: 0, uncorrected: 1 }),
            ]
        );
        for handle in 0..COUNTED_DIMMS as u16 + 2 {
            dimms.count(0x100 + handle, true, 1);
        }
        assert_eq!(dimms.get(0x1b), Some(Counts { corrected: 0, uncorrected: 1 }));
        assert_eq!(dimms.get(0x99), None);
        assert_eq!(dimms.iter().count(), COUNTED_DIMMS);
        assert_eq!(dimms.other.corrected, 4);
    }

    #[test]
    fn an_entry_is_one_line() {
        let mut line = String::new();
        let mut e = entry("memory read error");
        e.seq = 12;
        e.time_ns = 3_456_789_000;
        write_entry(&mut line, &e).unwrap();
        assert_eq!(line, "12 3.456 mce corrected processor x1 - CPU 3 bank 5: memory read error\n");
        line.clear();
        e.source = Source::Firmware(3);
        e.severity = Severity::Recoverable;
        e.kind = Kind::Memory;
        e.count = 4;
        e.dimm = Some(0x1a);
        e.part = Text::new();
        let _ = e.part.write_str("DIMM A3");
        write_entry(&mut line, &e).unwrap();
        assert_eq!(
            line,
            "12 3.456 ghes3 recoverable memory x4 dimm=0x001a DIMM A3: memory read error\n"
        );
        // No part, and a part that would end the fields early.
        line.clear();
        e.part = Text::new();
        e.dimm = None;
        e.source = Source::LastBoot;
        write_entry(&mut line, &e).unwrap();
        assert_eq!(line, "12 3.456 bert recoverable memory x4 - -: memory read error\n");
        line.clear();
        let _ = e.part.write_str("odd: name");
        write_entry(&mut line, &e).unwrap();
        assert_eq!(line, "12 3.456 bert recoverable memory x4 - odd name: memory read error\n");
    }

    #[test]
    fn totals_count_by_severity_and_kind() {
        let mut totals = Totals::new();
        totals.add(Severity::Corrected, Kind::Memory, 5);
        totals.add(Severity::Fatal, Kind::Pcie, 1);
        totals.add(Severity::Recoverable, Kind::Memory, 2);
        totals.add(Severity::Informational, Kind::Other, 1);
        assert_eq!(totals.corrected(), 6);
        assert_eq!(totals.uncorrected(), 3);
        assert_eq!(totals.by_kind, [7, 1, 0, 1]);
        assert!(Severity::Corrected.corrected() && !Severity::Recoverable.corrected());
    }
}
