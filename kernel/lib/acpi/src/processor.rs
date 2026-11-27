// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A processor's power management objects, read from what AML returned
//! (ACPI Specification 6.5, section 8 "Processor Configuration and
//! Control"): the idle states `_CST` lists and the performance states
//! `_PSS` lists, and what the operating system tells the firmware about
//! itself through `_OSC`, in the bits of Intel's *Processor Vendor-Specific
//! ACPI* interface.
//!
//! Nothing here evaluates AML: the caller passes in the numbers and the
//! bytes of a register descriptor an entry holds, and gets the state it
//! means, or `None` when the entry is one the kernel cannot use.

use crate::resources::{
    GenericRegister, SPACE_FUNCTIONAL_FIXED_HW, SPACE_SYSTEM_IO, generic_register,
};

/// The class of functional fixed hardware that is the `MWAIT`
/// instruction, as the `bit_offset` of a C-state's register. Intel's
/// firmware names its vendor (1) in the `bit_width`, AMD's another; the
/// vendor does not change what the class means, so it is not read.
const CLASS_MWAIT: u8 = 2;

/// How the processor enters a C-state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    /// The `HLT` instruction: C1.
    Halt,
    /// The `MWAIT` instruction (after `MONITOR`), with its hint in EAX and
    /// its extensions in ECX.
    Mwait {
        /// The hint: the C-state and sub-state, in EAX.
        hint: u32,
        /// The `_CST` entry's flags: bit 0 says an interrupt should wake
        /// the processor even with interrupts masked (ECX bit 0).
        flags: u32,
    },
    /// A read of an I/O port, which puts the processor to sleep.
    Port(u16),
}

/// One idle state of a processor, an entry of `_CST`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CState {
    /// How to enter it.
    pub entry: Entry,
    /// The ACPI C-state type: 1 for C1, 2 for C2, 3 for C3 or deeper.
    pub kind: u8,
    /// The worst-case time to leave it, in microseconds.
    pub latency_us: u32,
    /// The power it draws, in milliwatts.
    pub power_mw: u32,
}

/// Reads one `_CST` entry: the bytes of its register's resource template
/// (a Generic Register descriptor and an End Tag), its type, its latency
/// in microseconds and its power in milliwatts.
///
/// Functional fixed hardware of the `MWAIT` class, whatever its vendor, is
/// `MWAIT`, its hint the address's low 32 bits and its flags the access
/// size; for a type 1 state, functional fixed hardware of another class is
/// `HLT`; a system I/O register is a port to read. Anything else, a type
/// outside 1 to 3, or a number that does not fit is `None`.
pub fn cstate(register: &[u8], kind: u64, latency_us: u64, power_mw: u64) -> Option<CState> {
    let kind = u8::try_from(kind).ok().filter(|kind| (1..=3).contains(kind))?;
    let (latency_us, power_mw) = (u32::try_from(latency_us).ok()?, u32::try_from(power_mw).ok()?);
    let GenericRegister { space, bit_offset, access_size, address, .. } =
        generic_register(register)?;
    let entry = match space {
        SPACE_FUNCTIONAL_FIXED_HW if bit_offset == CLASS_MWAIT => {
            Entry::Mwait { hint: address as u32, flags: access_size.into() }
        }
        SPACE_FUNCTIONAL_FIXED_HW if kind == 1 => Entry::Halt,
        SPACE_SYSTEM_IO => Entry::Port(u16::try_from(address).ok()?),
        _ => return None,
    };
    Some(CState { entry, kind, latency_us, power_mw })
}

/// One performance state of a processor, an entry of `_PSS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PState {
    /// The core frequency, in megahertz.
    pub mhz: u32,
    /// The power it draws at most, in milliwatts.
    pub power_mw: u32,
    /// The time the processor is unavailable while changing to it, in
    /// microseconds.
    pub latency_us: u32,
    /// What to write to the control register (`_PCT`) to choose it; on
    /// Intel processors, the value of `IA32_PERF_CTL`.
    pub control: u64,
    /// What the status register reads once it is chosen.
    pub status: u64,
}

/// Reads one `_PSS` entry: core frequency in megahertz, power in
/// milliwatts, latency in microseconds, bus master latency in
/// microseconds (which is not kept), control and status. A frequency of
/// zero, or a number that does not fit, is `None`.
pub fn pstate(values: [u64; 6]) -> Option<PState> {
    let [mhz, power_mw, latency_us, bus_master_us, control, status] = values;
    u32::try_from(bus_master_us).ok()?;
    let mhz = u32::try_from(mhz).ok().filter(|&mhz| mhz != 0)?;
    Some(PState {
        mhz,
        power_mw: u32::try_from(power_mw).ok()?,
        latency_us: u32::try_from(latency_us).ok()?,
        control,
        status,
    })
}

