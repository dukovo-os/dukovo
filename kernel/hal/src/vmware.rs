// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! VMware's backdoor (Phase 14, ADR 0424): the I/O ports VMware's
//! hypervisors answer themselves, through which a guest's tools speak to
//! the host. The architecture makes the calls; what goes through them is
//! `radix-vmware`'s.

pub use radix_vmware::Regs;

/// The backdoor's instructions, which only x86 has.
///
/// Every method has a default for machines that have none: a call
/// answers as an absent backdoor does, with every bit of EAX set, and a
/// string is not taken.
pub trait Vmware {
    /// Whether the hypervisor names itself VMware's ("VMwareVMware" in
    /// CPUID's hypervisor leaf), so that the backdoor is there to call.
    fn vmware_signature() -> bool {
        false
    }

    /// `in eax, dx` on the backdoor, with `regs` in the registers before
    /// and read back after.
    fn vmware_call(regs: &mut Regs) {
        regs.ax = u32::MAX;
    }

    /// `rep outsb` of `bytes` through the high-bandwidth backdoor, RSI at
    /// them and RCX their length, the other registers from `regs`, and
    /// all read back into it.
    fn vmware_send(regs: &mut Regs, bytes: &[u8]) {
        let _ = bytes;
        regs.bx = 0;
    }

    /// `rep insb` of `bytes.len()` bytes into `bytes` from the
    /// high-bandwidth backdoor, RDI at them and RCX their length, the
    /// other registers from `regs`, and all read back into it.
    fn vmware_receive(regs: &mut Regs, bytes: &mut [u8]) {
        let _ = bytes;
        regs.bx = 0;
    }
}
