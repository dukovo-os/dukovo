// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The crash log: everything the consoles show, also kept in RAM that
//! survives a warm restart, so the next boot can show how this one ended.
//! A panic also writes a crash dump beside it, which the next boot gives
//! to dumps(8) as `/devices/crashdump` (ADR 0483).

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, Ordering};

use radix_hal::{Cpu, Platform};
use radix_klog::crashlog::{CrashLog, DUMP_FULL, Dump, DumpState, Ending};
use radix_klog::{Level, Sink, Style, TextBuf, info};
use radix_sync::SpinLock;

use crate::Arch;

/// The crash log. Its sink takes it from interrupts too (the log masks
/// them around its sinks), so everything else takes it with them masked.
static LOG: SpinLock<Option<CrashLog>> = SpinLock::new(None);

/// Set while the panic path writes the crash dump, which holds [`LOG`]:
/// the sink leaves the crash log alone meanwhile, and a panic on the way
/// restarts the machine rather than halting it.
static WRITING_DUMP: AtomicBool = AtomicBool::new(false);

fn flush(bytes: &[u8]) {
    Arch::flush_to_memory(bytes);
}

/// The log's console sink. It takes every message, debug ones included.
struct CrashLogSink;

impl Sink for CrashLogSink {
    fn write(&self, text: &str, _style: Style) {
        if WRITING_DUMP.load(Ordering::Acquire) {
            return;
        }
        if let Some(log) = LOG.lock().as_mut() {
            log.write(text, &mut flush);
        }
    }

    fn wants_all_levels(&self) -> bool {
        true
    }
}

static SINK: CrashLogSink = CrashLogSink;

/// Opens the crash log region, if the architecture reserved one, and
/// starts copying the console output into it (from the start of boot).
pub fn init() {
    let Some(region) = Arch::take_crash_log_region() else { return };
    let len = region.len();
    match CrashLog::open(region) {
        Some(log) => {
            *LOG.lock_irq() = Some(log);
            radix_klog::add_sink(&SINK, true);
        }
        None => radix_klog::warn!("Crash log: region of {len} bytes is too small"),
    }
}

/// Whether this boot has a crash log.
pub fn active() -> bool {
    LOG.lock_irq().is_some()
}

/// This boot's number, counted by the crash log (1 if there is none).
pub fn boot_number() -> u32 {
    LOG.lock_irq().as_ref().map_or(1, CrashLog::boot)
}

/// Records how this boot ends.
pub fn set_ending(ending: Ending) {
    if let Some(log) = LOG.lock_irq().as_mut() {
        log.set_ending(ending, &mut flush);
    }
}

/// Writes this boot's crash dump, from the panic path: its sections are
/// what `sections` writes through the [`DumpWriter`] it is given. Nothing
/// is written without a crash log, or in a region with no room for dumps.
/// `wall_ns` is the time of the panic, in nanoseconds since 1970 (UTC), if
/// the kernel knows it.
pub fn write_dump(wall_ns: Option<u64>, sections: &mut dyn FnMut(&mut DumpWriter<'_>)) {
    let mut guard = LOG.lock_irq();
    let Some(log) = guard.as_mut() else { return };
    WRITING_DUMP.store(true, Ordering::Release);
    if let Some(dump) = log.begin_dump(wall_ns.unwrap_or(0), &mut flush) {
        let mut writer = DumpWriter { dump };
        sections(&mut writer);
        writer.dump.seal(DumpState::Whole, &mut flush);
    }
    WRITING_DUMP.store(false, Ordering::Release);
}

/// Whether the panic path is writing the crash dump.
pub fn writing_dump() -> bool {
    WRITING_DUMP.load(Ordering::Acquire)
}

/// What a section of the dump writes each of its lines with.
pub type Line<'a> = dyn FnMut(fmt::Arguments<'_>) + 'a;

/// The crash dump as the panic path writes it: lines, in sections. The
/// dump is sealed after each section, so that one the kernel fails in
/// leaves the ones before it.
pub struct DumpWriter<'a> {
    dump: Dump<'a>,
}

impl DumpWriter<'_> {
    /// Writes one line, its first 512 bytes.
    pub fn line(&mut self, args: fmt::Arguments<'_>) {
        let mut text = TextBuf::<512>::new();
        let _ = text.write_fmt(args);
        self.dump.write(text.as_str(), &mut flush);
        self.dump.write("\n", &mut flush);
    }

    /// Writes the section `name`: a blank line, `== ` and the name, and
    /// the lines `body` writes.
    pub fn section(&mut self, name: fmt::Arguments<'_>, body: &mut dyn FnMut(&mut Line<'_>)) {
        self.line(format_args!(""));
        self.line(format_args!("== {name}"));
        body(&mut |args| self.line(args));
        self.dump.seal(DumpState::CutShort, &mut flush);
    }
}

