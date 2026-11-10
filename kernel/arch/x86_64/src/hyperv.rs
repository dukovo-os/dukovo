// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Microsoft's hypervisor, Hyper-V, as its Hypervisor Top Level Functional
//! Specification (TLFS) describes it (ADR 0423): finding it, its clock's
//! rates, its hypercalls and its synthetic interrupt controller (SynIC),
//! which VMBus's driver speaks through.
//!
//! - **Finding it.** CPUID leaf 0x4000_0000 names the hypervisor
//!   ("Microsoft Hv") and leaf 0x4000_0001 its interface ("Hv#1"); leaf
//!   0x4000_0003 says what the guest may use: the partition's privileges
//!   in EAX and more features in EDX. KVM can show the same leaves to a
//!   guest (QEMU's `hv-*` flags), with its own after them.
//! - **The clock.** With the frequency privilege, two registers give the
//!   TSC's rate and the local APIC timer's exactly, which a Generation 2
//!   machine, with no PIT and no HPET, needs; failing them, the reference
//!   counter, which counts 100 ns steps, is a timer to measure against
//!   ([`crate::time`]).
//! - **Hypercalls.** The guest names its operating system in the guest
//!   OS ID register, then gives the hypervisor a page, which the
//!   hypervisor fills with the instructions that call it (`vmcall` or
//!   `vmmcall`, as the CPU has); the kernel maps that page executable,
//!   and only readable, and calls into it with the call code in RCX and
//!   the physical addresses of the input and output blocks in RDX and R8,
//!   or with the arguments themselves in RDX and R8 for a fast call.
//! - **SynIC.** Each virtual processor has its own controller: a message
//!   page with sixteen 256-byte slots, one for each synthetic interrupt
//!   source (SINT), an event flags page with sixteen 256-byte blocks of
//!   flags, one for each SINT, a register for each SINT that names its
//!   vector, and the end-of-message register, which tells the hypervisor
//!   a slot was emptied so that it delivers the next message it holds.
//!   Everything is set up on the CPU that asks, and its registers are that
//!   CPU's: VMBus's thread runs on that CPU alone.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use radix_hal::Synic;
use radix_hw::Dma;
use radix_mm::Owner;

use crate::memory::{self, PAGE};
use crate::paging::{self, Access, Cache};
use crate::{cpu, devices};

/// CPUID 0x4000_0000: the hypervisor's name, in EBX, ECX and EDX.
const SIGNATURE: [u8; 12] = *b"Microsoft Hv";
/// CPUID 0x4000_0001, EAX: the interface, "Hv#1".
const INTERFACE: u32 = 0x3123_7648;
/// CPUID 0x4000_0002: the hypervisor's version.
const LEAF_IDENTITY: u32 = 0x4000_0002;
/// CPUID 0x4000_0003: the partition's privileges and features.
const LEAF_FEATURES: u32 = 0x4000_0003;

/// Privilege (EAX): the reference counter's register.
const PRIVILEGE_REFERENCE_COUNTER: u32 = 1 << 1;
/// Privilege (EAX): the SynIC's registers.
const PRIVILEGE_SYNIC: u32 = 1 << 2;
/// Privilege (EAX): the guest OS ID and hypercall registers.
const PRIVILEGE_HYPERCALL: u32 = 1 << 5;
/// Privilege (EAX): the virtual processor index register.
const PRIVILEGE_VP_INDEX: u32 = 1 << 6;
/// Privilege (EAX): the TSC and APIC frequency registers.
const PRIVILEGE_FREQUENCY: u32 = 1 << 11;
/// Feature (EDX): the frequency registers are there to read.
const FEATURE_FREQUENCY: u32 = 1 << 8;

/// Names the guest's operating system to the hypervisor.
const MSR_GUEST_OS_ID: u32 = 0x4000_0000;
/// The hypercall page: its frame, and bit 0 to enable it.
const MSR_HYPERCALL: u32 = 0x4000_0001;
/// This virtual processor's index.
const MSR_VP_INDEX: u32 = 0x4000_0002;
/// The partition's reference counter, in 100 ns steps.
const MSR_REFERENCE_COUNTER: u32 = 0x4000_0020;
/// The TSC's rate, in Hz.
const MSR_TSC_FREQUENCY: u32 = 0x4000_0022;
/// The local APIC timer's input rate, in Hz.
const MSR_APIC_FREQUENCY: u32 = 0x4000_0023;
/// The SynIC's control register: bit 0 enables it.
const MSR_SCONTROL: u32 = 0x4000_0080;
/// The event flags page: its frame, and bit 0 to enable it.
const MSR_SIEFP: u32 = 0x4000_0082;
/// The message page: its frame, and bit 0 to enable it.
const MSR_SIMP: u32 = 0x4000_0083;
/// Written to say a message slot was emptied.
const MSR_EOM: u32 = 0x4000_0084;
/// The first SINT's register; SINT `n`'s is `MSR_SINT0 + n`.
const MSR_SINT0: u32 = 0x4000_0090;

