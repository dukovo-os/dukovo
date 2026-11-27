// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Keeping the CPUs' time-stamp counters in agreement (ADR 0660).
//!
//! The kernel's clock is the TSC ([`crate::time`]), read on whichever CPU
//! a thread runs, so the CPUs' counters must agree. The CPUs of one
//! package count from one reset, each offset by its IA32_TSC_ADJUST
//! (Intel SDM volume 3, "Time-Stamp Counter Adjustment"), which firmware
//! should leave equal and now and then does not. So as each CPU starts,
//! before its tick does, its TSC_ADJUST is set to that of the first CPU
//! started in its package, as Linux does. The boot CPU's is left as it is,
//! where Linux sets it to 0: the clock already counts from the boot CPU's
//! counter, which a change would move.
//!
//! The packages of a multi-socket machine count from resets of their own,
//! which firmware brings together. The first CPU started in each package
//! other than the boot CPU's measures that against the boot CPU for 2 ms,
//! as Linux's check does: in turn, each takes a lock, reads its counter
//! and compares it with the last value either read, and the largest step
//! back is logged. Nothing is corrected yet; that waits for a machine that
//! shows a difference to test it on. Under a hypervisor the check is left
//! out: the host keeps its virtual CPUs' counters together, and two virtual
//! CPUs taking turns would measure only how the host schedules them.

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use radix_sync::RawSpinLock;

use crate::cpu::{self, msr};
use crate::{smp, time};

/// The most packages whose CPUs' TSC_ADJUST is matched, and that are
/// measured: more than any x86 machine Dukovo runs on has.
const MAX_PACKAGES: usize = 16;

/// Per package, the TSC_ADJUST of the first CPU started in it, once
/// [`RECORDED`] has the package's bit.
static FIRST_ADJUST: [AtomicU64; MAX_PACKAGES] = [const { AtomicU64::new(0) }; MAX_PACKAGES];
/// The packages whose first CPU's TSC_ADJUST is recorded.
static RECORDED: AtomicU32 = AtomicU32::new(0);
/// How many CPUs' TSC_ADJUST was changed to match their package's.
static MATCHED: AtomicU32 = AtomicU32::new(0);

/// The packages measured against the boot CPU, its own included.
static MEASURED: AtomicU32 = AtomicU32::new(0);
/// The CPU about to start that measures, or `usize::MAX` for none.
static MEASURING: AtomicUsize = AtomicUsize::new(usize::MAX);
/// How many of the two CPUs have arrived to measure, and finished.
static ARRIVED: AtomicU32 = AtomicU32::new(0);
static FINISHED: AtomicU32 = AtomicU32::new(0);
/// The last counter value either CPU read.
static LAST: RawSpinLock<u64> = RawSpinLock::new(0);
/// The largest step back seen.
static WARP: AtomicU64 = AtomicU64::new(0);

/// How long the two CPUs measure, in fractions of a second: 2 ms.
const MEASURE_PER_SECOND: u64 = 500;
/// How long either CPU waits for the other to arrive or finish: 100 ms.
const WAIT_PER_SECOND: u64 = 10;
/// The most rounds either CPU measures, should its counter stand still.
const MAX_ROUNDS: u32 = 10_000_000;

/// The package of CPU `index`, if it is one of the first [`MAX_PACKAGES`].
fn package(index: usize) -> Option<usize> {
    let package = smp::place(index)?.package as usize;
    if package >= MAX_PACKAGES {
        radix_klog::debug!(
            "TSC: CPU {index} is in package {package}, past the {MAX_PACKAGES} whose counters \
             are matched and measured"
        );
        return None;
    }
    Some(package)
}

/// Sets this CPU's TSC_ADJUST to that of the first CPU started in its
/// package, or records its own if it is that CPU. Runs on each CPU as it
/// starts, one at a time and the boot CPU first, before its tick starts.
pub fn match_adjust(index: usize) {
    if !cpu::feature::tsc_adjust() {
        return;
    }
    let Some(package) = package(index) else { return };
    // SAFETY: CPUID says the MSR exists, and reading it changes nothing.
    let own = unsafe { cpu::rdmsr(msr::TSC_ADJUST) };
    let bit = 1 << package;
    if RECORDED.load(Ordering::Acquire) & bit == 0 {
        FIRST_ADJUST[package].store(own, Ordering::Relaxed);
        RECORDED.fetch_or(bit, Ordering::Release);
        return;
    }
    let first = FIRST_ADJUST[package].load(Ordering::Relaxed);
    if own != first {
        // SAFETY: CPUID says the MSR exists. The write moves only this
        // CPU's counter, to agree with the rest of its package, before
        // its tick starts.
        unsafe { cpu::wrmsr(msr::TSC_ADJUST, first) };
        MATCHED.fetch_add(1, Ordering::Relaxed);
    }
}

