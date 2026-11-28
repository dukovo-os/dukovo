// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The processors' objects in the ACPI namespace, and the idle states
//! their `_CST` describes (ADR 0795).
//!
//! Each CPU's object is the one whose processor ID, or `_UID`, the
//! interrupt controller table gave the CPU. Before reading anything from
//! them, the kernel tells the firmware what it can do with Intel's
//! processor `_OSC`, or `_PDC` where that is missing: enter C1 with HLT
//! and deeper states with MWAIT, and keep each CPU's states its own. Many
//! firmwares load the tables that describe the deeper states only then.
//! Each CPU then takes the states it can use (`Devices::set_idle_states`),
//! on itself, and the scheduler's governor picks among them.
//!
//! The same objects' `_PSS` lists the CPU's performance states, which a
//! CPU without HWP or CPPC takes too (`Devices::set_performance_states`,
//! ADR 0796), where `_PCT` says they are chosen through the processor's
//! own register; `_PPC` keeps the fastest few off limits.

use alloc::vec::Vec;
use core::fmt::Write;

use radix_acpi::processor::{self as firmware, CState};
use radix_acpi::resources::{SPACE_FUNCTIONAL_FIXED_HW, generic_register};
use radix_hal::idle::{Entry, FirmwareState, NAMES};
use radix_hal::{Cpu, CpuLocal, Devices, Smp};
use radix_klog::{TextBuf, info};
use radix_mm::Kmalloc;
use radix_pstate::firmware::{MAX_STATES as MOST_PSTATES, State};
use radix_sync::Once;
use radix_uacpi::Node;
use radix_uacpi::power::Fields;

use crate::Arch;

/// The most states read from one `_CST`.
const MOST: usize = NAMES.len();

/// The states one CPU's firmware describes.
#[derive(Clone, Copy)]
struct Found {
    states: [FirmwareState; MOST],
    count: usize,
    /// Its performance states, fastest first, where the processor's own
    /// register chooses them.
    pstates: [State; MOST_PSTATES],
    pcount: usize,
    /// How many of the fastest are off limits (`_PPC`).
    limit: usize,
}

impl Found {
    const NONE: Found = Found {
        states: [FirmwareState { how: Entry::Halt, kind: 1, latency_us: 0 }; MOST],
        count: 0,
        pstates: [State { mhz: 0, control: 0 }; MOST_PSTATES],
        pcount: 0,
        limit: 0,
    };
}

/// The states each CPU's firmware describes, by CPU.
static FOUND: Once<Vec<Found, Kmalloc>> = Once::new();

/// What the kernel tells the firmware it can do: C1 with HLT, deeper
/// states each CPU enters on its own, and, with MWAIT, C1 and the deeper
/// states with MWAIT hints.
fn capabilities(mwait: bool) -> u32 {
    let common = firmware::CAP_C1_HALT
        | firmware::CAP_SMP_C1_THROTTLING_INDEPENDENT
        | firmware::CAP_SMP_C2_C3_INDEPENDENT;
    if mwait { common | firmware::CAP_C1_NATIVE | firmware::CAP_C2_C3_NATIVE } else { common }
}

/// Reads every CPU's idle states from its processor object and has each
/// CPU take those it can use. Called once, after uACPI has started, from
/// a thread.
pub fn start() {
    if !super::running() {
        return;
    }
    let cpus = Arch::cpu_count();
    let mut objects = Vec::new_in(super::MAY_SLEEP);
    let mut found = Vec::new_in(super::MAY_SLEEP);
    if objects.try_reserve_exact(cpus).is_err() || found.try_reserve_exact(cpus).is_err() {
        return;
    }
    let (mwait, _) = Arch::idle_mode();
    radix_uacpi::for_each_processor(|node, id| {
        let cpu = (0..cpus).find(|&cpu| Arch::acpi_processor_id(cpu) == Some(id));
        if let Some(cpu) = cpu
            && !objects.iter().any(|&(other, _)| other == cpu)
        {
            let _ = objects.push_within_capacity((cpu, node));
        }
    });
    // Every object hears what the kernel can do before any is read: the
    // firmware may load the tables of the deeper states as it hears it.
    for &(_, node) in &objects {
        declare(node, capabilities(mwait));
    }
    for cpu in 0..cpus {
        let node = objects.iter().find(|&&(other, _)| other == cpu).map(|&(_, node)| node);
        let _ = found.push_within_capacity(node.map_or(Found::NONE, read));
    }
    let _ = FOUND.set(found);
    let me = Arch::current_cpu();
    crate::pstate::on_every_cpu(take);
    report(me, objects.len());
}

/// Has the running CPU take the states found for it.
fn take() {
    let me = Arch::current_cpu();
    if let Some(found) = FOUND.get().and_then(|found| found.get(me)) {
        Arch::set_idle_states(&found.states[..found.count]);
        Arch::set_performance_states(&found.pstates[..found.pcount], found.limit);
    }
}

