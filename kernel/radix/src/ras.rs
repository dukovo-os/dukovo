// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The record of hardware errors (`/kernel/ras`, ADR 0662): the last
//! [`radix_mca::record::RING_ENTRIES`] errors reported, by the CPUs'
//! machine-check banks ([`crate::mce`]), the firmware ([`crate::apei`])
//! or PCIe devices, with the counts of them by severity, by kind and by
//! memory device, and how much of the log they may take.
//!
//! Every error is recorded and counted. The log is limited to
//! [`radix_mca::record::LOGGED_PER_WINDOW`] lines a minute for each part
//! (a memory device where the error names one, else what the reporter
//! keys it by): a failing DIMM that corrects thousands of errors a minute
//! fills the record, not the log, and the first line after a quiet
//! minute says how many were left out.
//!
//! The lock is a spin lock with interrupts off: machine checks are read
//! in the fault handler of a program that met bad memory, before
//! interrupts are on again. Nothing is done under it but copying a
//! fixed-size entry or counts; formatting and logging come before and
//! after. The firmware's NMI reports are recorded later, from the `apei`
//! thread, never from the NMI.

use core::fmt::{self, Write};

use radix_mca::record::{
    Counts, Dimms, Entry, Kind, Log, PART_BYTES, RateLimit, Ring, Severity, Source, Text, Totals,
    write_entry,
};
use radix_sync::SpinLock;
use radix_vfs::Text as FileText;

use crate::dmi;
use crate::sched;

struct Record {
    ring: Ring,
    limit: RateLimit,
    totals: Totals,
    dimms: Dimms,
    /// The memory errors reported by anything but the machine-check
    /// banks (whose own counts are [`crate::mce::memory_errors`]): those
    /// that named a memory device, and those that did not.
    memory_placed: Counts,
    memory_unplaced: Counts,
}

static RECORD: SpinLock<Record> = SpinLock::new(Record {
    ring: Ring::new(),
    limit: RateLimit::new(),
    totals: Totals::new(),
    dimms: Dimms::new(),
    memory_placed: Counts { corrected: 0, uncorrected: 0 },
    memory_unplaced: Counts { corrected: 0, uncorrected: 0 },
});

// What a reporter keys the log's limit by when no memory device is
// named: one of these in the top bits, its own number below. A memory
// device's handle is 16 bits, so the two never meet.

/// A machine-check bank, by its number.
pub const KEY_BANK: u64 = 1 << 32;
/// One of the firmware's error sources, by its id.
pub const KEY_FIRMWARE: u64 = 2 << 32;
/// A PCIe function, by its segment, bus, device and function.
pub const KEY_PCIE: u64 = 3 << 32;

/// An error to record.
pub struct Report<'a> {
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
    /// The part, by the name on the board where it is known.
    pub part: &'a dyn fmt::Display,
    /// What it was.
    pub what: &'a dyn fmt::Display,
    /// What the log's limit counts it by when `dimm` is `None`: a `KEY_`
    /// constant with the bank, source or device number.
    pub key: u64,
}

/// Records an error and counts it, in all and by memory device, and says
/// whether the log should have a line for it: `Log::Line` carries how
/// many of this part's errors the log left out in its last window. An
/// error the hardware did not correct always gets a line.
pub fn record(report: &Report<'_>) -> Log {
    let mut part = Text::<PART_BYTES>::new();
    let mut text = Text::new();
    // Text that does not fit is cut; it cannot fail.
    let _ = write!(part, "{}", report.part);
    let _ = write!(text, "{}", report.what);
    let now = sched::now_ns();
    let key = report.dimm.map_or(report.key, u64::from);
    let mut record = RECORD.lock_irq();
    let corrected = report.severity.corrected();
    record.totals.add(report.severity, report.kind, report.count);
    if let Some(handle) = report.dimm {
        record.dimms.count(handle, corrected, report.count);
    }
    if report.kind == Kind::Memory && report.source != Source::MachineCheck {
        let counts = if report.dimm.is_some() {
            &mut record.memory_placed
        } else {
            &mut record.memory_unplaced
        };
        counts.add(corrected, report.count);
    }
    record.ring.push(Entry {
        seq: 0,
        time_ns: now,
        source: report.source,
        severity: report.severity,
        kind: report.kind,
        count: report.count,
        dimm: report.dimm,
        part,
        text,
    });
    let log = record.limit.check(key, now);
    if report.severity.corrected() { log } else { Log::Line { left_out: 0 } }
}

