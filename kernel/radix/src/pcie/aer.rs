// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Native AER (PCIe 6.0, 6.2 and 7.8.4): the kernel's own handling of the
//! errors below the root ports whose host bridge gave it AER.
//!
//! [`start`] turns on each such function's error reporting (device
//! control's four enables), has each bridge pass the error messages from
//! below it on (bridge control's SERR enable, which Linux sets too), and
//! clears what the firmware's start left logged. Each root port and root
//! complex event collector with an AER capability stops turning the
//! messages into system errors for the firmware (root control) and
//! interrupts instead (Root Error Command), through MSI-X or MSI; one with
//! neither keeps its interrupts off and is read every second.
//!
//! The interrupt handler reads and clears the port's Root Error Status,
//! keeps it and the Error Source Identification for the `pcie-aer`
//! thread, counts the messages, and wakes the thread. The thread reads
//! the function the source names, and every function below the port, as
//! messages that came while it was busy name only the first: each
//! function's unmasked status says what it had, its severity register
//! which uncorrectable ones were fatal, and the header log the TLP of the
//! first. Each error is recorded in `/kernel/ras`, counted for the
//! function's files, logged (within the record's limit), and its status
//! cleared. A corrected one needs nothing more; after a non-fatal one the
//! driver carries on; a fatal one marks the function failed, until a
//! driver can be reset (step 14). Containment by the port, where it has
//! DPC, is [`super::dpc`]'s.

use alloc::vec::Vec;
use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use radix_hal::Devices;
use radix_hw::pci::{BRIDGE_CONTROL, BRIDGE_CONTROL_SERR, Function, Msi, MsiX, command};
use radix_hw::pcie::aer::{self as regs, Names, root};
use radix_hw::pcie::{PortType, dpc as dpc_regs, express, extended};
use radix_klog::{debug, error, info, warn};
use radix_mca::record::{Kind, Log, Severity, Source};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_sync::Once;

use super::{Entry, entry, osc, requester};
use crate::Arch;
use crate::ras::{self, Report};
use crate::sched::{self, Options, Policy, ThreadRef};

/// How often a port without MSI or MSI-X is read, in nanoseconds.
const POLL_NS: u64 = 1_000_000_000;

/// A port the kernel watches: a root port or event collector for the
/// error messages it receives, a downstream port for its containment.
pub(super) struct Port {
    /// Its function.
    pub(super) entry: &'static Entry,
    /// Its AER capability's offset, when it receives error messages.
    root: Option<usize>,
    /// Its DPC capability's offset, when it contains errors.
    pub(super) dpc: Option<usize>,
    /// It has no interrupt, and is read every second.
    polled: AtomicBool,
    /// The Root Error Status bits received since the thread last looked.
    pending: AtomicU32,
    /// The last Error Source Identification read with them.
    sources: AtomicU32,
    /// Its DPC trigger has fired, and the thread has not handled it.
    contained: AtomicBool,
}

static PORTS: Once<Vec<Port, Kmalloc>> = Once::new();
static THREAD: Once<ThreadRef> = Once::new();