/// Whether CPU `index`, which the boot CPU is about to start, is to
/// measure its counter against the boot CPU's once started: the first CPU
/// started in a package other than the boot CPU's. Called by the boot CPU
/// before starting each CPU, which it then passes to [`measure_source`] if
/// this said so and the CPU started.
pub fn due(index: usize) -> bool {
    if cpu::feature::hypervisor() || tsc_hz() == 0 {
        return false;
    }
    let (Some(boot), Some(package)) = (package(0), package(index)) else { return false };
    MEASURED.fetch_or(1 << boot, Ordering::Relaxed);
    if MEASURED.fetch_or(1 << package, Ordering::Relaxed) & (1 << package) != 0 {
        return false;
    }
    *LAST.lock() = 0;
    WARP.store(0, Ordering::Relaxed);
    ARRIVED.store(0, Ordering::Relaxed);
    FINISHED.store(0, Ordering::Relaxed);
    MEASURING.store(index, Ordering::Release);
    true
}

/// The boot CPU's side of the measurement with CPU `index`, started: logs
/// what it found.
pub fn measure_source(index: usize) {
    let were = cpu::disable_interrupts();
    let measured = measure();
    let limit = cpu::rdtsc().saturating_add(tsc_hz() / WAIT_PER_SECOND);
    while measured && FINISHED.load(Ordering::Acquire) < 2 && cpu::rdtsc() < limit {
        cpu::pause();
    }
    let finished = FINISHED.load(Ordering::Acquire) == 2;
    MEASURING.store(usize::MAX, Ordering::Release);
    if were {
        cpu::enable_interrupts();
    }
    let package = smp::place(index).map_or(0, |place| place.package);
    if !finished {
        radix_klog::warn!(
            "TSC: package {package}'s counter was not measured: CPU {index} did not take part"
        );
        return;
    }
    let warp = WARP.load(Ordering::Relaxed);
    if warp == 0 {
        radix_klog::info!("TSC: package {package}'s counter agrees with the boot CPU's");
    } else {
        let ns = u128::from(warp) * 1_000_000_000 / u128::from(tsc_hz());
        radix_klog::warn!(
            "TSC: package {package}'s counter and the boot CPU's differ by up to {warp} cycles \
             ({ns} ns); times taken on CPUs of different packages may go backwards"
        );
    }
}

/// Calls off the measurement [`due`] asked for, with a CPU that did not
/// start.
pub fn cancel() {
    MEASURING.store(usize::MAX, Ordering::Release);
}

/// CPU `index`'s side of the measurement, as it starts, if the boot CPU
/// asked for one.
pub fn measure_target(index: usize) {
    if MEASURING.load(Ordering::Acquire) == index && measure() {
        FINISHED.fetch_add(1, Ordering::Release);
    }
}

/// Measures with the other CPU once it arrives; false if it does not come
/// within 100 ms. Interrupts are masked on the CPU that runs it.
fn measure() -> bool {
    let hz = tsc_hz();
    ARRIVED.fetch_add(1, Ordering::AcqRel);
    let limit = cpu::rdtsc().saturating_add(hz / WAIT_PER_SECOND);
    while ARRIVED.load(Ordering::Acquire) < 2 {
        if cpu::rdtsc() > limit {
            return false;
        }
        cpu::pause();
    }
    let end = cpu::rdtsc().saturating_add(hz / MEASURE_PER_SECOND);
    for round in 0..MAX_ROUNDS {
        let (previous, now) = {
            let mut last = LAST.lock();
            let previous = *last;
            let now = ordered_tsc();
            *last = now;
            (previous, now)
        };
        if previous > now {
            WARP.fetch_max(previous - now, Ordering::Relaxed);
        }
        // Reading the time costs a little: not every round.
        if round % 8 == 0 && now > end {
            break;
        }
    }
    true
}

/// Logs how many CPUs' TSC_ADJUST [`match_adjust`] changed, once every CPU
/// has started.
pub fn report() {
    let matched = MATCHED.load(Ordering::Relaxed);
    if matched > 0 {
        radix_klog::warn!(
            "TSC: TSC_ADJUST differed on {matched} {} from the first CPU of their package, and \
             was set to match it",
            if matched == 1 { "CPU" } else { "CPUs" }
        );
    }
}

/// The counter, read after every earlier load and store, so that it is
/// not read before the lock is taken: MFENCE holds RDTSC back on AMD
/// processors and LFENCE on Intel's, where Linux picks one or the other.
fn ordered_tsc() -> u64 {
    cpu::mfence();
    cpu::lfence();
    cpu::rdtsc()
}

fn tsc_hz() -> u64 {
    time::calibration().map_or(0, |calibration| calibration.tsc_hz)
}
