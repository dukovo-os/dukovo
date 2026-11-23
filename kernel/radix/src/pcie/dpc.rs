// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Downstream Port Containment (PCIe 6.0, 6.2.11 and 7.9.14): a port
//! that receives ERR_FATAL from below takes its link down at once, so
//! that nothing the failed device sends goes further.
//!
//! The port interrupts ([`super::aer`] sets it up and hears it). Here the
//! kernel says why the port fired and for whom, reads the port's own
//! uncorrected errors, waits for the link to go down (up to a second),
//! and, where the port has the root port extensions, for it to finish
//! what it had started; then it clears the trigger, which lets the link
//! train again, waits the 100 ms the specification asks before the
//! device below is spoken to, and up to a second for the link. The
//! device below has been reset by then, so its functions are marked
//! failed: until a driver can be told to set its device up again (step
//! 14), it cannot use it. Each containment is recorded in `/kernel/ras`
//! as a fatal error of the function that sent the message.

use core::fmt::{self, Write};
use core::sync::atomic::Ordering;

use radix_hw::pci::Address;
use radix_hw::pcie::dpc::{self, Reason};
use radix_hw::pcie::express;
use radix_klog::{TextBuf, error};
use radix_mca::record::Severity;

use super::aer::{self, Port};
use super::{Entry, requester};
use crate::sched;

/// How long the link has to go down, and to come back, in nanoseconds.
const LINK_NS: u64 = 1_000_000_000;
/// How long after the trigger is cleared before the device below may be
/// spoken to (PCIe 6.0, 6.6.1), in nanoseconds.
const SETTLE_NS: u64 = 100_000_000;
/// How often the link is looked at while waiting, in nanoseconds.
const LOOK_NS: u64 = 10_000_000;

/// What a containment was, for the record.
struct Contained {
    /// Why the port fired.
    reason: Reason,
    /// The port.
    port: Address,
}

impl fmt::Display for Contained {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "uncorrected fatal error contained by {}: {}", self.port, self.reason)
    }
}

/// Handles `port`'s containment, if its trigger has fired. From the
/// `pcie-aer` thread.
pub(super) fn contain(port: &Port) {
    let Some(at) = port.dpc else { return };
    let entry = port.entry;
    let function = &entry.function;
    let status = function.read16(at + dpc::STATUS);
    if status == u16::MAX || status & dpc::TRIGGERED == 0 {
        return;
    }
    let reason = dpc::reason(status);
    let address = entry.address();
    // The source register names the sender of a message; for the other
    // reasons the error is the port's own.
    let source = match reason {
        Reason::Fatal | Reason::Nonfatal => {
            requester(address.segment, function.read16(at + dpc::SOURCE))
        }
        _ => address,
    };
    aer::uncorrected(entry, entry);
    wait_for_link(entry, false);
    if function.read16(at + dpc::CAPABILITY) & dpc::RP_EXTENSIONS != 0 {
        let busy = || function.read16(at + dpc::STATUS) & dpc::RP_BUSY != 0;
        wait(busy, LINK_NS);
    }
    function.write16(at + dpc::STATUS, dpc::TRIGGERED);
    sched::sleep_ns(SETTLE_NS);
    let back = wait_for_link(entry, true);
    let marked = mark_below(entry);
    if !aer::record(source, Severity::Fatal, &Contained { reason, port: address }) {
        return;
    }
    let link = if back { "the link was reset" } else { "the link was reset and is not back" };
    let mut failed = TextBuf::<48>::new();
    let _ = match marked {
        0 => Ok(()),
        1 => write!(failed, "; the device below is marked failed"),
        _ => write!(failed, "; the {marked} functions below are marked failed"),
    };
    error!("PCIe: {address} contained an error below it ({reason}, from {source}): {link}{failed}");
}

/// Waits up to [`LINK_NS`] for `port`'s link to be `up` or down; returns
/// whether it is. A port that cannot say (no Data Link Layer Link Active
/// reporting, which ports with DPC must have) is given the time and
/// taken to be.
fn wait_for_link(port: &Entry, up: bool) -> bool {
    let function = &port.function;
    let capabilities = function.read32(port.express + express::LINK_CAPABILITIES);
    if capabilities & express::LINK_ACTIVE_REPORTING == 0 {
        sched::sleep_ns(if up { LINK_NS } else { SETTLE_NS });
        return true;
    }
    let status = port.express + express::LINK_STATUS;
    wait(|| (function.read16(status) & express::LINK_ACTIVE != 0) != up, LINK_NS)
}

/// Waits up to `ns` for `pending` to be false; returns whether it is.
fn wait(pending: impl Fn() -> bool, ns: u64) -> bool {
    let deadline = sched::now_ns().saturating_add(ns);
    while pending() {
        if sched::now_ns() >= deadline {
            return false;
        }
        sched::sleep_ns(LOOK_NS);
    }
    true
}

/// Marks every function below `port` failed; returns how many.
fn mark_below(port: &Entry) -> usize {
    let Some(entries) = super::ENTRIES.get() else { return 0 };
    let mut marked = 0;
    for entry in entries.iter().filter(|entry| entry.is_below(port)) {
        entry.failed.store(true, Ordering::Relaxed);
        marked += 1;
    }
    marked
}