/// Turns on the error reporting of every function the kernel handles
/// the errors of, and starts watching their ports.
pub(super) fn start(entries: &'static [Entry]) {
    for entry in entries.iter().filter(|entry| entry.native()) {
        enable_reporting(entry);
    }
    let heap = HEAP.with(Flags::MAY_SLEEP);
    let mut ports = Vec::new_in(heap);
    let wanted = entries.iter().filter(|entry| entry.native() && is_port(entry)).count();
    if ports.try_reserve_exact(wanted).is_err() {
        warn!("PCIe: no memory to watch the root ports; their errors are not read");
        return;
    }
    for entry in entries.iter().filter(|entry| entry.native() && is_port(entry)) {
        let dpc = if entry.port.is_downstream() && entry.control & osc::DPC != 0 {
            entry.function.find_extended_capability(extended::DPC)
        } else {
            None
        };
        let root = entry.aer.as_ref().map(|(at, _)| *at).filter(|_| entry.port.is_root());
        if root.is_none() && dpc.is_none() {
            continue;
        }
        let _ = ports.push_within_capacity(Port {
            entry,
            root,
            dpc,
            polled: AtomicBool::new(true),
            pending: AtomicU32::new(0),
            sources: AtomicU32::new(0),
            contained: AtomicBool::new(false),
        });
    }
    if ports.is_empty() {
        if entries.iter().any(Entry::native) {
            info!("PCIe: the kernel handles PCIe errors, but no root port reports them");
        }
        return;
    }
    let Ok(ports) = PORTS.set(ports) else { return };
    let (mut roots, mut contained) = (0, 0);
    for (index, port) in ports.iter().enumerate() {
        roots += usize::from(port.root.is_some());
        contained += usize::from(port.dpc.is_some());
        watch(port, index);
    }
    let functions = entries
        .iter()
        .filter(|entry| entry.native() && !entry.port.is_root())
        .filter(|entry| {
            ports.iter().any(|port| port.root.is_some() && reports_to(entry, port.entry))
        })
        .count();
    let options = Options::new().policy(Policy::Fair { nice: -5 });
    if let Err(err) = sched::spawn("pcie-aer", run, 0, options) {
        error!("PCIe: could not start the thread `pcie-aer`: {err}; PCIe errors are not read");
        return;
    }
    info!(
        "PCIe: AER watches {roots} root port{}, {contained} with DPC, and {functions} function{} \
         below {}",
        if roots == 1 { "" } else { "s" },
        if functions == 1 { "" } else { "s" },
        if roots == 1 { "it" } else { "them" },
    );
}

/// Whether `entry` is a port the kernel may watch: a root port, an event
/// collector, or a switch's downstream port.
fn is_port(entry: &Entry) -> bool {
    entry.port.is_root() || entry.port == PortType::Downstream
}

/// Turns on `entry`'s error reporting, and clears the errors logged
/// before the kernel watched (the firmware's start leaves some, as link
/// training's receiver errors). A bridge passes the messages from below
/// it on.
fn enable_reporting(entry: &Entry) {
    let function = &entry.function;
    if let Some((aer, _)) = entry.aer {
        let correctable = function.read32(aer + regs::CORRECTABLE_STATUS);
        let uncorrectable = function.read32(aer + regs::UNCORRECTABLE_STATUS);
        if correctable != u32::MAX && uncorrectable != u32::MAX {
            if correctable | uncorrectable != 0 {
                debug!(
                    "PCIe: {} had errors logged before the kernel watched: corrected {}; \
                     uncorrected {}",
                    entry.address(),
                    Names { bits: correctable, correctable: true },
                    Names { bits: uncorrectable, correctable: false },
                );
            }
            function.write32(aer + regs::CORRECTABLE_STATUS, correctable);
            function.write32(aer + regs::UNCORRECTABLE_STATUS, uncorrectable);
        }
    }
    let status = entry.express + express::DEVICE_STATUS;
    function.write16(status, express::DEVICE_STATUS_ERRORS);
    let control = entry.express + express::DEVICE_CONTROL;
    function.write16(control, function.read16(control) | express::REPORTING);
    if entry.below.is_some() {
        let bridge = function.read16(BRIDGE_CONTROL);
        if bridge & BRIDGE_CONTROL_SERR == 0 {
            function.write16(BRIDGE_CONTROL, bridge | BRIDGE_CONTROL_SERR);
        }
    }
}