/// The UUID of Intel's processor `_OSC`, 4077A616-290C-47BE-9EBD-
/// D87058713953, as the 16 bytes of its buffer (its first three fields
/// little-endian, as `ToUUID` lays them out).
pub const OSC_UUID: [u8; 16] = [
    0x16, 0xa6, 0x77, 0x40, 0x0c, 0x29, 0xbe, 0x47, 0x9e, 0xbd, 0xd8, 0x70, 0x58, 0x71, 0x39, 0x53,
];

/// The revision of that `_OSC`.
pub const OSC_REVISION: u64 = 1;

/// Capability bit: the operating system can use the performance state
/// MSRs (`IA32_PERF_CTL` and `IA32_PERF_STATUS`) for `_PSS`.
pub const CAP_PSTATE_MSRS: u32 = 0x0001;
/// Capability bit: the operating system can enter C1 with `HLT`.
pub const CAP_C1_HALT: u32 = 0x0002;
/// Capability bit: the operating system can use the throttling MSR.
pub const CAP_THROTTLING_MSRS: u32 = 0x0004;
/// Capability bit: SMP systems may have independent C1 states and
/// throttling states for each processor.
pub const CAP_SMP_C1_THROTTLING_INDEPENDENT: u32 = 0x0008;
/// Capability bit: SMP systems may have independent C2 and C3 states.
pub const CAP_SMP_C2_C3_INDEPENDENT: u32 = 0x0010;
/// Capability bit: the operating system coordinates the performance
/// states of an SMP system's processors in software.
pub const CAP_SMP_PSTATE_SOFTWARE_COORDINATION: u32 = 0x0020;
/// Capability bit: the operating system coordinates the C-states of an
/// SMP system's processors in software.
pub const CAP_SMP_CSTATE_SOFTWARE_COORDINATION: u32 = 0x0040;
/// Capability bit: the operating system coordinates the throttling
/// states of an SMP system's processors in software.
pub const CAP_SMP_THROTTLING_SOFTWARE_COORDINATION: u32 = 0x0080;
/// Capability bit: the operating system can enter C1 with `MWAIT`.
pub const CAP_C1_NATIVE: u32 = 0x0100;
/// Capability bit: the operating system can enter C2 and C3 with `MWAIT`.
pub const CAP_C2_C3_NATIVE: u32 = 0x0200;
/// Capability bit: the operating system acknowledges a `_PPC`
/// notification by evaluating `_OST`.
pub const CAP_PPC_OST: u32 = 0x0400;
/// Capability bit: the processors' performance states are coordinated in
/// hardware in SMP systems.
pub const CAP_SMP_PSTATE_HARDWARE_COORDINATION: u32 = 0x0800;

#[cfg(test)]
mod tests {
    use super::*;

    /// A register's template with its End Tag.
    fn register(space: u8, width: u8, offset: u8, access: u8, address: u64) -> Vec<u8> {
        let mut bytes = vec![0x82, 0x0c, 0x00, space, width, offset, access];
        bytes.extend_from_slice(&address.to_le_bytes());
        bytes.extend_from_slice(&[0x79, 0x00]);
        bytes
    }

    #[test]
    fn an_mwait_register_is_mwait() {
        let bytes = register(0x7f, 1, 2, 1, 0x1_0000_0020);
        assert_eq!(
            cstate(&bytes, 3, 87, 350),
            Some(CState {
                entry: Entry::Mwait { hint: 0x20, flags: 1 },
                kind: 3,
                latency_us: 87,
                power_mw: 350
            })
        );
        // Whatever the type, the class decides.
        let state = cstate(&register(0x7f, 1, 2, 0, 0), 1, 1, 1000).unwrap();
        assert_eq!(state.entry, Entry::Mwait { hint: 0, flags: 0 });
    }

    #[test]
    fn the_vendor_does_not_change_mwait() {
        let state = cstate(&register(0x7f, 2, 2, 1, 0x10), 2, 18, 0).unwrap();
        assert_eq!(state.entry, Entry::Mwait { hint: 0x10, flags: 1 });
        let state = cstate(&register(0x7f, 0, 2, 0, 0x60), 3, 50, 0).unwrap();
        assert_eq!(state.entry, Entry::Mwait { hint: 0x60, flags: 0 });
    }