/// A page register's enable bit.
const ENABLE: u64 = 1;
/// A page register's reserved bits, which are kept as they were.
const PAGE_RESERVED: u64 = 0x0ffe;
/// A SINT register's bits besides its vector and mask, kept as they were.
const SINT_RESERVED: u64 = 0xffff_ffff_fffc_ff00;
/// A SINT register's mask bit.
const SINT_MASKED: u64 = 1 << 16;

/// The guest OS ID: bit 63 says an open-source system, bits 62..56 its
/// type, bits 55..48 its OS ID, bits 47..16 its version and bits 15..0
/// its build. Microsoft gives the types out; Dukovo takes 0x7f, the last,
/// which names none of the systems listed, with OS ID 0 and version 1.
/// Hyper-V only needs the value not to be 0.
const GUEST_OS_ID: u64 = (1 << 63) | (0x7f << 56) | (1 << 16);

/// HvPostMessage's call code.
const CALL_POST_MESSAGE: u64 = 0x5c;
/// HvSignalEvent's call code.
const CALL_SIGNAL_EVENT: u64 = 0x5d;
/// A call code's fast flag: the arguments are in registers.
const CALL_FAST: u64 = 1 << 16;

/// The kernel's executable mapping of the hypercall page, once it is
/// filled; 0 before.
static HYPERCALL_PAGE: AtomicUsize = AtomicUsize::new(0);
/// Whether the SynIC was started, which is done once.
static STARTED: AtomicBool = AtomicBool::new(false);

/// The hypervisor's privileges (EAX) and features (EDX) from CPUID
/// 0x4000_0003, if the machine runs under Hyper-V or a hypervisor that
/// shows Hyper-V's interface.
fn features() -> Option<(u32, u32)> {
    if !cpu::feature::hypervisor() {
        return None;
    }
    let id = cpu::cpuid(0x4000_0000, 0);
    let mut signature = [0u8; 12];
    signature[..4].copy_from_slice(&id.ebx.to_le_bytes());
    signature[4..8].copy_from_slice(&id.ecx.to_le_bytes());
    signature[8..].copy_from_slice(&id.edx.to_le_bytes());
    if signature != SIGNATURE || id.eax < LEAF_FEATURES {
        return None;
    }
    if cpu::cpuid(0x4000_0001, 0).eax != INTERFACE {
        return None;
    }
    let leaf = cpu::cpuid(LEAF_FEATURES, 0);
    Some((leaf.eax, leaf.edx))
}

/// Hyper-V's version: major, minor and build.
fn version() -> (u16, u16, u32) {
    let leaf = cpu::cpuid(LEAF_IDENTITY, 0);
    ((leaf.ebx >> 16) as u16, leaf.ebx as u16, leaf.eax)
}

/// The TSC's and the local APIC timer's rates in Hz, when Hyper-V gives
/// them: exact, and the only way to know them on a machine with no
/// legacy timers.
pub fn frequencies() -> Option<(u64, u64)> {
    let (privileges, features) = features()?;
    if privileges & PRIVILEGE_FREQUENCY == 0 || features & FEATURE_FREQUENCY == 0 {
        return None;
    }
    // SAFETY: the hypervisor gives the partition both registers, as its
    // privilege and feature bits say; reading them changes nothing.
    let (tsc, apic) = unsafe { (cpu::rdmsr(MSR_TSC_FREQUENCY), cpu::rdmsr(MSR_APIC_FREQUENCY)) };
    (tsc != 0 && apic != 0).then_some((tsc, apic))
}

/// Whether the partition's reference counter can be read, as a timer to
/// measure the TSC against.
pub fn has_reference_counter() -> bool {
    features().is_some_and(|(privileges, _)| privileges & PRIVILEGE_REFERENCE_COUNTER != 0)
}

/// The partition's reference counter: 100 ns steps since it started.
/// Only after [`has_reference_counter`] said there is one.
pub fn reference_counter() -> u64 {
    // SAFETY: the hypervisor gives the partition the register, as
    // `has_reference_counter` checked; reading it changes nothing.
    unsafe { cpu::rdmsr(MSR_REFERENCE_COUNTER) }
}

/// Makes a hypercall through the hypercall page: `control` holds the call
/// code and flags, `input` and `output` the physical addresses of the
/// input and output blocks, or the arguments of a fast call. Returns the
/// hypercall's status, the low 16 bits of RAX; 0 is success.
fn hypercall(control: u64, input: u64, output: u64) -> u16 {
    let page = HYPERCALL_PAGE.load(Ordering::Acquire);
    if page == 0 {
        return u16::MAX;
    }
    let status: u64;
    // SAFETY: `page` maps the hypercall page, which the hypervisor filled
    // with the code that calls it and a return; the code takes its
    // arguments in RCX, RDX and R8, returns its status in RAX, and keeps
    // to the C calling convention's saved registers. The input block, if
    // any, is the caller's and the hypervisor only reads it.
    unsafe {
        core::arch::asm!(
            "call {page}",
            page = in(reg) page,
            in("rcx") control,
            in("rdx") input,
            in("r8") output,
            lateout("rax") status,
            clobber_abi("C"),
        );
    }
    status as u16
}