/// Starts watching `port`, number `index`: its messages go to the kernel,
/// through its interrupt if it gets one.
fn watch(port: &Port, index: usize) {
    let entry = port.entry;
    let function = &entry.function;
    let mut messages = [0u8; 2];
    let mut count = 0;
    if let Some(aer) = port.root {
        // The firmware no longer hears of them as system errors.
        let control = entry.express + express::ROOT_CONTROL;
        function.write16(control, function.read16(control) & !express::SYSTEM_ERRORS);
        let status = function.read32(aer + regs::ROOT_STATUS);
        function.write32(aer + regs::ROOT_STATUS, status & root::RECEIVED);
        messages[count] = root::interrupt_message(status);
        count += 1;
    }
    if let Some(dpc) = port.dpc {
        messages[count] = dpc_regs::interrupt_message(function.read16(dpc + dpc_regs::CAPABILITY));
        count += 1;
    }
    let vector = interrupt(function, index, &messages[..count]);
    // Before the thread starts, which reads it.
    port.polled.store(vector.is_none(), Ordering::Release);
    match vector {
        Some(vector) => debug!("PCIe: {} reports errors at vector {vector:#x}", entry.address()),
        None => debug!("PCIe: {} has no interrupt; it is read every second", entry.address()),
    }
    // The interrupts go on only with a vector for them: a polled port's
    // would be its legacy INTx, which nothing here handles and which may
    // share a line with a driver's.
    if let Some(aer) = port.root {
        let command = aer + regs::ROOT_COMMAND;
        let enables = function.read32(command);
        let enables = if vector.is_some() {
            enables | regs::ROOT_COMMAND_ALL
        } else {
            enables & !regs::ROOT_COMMAND_ALL
        };
        function.write32(command, enables);
    }
    if let Some(dpc) = port.dpc {
        let status = function.read16(dpc + dpc_regs::STATUS);
        if status != u16::MAX && status & dpc_regs::INTERRUPT != 0 {
            function.write16(dpc + dpc_regs::STATUS, dpc_regs::INTERRUPT);
        }
        if status != u16::MAX && status & dpc_regs::TRIGGERED != 0 {
            port.contained.store(true, Ordering::Release);
        }
        let control = dpc + dpc_regs::CONTROL;
        let mut wanted = dpc_regs::control_for_fatal(function.read16(control));
        if vector.is_none() {
            wanted &= !dpc_regs::INTERRUPT_ENABLE;
        }
        function.write16(control, wanted);
    }
}

/// Gives `function` (port number `index`) one vector for its interrupt
/// `messages`: MSI-X's entries of those numbers, or MSI's one message.
/// Returns the vector, or `None` when it has neither or none is free.
fn interrupt(function: &Function, index: usize, messages: &[u8]) -> Option<u32> {
    let source = function.address();
    let on = |function: &Function| {
        function.set_command(function.command() | command::BUS_MASTER | command::INTX_DISABLE);
    };
    if let Some(msix) = MsiX::find(function)
        && messages.iter().all(|&message| u16::from(message) < msix.entries())
        && let (bar, offset) = msix.table_location()
        && let Some(base) = function.bar_address(bar).filter(|&base| base != 0)
        && function.command() & command::MEMORY != 0
        && let Some(table) = <Arch as Devices>::map_device(base + offset, msix.table_len())
    {
        let (vector, message) = <Arch as Devices>::allocate_interrupt(source, on_interrupt, index)?;
        on(function);
        msix.enable_masked(function);
        for &entry in messages {
            msix.set_entry(&table, u16::from(entry), message);
        }
        msix.unmask_all(function);
        return Some(vector);
    }
    let msi = Msi::find(function)?;
    let (vector, message) = <Arch as Devices>::allocate_interrupt(source, on_interrupt, index)?;
    on(function);
    msi.enable(function, message);
    Some(vector)
}

/// Port `index`'s interrupt.
fn on_interrupt(index: usize) {
    let Some(port) = PORTS.get().and_then(|ports| ports.get(index)) else { return };
    if port.collect()
        && let Some(thread) = THREAD.get()
    {
        sched::wake(thread);
    }
}

impl Port {
    /// Whether it has no interrupt, and is read every second.
    fn polled(&self) -> bool {
        self.polled.load(Ordering::Acquire)
    }