    #[test]
    fn functional_fixed_hardware_of_another_class_is_halt_for_c1() {
        let state = cstate(&register(0x7f, 0, 1, 0, 0), 1, 1, 0).unwrap();
        assert_eq!(state.entry, Entry::Halt);
        assert_eq!(cstate(&register(0x7f, 0, 1, 0, 0), 2, 1, 0), None);
    }

    #[test]
    fn a_system_io_register_is_a_port() {
        let bytes = register(0x01, 8, 0, 1, 0x814);
        assert_eq!(cstate(&bytes, 2, 100, 0).map(|state| state.entry), Some(Entry::Port(0x814)));
        assert_eq!(cstate(&register(0x01, 8, 0, 1, 0x1_0814), 2, 100, 0), None);
    }

    #[test]
    fn other_entries_are_not_states() {
        let port = register(0x01, 8, 0, 1, 0x814);
        assert_eq!(cstate(&port, 0, 1, 1), None);
        assert_eq!(cstate(&port, 4, 1, 1), None);
        assert_eq!(cstate(&port, 2, u64::from(u32::MAX) + 1, 1), None);
        assert_eq!(cstate(&port, 2, 1, u64::from(u32::MAX) + 1), None);
        assert!(cstate(&port, 2, u64::from(u32::MAX), u64::from(u32::MAX)).is_some());
        // System memory, and bytes that are no register.
        assert_eq!(cstate(&register(0x00, 8, 0, 1, 0x814), 2, 1, 1), None);
        assert_eq!(cstate(&port[..5], 2, 1, 1), None);
        assert_eq!(cstate(&[0x79, 0x00], 2, 1, 1), None);
    }

    #[test]
    fn a_pss_entry_is_a_pstate() {
        assert_eq!(
            pstate([2400, 35_000, 10, 10, 0x1800, 0x1800]),
            Some(PState {
                mhz: 2400,
                power_mw: 35_000,
                latency_us: 10,
                control: 0x1800,
                status: 0x1800
            })
        );
        assert_eq!(pstate([0, 1, 1, 1, 1, 1]), None);
        assert_eq!(pstate([1 << 32, 1, 1, 1, 1, 1]), None);
        assert_eq!(pstate([1, 1 << 32, 1, 1, 1, 1]), None);
        assert_eq!(pstate([1, 1, 1 << 32, 1, 1, 1]), None);
        assert_eq!(pstate([1, 1, 1, 1 << 32, 1, 1]), None);
        // A 64-bit control value is kept whole.
        assert_eq!(pstate([1, 1, 1, 1, u64::MAX, u64::MAX - 1]).unwrap().control, u64::MAX);
    }

    #[test]
    fn the_osc_uuid_is_laid_out_as_touuid_does() {
        // 4077A616-290C-47BE-9EBD-D87058713953: the first three fields
        // reversed, the last eight bytes as written.
        assert_eq!(OSC_UUID[..4], [0x16, 0xa6, 0x77, 0x40]);
        assert_eq!(OSC_UUID[4..6], [0x0c, 0x29]);
        assert_eq!(OSC_UUID[6..8], [0xbe, 0x47]);
        assert_eq!(OSC_UUID[8..], [0x9e, 0xbd, 0xd8, 0x70, 0x58, 0x71, 0x39, 0x53]);
    }

    #[test]
    fn the_capability_bits_are_distinct() {
        let bits = [
            CAP_PSTATE_MSRS,
            CAP_C1_HALT,
            CAP_THROTTLING_MSRS,
            CAP_SMP_C1_THROTTLING_INDEPENDENT,
            CAP_SMP_C2_C3_INDEPENDENT,
            CAP_SMP_PSTATE_SOFTWARE_COORDINATION,
            CAP_SMP_CSTATE_SOFTWARE_COORDINATION,
            CAP_SMP_THROTTLING_SOFTWARE_COORDINATION,
            CAP_C1_NATIVE,
            CAP_C2_C3_NATIVE,
            CAP_PPC_OST,
            CAP_SMP_PSTATE_HARDWARE_COORDINATION,
        ];
        let mut all = 0;
        for bit in bits {
            assert_eq!(bit.count_ones(), 1);
            assert_eq!(all & bit, 0);
            all |= bit;
        }
        assert_eq!(all, 0x0fff);
    }
}
