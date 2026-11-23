// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Where firmware says the KCS interface is: its ACPI device's `_CRS`,
//! or the SMBIOS structure of type 38. Both give I/O ports for the two
//! registers; interfaces in memory are not read.

/// Where a KCS interface's registers are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interface {
    /// The data register's port.
    pub base: u16,
    /// How many ports apart the registers are: the command and status
    /// register is at `base + spacing`.
    pub spacing: u16,
}

impl Interface {
    /// The data register's port.
    pub fn data(self) -> u16 {
        self.base
    }

    /// The command and status register's port.
    pub fn command(self) -> u16 {
        self.base.saturating_add(self.spacing)
    }
}

/// The spacings the specifications allow, in ports.
fn is_spacing(spacing: u16) -> bool {
    matches!(spacing, 1 | 4 | 16)
}

/// The interface the `_CRS` of an `IPI0001` device gives, from its I/O
/// port resources as (first port, length) pairs, in order.
///
/// Two ranges are the two registers, the data one first. One range holds
/// both, and its length says the spacing: the last register is at its
/// length less one (a range of 2, 5 or 17 ports).
pub fn from_io_resources(ports: &[(u16, u16)]) -> Option<Interface> {
    let interface = match *ports {
        [(data, _), (command, _)] => Interface { base: data, spacing: command.checked_sub(data)? },
        [(base, length)] => Interface { base, spacing: length.checked_sub(1)? },
        _ => return None,
    };
    is_spacing(interface.spacing).then_some(interface)
}

/// The interface an SMBIOS type 38 structure (IPMI device information,
/// DSP0134's 7.39) gives, from its formatted area (header included), if
/// it is a KCS one in I/O port space.
pub fn from_smbios(formatted: &[u8]) -> Option<Interface> {
    // The interface type, at offset 4: 1 is KCS (2 SMIC, 3 BT, 4 SSIF).
    if *formatted.get(4)? != 1 {
        return None;
    }
    // The base address at 8, 8 bytes; its bit 0 says I/O space (1) or
    // memory (0).
    let address = u64::from_le_bytes(formatted.get(8..16)?.try_into().ok()?);
    if address & 1 == 0 {
        return None;
    }
    // The modifier byte at 0x10, absent before version 2.3: bits 7:6 the
    // spacing (0 one byte, 1 four bytes, 2 sixteen), and bit 4 the lowest
    // bit of the address, which the I/O flag took the place of.
    let modifier = formatted.get(0x10).copied().unwrap_or(0);
    let spacing = match modifier >> 6 {
        0 => 1,
        1 => 4,
        2 => 16,
        _ => return None,
    };
    let base = u16::try_from((address & !1) | u64::from(modifier >> 4 & 1)).ok()?;
    Some(Interface { base, spacing })
}
