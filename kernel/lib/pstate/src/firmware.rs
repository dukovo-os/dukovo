// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Speed control through the P-states the firmware describes (ADR 0796),
//! for CPUs that have neither HWP nor CPPC: Intel's from before Skylake,
//! and Skylake's and later's whose firmware keeps HWP off; AMD's Zen 1
//! and 2, and later ones whose firmware does not offer CPPC.
//!
//! The firmware's `_PSS` lists the states, fastest first, each with its
//! frequency and the value that chooses it, and `_PCT` says where that
//! value goes: on Intel's processors with Enhanced SpeedStep, the low 16
//! bits of IA32_PERF_CTL, whose bits 15:8 are the state's bus ratio; on
//! AMD's with hardware P-states, the P-state's number, in
//! MSRC001_0062. `_PPC` keeps the fastest few off limits.
//!
//! Such a CPU runs at the state it was last asked for, so the kernel picks
//! one itself. Levels are 100 MHz apart, as with Intel's HWP: an Intel
//! state's level is its ratio, an AMD state's its frequency in hundreds of
//! megahertz. [`Table`] keeps a CPU's states by level; [`demand`] works out
//! the level to ask for from how busy the CPU was and how fast it ran.
//!
//! References: ACPI 6.5, 8.4.5 (`_PCT`, `_PSS`, `_PPC`); Intel's SDM,
//! volume 3, section 15.3 (Enhanced Intel SpeedStep Technology); AMD's
//! Processor Programming Reference for family 17h (MSRC001_0061 to
//! MSRC001_0063).

use crate::Levels;

/// IA32_PERF_STATUS: bits 15:0 are the state the CPU is in.
pub const PERF_STATUS: u32 = 0x198;
/// IA32_PERF_CTL: bits 15:0 choose the state.
pub const PERF_CTL: u32 = 0x199;
/// MSRC001_0062, P-state control: the number of the P-state to run at.
pub const PSTATE_CTL: u32 = 0xc001_0062;
/// MSRC001_0063, P-state status: the number of the one it runs at.
pub const PSTATE_STATUS: u32 = 0xc001_0063;

/// CPUID leaf 1, ECX: Enhanced Intel SpeedStep Technology.
pub const LEAF1_EIST: u32 = 1 << 7;
/// CPUID leaf 8000_0007h, EDX: hardware P-states.
pub const EXT7_HW_PSTATE: u32 = 1 << 7;

/// The most states a CPU keeps.
pub const MAX_STATES: usize = 16;

/// The bits of IA32_PERF_CTL and IA32_PERF_STATUS that name a state.
const INTEL_STATE: u64 = 0xffff;

/// Whose registers the states are chosen through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vendor {
    /// IA32_PERF_CTL.
    Intel,
    /// MSRC001_0062.
    Amd,
}

/// One `_PSS` state: its frequency, and the value that chooses it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// Its frequency, in megahertz.
    pub mhz: u32,
    /// What `_PCT`'s control register is written with to choose it.
    pub control: u64,
}

/// A CPU's states, slowest first, as levels and the values that choose
/// them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Table {
    vendor: Vendor,
    levels: [u8; MAX_STATES],
    controls: [u64; MAX_STATES],
    count: usize,
    /// The level of the fastest state that is not a boost.
    nominal: u8,
}

impl Table {
    /// The table of `states`, as `_PSS` lists them, fastest first, of
    /// which the first `limit` are off limits (`_PPC`). On Intel's
    /// processors a fastest state 1 MHz above the next is the turbo
    /// range, as firmware writes it, and the next is the nominal one.
    /// States past [`MAX_STATES`], and any whose level repeats a faster
    /// one's, are left out. `None` if fewer than two remain.
    pub fn new(vendor: Vendor, states: &[State], limit: usize) -> Option<Table> {
        let turbo = vendor == Vendor::Intel
            && matches!(states, [first, second, ..] if first.mhz == second.mhz + 1);
        let mut table = Table {
            vendor,
            levels: [0; MAX_STATES],
            controls: [0; MAX_STATES],
            count: 0,
            nominal: 0,
        };
        // From the fastest allowed state down, keeping each level once.
        let mut slowest_kept = u8::MAX;
        for state in states.iter().skip(limit) {
            let level = level(vendor, *state);
            if level == 0 || (table.count > 0 && level >= slowest_kept) {
                continue;
            }
            if table.count == MAX_STATES {
                break;
            }
            table.levels[table.count] = level;
            table.controls[table.count] = state.control;
            table.count += 1;
            slowest_kept = level;
        }
        if table.count < 2 {
            return None;
        }
        // Slowest first.
        table.levels[..table.count].reverse();
        table.controls[..table.count].reverse();
        let fastest = table.levels[table.count - 1];
        table.nominal = if turbo && limit == 0 { table.levels[table.count - 2] } else { fastest };
        Some(table)
    }

