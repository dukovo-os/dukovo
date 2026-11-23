// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The firmware's PCI host bridges: which root bus each is, and which
//! NUMA node it sits on (Phase 19, ADR 0660).
//!
//! The PCI scan (`fs::disk`) runs before uACPI starts, so it cannot ask
//! the namespace where the root buses are; it finds them by probing
//! every bus of each configuration window instead, as Linux does where
//! ACPI names none. [`find`] runs once uACPI does. Each present
//! `PNP0A08` or `PNP0A03` device is a host bridge: its `_SEG` names the
//! segment, and the first of the bus numbers its `_CRS` decodes names
//! the root bus, or its `_BBN` where `_CRS` has none, as in Linux (some
//! firmware's `_BBN` is wrong). Its `_PXM` (or the nearest parent's, as
//! Linux's `acpi_get_node` looks) names the proximity domain the SRAT
//! maps to a node. The node goes to `fs::disk::set_numa_node`, which
//! `/kernel/bus/pci/devices/<address>/numa_node` reads.
//!
//! One info line per host bridge says what it names. A root the firmware
//! names that the scan found nothing on, and a root the scan found that
//! no host bridge names (uncore buses on a Xeon E5 are like that), are
//! logged too. The boot test `pci-roots` covers it.
//!
//! Each host bridge is then asked, through its `_OSC`, for control of
//! PCIe error handling ([`pcie::osc`], ADR 0663), and says what it gives
//! in a line of its own.

use radix_acpi::resources::{Buses, Resource, Resources};
use radix_hal::Devices;
use radix_klog::{debug, info};
use radix_uacpi::Node;

use crate::fs::disk;
use crate::{Arch, pcie};

/// The most host bridges remembered for the check against the scan.
const MAX_BRIDGES: usize = 64;

/// The ACPI proximity domain of `bridge`: its own `_PXM`, or the nearest
/// parent's.
fn proximity_domain(bridge: Node) -> Option<u32> {
    let mut node = Some(bridge);
    while let Some(current) = node {
        if let Ok(domain) = current.integer(c"_PXM") {
            return u32::try_from(domain).ok();
        }
        node = current.parent();
    }
    None
}

/// The buses of `bridge`, first (its root bus) and last: those its `_CRS`
/// decodes, else its `_BBN` alone, else bus 0 as the ACPI specification
/// has it. A value that does not fit a bus number is read as absent.
fn buses(bridge: Node) -> (u8, u8) {
    let decoded = bridge.bytes(c"_CRS", |bytes| {
        Resources::new(bytes).find_map(|resource| match resource {
            Ok(Resource::Buses(Buses { first, last })) => Some((first, last.max(first))),
            _ => None,
        })
    });
    if let Ok(Some(buses)) = decoded {
        return buses;
    }
    let root = bridge.integer(c"_BBN").ok().and_then(|n| u8::try_from(n).ok()).unwrap_or(0);
    (root, root)
}

/// Finds the host bridges, gives their root buses their nodes, and logs
/// what they name. Called once uACPI runs, from a thread that may sleep.
pub fn find() {
    let mut named = [None; MAX_BRIDGES];
    let mut count = 0;
    radix_uacpi::find_devices(c"PNP0A03", &mut |bridge| {
        if !bridge.present() {
            return;
        }
        // A bridge without `_SEG` is segment 0, as in the ACPI
        // specification. A value that does not fit is read as absent.
        let segment = bridge.integer(c"_SEG").ok().and_then(|n| u16::try_from(n).ok());
        let (segment, buses) = (segment.unwrap_or(0), buses(bridge));
        let root = buses.0;
        let node = proximity_domain(bridge).and_then(<Arch as Devices>::numa_node_of_domain);
        let name = bridge.name_text();
        match node {
            Some(node) => {
                disk::set_numa_node(segment, root, node);
                info!(
                    "PCI: ACPI host bridge {name} is root bus {segment:04x}:{root:02x}, node {node}"
                );
            }
            None => {
                info!("PCI: ACPI host bridge {name} is root bus {segment:04x}:{root:02x}, no node");
            }
        }
        // Who handles its PCIe errors (ADR 0663).
        pcie::osc::negotiate(bridge, &name, segment, buses);
        if !disk::has_pci_root(segment, root) {
            info!(
                "PCI: the firmware names root bus {segment:04x}:{root:02x}, which has no devices"
            );
        }
        if count < MAX_BRIDGES {
            named[count] = Some((segment, root));
            count += 1;
        }
    });
    disk::pci_roots(|segment, root| {
        if !named.contains(&Some((segment, root))) {
            debug!("PCI: root bus {segment:04x}:{root:02x} is named by no ACPI host bridge");
        }
    });
}
