// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! PCI Express errors (Phase 19, ADR 0663): who handles them, the kernel
//! or the firmware, and what each function has had.
//!
//! Each PCI host bridge's `_OSC` says whether the kernel takes AER
//! ([`osc`]). Where it does, [`start`] turns on every function's error
//! reporting and watches each root port for the error messages the
//! functions below it send ([`aer`]), and where the bridge also gives DPC,
//! each downstream port contains a fatal error by taking its link down
//! ([`dpc`]). Where the firmware keeps AER, its error records bring the
//! errors ([`crate::apei`]), and [`count_firmware`] counts them here too.
//!
//! Every error is recorded in `/kernel/ras` and counted by function, as
//! Linux counts them for its `aer_dev_*` files in
//! `/kernel/bus/pci/devices/<address>` ([`files`], [`write_counts`],
//! [`root_total`], [`owner`], [`failed`]). The walk reads each function's
//! configuration space through a mapping of its own, as the scan's are
//! not kept: [`Function::attach`] reads it without sizing its BARs, which
//! drivers use by then.
//!
//! What is left for later: a driver's recovery after a fatal error and
//! after containment (the device is marked failed until then; step 14,
//! with hot-plug), functions behind an Intel VMD (their configuration
//! space is the VMD's), Error Disconnect Recover, ECRC, and the root port
//! programmed I/O errors of enhanced DPC.

mod aer;
mod dpc;
pub mod osc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use radix_hal::Devices;
use radix_hw::pci::{self, Address, Function};
use radix_hw::pcie::aer::{self as regs, Counters};
use radix_hw::pcie::{PortType, express, extended};
use radix_klog::warn;
use radix_mca::record::Severity;
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_sync::{Once, SpinLock};

use crate::Arch;
use crate::fs::disk;

/// A PCIe function the kernel knows of: one whose errors it handles, or
/// one with an AER capability whose counts its files show.
struct Entry {
    /// Its configuration space.
    function: Function,
    /// Its vendor and device IDs.
    ids: (u16, u16),
    /// Its PCI Express capability's offset.
    express: usize,
    /// What it is in the hierarchy.
    port: PortType,
    /// Its AER capability's offset, and its counts.
    aer: Option<(usize, Box<SpinLock<Counters>, Kmalloc>)>,
    /// The `_OSC` control its host bridge gave: [`osc::AER`] set when the
    /// kernel handles its errors.
    control: u32,
    /// A bridge's secondary and subordinate bus: the buses below it.
    below: Option<(u8, u8)>,
    /// It had a fatal error, or a port contained one below it: its
    /// driver cannot rely on it until it is recovered.
    failed: AtomicBool,
    /// A root port's or event collector's error messages received:
    /// correctable, non-fatal, fatal.
    received: [AtomicU64; 3],
}

impl Entry {
    /// Where it is.
    fn address(&self) -> Address {
        self.function.address()
    }

    /// Whether the kernel handles its errors.
    fn native(&self) -> bool {
        self.control & osc::AER != 0
    }

    /// Whether it is on one of the buses below bridge `port`, in its
    /// segment.
    fn is_below(&self, port: &Entry) -> bool {
        let (here, there) = (self.address(), port.address());
        here.segment == there.segment
            && port.below.is_some_and(|(first, last)| (first..=last).contains(&here.bus))
    }
}

/// Every PCIe function the kernel knows of, by address.
static ENTRIES: Once<Vec<Entry, Kmalloc>> = Once::new();

/// The configuration space of the function at `address`, mapped anew
/// and read without sizing its BARs; `None` when no window of the
/// firmware's holds it (a function behind a VMD, or a Hyper-V one).
fn config(address: Address) -> Option<Function> {
    let mut window = None;
    <Arch as Devices>::pci_segments(&mut |found| {
        if found.segment == address.segment
            && (found.first_bus..=found.last_bus).contains(&address.bus)
        {
            window = Some(found.base);
        }
    });
    let base = window? + (u64::from(address.bus) << 20) + address.offset_in_bus() as u64;
    let config = <Arch as Devices>::map_device(base, pci::CONFIG_SIZE)?;
    Function::attach(address, config)
}

/// What the kernel knows of the function the PCI scan found as `found`,
/// if it is a PCIe function the kernel handles the errors of, or one with
/// an AER capability.
fn entry_of(found: &disk::PciFunction) -> Option<Entry> {
    let function = config(found.address)?;
    let express = function.find_capability(pci::capability::EXPRESS)?;
    let port = PortType::of(function.read16(express + express::CAPABILITIES));
    let (segment, root) = found.numa_root;
    let control = osc::control(segment, root);
    let aer = function.find_extended_capability(extended::AER);
    if control & osc::AER == 0 && aer.is_none() {
        return None;
    }
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let aer = match aer {
        Some(at) => Some((at, Box::try_new_in(SpinLock::new(Counters::new()), heap).ok()?)),
        None => None,
    };
    let below = function.secondary_bus().zip(function.subordinate_bus());
    Some(Entry {
        function,
        ids: (found.vendor, found.device),
        express,
        port,
        aer,
        control,
        below,
        failed: AtomicBool::new(false),
        received: [const { AtomicU64::new(0) }; 3],
    })
}