    /// Whose registers choose the states.
    pub fn vendor(&self) -> Vendor {
        self.vendor
    }

    /// The states' levels, slowest first.
    pub fn levels(&self) -> &[u8] {
        &self.levels[..self.count]
    }

    /// The CPU's levels: its slowest state's, which is also the most
    /// efficient the table can tell, its nominal and its fastest.
    pub fn range(&self) -> Levels {
        let lowest = self.levels[0];
        let highest = self.levels[self.count - 1];
        Levels { lowest, efficient: lowest, nominal: self.nominal, highest }
    }

    /// The index of the slowest state at `level` or above; the fastest if
    /// none is.
    pub fn state_at(&self, level: u8) -> usize {
        self.levels().iter().position(|&at| at >= level).unwrap_or(self.count - 1)
    }

    /// The index of the state to run at for a request of `desired`, at
    /// most `max`: the slowest at `desired` or above, unless that is
    /// faster than `max`, then the fastest at `max` or below; the slowest
    /// if every state is faster than `max`.
    pub fn pick(&self, desired: u8, max: u8) -> usize {
        let at = self.state_at(desired.min(max));
        if self.levels[at] <= max {
            return at;
        }
        self.levels().iter().rposition(|&level| level <= max).unwrap_or(0)
    }

    /// The level of state `index`.
    pub fn level(&self, index: usize) -> u8 {
        self.levels[index.min(self.count - 1)]
    }

    /// The register that chooses the state.
    pub fn control_register(&self) -> u32 {
        match self.vendor {
            Vendor::Intel => PERF_CTL,
            Vendor::Amd => PSTATE_CTL,
        }
    }

    /// The register that says which state the CPU is in.
    pub fn status_register(&self) -> u32 {
        match self.vendor {
            Vendor::Intel => PERF_STATUS,
            Vendor::Amd => PSTATE_STATUS,
        }
    }

    /// What the control register, now `current`, is written with to
    /// choose state `index`: on Intel's processors its other bits stay.
    pub fn control_value(&self, current: u64, index: usize) -> u64 {
        let control = self.controls[index.min(self.count - 1)];
        match self.vendor {
            Vendor::Intel => current & !INTEL_STATE | control & INTEL_STATE,
            Vendor::Amd => control,
        }
    }

    /// The index of the state the status register's `status` names: the
    /// one whose control value it matches, or on Intel's processors the
    /// slowest at or above its ratio, which is what the CPU runs at under
    /// turbo.
    pub fn state_of(&self, status: u64) -> usize {
        match self.vendor {
            Vendor::Intel => {
                let wanted = status & INTEL_STATE;
                let exact = self.controls[..self.count]
                    .iter()
                    .position(|&control| control & INTEL_STATE == wanted);
                exact.unwrap_or_else(|| self.state_at(ratio(wanted)))
            }
            Vendor::Amd => self.controls[..self.count]
                .iter()
                .position(|&control| control == status & 0x7)
                .unwrap_or(self.count - 1),
        }
    }
}

/// The level of `state`: on Intel's processors its ratio, if it gives
/// one, else its frequency in hundreds of megahertz, rounded; 0 for none.
fn level(vendor: Vendor, state: State) -> u8 {
    let from_mhz = u8::try_from(state.mhz.saturating_add(50) / 100).unwrap_or(u8::MAX);
    match vendor {
        Vendor::Intel => match ratio(state.control) {
            0 => from_mhz,
            ratio => ratio,
        },
        Vendor::Amd => from_mhz,
    }
}

/// The bus ratio in bits 15:8 of an Intel state's value.
fn ratio(control: u64) -> u8 {
    ((control >> 8) & 0xff) as u8
}

/// The share of a period the CPU must be busy, in thousandths, for it to
/// go to its fastest allowed state at once: it has no idle time to tell
/// how much faster it should go.
pub const SATURATED: u32 = 950;

