// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The CPUs' control of their speed (ADR 0202): CPUID says whether the
//! running CPU has Intel's HWP or AMD's CPPC, and its registers are read
//! and written here, for [`radix_pstate`], which works out their values.
//!
//! A CPU with neither runs at one of the P-states its firmware describes
//! (ADR 0796), if it has the register that chooses them: Intel's Enhanced
//! SpeedStep or AMD's hardware P-states. Each such CPU keeps its states
//! here, in a [`Table`] it shares with the CPUs whose states are the same,
//! and the kernel's request picks one of them.

use core::sync::atomic::{AtomicU8, Ordering};

use radix_hal::MAX_CPUS;
use radix_pstate::firmware::{EXT7_HW_PSTATE, LEAF1_EIST, State, Table, Vendor};
use radix_pstate::{Cpuid, Driver, LEAF6_APERF_MPERF, Levels, Msrs, Request, Scale};
use radix_sync::{Once, SpinLock};

use crate::cpu::{self, LEAF7_HYBRID, cpuid, rdmsr, wrmsr};
use crate::{percpu, time};

/// The sets of firmware P-states the CPUs have: usually one, for all.
static TABLES: [Once<Table>; 4] = [const { Once::new() }; 4];
/// Held while a CPU looks for its set among [`TABLES`] or adds it.
static ADDING: SpinLock<()> = SpinLock::new(());
/// Each CPU's set: 0 for none, else its place in [`TABLES`] plus 1.
static PICKED: [AtomicU8; MAX_CPUS] = [const { AtomicU8::new(0) }; MAX_CPUS];
/// The state each CPU was last put in, plus 1; 0 for none yet.
static RUNNING: [AtomicU8; MAX_CPUS] = [const { AtomicU8::new(0) }; MAX_CPUS];

/// The running CPU's registers of the control [`driver`] found, which
/// is all [`radix_pstate`] reads and writes.
struct Registers;

impl Msrs for Registers {
    fn read(&mut self, msr: u32) -> u64 {
        // SAFETY: a `Registers` is made only once CPUID said that the
        // running CPU has the control whose registers `radix_pstate`
        // reads, which reading does not change.
        unsafe { rdmsr(msr) }
    }

    fn write(&mut self, msr: u32, value: u64) {
        // SAFETY: as in `read`; the values turn the control on or ask the
        // CPU for a speed, which touches no memory.
        unsafe { wrmsr(msr, value) }
    }
}

/// The control the running CPU has: the one CPUID names, or else its
/// firmware's P-states, once it has taken them ([`set_firmware`]).
pub fn driver() -> Option<Driver> {
    own_control().or_else(|| table(percpu::current_index()).map(|_| Driver::Firmware))
}

/// The control CPUID says the running CPU has: HWP or CPPC.
fn own_control() -> Option<Driver> {
    let max = cpu::max_leaf();
    let leaf6 = (max >= 6).then(|| cpuid(6, 0));
    let ext8_ebx =
        if cpu::max_extended_leaf() >= 0x8000_0008 { cpuid(0x8000_0008, 0).ebx } else { 0 };
    Driver::find(&Cpuid {
        intel: cpu::is_intel(),
        amd: cpu::is_amd(),
        leaf6_eax: leaf6.map_or(0, |leaf| leaf.eax),
        ext8_ebx,
    })
}

/// Turns the running CPU's control on, if it has one; returns its levels
/// and the request the firmware left.
pub fn start() -> Option<(Levels, Request)> {
    match driver()? {
        Driver::Firmware => {
            let table = table(percpu::current_index())?;
            // SAFETY: the status register of the CPU's states, which the
            // CPU has (`set_firmware`); reading it changes nothing.
            let status = unsafe { cpu::rdmsr_safe(table.status_register()) }?;
            let levels = table.range();
            let running = table.level(table.state_of(status));
            Some((
                levels,
                Request {
                    min: levels.lowest,
                    max: levels.highest,
                    desired: running,
                    preference: 0,
                },
            ))
        }
        driver => Some(driver.start(&mut Registers)),
    }
}

