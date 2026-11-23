// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Who handles each host bridge's PCIe errors: the kernel, if the
//! bridge's `_OSC` gives it AER, else the firmware (ADR 0663).
//!
//! The PCI Firmware specification (3.3, 4.5) has the kernel say what it
//! supports and ask for the features it wants to drive: first as a query,
//! which changes nothing, then for real, for what the query offered. The
//! kernel asks for AER, the PCIe capability (which AER's device control
//! writes need) and DPC; native hot-plug waits for step 14, and PME and
//! LTR for the power work that drives them. Whether the firmware keeps
//! AER is the bridge's `_OSC`'s answer alone, as in Linux since 5.8: a
//! bridge with no `_OSC` keeps it, and its errors arrive as the
//! firmware's error records ([`crate::apei`]).

use core::fmt::Display;

use radix_hw::pcie::osc::{self, Granted};

pub use radix_hw::pcie::osc::{AER, DPC};
use radix_klog::{info, warn};
use radix_sync::SpinLock;
use radix_uacpi::Node;

/// The most host bridges whose answer is kept: an eight-socket machine
/// has about fifty. The errors of one past it are not watched, and a
/// warning says so.
const MAX_BRIDGES: usize = 64;

/// What one host bridge gave the kernel.
#[derive(Clone, Copy)]
struct Grant {
    /// Its segment.
    segment: u16,
    /// The bus numbers it decodes, first and last.
    buses: (u8, u8),
    /// The `_OSC` control bits it granted.
    control: u32,
}

static GRANTS: SpinLock<[Option<Grant>; MAX_BRIDGES]> = SpinLock::new([None; MAX_BRIDGES]);

/// Asks host bridge `bridge` (named `name`, root of `buses` in `segment`)
/// for control of AER, the PCIe capability and DPC, records what it
/// gives, and logs it. From a thread that may sleep, once uACPI runs.
pub fn negotiate(bridge: Node, name: &dyn Display, segment: u16, buses: (u8, u8)) {
    let control = match ask(bridge) {
        Ok(control) => control,
        Err(Refusal::NoOsc) => {
            info!(
                "PCI: ACPI host bridge {name} has no _OSC: PCIe errors come through the firmware"
            );
            return;
        }
        Err(Refusal::Failed(why)) => {
            warn!(
                "PCI: ACPI host bridge {name}'s _OSC failed ({why}): PCIe errors come through the \
                 firmware"
            );
            return;
        }
    };
    if control & osc::AER == 0 {
        info!("PCI: ACPI host bridge {name} keeps AER: PCIe errors come through the firmware");
    } else {
        info!("PCI: ACPI host bridge {name} gives the kernel {}", Granted(control));
    }
    if control == 0 {
        return;
    }
    let kept = {
        let mut grants = GRANTS.lock();
        let free = grants.iter_mut().find(|slot| slot.is_none());
        free.map(|slot| *slot = Some(Grant { segment, buses, control })).is_some()
    };
    if !kept {
        warn!(
            "PCI: no room to keep what ACPI host bridge {name} gave; its PCIe errors are not watched"
        );
    }
}

/// Why a host bridge gave nothing.
enum Refusal {
    /// It has no `_OSC`.
    NoOsc,
    /// Its `_OSC` failed, or said it does not know the question.
    Failed(&'static str),
}

/// The query, then the request for what the query offered; returns the
/// control bits granted.
fn ask(bridge: Node) -> Result<u32, Refusal> {
    let mut query = osc::dwords(true, osc::ASKED);
    match bridge.osc(&osc::UUID, osc::REVISION, &mut query) {
        Ok(()) => {}
        Err(err) if err.is_not_found() => return Err(Refusal::NoOsc),
        Err(_) => return Err(Refusal::Failed("it could not be run")),
    }
    complaint(query[0])?;
    let wanted = osc::to_take(query[2]);
    if wanted == 0 {
        return Ok(0);
    }
    let mut request = osc::dwords(false, wanted);
    if bridge.osc(&osc::UUID, osc::REVISION, &mut request).is_err() {
        return Err(Refusal::Failed("the request after the query could not be run"));
    }
    complaint(request[0])?;
    // Only what was asked for counts, whatever else the answer has set.
    Ok(request[2] & wanted)
}

/// The firmware's complaint in the first dword of its answer, if it has
/// one that leaves the answer unusable. A masked capability is not one:
/// the third dword says what was granted.
fn complaint(status: u32) -> Result<(), Refusal> {
    if status & osc::UNKNOWN_UUID != 0 {
        return Err(Refusal::Failed("it does not know the PCI host bridge question"));
    }
    if status & osc::UNKNOWN_REVISION != 0 {
        return Err(Refusal::Failed("it does not know revision 1 of the question"));
    }
    if status & osc::FAILED != 0 {
        return Err(Refusal::Failed("it says it failed"));
    }
    Ok(())
}

/// The `_OSC` control bits granted for the functions under root bus
/// `root` of `segment`: those of the host bridge whose buses hold it, 0
/// when none gave any.
pub fn control(segment: u16, root: u8) -> u32 {
    GRANTS
        .lock()
        .iter()
        .flatten()
        .find(|grant| grant.segment == segment && (grant.buses.0..=grant.buses.1).contains(&root))
        .map_or(0, |grant| grant.control)
}