/// The level to ask a CPU for next, between `min` and `max`, after a
/// period in which it was busy `busy` thousandths of the time and ran at
/// level `ran` on average while busy. The work it did takes `busy × ran`;
/// the level asked for lets the same work keep it busy four fifths of the
/// time, so that a little more work does not saturate it. A CPU busy
/// almost throughout goes to `max` at once.
pub fn demand(busy: u32, ran: u32, min: u8, max: u8) -> u8 {
    if busy >= SATURATED {
        return max;
    }
    let wanted = (u64::from(busy) * u64::from(ran) * 5).div_ceil(4 * 1000);
    let wanted = u8::try_from(wanted).unwrap_or(u8::MAX);
    wanted.clamp(min, max.max(min))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A desktop Haswell's `_PSS`: turbo (3401 MHz, ratio 39), then 3400
    /// down to 800 MHz in eight steps.
    fn haswell() -> [State; 9] {
        let state = |mhz: u32, ratio: u64| State { mhz, control: ratio << 8 };
        [
            state(3401, 39),
            state(3400, 34),
            state(3000, 30),
            state(2600, 26),
            state(2200, 22),
            state(1800, 18),
            state(1400, 14),
            state(1100, 11),
            state(800, 8),
        ]
    }

    /// A Zen 2's `_PSS`: three P-states, chosen by their numbers.
    fn zen2() -> [State; 3] {
        [
            State { mhz: 3600, control: 0 },
            State { mhz: 2800, control: 1 },
            State { mhz: 2200, control: 2 },
        ]
    }

    #[test]
    fn intel_levels_are_ratios_and_turbo_is_above_nominal() {
        let table = Table::new(Vendor::Intel, &haswell(), 0).unwrap();
        assert_eq!(table.levels(), [8, 11, 14, 18, 22, 26, 30, 34, 39]);
        assert_eq!(table.range(), Levels { lowest: 8, efficient: 8, nominal: 34, highest: 39 });
        assert_eq!(table.control_register(), PERF_CTL);
    }

    #[test]
    fn ppc_keeps_the_fastest_states_off_limits() {
        let table = Table::new(Vendor::Intel, &haswell(), 2).unwrap();
        assert_eq!(table.range(), Levels { lowest: 8, efficient: 8, nominal: 30, highest: 30 });
        assert_eq!(Table::new(Vendor::Intel, &haswell(), 8), None);
    }

    #[test]
    fn amd_levels_are_hundreds_of_megahertz() {
        let table = Table::new(Vendor::Amd, &zen2(), 0).unwrap();
        assert_eq!(table.levels(), [22, 28, 36]);
        assert_eq!(table.range(), Levels { lowest: 22, efficient: 22, nominal: 36, highest: 36 });
        assert_eq!(table.control_value(0xdead, table.state_at(28)), 1);
        assert_eq!(table.state_of(2), 0);
        assert_eq!(table.control_register(), PSTATE_CTL);
    }

    #[test]
    fn a_state_is_the_slowest_at_or_above_the_level() {
        let table = Table::new(Vendor::Intel, &haswell(), 0).unwrap();
        assert_eq!(table.level(table.state_at(20)), 22);
        assert_eq!(table.level(table.state_at(22)), 22);
        assert_eq!(table.level(table.state_at(0)), 8);
        assert_eq!(table.level(table.state_at(200)), 39);
    }

    #[test]
    fn a_pick_is_never_faster_than_the_maximum() {
        let table = Table::new(Vendor::Intel, &haswell(), 0).unwrap();
        // Cooled to 31: 30 is the fastest allowed.
        assert_eq!(table.level(table.pick(31, 31)), 30);
        assert_eq!(table.level(table.pick(20, 31)), 22);
        assert_eq!(table.level(table.pick(39, 39)), 39);
        // Not boosted: the nominal state.
        assert_eq!(table.level(table.pick(39, 34)), 34);
        // Below every state: the slowest.
        assert_eq!(table.level(table.pick(5, 5)), 8);
    }

    #[test]
    fn intel_values_keep_the_registers_other_bits() {
        let table = Table::new(Vendor::Intel, &haswell(), 0).unwrap();
        let index = table.state_at(22);
        assert_eq!(table.control_value(0x1_0000_ff00, index), 0x1_0000_1600);
        // A ratio the CPU runs at under turbo, 37, is the turbo state.
        assert_eq!(table.state_of(0x2500), table.count - 1);
        assert_eq!(table.state_of(0x1600), index);
    }

    #[test]
    fn repeated_and_empty_states_are_left_out() {
        let states = [
            State { mhz: 2000, control: 0 },
            State { mhz: 2000, control: 1 },
            State { mhz: 0, control: 2 },
            State { mhz: 1000, control: 3 },
        ];
        let table = Table::new(Vendor::Amd, &states, 0).unwrap();
        assert_eq!(table.levels(), [10, 20]);
        assert_eq!(table.control_value(0, 1), 0);
        assert_eq!(Table::new(Vendor::Amd, &states[..2], 0), None);
    }

    #[test]
    fn demand_keeps_a_cpu_four_fifths_busy() {
        // Half busy at 2.2 GHz: 1.4 GHz would keep it 79% busy.
        assert_eq!(demand(500, 22, 8, 39), 14);
        // Settled there, it stays.
        assert_eq!(demand(786, 14, 8, 39), 14);
        // Idle: the slowest.
        assert_eq!(demand(0, 34, 8, 39), 8);
        // Saturated: the fastest allowed.
        assert_eq!(demand(990, 8, 8, 39), 39);
        assert_eq!(demand(990, 8, 8, 30), 30);
        // Never outside the range.
        assert_eq!(demand(900, 40, 8, 30), 30);
    }
}