    /// Takes what the port's registers say came, and clears it so that
    /// the port can interrupt again; returns whether the thread has work.
    /// From its interrupt handler, or the thread for a polled port.
    fn collect(&self) -> bool {
        let function = &self.entry.function;
        let mut work = false;
        if let Some(aer) = self.root {
            let status = function.read32(aer + regs::ROOT_STATUS);
            let received = status & root::RECEIVED;
            // All ones: the port has gone.
            if status != u32::MAX && received != 0 {
                let sources = function.read32(aer + regs::ERROR_SOURCE);
                function.write32(aer + regs::ROOT_STATUS, received);
                self.sources.store(sources, Ordering::Relaxed);
                self.pending.fetch_or(received, Ordering::AcqRel);
                // As Linux counts them for its aer_rootport_total_err_*.
                let received_by = &self.entry.received;
                if received & root::CORRECTABLE != 0 {
                    received_by[0].fetch_add(1, Ordering::Relaxed);
                }
                if received & root::UNCORRECTABLE != 0 {
                    let fatal = received & root::FATAL_RECEIVED != 0;
                    received_by[if fatal { 2 } else { 1 }].fetch_add(1, Ordering::Relaxed);
                }
                work = true;
            }
        }
        if let Some(dpc) = self.dpc {
            let status = function.read16(dpc + dpc_regs::STATUS);
            if status != u16::MAX {
                if status & dpc_regs::INTERRUPT != 0 {
                    function.write16(dpc + dpc_regs::STATUS, dpc_regs::INTERRUPT);
                }
                if status & dpc_regs::TRIGGERED != 0 {
                    self.contained.store(true, Ordering::Release);
                    work = true;
                }
            }
        }
        work
    }
}

/// The `pcie-aer` thread: handles what the ports' interrupts collect, and
/// reads the polled ports every second.
fn run(_: usize) {
    if let Some(me) = sched::current() {
        let _ = THREAD.set(me);
    }
    let Some(ports) = PORTS.get() else { return };
    let polled = ports.iter().any(Port::polled);
    loop {
        for port in ports {
            if port.polled() {
                port.collect();
            }
            if port.contained.swap(false, Ordering::AcqRel) {
                super::dpc::contain(port);
            }
            service(port);
        }
        let wait = if polled { POLL_NS } else { 3_600_000_000_000 };
        sched::block_until(sched::now_ns().saturating_add(wait), "pcie-aer");
    }
}

/// Handles the error messages `port` collected.
fn service(port: &Port) {
    let status = port.pending.swap(0, Ordering::AcqRel);
    if status == 0 {
        return;
    }
    let sources = port.sources.load(Ordering::Relaxed);
    let segment = port.entry.address().segment;
    if status & root::CORRECTABLE != 0 {
        let source = requester(segment, sources as u16);
        each_below(port, source, &mut |entry| corrected(entry, port.entry), "a corrected");
    }
    if status & root::UNCORRECTABLE != 0 {
        let source = requester(segment, (sources >> 16) as u16);
        each_below(port, source, &mut |entry| uncorrected(entry, port.entry), "an uncorrected");
    }
}