/// Asks the running CPU for `request`, if it has a control. With the
/// firmware's P-states, it is put in the state [`Table::pick`] picks for
/// the request's desired level, or for its highest where it has none,
/// unless it is in that state already.
pub fn request(request: Request) {
    match driver() {
        Some(Driver::Firmware) => {
            let me = percpu::current_index();
            let Some(table) = table(me) else { return };
            let desired = if request.desired == 0 { request.max } else { request.desired };
            let state = table.pick(desired.max(request.min), request.max);
            if usize::from(RUNNING[me].load(Ordering::Relaxed)) == state + 1 {
                return;
            }
            let register = table.control_register();
            // SAFETY: the control register of the CPU's states, which the
            // CPU has; the value is one its firmware gives for a state,
            // and choosing a state touches no memory. A CPU that refuses
            // it keeps its state.
            let written = unsafe {
                cpu::rdmsr_safe(register).is_some_and(|current| {
                    cpu::wrmsr_safe(register, table.control_value(current, state))
                })
            };
            if written {
                RUNNING[me].store(state as u8 + 1, Ordering::Relaxed);
            }
        }
        Some(driver) => driver.set_request(&mut Registers, request),
        None => {}
    }
}

/// Has the running CPU take `states`, its firmware's `_PSS`, of which the
/// first `limit` are off limits (`_PPC`), if it has neither HWP nor CPPC
/// and has the register that chooses them; returns how many it took.
/// Only the first call on a CPU counts.
pub fn set_firmware(states: &[State], limit: usize) -> usize {
    let me = percpu::current_index();
    if let Some(table) = table(me) {
        return table.levels().len();
    }
    // Under a hypervisor the registers are usually not passed through,
    // and the host picks the speed.
    if cpu::feature::hypervisor() {
        return 0;
    }
    let ext7_edx =
        if cpu::max_extended_leaf() >= 0x8000_0007 { cpuid(0x8000_0007, 0).edx } else { 0 };
    let vendor = if cpu::is_intel() && cpuid(1, 0).ecx & LEAF1_EIST != 0 {
        Vendor::Intel
    } else if cpu::is_amd() && ext7_edx & EXT7_HW_PSTATE != 0 {
        Vendor::Amd
    } else {
        return 0;
    };
    if own_control().is_some() {
        return 0;
    }
    let Some(new) = Table::new(vendor, states, limit) else { return 0 };
    let _adding = ADDING.lock();
    let place = TABLES.iter().position(|kind| kind.get().is_none_or(|kind| *kind == new));
    // A fifth kind of CPU is left to its firmware.
    let Some(place) = place else { return 0 };
    if TABLES[place].get().is_none() {
        let _ = TABLES[place].set(new);
    }
    PICKED[me].store(place as u8 + 1, Ordering::Release);
    new.levels().len()
}

/// CPU `cpu`'s firmware P-states, once it has taken them.
fn table(cpu: usize) -> Option<&'static Table> {
    let picked = usize::from(PICKED.get(cpu)?.load(Ordering::Acquire));
    TABLES.get(picked.checked_sub(1)?)?.get()
}

/// The running CPU's APERF and MPERF counts, if it has the counters.
pub fn counts() -> Option<(u64, u64)> {
    if cpu::max_leaf() < 6 || cpuid(6, 0).ecx & LEAF6_APERF_MPERF == 0 {
        return None;
    }
    // SAFETY: CPUID says the CPU has both counters, which reading does
    // not change.
    Some(unsafe { (rdmsr(radix_pstate::APERF), rdmsr(radix_pstate::MPERF)) })
}

/// The frequencies of the running CPU's levels, and the level its highest
/// frequency is that of where the CPUs rank their cores.
pub fn scale(levels: Levels) -> Option<(Scale, u8)> {
    match driver()? {
        Driver::Hwp { .. } => {
            let max = cpu::max_leaf();
            let hybrid = max >= 7 && cpuid(7, 0).edx & LEAF7_HYBRID != 0;
            let base_mhz = (hybrid && max >= 0x16).then(|| cpuid(0x16, 0).eax & 0xffff);
            Some((Scale::intel(levels, base_mhz), levels.highest))
        }
        Driver::Cppc => {
            let khz = time::calibration()?.tsc_hz / 1000;
            let (family, model, _) = cpu::family_model_stepping();
            Some((Scale::amd(levels, khz), radix_pstate::amd_highest(levels, true, family, model)))
        }
        // The table's levels are 100 MHz apart on either vendor.
        Driver::Firmware => Some((Scale::intel(levels, None), levels.highest)),
    }
}
