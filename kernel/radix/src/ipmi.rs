// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The server's baseboard management controller (BMC), through its KCS
//! interface (`radix_ipmi`, Phase 19, ADR 0661).
//!
//! [`find`] looks for it once uACPI has started (or found it is not to
//! run), in Linux's order: the
//! namespace's `IPI0001` device, whose `_IFT` must say KCS and whose
//! `_CRS` gives the registers' ports; then the SMBIOS structure of type
//! 38. The first that answers Get Device ID is kept and logged:
//!
//! ```text
//! IPMI: BMC Dell, firmware 3.40, IPMI 2.0, KCS at I/O 0xCA8 (spacing 4)
//! ```
//!
//! The ports go through [`IoWindow`]s of the two registers and no more.
//! Messages go one at a time, under a sleeping lock, so [`request`] is
//! for threads that may sleep; [`request_spinning`] is for the debugger,
//! which waits by spinning. The ACPI IPMI operation region, through
//! which AML would reach the BMC, is not handled.

use core::fmt::{self, Write};

use radix_acpi::resources::{Resource, Resources};
use radix_hal::{Platform as _, Timer as _};
use radix_hw::IoPorts;
use radix_ipmi::locate::{from_io_resources, from_smbios};
use radix_ipmi::{Bmc, DeviceId, Error, Interface, Platform};
use radix_klog::{TextBuf, info, warn};
use radix_smbios::{Entry, Table, kind};
use radix_sync::{Mutex, Once};

use crate::ioports::IoWindow;
use crate::{Arch, sched};

/// The BMC's two registers, and how the kernel waits for it.
pub struct Ports {
    data_port: u16,
    data: IoWindow,
    command_port: u16,
    command: IoWindow,
    /// Whether a wait gives the CPU up. The debugger's waits spin.
    sleeps: bool,
}

impl Ports {
    /// The registers of `interface`, a window of one port for each.
    fn new(interface: Interface) -> Ports {
        Ports {
            data_port: interface.data(),
            data: IoWindow::new(interface.data(), 1),
            command_port: interface.command(),
            command: IoWindow::new(interface.command(), 1),
            sleeps: true,
        }
    }
}

impl Platform for Ports {
    fn in8(&self, port: u16) -> u8 {
        if port == self.data_port {
            self.data.read8(0)
        } else if port == self.command_port {
            self.command.read8(0)
        } else {
            u8::MAX
        }
    }

    fn out8(&self, port: u16, value: u8) {
        if port == self.data_port {
            self.data.write8(0, value);
        } else if port == self.command_port {
            self.command.write8(0, value);
        }
    }

    fn wait_us(&self, us: u64) {
        // A sleep lasts at least a timer tick, far over a poll of tens
        // of microseconds.
        if self.sleeps && us >= 1000 {
            sched::sleep_ns(us * 1000);
        } else {
            Arch::delay_us(us);
        }
    }
}

/// The BMC found.
struct Found {
    bmc: Mutex<Bmc<Ports>>,
    id: DeviceId,
    interface: Interface,
}

static FOUND: Once<Found> = Once::new();

/// Why a message did not get through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failed {
    /// The machine has no BMC the kernel reaches.
    NotFound,
    /// Another message is in progress, and the caller may not wait.
    Busy,
    /// The BMC or its interface failed the exchange.
    Bmc(Error),
}

impl fmt::Display for Failed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failed::NotFound => f.write_str("there is no BMC"),
            Failed::Busy => f.write_str("the BMC is in use"),
            Failed::Bmc(err) => err.fmt(f),
        }
    }
}

/// Where firmware says the interface is, and which of its tables says so.
struct Candidate {
    interface: Interface,
    source: &'static str,
}

/// Looks for the BMC, logs it, and adds `/devices/ipmi0` for it. Called
/// once, after uACPI's start, from a thread that may sleep.
pub fn find() {
    let acpi = from_acpi().map(|interface| Candidate { interface, source: "ACPI IPI0001" });
    let smbios = from_smbios_table().map(|interface| Candidate { interface, source: "SMBIOS" });
    let mut tried = None;
    for candidate in [acpi, smbios].into_iter().flatten() {
        if tried == Some(candidate.interface) {
            continue;
        }
        tried = Some(candidate.interface);
        if try_candidate(&candidate) {
            return;
        }
    }
}

