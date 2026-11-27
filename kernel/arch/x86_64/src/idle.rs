// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! How an idle CPU waits (ADR 0795): its first state, MWAIT's C1 or HLT,
//! and the deeper states the firmware's `_CST` describes, entered with an
//! MWAIT hint or a read of an I/O port.
//!
//! A deeper state is used only if the CPU can enter it and still keeps
//! time: an MWAIT hint the processor lists among its C-states (CPUID leaf
//! 5), and a local APIC timer that runs in every C-state (ARAT) and an
//! invariant TSC, since a deeper state may stop both and the kernel keeps
//! no other clock running for it. Every processor of the last fifteen
//! years has both. An I/O-port state of type 3 asks the kernel to stop
//! bus masters around it, which it does not, so it is left out, except on
//! AMD's processors since Zen, whose caches stay coherent in it without
//! that; so are I/O ports under a hypervisor. A state MWAIT enters still
//! ends on a write to the watched line, so a waker need not send an
//! interrupt; one an I/O port enters does not, and the scheduler then asks
//! wakers for one.
//!
//! On Intel's processors a C1 request may be promoted to C1E by the
//! processor itself, which costs a C1 that should be short several
//! microseconds. Where the firmware lists C1E as a state of its own, the
//! governor picks it when it pays, and the promotion is turned off.

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};

use radix_hal::MAX_CPUS;
use radix_hal::idle::{Entry, FirmwareState, IdleState, NAMES};
use radix_sync::{Once, SpinLock};

use crate::{cpu, percpu, speculation, time};

/// Whether the first state is MWAIT's C1, rather than HLT.
static MWAIT: AtomicBool = AtomicBool::new(false);
/// Whether only the first state is used (`idle=halt`).
static FIRST_ONLY: AtomicBool = AtomicBool::new(false);
/// The most states a CPU uses (`max_cstate`).
static MOST: AtomicUsize = AtomicUsize::new(NAMES.len());

/// The most states a CPU has.
const MAX: usize = NAMES.len();

/// IA32_POWER_CTL, Intel's, whose bit 1 promotes C1 to C1E.
const POWER_CTL: u32 = 0x1fc;
const C1E_PROMOTION: u64 = 1 << 1;

/// The first state, MWAIT's C1.
static MWAIT_C1: IdleState =
    IdleState { name: "C1", how: Entry::Mwait(0), exit_ns: 0, residency_ns: 0, watches: true };

/// The first state, HLT.
static HALT: IdleState =
    IdleState { name: "C1", how: Entry::Halt, exit_ns: 0, residency_ns: 0, watches: false };

/// A set of states, which every CPU that has the same uses.
#[derive(PartialEq)]
struct Table {
    states: [IdleState; MAX],
    count: usize,
}

/// The sets of states the CPUs have: usually one, for all of them.
static TABLES: [Once<Table>; 4] = [const { Once::new() }; 4];
/// Held while a CPU looks for its set among [`TABLES`] or adds it.
static ADDING: SpinLock<()> = SpinLock::new(());
/// Each CPU's set: 0 for none yet, else its place in [`TABLES`] plus 1.
static PICKED: [AtomicU8; MAX_CPUS] = [const { AtomicU8::new(0) }; MAX_CPUS];

/// Chooses the first state from the processor and the command line's
/// `idle` and `max_cstate`. Called once, by the boot CPU.
pub fn init(idle: Option<&str>, max_cstate: Option<&str>) {
    // Under a hypervisor MWAIT is usually not passed through, and when it
    // is emulated it may spin; HLT lets the host run something else.
    let mwait = cpu::feature::monitor() && !cpu::feature::hypervisor();
    MWAIT.store(mwait && !matches!(idle, Some("halt" | "nomwait")), Ordering::Relaxed);
    FIRST_ONLY.store(idle == Some("halt"), Ordering::Relaxed);
    if let Some(most) = max_cstate.and_then(|value| value.parse::<usize>().ok()) {
        MOST.store(most.clamp(1, MAX), Ordering::Relaxed);
    }
}

/// Whether the first state is MWAIT's C1.
pub fn mwait() -> bool {
    MWAIT.load(Ordering::Relaxed)
}

/// CPU `cpu`'s states, shallowest first.
pub fn states(cpu: usize) -> &'static [IdleState] {
    match table(cpu) {
        Some(table) => &table.states[..table.count],
        None if mwait() => core::slice::from_ref(&MWAIT_C1),
        None => core::slice::from_ref(&HALT),
    }
}

/// CPU `cpu`'s set of states, once it has one.
fn table(cpu: usize) -> Option<&'static Table> {
    let picked = usize::from(PICKED.get(cpu)?.load(Ordering::Acquire));
    TABLES.get(picked.checked_sub(1)?)?.get()
}