/// Finds the PCIe functions, and watches the errors of those whose host
/// bridge gave the kernel AER. Called once, after uACPI has asked the
/// host bridges ([`osc::negotiate`]) and before the firmware's error
/// sources are read, from a thread that may sleep.
pub fn start() {
    let mut count = 0;
    disk::pci_functions(|_| count += 1);
    let mut entries = Vec::new_in(HEAP.with(Flags::MAY_SLEEP));
    if entries.try_reserve_exact(count).is_err() {
        warn!("PCIe: no memory to list the PCIe functions; their errors are not watched");
        return;
    }
    disk::pci_functions(|found| {
        if let Some(entry) = entry_of(found) {
            let _ = entries.push_within_capacity(entry);
        }
    });
    entries.sort_unstable_by_key(Entry::address);
    let Ok(entries) = ENTRIES.set(entries) else { return };
    aer::start(entries);
}

/// The entry of the function at `address`.
fn entry(address: Address) -> Option<&'static Entry> {
    let entries = ENTRIES.get()?;
    entries.binary_search_by_key(&address, Entry::address).ok().map(|at| &entries[at])
}

/// The address a requester ID names in `segment`: bus, device, function.
fn requester(segment: u16, id: u16) -> Address {
    Address {
        segment,
        bus: (id >> 8) as u8,
        device: ((id >> 3) & 0x1f) as u8,
        function: (id & 0x7) as u8,
    }
}

/// Which of the error files `/kernel/bus/pci/devices/<address>` has.
#[derive(Clone, Copy, Default)]
pub struct Files {
    /// `aer_dev_correctable`, `aer_dev_nonfatal`, `aer_dev_fatal` and
    /// `aer_owner`: the function has an AER capability.
    pub counts: bool,
    /// The `aer_rootport_total_err_*` files: it is a root port or an
    /// event collector with one.
    pub root: bool,
}

/// Which of the error files the function at `address` has.
pub fn files(address: Address) -> Files {
    let Some(entry) = entry(address) else { return Files::default() };
    let counts = entry.aer.is_some();
    Files { counts, root: counts && entry.port.is_root() }
}

/// The kinds of error the files count.
#[derive(Clone, Copy)]
pub enum Class {
    /// Corrected.
    Correctable,
    /// Uncorrected, and the link and device went on.
    Nonfatal,
    /// Uncorrected, and the link or device could not.
    Fatal,
}

/// Writes the `aer_dev_*` file of `class` for the function at `address`,
/// in Linux's format; nothing when it has no AER capability.
pub fn write_counts(address: Address, class: Class, out: &mut dyn fmt::Write) -> fmt::Result {
    let Some((_, counters)) = entry(address).and_then(|entry| entry.aer.as_ref()) else {
        return Ok(());
    };
    // A copy, written without the lock held.
    let counters = *counters.lock();
    match class {
        Class::Correctable => counters.write_correctable(out),
        Class::Nonfatal => counters.write_nonfatal(out),
        Class::Fatal => counters.write_fatal(out),
    }
}

/// The error messages of `class` the root port or event collector at
/// `address` has received since boot.
pub fn root_total(address: Address, class: Class) -> u64 {
    entry(address).map_or(0, |entry| entry.received[class as usize].load(Ordering::Relaxed))
}

/// Who handles the errors of the function at `address`: `kernel` or
/// `firmware`.
pub fn owner(address: Address) -> &'static str {
    if entry(address).is_some_and(Entry::native) { "kernel" } else { "firmware" }
}

/// Whether the function at `address` is marked failed.
pub fn failed(address: Address) -> bool {
    entry(address).is_some_and(|entry| entry.failed.load(Ordering::Relaxed))
}

/// Counts an error the firmware reported for the function at `address`
/// (a CPER PCIe section, [`crate::apei`]), from the section's copy of its
/// AER capability, as Linux counts them: by the record's severity. A
/// fatal one marks the function failed. Called from a thread.
pub fn count_firmware(address: Address, aer: &[u8; 96], severity: Severity) {
    let Some(entry) = entry(address) else { return };
    let Some((_, counters)) = entry.aer.as_ref() else { return };
    let (correctable, uncorrectable, _) = regs::from_cper(aer);
    match severity {
        Severity::Informational => {}
        Severity::Corrected => counters.lock().add_correctable(correctable),
        Severity::Recoverable => counters.lock().add_uncorrectable(uncorrectable, false),
        Severity::Fatal => {
            counters.lock().add_uncorrectable(uncorrectable, true);
            entry.failed.store(true, Ordering::Relaxed);
        }
    }
}
