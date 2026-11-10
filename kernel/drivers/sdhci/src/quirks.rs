// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What some controllers need beyond the specification (ADR 0445), by
//! their PCI vendor and device, as the BSDs' drivers record it: FreeBSD's
//! `sys/dev/sdhci/sdhci_pci.c`, OpenBSD's and NetBSD's
//! `sys/dev/pci/sdhc_pci.c`. [`quirks`] looks a controller up; the kernel
//! writes [`RICOH_MODE`] to a Ricoh's configuration space before it reads
//! the slots, and [`crate::Slot::new`] takes the rest.
//!
//! A controller not in the table is driven as the specification says,
//! O2 Micro's among them: none of the BSDs records a quirk of theirs.

/// What a controller needs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Quirks {
    /// Its name, for the log.
    pub name: Option<&'static str>,
    /// Its ADMA2 does not work, whatever its capabilities say: transfers
    /// go by SDMA (NetBSD).
    pub broken_adma: bool,
    /// Its bus power comes on only when the voltage and the power bit are
    /// written together (NetBSD).
    pub single_power_write: bool,
    /// Its command and data lines are reset after each transfer, to keep
    /// it stable (FreeBSD).
    pub reset_after_request: bool,
    /// [`RICOH_MODE`] is written to its configuration space: SD 2.0 mode,
    /// with a base clock of 50 MHz, as some cards do not work at its
    /// 200 MHz (OpenBSD, FreeBSD).
    pub ricoh_mode: bool,
}

/// The PCI vendor IDs of Ricoh and JMicron.
const RICOH: u16 = 0x1180;
const JMICRON: u16 = 0x197b;

/// The controllers that need something.
const TABLE: [(u16, u16, Quirks); 4] = [
    (
        RICOH,
        0x0822,
        Quirks {
            name: Some("Ricoh R5C822"),
            broken_adma: true,
            single_power_write: false,
            reset_after_request: false,
            ricoh_mode: false,
        },
    ),
    (
        RICOH,
        0xe822,
        Quirks {
            name: Some("Ricoh R5U822"),
            broken_adma: true,
            single_power_write: false,
            reset_after_request: false,
            ricoh_mode: true,
        },
    ),
    (
        RICOH,
        0xe823,
        Quirks {
            name: Some("Ricoh R5U823"),
            broken_adma: true,
            single_power_write: true,
            reset_after_request: false,
            ricoh_mode: true,
        },
    ),
    (
        JMICRON,
        0x2381,
        Quirks {
            name: Some("JMicron JMB38x"),
            broken_adma: false,
            single_power_write: false,
            reset_after_request: true,
            ricoh_mode: false,
        },
    ),
];

/// What the controller of PCI `vendor` and `device` needs: nothing for
/// one the table does not name.
pub fn quirks(vendor: u16, device: u16) -> Quirks {
    TABLE
        .iter()
        .find(|&&(v, d, _)| v == vendor && d == device)
        .map_or_default(|&(_, _, quirks)| quirks)
}

/// The configuration space writes that put a Ricoh R5U822 or R5U823 in
/// SD 2.0 mode with a base clock of 50 MHz, in order: each register is
/// unlocked by a key, written, and locked again. The R5U823 then calls
/// itself an R5U822.
pub const RICOH_MODE: [(usize, u8); 6] = [
    // The mode's key, then SD 2.0 mode.
    (0xf9, 0xfc),
    (0x150, 0x10),
    (0xf9, 0x00),
    // The base clock's key, then 50 MHz.
    (0xfc, 0x01),
    (0xe1, 50),
    (0xfc, 0x00),
];