/// The memory errors reported by anything but the machine-check banks:
/// those that named a memory device, and those that did not.
pub fn memory_errors() -> (Counts, Counts) {
    let record = RECORD.lock_irq();
    (record.memory_placed, record.memory_unplaced)
}

/// The errors counted for the memory device with SMBIOS handle `handle`.
pub fn dimm_counts(handle: u16) -> Counts {
    RECORD.lock_irq().dimms.get(handle).unwrap_or_default()
}

/// `/kernel/ras/errors`: a line for each error the record holds, the
/// oldest first ([`radix_mca::record::write_entry`]).
pub fn write_errors(out: &mut FileText) -> fmt::Result {
    let mut next = 1;
    loop {
        // One entry at a time, so that the lock is not held while the
        // text grows.
        let entry = {
            let record = RECORD.lock_irq();
            match record.ring.from(next) {
                Some(entry) => *entry,
                None => return Ok(()),
            }
        };
        write_entry(out, &entry)?;
        next = entry.seq + 1;
    }
}

/// `/kernel/ras/counts`: the errors counted since boot, by severity and
/// by kind, how many the record has recorded and dropped, and for each
/// memory device the tables list as populated, and any other that an
/// error named: its SMBIOS handle, its locator and size if the tables
/// say, and its counts.
pub fn write_counts(out: &mut FileText) -> fmt::Result {
    let (totals, total, kept, other) = {
        let record = RECORD.lock_irq();
        (record.totals, record.ring.total(), record.ring.len(), record.dimms.other)
    };
    writeln!(out, "corrected {}", totals.corrected())?;
    writeln!(out, "uncorrected {}", totals.uncorrected())?;
    for (severity, count) in
        [Severity::Informational, Severity::Corrected, Severity::Recoverable, Severity::Fatal]
            .iter()
            .zip(totals.by_severity)
    {
        writeln!(out, "severity {severity} {count}")?;
    }
    for (kind, count) in
        [Kind::Memory, Kind::Pcie, Kind::Processor, Kind::Other].iter().zip(totals.by_kind)
    {
        writeln!(out, "kind {kind} {count}")?;
    }
    writeln!(out, "recorded {total}")?;
    writeln!(out, "dropped {}", total - kept as u64)?;
    if let Some(dimms) = dmi::dimms() {
        for dimm in dimms.all() {
            let counts = dimm_counts(dimm.handle);
            write!(out, "dimm {:#06x}", dimm.handle)?;
            if let Some(locator) = dimm.locator {
                write!(out, " {locator}")?;
            }
            if let Some(size) = dimm.size {
                write!(out, " {}M", size >> 20)?;
            }
            writeln!(out, " corrected {} uncorrected {}", counts.corrected, counts.uncorrected)?;
        }
    }
    // Devices the tables do not list as populated, one at a time so that
    // the lock is not held while the text grows.
    for index in 0..radix_mca::record::COUNTED_DIMMS {
        let Some((handle, counts)) = RECORD.lock_irq().dimms.iter().nth(index) else { break };
        if dmi::dimm_by_handle(handle).is_none() {
            writeln!(
                out,
                "dimm {handle:#06x} corrected {} uncorrected {}",
                counts.corrected, counts.uncorrected
            )?;
        }
    }
    if other != Counts::default() {
        writeln!(
            out,
            "dimm other corrected {} uncorrected {}",
            other.corrected, other.uncorrected
        )?;
    }
    Ok(())
}