/// What processor object `node` describes: its idle states, and its
/// performance states where its own register chooses them.
fn read(node: Node) -> Found {
    let mut found = states(node);
    if own_registers(node) {
        let read = |pss: &radix_uacpi::Package<'_>| {
            let (mut pstates, mut count) = ([State::default(); MOST_PSTATES], 0);
            for index in 0..pss.len().min(MOST_PSTATES) {
                // A state the firmware gets wrong ends the list: the ones
                // after it would be out of place.
                let Some(Some(state)) = pss.package(index, pstate) else { break };
                pstates[count] = state;
                count += 1;
            }
            (pstates, count)
        };
        if let Ok((pstates, count)) = node.package(c"_PSS", read) {
            found.pstates = pstates;
            found.pcount = count;
            // No `_PPC`: none is off limits.
            found.limit = node
                .integer(c"_PPC")
                .map_or(0, |limit| usize::try_from(limit).unwrap_or(usize::MAX));
        }
    }
    found
}

/// Whether `_PCT` of processor object `node` names functional fixed
/// hardware for both its control and its status register: the
/// processor's own registers, which `_PSS`'s values are written to.
fn own_registers(node: Node) -> bool {
    let read = |pct: &radix_uacpi::Package<'_>| {
        (0..2).all(|index| {
            pct.text(index)
                .and_then(generic_register)
                .is_some_and(|register| register.space == SPACE_FUNCTIONAL_FIXED_HW)
        })
    };
    node.package(c"_PCT", read).unwrap_or(false)
}

/// One `_PSS` entry: its frequency, power, latencies, control and status.
fn pstate(fields: &radix_uacpi::Package<'_>) -> Option<State> {
    let mut values = [0; 6];
    for (index, value) in values.iter_mut().enumerate() {
        *value = fields.integer(index)?;
    }
    let state = firmware::pstate(values)?;
    Some(State { mhz: state.mhz, control: state.control })
}

/// Tells the firmware of processor object `node` what the kernel can do:
/// through `_OSC`, or `_PDC` where it has none.
fn declare(node: Node, capabilities: u32) {
    // No query: the capabilities are what the kernel will use.
    let mut dwords = [0, capabilities];
    if node.osc(&firmware::OSC_UUID, firmware::OSC_REVISION, &mut dwords).is_ok() {
        return;
    }
    // `_PDC`'s buffer: its revision, its count, then the capabilities.
    let mut buffer = [0u8; 12];
    for (bytes, dword) in buffer.as_chunks_mut::<4>().0.iter_mut().zip([1, 1, capabilities]) {
        bytes.copy_from_slice(&dword.to_le_bytes());
    }
    let _ = node.run_with_buffer(c"_PDC", &buffer);
}

/// The idle states the `_CST` of processor object `node` describes, in
/// its order; none if it has none.
fn states(node: Node) -> Found {
    let read = |cst: &radix_uacpi::Package<'_>| {
        let mut found = Found::NONE;
        // A count, then a package for each state.
        for index in 1..cst.len() {
            if found.count == MOST {
                break;
            }
            if let Some(Some(state)) = cst.package(index, entry) {
                found.states[found.count] = state;
                found.count += 1;
            }
        }
        found
    };
    node.package(c"_CST", read).unwrap_or(Found::NONE)
}

/// One `_CST` entry: the register that enters it, its type, its latency
/// and its power.
fn entry(fields: &radix_uacpi::Package<'_>) -> Option<FirmwareState> {
    let state = firmware::cstate(
        fields.text(0)?,
        fields.integer(1)?,
        fields.integer(2)?,
        fields.integer(3)?,
    )?;
    Some(hal_state(state))
}

fn hal_state(state: CState) -> FirmwareState {
    let how = match state.entry {
        firmware::Entry::Halt => Entry::Halt,
        firmware::Entry::Mwait { hint, .. } => Entry::Mwait(hint),
        firmware::Entry::Port(port) => Entry::Port(port),
    };
    FirmwareState { how, kind: state.kind, latency_us: state.latency_us }
}

/// Says in the log what CPU `cpu`, which stands for the others, waits in.
fn report(cpu: usize, objects: usize) {
    let states = Arch::idle_states(cpu);
    if states.len() <= 1 {
        info!(
            "Idle: C1 only; the firmware describes no deeper state the CPUs can use \
             ({objects} processor objects)"
        );
        return;
    }
    let mut line = TextBuf::<240>::new();
    for state in states {
        let comma = if line.is_empty() { "" } else { ", " };
        let _ = write!(line, "{comma}{} ({}, {} µs)", state.name, state.how, state.exit_ns / 1_000);
    }
    info!("Idle: {}", line.as_str());
}