/// Keeps the states of `firmware` that this CPU can use, behind its first,
/// sorted by exit latency, and returns how many it kept. Only the first
/// call on a CPU counts.
pub fn set(firmware: &[FirmwareState]) -> usize {
    let me = percpu::current_index();
    if let Some(table) = table(me) {
        return table.count - 1;
    }
    let first = if mwait() { MWAIT_C1 } else { HALT };
    let mut table = Table { states: [first; MAX], count: 1 };
    let most = if FIRST_ONLY.load(Ordering::Relaxed) { 1 } else { MOST.load(Ordering::Relaxed) };
    let keeps_time = cpu::feature::arat() && cpu::feature::invariant_tsc();
    let coherent_c3 = cpu::is_amd() && cpu::family_model_stepping().0 >= 0x17;
    for state in firmware {
        if table.count >= most {
            break;
        }
        let usable = match state.how {
            Entry::Mwait(hint) => mwait() && hint != 0 && hint_listed(hint),
            Entry::Port(_) => (state.kind < 3 || coherent_c3) && !cpu::feature::hypervisor(),
            // The first state is C1 already.
            Entry::Halt | Entry::Wait => false,
        };
        if !usable || !keeps_time {
            continue;
        }
        let exit_ns = u64::from(state.latency_us) * 1_000;
        table.states[table.count] = IdleState {
            name: NAMES[table.count],
            how: state.how,
            exit_ns,
            // Three times what leaving it takes: entering takes about as
            // long, and the state must save what both cost.
            residency_ns: exit_ns * 3,
            watches: matches!(state.how, Entry::Mwait(_)),
        };
        table.count += 1;
    }
    let deeper = &mut table.states[1..table.count];
    // By exit latency, keeping the firmware's order where they are equal.
    for end in 1..deeper.len() {
        let mut at = end;
        while at > 0 && deeper[at - 1].exit_ns > deeper[at].exit_ns {
            deeper.swap(at - 1, at);
            at -= 1;
        }
    }
    for (place, state) in deeper.iter_mut().enumerate() {
        state.name = NAMES[place + 1];
    }
    // An MWAIT hint of C1 with a sub-state is C1E.
    let c1e = deeper.iter().any(|state| matches!(state.how, Entry::Mwait(hint) if hint >> 4 == 0));
    if c1e && cpu::is_intel() {
        // SAFETY: an MSR of Intel's processors since Nehalem; a processor
        // without it faults, which the safe access turns into `None`. The
        // bit only stops the processor turning C1 into C1E.
        unsafe {
            if let Some(control) = cpu::rdmsr_safe(POWER_CTL)
                && control & C1E_PROMOTION != 0
            {
                cpu::wrmsr_safe(POWER_CTL, control & !C1E_PROMOTION);
            }
        }
    }
    let count = table.count;
    let _adding = ADDING.lock();
    let place = TABLES.iter().position(|kind| kind.get().is_none_or(|kind| *kind == table));
    // A fifth kind of CPU keeps its first state alone.
    if let Some(place) = place {
        if TABLES[place].get().is_none() {
            let _ = TABLES[place].set(table);
        }
        PICKED[me].store(place as u8 + 1, Ordering::Release);
        return count - 1;
    }
    0
}

/// Whether the processor lists the C-state and sub-state of MWAIT hint
/// `hint` (CPUID leaf 5, EDX: four bits of sub-state count for each
/// C-state, C0 first).
fn hint_listed(hint: u32) -> bool {
    if cpu::max_leaf() < 5 {
        return false;
    }
    let cstate = ((hint >> 4) & 0xf) + 1;
    let substate = hint & 0xf;
    cstate < 8 && (cpu::cpuid(5, 0).edx >> (cstate * 4)) & 0xf > substate
}

/// Waits in state `state` of this CPU's [`states`]; see
/// `radix_hal::Cpu::idle_until`.
pub fn until(wake: &AtomicUsize, state: usize) {
    time::doorbell_off();
    let states = states(percpu::current_index());
    let how = states.get(state).unwrap_or(&states[0]).how;
    match how {
        Entry::Mwait(hint) => {
            cpu::monitor(core::ptr::from_ref::<AtomicUsize>(wake) as usize);
            // A write between the check and MWAIT still ends the wait: the
            // monitor is armed already.
            if wake.load(Ordering::Acquire) & 1 == 0 {
                speculation::before_idle();
                cpu::sti_mwait(hint);
            }
        }
        Entry::Port(port) => {
            if wake.load(Ordering::Acquire) & 1 == 0 {
                speculation::before_idle();
                // SAFETY: the firmware names this port as the one whose
                // read puts the CPU in the state; reading it changes
                // nothing else. An interrupt ends the state even while
                // they are masked.
                unsafe { crate::io::inb(port) };
            }
            // The interrupt that ended it, handled.
            cpu::interrupt_window();
        }
        Entry::Halt | Entry::Wait => {
            if wake.load(Ordering::Acquire) & 1 == 0 {
                speculation::before_idle();
                cpu::sti_hlt();
            }
        }
    }
    cpu::disable_interrupts();
}
