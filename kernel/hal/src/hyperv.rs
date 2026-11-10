// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Microsoft's hypervisor, Hyper-V, which Azure runs too (Phase 14,
//! ADR 0423): what the VMBus driver needs from the machine.
//!
//! Hyper-V gives a virtual machine its devices on VMBus, not on PCI. The
//! host and the guest speak through the hypervisor's synthetic interrupt
//! controller (SynIC): the host posts messages into a page of the
//! guest's, one slot for each of its sixteen synthetic interrupt sources
//! (SINTs), and sets bits in another page, the event flags, and raises
//! the source's interrupt; the guest posts messages to the host and
//! signals it with hypercalls. The architecture sets the controller up,
//! as Microsoft's Hypervisor Top Level Functional Specification (TLFS)
//! describes it, and makes the hypercalls; everything VMBus says over
//! them is the driver's (`radix-hyperv`).

use radix_hw::Dma;

/// The synthetic interrupt controller, as [`Hyperv::hyperv_start`] set it
/// up on the boot CPU.
#[derive(Debug)]
pub struct Synic {
    /// The hypervisor's own index of the boot CPU (its virtual
    /// processor), which VMBus names as the CPU its interrupts go to.
    pub vp_index: u32,
    /// The message page: sixteen 256-byte slots, one for each SINT, which
    /// the hypervisor fills.
    pub messages: Dma,
    /// The event flags page: sixteen 256-byte blocks of 2048 flags, one
    /// for each SINT, which the hypervisor sets.
    pub events: Dma,
    /// The interrupt vector of VMBus's SINT, for the log.
    pub vector: u32,
    /// Hyper-V's version, from its identity leaf: major, minor and
    /// build.
    pub version: (u16, u16, u32),
}

/// Hyper-V's hypercalls and synthetic interrupt controller (TLFS).
///
/// Every method has a default for machines that have none: an
/// architecture that cannot run under Hyper-V implements nothing.
pub trait Hyperv {
    /// Starts the synthetic interrupt controller on the boot CPU, if the
    /// machine runs under Hyper-V and the hypervisor gives the guest its
    /// controller and hypercalls: names the guest's operating system to
    /// the hypervisor, maps the hypercall page, gives the hypervisor the
    /// message and event flags pages, and routes SINT `sint` to a vector
    /// whose interrupts call `handler(arg)`, with interrupts masked, on
    /// the boot CPU. `None` outside Hyper-V, and the second time.
    fn hyperv_start(sint: u8, handler: fn(usize), arg: usize) -> Option<Synic> {
        let _ = (sint, handler, arg);
        None
    }

    /// Posts a message to the host (HvPostMessage): `input` is the
    /// physical address of its 256-byte input block (the connection,
    /// the message's type and length, and up to 240 bytes), 8-byte
    /// aligned. Returns the hypercall's status: 0 when the host has it,
    /// otherwise the hypervisor's code (such as `HV_STATUS_INSUFFICIENT_BUFFERS`,
    /// 0x13, when the host's queue is full, and the message may be posted
    /// again).
    fn hyperv_post_message(input: u64) -> u16 {
        let _ = input;
        1
    }

    /// Signals the host's event `connection` (HvSignalEvent, as a fast
    /// hypercall, flag 0). Returns the hypercall's status.
    fn hyperv_signal_event(connection: u32) -> u16 {
        let _ = connection;
        1
    }

    /// Tells the hypervisor the message slot it filled has been emptied
    /// (the end-of-message register), so that it delivers the next one it
    /// holds.
    fn hyperv_end_of_message() {}

    /// Allocates `pages` pages of zeroed, physically contiguous memory to
    /// share with the host: ring buffers and buffers that VMBus names to
    /// the host by their pages. The host reaches guest memory by its
    /// physical address, past any IOMMU. Given back when the [`Dma`] is
    /// dropped; `None` when memory runs out.
    fn hyperv_alloc(pages: usize) -> Option<Dma> {
        let _ = pages;
        None
    }
}
