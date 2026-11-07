// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the locks need from the kernel (`radix_sync::Hooks`), and the
//! lock-order checker (`radix-lockdep`) they feed.
//!
//! Spin locks hold off preemption through the CPU's preemption count, and
//! letting go of the last one switches threads if a more urgent one waits.
//! A contended mutex sleeps in the wait table. Every acquisition goes to
//! the checker when it is on: by default in debug builds, and in any build
//! with the `lockdep` flag (`lockdep=off` turns it off). The checker runs
//! with interrupts off, and a report is logged as an error with the lines
//! `Lock checker: ...`, after which the checker turns itself off (one
//! report is trustworthy; the next ones may follow from the first).

use core::fmt;
use core::sync::atomic::{AtomicBool, Ordering};

use radix_hal::{Cpu, CpuLocal};
use radix_klog::error;
use radix_lockdep::{Checker, Context, Report};
use radix_sync::{Acquire, Hooks, LockSite, RawMutex};

use super::thread::ThreadRef;
use crate::Arch;

/// CPUs the checker follows; locks taken on CPUs beyond are not checked.
const CHECKED_CPUS: usize = 64;

static CHECKER: Checker<CHECKED_CPUS> = Checker::new();
/// Whether the checker should be on once the hooks are registered.
static WANTED: AtomicBool = AtomicBool::new(false);

struct KernelHooks;

static HOOKS: KernelHooks = KernelHooks;

/// Registers the hooks. With `lockdep`, the checker starts on.
pub(super) fn init(lockdep: bool) {
    WANTED.store(lockdep, Ordering::Relaxed);
    CHECKER.set_reporter(report);
    set_checker(lockdep);
    radix_sync::set_hooks(&HOOKS);
}

/// Counts a reference to a value counted on the CPUs, such as a mount
/// (`radix_sync::cpu_ref`, ADR 0404), on the running CPU. Every open and
/// close of a file does it once, and fat LTO inlines it there.
///
/// Interrupts are held off around the count, not preemption: no timer
/// tick, switch or idle can come between, so no grace period passes on
/// the CPU (`super::rcu`), and it is the RCU read section the count needs.
/// Letting interrupts in again needs no check for a reschedule, as
/// letting preemption back does: nothing in between asks for one, and one
/// another CPU asks for comes as the interrupt that was held off.
// `radix_sync::cpu_ref` declares this symbol with the same signature, so
// exporting it unmangled is sound.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "Rust" fn radix_count_here(count: &radix_sync::cpu_ref::CpuCount, delta: u32) -> bool {
    let were = Arch::disable_interrupts();
    let counted = radix_sync::cpu_ref::count_on(Arch::current_cpu(), count, delta);
    Arch::restore_interrupts(were);
    counted
}

// Each CPU's state that `radix_sync::cpu` keeps is picked by the CPU's
// number modulo its `CPUS`: no two CPUs may share it.
const _: () = assert!(radix_sync::cpu::CPUS >= radix_hal::MAX_CPUS);

/// Holds the running CPU for state it keeps for itself
/// (`radix_sync::cpu::HeldCpu`, ADR 0405): interrupts go off, so nothing
/// else runs on the CPU, and no thread moves off it, until
/// [`radix_let_go_cpu`]. Gives the CPU's number shifted up one, with bit 0
/// set if interrupts were on. Every open and close of a file does it, and
/// fat LTO inlines it there.
// `radix_sync::cpu` declares this symbol with the same signature, so
// exporting it unmangled is sound.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "Rust" fn radix_hold_cpu() -> usize {
    let were = Arch::disable_interrupts();
    (Arch::current_cpu() << 1) | usize::from(were)
}

/// Lets go of the CPU [`radix_hold_cpu`] held: interrupts come back on if
/// they were. As in [`radix_count_here`], nothing held off can have asked
/// for a reschedule but the interrupt that brings it.
// `radix_sync::cpu` declares this symbol with the same signature, so
// exporting it unmangled is sound.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "Rust" fn radix_let_go_cpu(were: bool) {
    Arch::restore_interrupts(were);
}

/// Whether the lock checker is on.
pub fn lockdep_enabled() -> bool {
    CHECKER.enabled()
}

/// How many problems the checker has reported.
pub fn lockdep_reports() -> u32 {
    CHECKER.reports()
}

/// The last problem the checker reported, if any.
pub fn lockdep_last_problem() -> Option<radix_lockdep::Problem> {
    CHECKER.last_problem()
}

/// For the self-test: whether the checker should keep going after a
/// report (and turn itself on for that).
pub fn set_lockdep_expecting(expecting: bool) {
    CHECKER.expect_reports(expecting);
    set_checker(expecting || WANTED.load(Ordering::Relaxed));
}

/// Turns the checker on or off, and the locks' reports to it with it.
fn set_checker(on: bool) {
    CHECKER.set_enabled(on);
    radix_sync::set_checking(on);
}