/// Starts the SynIC of the CPU that runs this; see
/// [`radix_hal::Hyperv::hyperv_start`].
pub fn start(sint: u8, handler: fn(usize), arg: usize) -> Option<Synic> {
    let (privileges, _) = features()?;
    let needed = PRIVILEGE_SYNIC | PRIVILEGE_HYPERCALL | PRIVILEGE_VP_INDEX;
    if privileges & needed != needed || sint >= 16 {
        return None;
    }
    if STARTED.swap(true, Ordering::AcqRel) {
        return None;
    }
    // The hypercall page, which the hypervisor overlays with its code: the
    // page given stays the kernel's, mapped executable and read-only.
    let page = memory::alloc_pages(1, false, Owner::Kernel)?;
    // SAFETY: the privilege bits give the partition both registers. The
    // guest OS ID comes first, as the TLFS asks, and the hypercall
    // register takes a page of RAM that the kernel keeps for good; its
    // reserved bits are kept as the hypervisor set them.
    let enabled = unsafe {
        cpu::wrmsr(MSR_GUEST_OS_ID, GUEST_OS_ID);
        let old = cpu::rdmsr(MSR_HYPERCALL);
        cpu::wrmsr(MSR_HYPERCALL, (old & PAGE_RESERVED) | page | ENABLE);
        cpu::rdmsr(MSR_HYPERCALL) & ENABLE != 0
    };
    if !enabled {
        // SAFETY: as above; 0 says no guest OS is there.
        unsafe { cpu::wrmsr(MSR_GUEST_OS_ID, 0) };
        memory::free_pages(page, 1, Owner::Kernel);
        return None;
    }
    let code = paging::map_physical(page, PAGE, Access::Code, Cache::WriteBack);
    HYPERCALL_PAGE.store(code, Ordering::Release);

    let messages = alloc(1)?;
    let events = alloc(1)?;
    let (vector, _) = devices::allocate(None, 0, handler, arg)?;
    // SAFETY: the privilege bits give the partition the SynIC's
    // registers. The two pages are the kernel's for as long as the SynIC
    // has them, which is for good (the `Synic` that holds them is kept by
    // VMBus's driver, which never stops); the vector was just taken, with
    // its handler. Reserved bits are kept as the hypervisor set them.
    let vp_index = unsafe {
        let old = cpu::rdmsr(MSR_SIMP);
        cpu::wrmsr(MSR_SIMP, (old & PAGE_RESERVED) | messages.phys() | ENABLE);
        let old = cpu::rdmsr(MSR_SIEFP);
        cpu::wrmsr(MSR_SIEFP, (old & PAGE_RESERVED) | events.phys() | ENABLE);
        let register = MSR_SINT0 + u32::from(sint);
        let old = cpu::rdmsr(register);
        cpu::wrmsr(register, (old & SINT_RESERVED & !SINT_MASKED) | u64::from(vector));
        let old = cpu::rdmsr(MSR_SCONTROL);
        cpu::wrmsr(MSR_SCONTROL, old | ENABLE);
        cpu::rdmsr(MSR_VP_INDEX) as u32
    };
    Some(Synic { vp_index, messages, events, vector, version: version() })
}

/// Posts a message to the host; see [`radix_hal::Hyperv::hyperv_post_message`].
pub fn post_message(input: u64) -> u16 {
    hypercall(CALL_POST_MESSAGE, input, 0)
}

/// Signals an event of the host's; see
/// [`radix_hal::Hyperv::hyperv_signal_event`].
pub fn signal_event(connection: u32) -> u16 {
    hypercall(CALL_SIGNAL_EVENT | CALL_FAST, u64::from(connection), 0)
}

/// Says a message slot was emptied; see
/// [`radix_hal::Hyperv::hyperv_end_of_message`].
pub fn end_of_message() {
    if !STARTED.load(Ordering::Acquire) {
        return;
    }
    // SAFETY: the SynIC was started, so the partition has the register;
    // writing it only makes the hypervisor deliver its next message.
    unsafe { cpu::wrmsr(MSR_EOM, 0) };
}

fn release(_virt: usize, phys: u64, len: usize, _tag: u32) {
    memory::free_pages(phys, (len as u64).div_ceil(PAGE), Owner::Kernel);
}

/// Allocates memory to share with the host; see
/// [`radix_hal::Hyperv::hyperv_alloc`].
pub fn alloc(pages: usize) -> Option<Dma> {
    let pages = pages.max(1) as u64;
    let phys = memory::alloc_pages(pages, false, Owner::Kernel)?;
    let len = pages * PAGE;
    let virt = memory::phys_to_virt(phys);
    // SAFETY: the frame allocator gave these pages to us alone, zeroed;
    // the direct map maps all RAM writable at `phys_to_virt`, and the
    // block is contiguous. `release` gives them back exactly once.
    Some(unsafe { Dma::new(virt, phys, len as usize, 0, release) })
}
