// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! EXPERIMENT, not for merging: where a `poll` system call's time goes
//! between the handler's first instruction with interrupts on and its
//! last, by the marks it passes. Each CPU notes when it passed each mark;
//! a call that reached `poll` adds the stretches between them to its
//! CPU's sums, and every [`EVERY`] such calls that did not wait the CPU logs their means and
//! starts again. Only the CPU itself reads or writes its line.

use core::sync::atomic::{AtomicU64, Ordering};

use radix_hal::{CpuLocal, Timer};
use radix_klog::info;

use crate::Arch;

pub const SYS: usize = 0;
pub const CAL: usize = 1;
pub const FOUND: usize = 2;
pub const CALL: usize = 3;
pub const PROCESS: usize = 4;
pub const POSIX: usize = 5;
pub const POLL: usize = 6;
pub const LOOKED: usize = 7;
pub const POLLED: usize = 8;
pub const ANSWERED: usize = 9;
pub const ENDED: usize = 10;
pub const HANDLED: usize = 11;

const MARKS: usize = 12;
const NAMES: [&str; MARKS] = [
    "sys", "cal", "found", "call", "process", "posix", "poll", "looked", "polled", "answered",
    "ended", "handled",
];
const CPUS: usize = 8;
const EVERY: u64 = 50_000;
/// A call longer than this waited, and is left out.
const LONGEST_NS: u64 = 20_000;

#[repr(align(64))]
struct Line {
    at: [AtomicU64; MARKS],
    polled: AtomicU64,
    sums: [AtomicU64; MARKS],
    calls: AtomicU64,
}

static LINES: [Line; CPUS] = [const {
    Line {
        at: [const { AtomicU64::new(0) }; MARKS],
        polled: AtomicU64::new(0),
        sums: [const { AtomicU64::new(0) }; MARKS],
        calls: AtomicU64::new(0),
    }
}; CPUS];

fn line() -> Option<&'static Line> {
    LINES.get(Arch::current_cpu())
}

/// The running CPU passed `mark`.
#[inline(always)]
pub fn mark(mark: usize) {
    let Some(line) = line() else { return };
    line.at[mark].store(Arch::now_ns(), Ordering::Relaxed);
    if mark == POLL {
        line.polled.store(1, Ordering::Relaxed);
    }
}

/// The call ends: one that reached `poll` adds its stretches.
#[inline(always)]
pub fn handled() {
    let Some(line) = line() else { return };
    let now = Arch::now_ns();
    line.at[HANDLED].store(now, Ordering::Relaxed);
    if line.polled.load(Ordering::Relaxed) == 0 {
        return;
    }
    line.polled.store(0, Ordering::Relaxed);
    let at: [u64; MARKS] = core::array::from_fn(|m| line.at[m].load(Ordering::Relaxed));
    if (1..MARKS).any(|m| at[m] < at[m - 1]) || at[HANDLED] - at[SYS] > LONGEST_NS {
        return;
    }
    for m in 1..MARKS {
        let sum = &line.sums[m];
        sum.store(sum.load(Ordering::Relaxed) + (at[m] - at[m - 1]), Ordering::Relaxed);
    }
    let calls = line.calls.load(Ordering::Relaxed) + 1;
    line.calls.store(calls, Ordering::Relaxed);
    if calls >= EVERY {
        report(line, calls);
    }
}

/// Logs `line`'s means over its `calls` and starts it again: out of line,
/// so that the calls' own frames stay as they were.
#[cold]
#[inline(never)]
fn report(line: &Line, calls: u64) {
    let mean = |m: usize| line.sums[m].load(Ordering::Relaxed) * 10 / calls;
    let mut text = heapless_line();
    for (m, name) in NAMES.iter().enumerate().skip(1) {
        let tenths = mean(m);
        let _ = core::fmt::Write::write_fmt(
            &mut text,
            format_args!(" {}={}.{}", name, tenths / 10, tenths % 10),
        );
    }
    info!(
        "callprobe: cpu {} calls {} mean ns from the mark before:{}",
        Arch::current_cpu(),
        calls,
        text.as_str()
    );
    for sum in &line.sums {
        sum.store(0, Ordering::Relaxed);
    }
    line.calls.store(0, Ordering::Relaxed);
}

/// A line of text on the stack, for the report.
struct Text {
    bytes: [u8; 256],
    len: usize,
}

fn heapless_line() -> Text {
    Text { bytes: [0; 256], len: 0 }
}

impl Text {
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("?")
    }
}

impl core::fmt::Write for Text {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.len + s.len();
        if end > self.bytes.len() {
            return Err(core::fmt::Error);
        }
        self.bytes[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}