/// Runs `handle` on the function `source` names, then on every other
/// function the kernel handles below `port` (and the port itself), as a
/// message that came while the thread was busy names only the first;
/// says so when none had anything to tell.
fn each_below(
    port: &Port,
    source: radix_hw::pci::Address,
    handle: &mut dyn FnMut(&'static Entry) -> bool,
    kind: &str,
) {
    let mut told = false;
    let named = entry(source).filter(|entry| entry.native());
    if let Some(named) = named {
        told |= handle(named);
    }
    let Some(entries) = super::ENTRIES.get() else { return };
    let here = port.entry;
    for entry in entries.iter().filter(|entry| entry.native()) {
        let mine = core::ptr::eq(entry, here) || reports_to(entry, here);
        if mine && !named.is_some_and(|named| core::ptr::eq(named, entry)) {
            told |= handle(entry);
        }
    }
    if !told {
        match named {
            Some(named) if named.aer.is_none() => warn!(
                "PCIe: {source} ({:04x}:{:04x}) sent {kind} error message, but has no AER \
                 capability to say what it was; via {}",
                named.ids.0,
                named.ids.1,
                here.address()
            ),
            _ => warn!(
                "PCIe: {} received {kind} error message from {source}, and no function below it \
                 says what it was",
                here.address()
            ),
        }
    }
}

/// Whether `entry`'s error messages reach `port`: it is on a bus below a
/// root port, or it is one of the root complex's own functions on an
/// event collector's bus (the collector's association capability is not
/// read).
fn reports_to(entry: &Entry, port: &Entry) -> bool {
    entry.is_below(port)
        || (port.port == PortType::EventCollector
            && entry.port == PortType::Integrated
            && entry.address().segment == port.address().segment
            && entry.address().bus == port.address().bus)
}

/// What a corrected error was, for the log and the record.
struct Corrected {
    /// The unmasked correctable status.
    bits: u32,
    /// The non-fatal uncorrectable errors it reported as advisory.
    advisory: u32,
}

impl fmt::Display for Corrected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "corrected error: {}", Names { bits: self.bits, correctable: true })?;
        if self.advisory != 0 {
            write!(f, " (advisory: {})", Names { bits: self.advisory, correctable: false })?;
        }
        Ok(())
    }
}

/// What an uncorrected error was, for the log and the record.
struct Uncorrected {
    /// The unmasked uncorrectable status bits of one severity.
    bits: u32,
    /// Whether they are fatal.
    fatal: bool,
    /// The first error's bit, when it is one of them and not alone.
    first: Option<u8>,
    /// The TLP of the first error, when the function logged it.
    header: Option<[u32; 4]>,
}

impl fmt::Display for Uncorrected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let severity = if self.fatal { "fatal" } else { "non-fatal" };
        write!(
            f,
            "uncorrected {severity} error: {}",
            Names { bits: self.bits, correctable: false }
        )?;
        if let Some(first) = self.first {
            write!(f, " (first {})", Names { bits: 1 << first, correctable: false })?;
        }
        if let Some([a, b, c, d]) = self.header {
            write!(f, "; header {a:08x} {b:08x} {c:08x} {d:08x}")?;
        }
        Ok(())
    }
}

/// Handles `entry`'s corrected errors, if it has any; returns whether it
/// had.
fn corrected(entry: &Entry, via: &Entry) -> bool {
    let Some((aer, counters)) = entry.aer.as_ref() else { return false };
    let function = &entry.function;
    let status = function.read32(aer + regs::CORRECTABLE_STATUS);
    let bits = status & !function.read32(aer + regs::CORRECTABLE_MASK) & 0xffff;
    if status == u32::MAX || bits == 0 {
        return false;
    }
    function.write32(aer + regs::CORRECTABLE_STATUS, bits);
    // Advisory Non-Fatal: an uncorrectable error of non-fatal severity the
    // function reported as correctable (PCIe 6.0, 6.2.3.2.4); its
    // uncorrectable status bit is cleared with it.
    let mut advisory = 0;
    if bits & ADVISORY != 0 {
        let uncorrectable = function.read32(aer + regs::UNCORRECTABLE_STATUS);
        if uncorrectable != u32::MAX {
            advisory = uncorrectable
                & !function.read32(aer + regs::UNCORRECTABLE_MASK)
                & !function.read32(aer + regs::UNCORRECTABLE_SEVERITY);
            function.write32(aer + regs::UNCORRECTABLE_STATUS, advisory);
        }
    }
    function.write16(entry.express + express::DEVICE_STATUS, express::CORRECTABLE_DETECTED);
    counters.lock().add_correctable(bits);
    report(entry, via, Severity::Corrected, &Corrected { bits, advisory }, "");
    true
}