/// Probes the interface of `candidate`; keeps and logs the BMC if it
/// answers.
fn try_candidate(candidate: &Candidate) -> bool {
    let Candidate { interface, source } = *candidate;
    let mut bmc = Bmc::new(Ports::new(interface), interface);
    let id = match bmc.probe() {
        Ok(id) => id,
        Err(why) => {
            warn!(
                "IPMI: {source} gives a KCS interface at I/O {:#X} (spacing {}), but {why}; \
                 it is not used",
                interface.base, interface.spacing
            );
            return false;
        }
    };
    let mut maker = TextBuf::<24>::new();
    match radix_ipmi::manufacturer_name(id.manufacturer) {
        Some(name) => maker.push_str(name),
        None => {
            let _ = write!(maker, "{}", id.manufacturer);
        }
    }
    info!(
        "IPMI: BMC {}, firmware {}.{:02}, IPMI {}.{}, KCS at I/O {:#X} (spacing {})",
        maker.as_str(),
        id.firmware_major,
        id.firmware_minor,
        id.ipmi_version.0,
        id.ipmi_version.1,
        interface.base,
        interface.spacing,
    );
    let _ = FOUND.set(Found { bmc: Mutex::new(bmc), id, interface });
    crate::fs::ipmi::publish();
    true
}

/// The interface of the first `IPI0001` device of the namespace that
/// says it is KCS.
fn from_acpi() -> Option<Interface> {
    if !crate::acpi::running() {
        return None;
    }
    let mut found = None;
    radix_uacpi::find_devices(c"IPI0001", &mut |node| {
        // `_IFT` is the interface type: 1 is KCS (the ACPI specification's
        // IPMI device description).
        if found.is_some() || !node.present() || node.integer(c"_IFT") != Ok(1) {
            return;
        }
        let ranges = node.bytes(c"_CRS", |bytes| {
            let mut ranges = [(0, 0); 3];
            let mut count = 0;
            for resource in Resources::new(bytes) {
                if let Ok(Resource::Io(io)) = resource
                    && let Some(slot) = ranges.get_mut(count)
                {
                    *slot = (io.base, io.length);
                    count += 1;
                }
            }
            (ranges, count)
        });
        if let Ok((ranges, count)) = ranges {
            found = from_io_resources(&ranges[..count]);
        }
    });
    found
}

/// The interface the first usable SMBIOS type 38 structure gives.
fn from_smbios_table() -> Option<Interface> {
    let radix_hal::Smbios { entry, table } = Arch::smbios()?;
    let entry = Entry::parse(entry)?;
    let table = Table::new(table, entry.count);
    table
        .of(kind::IPMI_DEVICE)
        .find_map(|structure| from_smbios(structure.bytes(0, structure.len())?))
}

/// What Get Device ID said, and where the interface is, if there is a
/// BMC.
pub fn device() -> Option<(DeviceId, Interface)> {
    FOUND.get().map(|found| (found.id, found.interface))
}

/// Sends command `cmd` of network function `netfn` with `data` to the
/// BMC, and puts the completion code and the data after it in
/// `response`. Returns how many bytes that is. One message at a time:
/// this waits for another to finish, so only a thread that may sleep
/// calls it.
///
/// # Errors
///
/// [`Failed::NotFound`] without a BMC, and the interface's errors.
pub fn request(netfn: u8, cmd: u8, data: &[u8], response: &mut [u8]) -> Result<usize, Failed> {
    let found = FOUND.get().ok_or(Failed::NotFound)?;
    found.bmc.lock().request(netfn, cmd, data, response).map_err(Failed::Bmc)
}

/// As [`request`], for the debugger and what it runs: it does not wait
/// for another message (the thread it stopped may be in the middle of
/// one), and its own waits spin.
///
/// # Errors
///
/// [`Failed::Busy`] when a message is in progress, and those of
/// [`request`].
pub fn request_spinning(
    netfn: u8,
    cmd: u8,
    data: &[u8],
    response: &mut [u8],
) -> Result<usize, Failed> {
    let found = FOUND.get().ok_or(Failed::NotFound)?;
    let mut bmc = found.bmc.try_lock().ok_or(Failed::Busy)?;
    bmc.platform_mut().sleeps = false;
    let outcome = bmc.request(netfn, cmd, data, response);
    bmc.platform_mut().sleeps = true;
    outcome.map_err(Failed::Bmc)
}