/// What `/devices/crashdump` holds after the previous boot's dump: that
/// boot's whole log.
const LOG_HEADING: &[u8] = b"\n== Log\n";
/// The line after a dump that the kernel did not finish, unless the dump
/// says why itself.
const UNFINISHED: &[u8] = b"\n(the dump ends here: the kernel stopped before it was whole)\n";

/// Calls `read` with the parts of `/devices/crashdump`, in order, if the
/// previous boot left a dump: the dump, a note if it is cut short, and that
/// boot's log. The previous boot's slots are not written in this one, but
/// the lock is held while they are read, as [`CrashLog`] keeps both.
fn with_dump_file<R>(read: impl FnOnce([&[u8]; 5]) -> R) -> Option<R> {
    let guard = LOG.lock_irq();
    let log = guard.as_ref()?;
    let dump = log.previous_dump()?;
    let (older, newer) = log.previous_text()?;
    let note = match dump.state {
        DumpState::CutShort if !dump.text.ends_with(DUMP_FULL.as_bytes()) => UNFINISHED,
        _ => &[],
    };
    Some(read([dump.text, note, LOG_HEADING, older, newer]))
}

/// Bytes of `/devices/crashdump`: 0 when the previous boot left no dump.
pub fn previous_dump_len() -> u64 {
    with_dump_file(|parts| parts.iter().map(|part| part.len() as u64).sum()).unwrap_or(0)
}

/// Most bytes one read of `/devices/crashdump` copies, so that the lock,
/// which masks interrupts, is held briefly.
const READ_MAX: usize = 64 * 1024;

/// Copies `/devices/crashdump` from `offset` into `out`: how many bytes,
/// 0 at its end.
pub fn read_previous_dump(offset: u64, out: &mut [u8]) -> usize {
    with_dump_file(|parts| {
        let room = out.len().min(READ_MAX);
        let (mut skip, mut done) = (offset, 0);
        for part in parts {
            let len = part.len() as u64;
            if skip >= len {
                skip -= len;
                continue;
            }
            let from = &part[skip as usize..];
            skip = 0;
            let n = from.len().min(room - done);
            out[done..done + n].copy_from_slice(&from[..n]);
            done += n;
            if done == room {
                break;
            }
        }
        done
    })
    .unwrap_or(0)
}

/// How the previous boot ended, and whether its log contains `needle`.
pub fn previous(needle: &str) -> Option<(Ending, bool)> {
    let log = LOG.lock_irq();
    let previous = log.as_ref()?.previous()?;
    Some((previous.ending, previous.contains(needle)))
}

/// Lines of the previous boot's log shown at boot.
const SHOWN_AFTER_PANIC: usize = 40;
const SHOWN_OTHERWISE: usize = 12;

/// Reports the previous boot's log, if the region held one: how that boot
/// ended and its last lines.
pub fn report_previous() {
    let guard = LOG.lock_irq();
    let Some(current) = guard.as_ref() else {
        drop(guard);
        info!("Crash log: none (no region reserved, or disabled with crashlog=off)");
        return;
    };
    let boot = current.boot();
    let Some(previous) = current.previous() else {
        // Logging writes to the crash log, so its lock must be free.
        drop(guard);
        info!("Crash log: boot {boot}; no log from a previous boot in memory");
        return;
    };
    let shown =
        if previous.ending == Ending::Panicked { SHOWN_AFTER_PANIC } else { SHOWN_OTHERWISE };
    let total = previous.line_count();
    let skip = total.saturating_sub(shown);
    let level = if previous.ending == Ending::Panicked { Level::Warn } else { Level::Info };
    // Copy the lines out before logging: logging writes to this region.
    let mut lines = [const { None }; SHOWN_AFTER_PANIC];
    for (slot, line) in lines.iter_mut().zip(previous.lines().skip(skip)) {
        *slot = Some(line);
    }
    let (previous_boot, ending) = (previous.boot, previous.ending);
    let dump = current.previous_dump().map(|dump| (dump.text.len(), dump.state));
    drop(guard);
    radix_klog::log(
        level,
        format_args!(
            "Crash log: boot {boot}; the previous boot ({previous_boot}) {ending}. Its last {} \
             lines:",
            total - skip
        ),
    );
    for line in lines.iter().flatten() {
        radix_klog::log(level, format_args!("  | {line}"));
    }
    if let Some((len, state)) = dump {
        let kib = len.div_ceil(1024);
        let cut = if state == DumpState::CutShort { ", cut short" } else { "" };
        radix_klog::log(
            level,
            format_args!(
                "Crash log: the previous boot's dump is kept for dumps(8) ({kib} KiB{cut})"
            ),
        );
    }
}