/// The correctable status's Advisory Non-Fatal bit.
const ADVISORY: u32 = 1 << 13;

/// Handles `entry`'s uncorrected errors, if it has any: a fatal one marks
/// it failed. Returns whether it had. Also [`super::dpc`]'s, for a port's
/// own errors.
pub(super) fn uncorrected(entry: &Entry, via: &Entry) -> bool {
    let Some((aer, counters)) = entry.aer.as_ref() else { return false };
    let function = &entry.function;
    let status = function.read32(aer + regs::UNCORRECTABLE_STATUS);
    let bits = status & !function.read32(aer + regs::UNCORRECTABLE_MASK);
    if status == u32::MAX || bits == 0 {
        return false;
    }
    let severity = function.read32(aer + regs::UNCORRECTABLE_SEVERITY);
    let first = regs::first_error(function.read32(aer + regs::CONTROL));
    let first_bit = 1u32.checked_shl(u32::from(first)).unwrap_or(0) & bits;
    // Read before the status is cleared, which frees the log.
    let header = (first_bit & regs::HEADER_LOGGED != 0)
        .then(|| core::array::from_fn(|word| function.read32(aer + regs::HEADER_LOG + 4 * word)));
    function.write32(aer + regs::UNCORRECTABLE_STATUS, bits);
    let device_errors =
        express::NONFATAL_DETECTED | express::FATAL_DETECTED | express::UNSUPPORTED_DETECTED;
    function.write16(entry.express + express::DEVICE_STATUS, device_errors);
    for fatal in [false, true] {
        let these = if fatal { bits & severity } else { bits & !severity };
        if these == 0 {
            continue;
        }
        counters.lock().add_uncorrectable(these, fatal);
        let mine = first_bit & these != 0;
        let what = Uncorrected {
            bits: these,
            fatal,
            first: (mine && these.count_ones() > 1).then_some(first),
            header: header.filter(|_| mine),
        };
        if fatal {
            entry.failed.store(true, Ordering::Relaxed);
            report(entry, via, Severity::Fatal, &what, "; the device is marked failed");
        } else {
            report(entry, via, Severity::Recoverable, &what, "");
        }
    }
    true
}

/// The part a PCIe error names in the record, as the firmware's records
/// name it ([`crate::apei`]).
struct Part(radix_hw::pci::Address);

impl fmt::Display for Part {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PCIe {}", self.0)
    }
}

/// Records an error of the function at `address` in `/kernel/ras`, and
/// says whether the log has room for its line (and logs how many the
/// last minute left out).
pub(super) fn record(
    address: radix_hw::pci::Address,
    severity: Severity,
    what: &dyn fmt::Display,
) -> bool {
    let key = ras::KEY_PCIE
        | u64::from(address.segment) << 16
        | u64::from(address.bus) << 8
        | u64::from(address.device & 0x1f) << 3
        | u64::from(address.function & 0x7);
    let log = ras::record(&Report {
        source: Source::Pcie,
        severity,
        kind: Kind::Pcie,
        count: 1,
        dimm: None,
        part: &Part(address),
        what,
        key,
    });
    let Log::Line { left_out } = log else { return false };
    if left_out > 0 {
        warn!(
            "PCIe: {left_out} more errors of {address} in the last minute were left out of the log"
        );
    }
    true
}

/// Records an error of `entry`'s, reported through port `via`, and logs
/// it within the record's limit, with `tail` after the port.
fn report(entry: &Entry, via: &Entry, severity: Severity, what: &dyn fmt::Display, tail: &str) {
    let address = entry.address();
    if !record(address, severity, what) {
        return;
    }
    let (vendor, device) = entry.ids;
    let via = via.address();
    if severity.corrected() {
        warn!("PCIe: {address} ({vendor:04x}:{device:04x}) {what}; via {via}{tail}");
    } else {
        error!("PCIe: {address} ({vendor:04x}:{device:04x}) {what}; via {via}{tail}");
    }
}
