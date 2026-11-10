// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Hyper-V's VMBus and the devices on it (ADR 0423): what a virtual
//! machine under Microsoft's hypervisor, or in Azure, has in place of PCI
//! devices.
//!
//! VMBus is a set of channels between the guest and the host's
//! partition. The host offers one channel for each device, named by two
//! GUIDs, the device's kind and its instance, in messages that the
//! hypervisor puts in the guest's SynIC message page ([`vmbus`]). The
//! guest opens a channel by giving the host a pair of ring buffers in its
//! own memory, one each way, named to the host by their pages (a GPADL);
//! from then on the two sides write packets into the rings ([`ring`]) and
//! signal each other: the guest with a hypercall, the host by setting the
//! channel's bit in the guest's event flags and raising VMBus's
//! interrupt. Packets carry each device's own protocol:
//!
//! - [`storage`], the synthetic SCSI controller (storvsc): SCSI commands
//!   in request blocks, with the pages to read into or write from listed
//!   in the packet;
//! - [`network`], the synthetic network card (netvsc): its own protocol
//!   (NVSP) around Microsoft's Remote NDIS, frames received through a
//!   receive buffer the guest gives the host, and sent from pages the
//!   packet lists;
//! - [`keyboard`], the synthetic keyboard: set 1 scan codes;
//! - [`ic`], the integration services: the host's requests to shut down,
//!   its heartbeat and its clock;
//! - [`pci`], the virtual PCI bus, through which the host passes a device
//!   through: Azure's network adapter, for accelerated networking, which
//!   [`bond`] joins to its synthetic card.
//!
//! The layouts and sequences follow FreeBSD's Hyper-V drivers (BSD-2-Clause,
//! credits/freebsd-hyperv.md) and Microsoft's Hypervisor Top Level
//! Functional Specification. The crate forbids unsafe code: it reads and
//! writes the shared pages through `radix-hw`'s [`radix_hw::Dma`], and
//! the kernel gives it the hypercalls as plain functions.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

pub mod bond;
pub mod guid;
pub mod ic;
pub mod keyboard;
pub mod network;
pub mod pci;
pub mod ring;
pub mod storage;
pub mod vmbus;

#[cfg(test)]
mod tests;

pub use guid::Guid;

/// The size of a page, as VMBus counts pages.
pub const PAGE: usize = 4096;

/// What the kernel gives a channel to reach the host: the hypercall that
/// signals the host's end of a channel, by the connection its offer
/// named. It returns the hypercall's status, 0 when it was signalled.
pub type Signal = fn(connection: u32) -> u16;
