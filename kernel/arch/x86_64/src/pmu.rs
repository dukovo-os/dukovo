// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The processor's performance counters, counting what user code makes
//! the processor do while a profile is taken (`/processes/profile`):
//! instructions, cycles, page walks after TLB misses and cache misses.
//!
//! Every CPU programs its own counters at its first sample of a profile
//! ([`tick`]), counting in user mode, but the `kernel_` events of
//! Skylake-derived cores, which count in kernel mode. At each later sample it adds
//! what they counted since the one before to its totals, and when the
//! profile stops it turns them off again. [`take`] sums the totals of
//! every CPU and starts them over, for each read of the profile, so the
//! counts of a read are those of the time since the read before, give or
//! take a sample period at either end.
//!
//! Which events are counted depends on the processor ([`plan`]):
//! instructions and cycles on any Intel processor with architectural
//! performance monitoring of version 2 or later (its fixed counters), the
//! page walks and cache misses of Intel's Skylake-derived cores (Skylake
//! to Comet Lake, the lab's OptiPlexes), and AMD's equivalents on Zen. A
//! processor or hypervisor without counters gets none. serverbench counts
//! the same events on Linux with perf, under the same names, so that the
//! two can be compared.

use core::sync::atomic::Ordering;

use radix_sync::Once;

use crate::cpu;
use crate::percpu;
use crate::smp;

/// The most events counted at once.
pub const EVENTS: usize = 10;

/// Intel's general counters' event selects and counters, fixed counters,
/// and their controls.
const INTEL_EVENT_SELECT: u32 = 0x186;
const INTEL_COUNTER: u32 = 0xc1;
const INTEL_FIXED_COUNTER: u32 = 0x309;
const INTEL_FIXED_CONTROL: u32 = 0x38d;
const INTEL_GLOBAL_CONTROL: u32 = 0x38f;

/// AMD's four legacy counters, and the six of the core extension
/// (event select and counter interleaved).
const AMD_EVENT_SELECT: u32 = 0xc001_0000;
const AMD_COUNTER: u32 = 0xc001_0004;
const AMD_CORE_EVENT_SELECT: u32 = 0xc001_0200;

/// Event select bits: count in user mode, count in kernel mode, and
/// enable.
const USER: u64 = 1 << 16;
const KERNEL: u64 = 1 << 17;
const ENABLE: u64 = 1 << 22;

/// The events of Intel's Skylake-derived cores, in the order they take
/// the general counters: what the processor has counters for is counted,
/// the first ones first. Event number, unit mask, and whether it counts
/// in kernel mode rather than user mode. The kernel's own instructions,
/// cycles and misses come first, for the rows whose time is the kernel's.
const SKYLAKE: [(&str, u16, u8, bool); 8] = [
    // INST_RETIRED.ANY_P and CPU_CLK_UNHALTED.THREAD_P in kernel mode.
    ("kernel_instructions", 0xc0, 0x00, true),
    ("kernel_cycles", 0x3c, 0x00, true),
    // L2_RQSTS.MISS: requests the L2 cache missed.
    ("kernel_l2_misses", 0x24, 0x3f, true),
    // LONGEST_LAT_CACHE.MISS: requests the last-level cache missed.
    ("kernel_llc_misses", 0x2e, 0x41, true),
    // DTLB_LOAD_MISSES.WALK_COMPLETED: loads whose translation took a
    // page walk.
    ("kernel_dtlb_load_walks", 0x08, 0x0e, true),
    ("l2_misses", 0x24, 0x3f, false),
    ("llc_misses", 0x2e, 0x41, false),
    ("dtlb_load_walks", 0x08, 0x0e, false),
];

/// Family 6 models with [`SKYLAKE`]'s events: Skylake, Kaby Lake, Coffee
/// Lake and Comet Lake, the client parts.
const SKYLAKE_MODELS: [u32; 6] = [0x4e, 0x5e, 0x8e, 0x9e, 0xa5, 0xa6];

/// The events of AMD's Zen cores (families 17h to 1Ah).
const ZEN: [(&str, u16, u8); 5] = [
    // Retired instructions and cycles not halted.
    ("instructions", 0xc0, 0x00),
    ("cycles", 0x76, 0x00),
    // L1 DTLB misses that missed the L2 TLB too: page walks.
    ("dtlb_walks", 0x45, 0xf0),
    // Instruction fetches that missed both TLBs.
    ("itlb_walks", 0x85, 0x0f),
    // L2 misses of instruction fetches and data reads.
    ("l2_misses", 0x64, 0x09),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    /// Intel fixed counter `n`.
    Fixed(u32),
    /// General counter `n` with this event select (with the mode bit,
    /// without the enable bit).
    General(u32, u64),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Intel,
    /// AMD, with the core extension's six counters or the legacy four.
    Amd {
        core: bool,
    },
}