fn report(report: &Report) {
    report.write_lines(&mut |line| error!("{line}"));
}

fn context(irqs_enabled: bool) -> Context {
    Context {
        cpu: Arch::current_cpu(),
        irq_depth: Arch::interrupt_depth().min(u32::from(u8::MAX)) as u8,
        irqs_enabled,
    }
}

/// The running thread is being switched out on `cpu`: the mutexes it holds
/// go with it.
pub(super) fn switch_out(cpu: usize, thread: &ThreadRef) {
    if CHECKER.enabled() {
        CHECKER.switch_out(cpu, &mut thread.lockdep.lock());
    }
}

/// `thread` runs on `cpu` again: its mutexes come back to the CPU.
pub(super) fn switch_in(cpu: usize, thread: &ThreadRef) {
    if CHECKER.enabled() {
        CHECKER.switch_in(cpu, &mut thread.lockdep.lock());
    }
}

impl Hooks for KernelHooks {
    fn disable_interrupts(&self) -> bool {
        Arch::disable_interrupts()
    }

    fn restore_interrupts(&self, were_enabled: bool) {
        Arch::restore_interrupts(were_enabled);
    }

    fn preempt_disable(&self) {
        Arch::preempt_disable();
    }

    fn preempt_enable(&self) {
        super::preempt_enable();
    }

    fn lock_acquire(&self, lock: Acquire<'_>) {
        if !CHECKER.enabled() {
            return;
        }
        let were = Arch::disable_interrupts();
        CHECKER.acquire(context(were), &lock);
        Arch::restore_interrupts(were);
    }

    fn lock_release(&self, _site: &LockSite, addr: usize) {
        if !CHECKER.enabled() {
            return;
        }
        let were = Arch::disable_interrupts();
        CHECKER.release(Arch::current_cpu(), addr);
        Arch::restore_interrupts(were);
    }

    fn current_thread(&self) -> usize {
        super::current_token()
    }

    fn current_cpu(&self) -> usize {
        Arch::current_cpu()
    }

    fn mutex_wait(&self, mutex: &RawMutex) {
        super::wait::mutex_wait(mutex);
    }

    fn mutex_wake(&self, mutex: &RawMutex) {
        super::wait::mutex_wake(mutex);
    }

    fn synchronize_rcu(&self) {
        super::rcu::synchronize();
    }

    fn try_synchronize_rcu(&self) -> bool {
        super::rcu::try_synchronize()
    }

    #[allow(unsafe_code)]
    unsafe fn defer_rcu(&self, item: core::ptr::NonNull<radix_sync::rcu::Deferred>) {
        // SAFETY: the caller's guarantee.
        unsafe { super::rcu::defer(item) }
    }
}

/// Writes the checker's state, for `kdb`.
pub fn write_lockdep(line: &mut dyn FnMut(fmt::Arguments<'_>)) {
    let stats = CHECKER.stats();
    line(format_args!(
        "Lock checker: {}; {} lock classes, {} lock-order edges, {} problems reported{}",
        if stats.enabled { "on" } else { "off" },
        stats.classes,
        stats.edges,
        stats.reports,
        match CHECKER.last_problem() {
            Some(problem) => match problem {
                radix_lockdep::Problem::Inversion => " (last: lock-order inversion)",
                radix_lockdep::Problem::Recursion => " (last: recursive locking)",
                radix_lockdep::Problem::IrqUnsafe => " (last: interrupt-unsafe lock)",
                radix_lockdep::Problem::IrqInversion => " (last: interrupt-unsafe lock order)",
                radix_lockdep::Problem::SleepInAtomic => " (last: sleeping where it may not)",
                radix_lockdep::Problem::OutOfRoom => " (last: out of room)",
            },
            None => "",
        }
    ));
    CHECKER.classes(|class| {
        line(format_args!(
            "  #{:<3} {} {}{}{}{}",
            class.id,
            if class.kind == radix_sync::LockKind::Spin { "spin " } else { "mutex" },
            class.created,
            if class.subclass > 0 { " (nested)" } else { "" },
            if class.in_irq { ", in interrupts" } else { "" },
            if class.irqs_on { ", with interrupts on" } else { "" }
        ));
    });
    for cpu in 0..CHECKED_CPUS.min(<Arch as radix_hal::Smp>::cpu_count()) {
        let mut first = true;
        CHECKER.held(cpu, |held| {
            if first {
                line(format_args!("  cpu{cpu} holds:"));
                first = false;
            }
            line(format_args!(
                "    {} {} (taken at {}{})",
                if held.kind == radix_sync::LockKind::Spin { "spin lock" } else { "mutex" },
                held.created,
                held.at,
                if held.context > 0 { ", in an interrupt" } else { "" }
            ));
        });
    }
}