/// What this machine counts.
struct Plan {
    kind: Kind,
    events: [Option<(&'static str, Source)>; EVENTS],
    /// Bits of the general and the fixed counters.
    general_mask: u64,
    fixed_mask: u64,
}

impl Plan {
    fn events(&self) -> impl Iterator<Item = (usize, &'static str, Source)> + '_ {
        self.events.iter().enumerate().filter_map(|(at, event)| event.map(|(n, s)| (at, n, s)))
    }
}

static PLAN: Once<Plan> = Once::new();

fn plan() -> &'static Plan {
    PLAN.call_once(|| {
        let mut plan = Plan {
            kind: Kind::Intel,
            events: [None; EVENTS],
            general_mask: u64::MAX,
            fixed_mask: u64::MAX,
        };
        if cpu::is_intel() {
            intel(&mut plan);
        } else if cpu::is_amd() {
            amd(&mut plan);
        }
        plan
    })
}

fn width_mask(bits: u32) -> u64 {
    if bits == 0 || bits >= 64 { u64::MAX } else { (1 << bits) - 1 }
}

fn intel(plan: &mut Plan) {
    if cpu::max_leaf() < 0xa {
        return;
    }
    let leaf = cpu::cpuid(0xa, 0);
    // Version 2 brought the fixed counters and the global control.
    if leaf.eax & 0xff < 2 {
        return;
    }
    let general = (leaf.eax >> 8) & 0xff;
    let fixed = leaf.edx & 0x1f;
    plan.general_mask = width_mask((leaf.eax >> 16) & 0xff);
    plan.fixed_mask = width_mask((leaf.edx >> 5) & 0xff);
    // A hypervisor may say it has counters it does not emulate: one that
    // faults on a read of a control is taken at its word that it has none.
    // SAFETY: reading the controls changes nothing.
    let fixed_ok = fixed >= 2 && unsafe { cpu::rdmsr_safe(INTEL_FIXED_CONTROL) }.is_some();
    // SAFETY: as above.
    let general_ok = unsafe { cpu::rdmsr_safe(INTEL_EVENT_SELECT) }.is_some()
        && unsafe { cpu::rdmsr_safe(INTEL_GLOBAL_CONTROL) }.is_some();
    if !general_ok {
        return;
    }
    let mut at = 0;
    // Fixed counter 0 counts retired instructions and 1 unhalted cycles;
    // EBX bit 1 and bit 0 set would say that they do not.
    if fixed_ok && leaf.ebx & 0b10 == 0 {
        plan.events[at] = Some(("instructions", Source::Fixed(0)));
        at += 1;
    }
    if fixed_ok && leaf.ebx & 0b01 == 0 {
        plan.events[at] = Some(("cycles", Source::Fixed(1)));
        at += 1;
    }
    let (family, model, _) = cpu::family_model_stepping();
    if family == 6 && SKYLAKE_MODELS.contains(&model) {
        for (n, &(name, event, mask, kernel)) in SKYLAKE.iter().take(general as usize).enumerate() {
            if at == EVENTS {
                break;
            }
            let mode = if kernel { KERNEL } else { USER };
            let select = u64::from(event & 0xff) | (u64::from(mask) << 8) | mode;
            plan.events[at] = Some((name, Source::General(n as u32, select)));
            at += 1;
        }
    }
}

fn amd(plan: &mut Plan) {
    let (family, _, _) = cpu::family_model_stepping();
    if !(0x17..=0x1a).contains(&family) {
        return;
    }
    let core =
        cpu::max_extended_leaf() >= 0x8000_0001 && cpu::cpuid(0x8000_0001, 0).ecx & (1 << 23) != 0;
    plan.kind = Kind::Amd { core };
    plan.general_mask = width_mask(48);
    let first = if core { AMD_CORE_EVENT_SELECT } else { AMD_EVENT_SELECT };
    // SAFETY: reading an event select changes nothing; under a hypervisor
    // without counters it faults, and none are used.
    if unsafe { cpu::rdmsr_safe(first) }.is_none() {
        return;
    }
    let counters = if core { 6 } else { 4 };
    for (n, &(name, event, mask)) in ZEN.iter().take(counters).enumerate() {
        let select =
            u64::from(event & 0xff) | (u64::from(mask) << 8) | (u64::from(event >> 8) << 32) | USER;
        plan.events[n] = Some((name, Source::General(n as u32, select)));
    }
}

fn select_msr(kind: Kind, n: u32) -> u32 {
    match kind {
        Kind::Intel => INTEL_EVENT_SELECT + n,
        Kind::Amd { core: true } => AMD_CORE_EVENT_SELECT + 2 * n,
        Kind::Amd { core: false } => AMD_EVENT_SELECT + n,
    }
}

fn counter_msr(kind: Kind, source: Source) -> u32 {
    match (kind, source) {
        (_, Source::Fixed(n)) => INTEL_FIXED_COUNTER + n,
        (Kind::Intel, Source::General(n, _)) => INTEL_COUNTER + n,
        (Kind::Amd { core: true }, Source::General(n, _)) => AMD_CORE_EVENT_SELECT + 2 * n + 1,
        (Kind::Amd { core: false }, Source::General(n, _)) => AMD_COUNTER + n,
    }
}

fn read(plan: &Plan, source: Source) -> u64 {
    // SAFETY: `plan` lists only counters the processor has (checked when
    // it was made); reading one changes nothing.
    unsafe { cpu::rdmsr(counter_msr(plan.kind, source)) }
}

fn mask(plan: &Plan, source: Source) -> u64 {
    match source {
        Source::Fixed(_) => plan.fixed_mask,
        Source::General(..) => plan.general_mask,
    }
}

/// Programs this CPU's counters for `plan`, or turns them off, and takes
/// where they stand.
fn program(plan: &Plan, on: bool) {
    let here = percpu::current();
    let mut fixed_control = 0u64;
    let mut global = 0u64;
    for (_, _, source) in plan.events() {
        match source {
            Source::Fixed(n) => {
                // Two bits a counter: 2 counts in user mode.
                fixed_control |= 2 << (4 * n);
                global |= 1 << (32 + n);
            }
            Source::General(n, select) => {
                let value = if on { select | ENABLE } else { 0 };
                // SAFETY: the plan lists only counters the processor has;
                // counting changes nothing the kernel relies on, and no
                // other part of the kernel uses them.
                unsafe { cpu::wrmsr(select_msr(plan.kind, n), value) };
                global |= 1 << n;
            }
        }
    }
    if plan.kind == Kind::Intel {
        // SAFETY: as above; version 2 has both controls (checked by the
        // plan).
        unsafe {
            if fixed_control != 0 {
                cpu::wrmsr(INTEL_FIXED_CONTROL, if on { fixed_control } else { 0 });
            }
            cpu::wrmsr(INTEL_GLOBAL_CONTROL, if on { global } else { 0 });
        }
    }
    for (at, _, source) in plan.events() {
        here.pmu_last[at].store(read(plan, source), Ordering::Relaxed);
    }
}

/// This CPU's part, at each sample of a profile (`sampling`) and at its
/// first timer interrupt after one: starts the counters at the first
/// sample, adds what they counted to this CPU's totals at the others, and
/// stops them afterwards. Called with interrupts masked.
pub fn tick(sampling: bool) {
    let plan = plan();
    if plan.events[0].is_none() {
        return;
    }
    let here = percpu::current();
    let on = here.pmu_on.load(Ordering::Relaxed);
    match (sampling, on) {
        (true, false) => {
            program(plan, true);
            here.pmu_on.store(true, Ordering::Relaxed);
        }
        (true, true) => {
            for (at, _, source) in plan.events() {
                let now = read(plan, source);
                let last = here.pmu_last[at].swap(now, Ordering::Relaxed);
                let counted = now.wrapping_sub(last) & mask(plan, source);
                here.pmu_counts[at].fetch_add(counted, Ordering::Relaxed);
            }
        }
        (false, true) => {
            program(plan, false);
            here.pmu_on.store(false, Ordering::Relaxed);
        }
        (false, false) => {}
    }
}

/// Calls `each` with every event counted and its count over all CPUs
/// since the last call, and starts the counts over.
pub fn take(each: &mut dyn FnMut(&'static str, u64)) {
    let plan = plan();
    for (at, name, _) in plan.events() {
        let total = (0..smp::count())
            .filter_map(percpu::get)
            .map(|cpu| cpu.pmu_counts[at].swap(0, Ordering::Relaxed))
            .sum();
        each(name, total);
    }
}
