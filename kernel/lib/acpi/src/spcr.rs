// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Serial Port Console Redirection table (`SPCR`): the serial port
//! the firmware sends its own console to, and at what speed, as a server
//! whose BMC offers Serial-over-LAN has it name the BMC's port (ADR 0481).
//!
//! Microsoft defines it (revision 2 in the "Serial Port Console
//! Redirection Table" specification, revisions 3 and 4 adding fields at
//! the end that the kernel does not read). Revision 1 has the same
//! fields; GRUB reads both alike, and so does this.
//!
//! The fields after the 36-byte header, by their offset in the table:
//!
//! | Offset | Size | Field |
//! | --- | --- | --- |
//! | 36 | 1 | Interface type: 0 a full 16550, 1 a 16550 subset, 0x12 a 16550 accessed as the address says |
//! | 40 | 12 | Base address, a Generic Address Structure |
//! | 52 | 1 | Interrupt type |
//! | 53 | 1 | IRQ |
//! | 54 | 4 | Global system interrupt |
//! | 58 | 1 | Speed: 0 as the firmware left it, 3 9600, 4 19200, 6 57600, 7 115200 baud |
//! | 59 | 1 | Parity: 0 none |
//! | 60 | 1 | Stop bits: 1 one |
//! | 61 | 1 | Flow control: 1 DCD, 2 RTS/CTS, 4 XON/XOFF |
//! | 62 | 1 | Terminal type: 0 VT100, 1 VT100+, 2 VT-UTF8, 3 ANSI |
//! | 64 | 2 | PCI device ID, 0xFFFF when the port is not on PCI |
//! | 66 | 2 | PCI vendor ID |
//! | 68 | 1 | PCI bus |
//! | 69 | 1 | PCI device |
//! | 70 | 1 | PCI function |

use crate::{Error, GenericAddress, Sdt, u16_at};

/// The interface types that are 16550s: a full one, a subset (the
/// Debug Port table's word for one without some registers), and one
/// accessed as its Generic Address Structure says.
const UART_16550: [u8; 3] = [0x00, 0x01, 0x12];

/// What the firmware's `SPCR` table says of its console port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spcr {
    /// The interface type: [`Spcr::is_16550`] tells the ones the kernel
    /// drives.
    pub interface: u8,
    /// Where the port's registers are.
    pub address: GenericAddress,
    /// Its speed in baud, `None` when the firmware leaves it as it was or
    /// gives a code the table does not define.
    pub rate: Option<u32>,
    /// Its parity: 0 for none, the only one the table defines.
    pub parity: u8,
    /// Its stop bits: 1 for one, the only one the table defines.
    pub stop_bits: u8,
    /// Its flow control, as bits: 1 DCD, 2 RTS/CTS, 4 XON/XOFF.
    pub flow_control: u8,
    /// The terminal the firmware expects at the far end: 0 VT100,
    /// 1 VT100+, 2 VT-UTF8, 3 ANSI.
    pub terminal: u8,
    /// The port's PCI function, as bus, device and function, when it is
    /// on PCI and the table says which.
    pub pci: Option<(u8, u8, u8)>,
}

impl Spcr {
    /// Parses an `SPCR` table.
    pub fn parse(sdt: &Sdt<'_>) -> Result<Spcr, Error> {
        if &sdt.signature != b"SPCR" {
            return Err(Error::BadSignature);
        }
        let body = sdt.body();
        // Up to the terminal type, which every revision has; the PCI
        // fields after it are read when they are there.
        let byte = |offset: usize| body.get(offset - 36).copied().ok_or(Error::TooShort);
        let address =
            body.get(40 - 36..52 - 36).and_then(GenericAddress::parse).ok_or(Error::TooShort)?;
        let rate = match byte(58)? {
            3 => Some(9600),
            4 => Some(19200),
            6 => Some(57600),
            7 => Some(115_200),
            _ => None,
        };
        let pci = match (u16_at(body, 64 - 36), body.get(68 - 36..71 - 36)) {
            (Some(device), Some(&[bus, slot, function])) if device != 0xffff => {
                Some((bus, slot, function))
            }
            _ => None,
        };
        Ok(Spcr {
            interface: byte(36)?,
            address,
            rate,
            parity: byte(59)?,
            stop_bits: byte(60)?,
            flow_control: byte(61)?,
            terminal: byte(62)?,
            pci,
        })
    }

    /// Whether the port is a 16550 of some kind.
    pub fn is_16550(&self) -> bool {
        UART_16550.contains(&self.interface)
    }

    /// The port's first I/O port, when it is a 16550 at an I/O port, as
    /// PC serial ports and PCI cards' are, rather than one whose
    /// registers are in memory.
    pub fn io_port(&self) -> Option<u16> {
        let in_io = self.address.space == GenericAddress::IO;
        (self.is_16550() && in_io).then(|| u16::try_from(self.address.address).ok()).flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::table;

    /// The body of an `SPCR` as QEMU's `-acpitable` and a BMC's firmware
    /// give one: `interface`, a Generic Address Structure in `space` at
    /// `address`, speed code `speed`, and the PCI fields as `pci` says,
    /// to the length of revision 2's table.
    fn body(interface: u8, space: u8, address: u64, speed: u8, pci: Option<[u8; 3]>) -> Vec<u8> {
        let mut body = vec![interface, 0, 0, 0];
        body.extend_from_slice(&[space, 8, 0, 1]);
        body.extend_from_slice(&address.to_le_bytes());
        // Interrupt type, IRQ and global system interrupt: COM2's.
        body.extend_from_slice(&[1, 3]);
        body.extend_from_slice(&3u32.to_le_bytes());
        // Speed, no parity, one stop bit, RTS/CTS, VT-UTF8, a reserved byte.
        body.extend_from_slice(&[speed, 0, 1, 2, 2, 0]);
        match pci {
            Some([bus, device, function]) => {
                body.extend_from_slice(&0xa13du16.to_le_bytes());
                body.extend_from_slice(&0x8086u16.to_le_bytes());
                body.extend_from_slice(&[bus, device, function]);
            }
            None => {
                body.extend_from_slice(&0xffffu16.to_le_bytes());
                body.extend_from_slice(&0xffffu16.to_le_bytes());
                body.extend_from_slice(&[0, 0, 0]);
            }
        }
        // PCI flags, segment and the reserved bytes, to 80 bytes in all.
        body.resize(80 - 36, 0);
        body
    }

    fn parse(bytes: &[u8]) -> Result<Spcr, Error> {
        Spcr::parse(&Sdt::parse(0x1000, bytes).unwrap())
    }

    #[test]
    fn a_bmc_on_com2_at_57600() {
        let spcr = parse(&table(*b"SPCR", 2, &body(0, 1, 0x2f8, 6, None))).unwrap();
        assert_eq!(spcr.io_port(), Some(0x2f8));
        assert_eq!(spcr.rate, Some(57600));
        assert_eq!((spcr.parity, spcr.stop_bits, spcr.flow_control, spcr.terminal), (0, 1, 2, 2));
        assert_eq!(spcr.pci, None);
    }

    #[test]
    fn every_speed_the_table_defines_and_as_left() {
        for (code, rate) in
            [(3, Some(9600)), (4, Some(19200)), (6, Some(57600)), (7, Some(115_200)), (0, None)]
        {
            let spcr = parse(&table(*b"SPCR", 2, &body(0, 1, 0x3f8, code, None))).unwrap();
            assert_eq!(spcr.rate, rate, "code {code}");
        }
        // 38400's code, 5, is reserved; so is anything past 7.
        let spcr = parse(&table(*b"SPCR", 2, &body(0, 1, 0x3f8, 5, None))).unwrap();
        assert_eq!(spcr.rate, None);
    }

    #[test]
    fn intel_amts_port_on_pci() {
        let bytes = table(*b"SPCR", 2, &body(0, 1, 0xf0e0, 7, Some([0, 0x16, 3])));
        let spcr = parse(&bytes).unwrap();
        assert_eq!(spcr.io_port(), Some(0xf0e0));
        assert_eq!(spcr.pci, Some((0, 0x16, 3)));
    }

    #[test]
    fn a_port_in_memory_or_of_another_kind_is_none_the_kernel_drives() {
        // AMD's FCH UART, whose registers are in memory.
        let mmio = parse(&table(*b"SPCR", 2, &body(0x12, 0, 0xfedc_9000, 7, None))).unwrap();
        assert!(mmio.is_16550());
        assert_eq!(mmio.io_port(), None);
        // An Arm PL011.
        let pl011 = parse(&table(*b"SPCR", 2, &body(0x03, 1, 0x3f8, 7, None))).unwrap();
        assert!(!pl011.is_16550());
        assert_eq!(pl011.io_port(), None);
        // An I/O port past the I/O space.
        let far = parse(&table(*b"SPCR", 2, &body(0, 1, 0x1_0000, 7, None))).unwrap();
        assert_eq!(far.io_port(), None);
    }

    #[test]
    fn a_table_cut_short_after_the_terminal_type_has_no_pci_function() {
        let mut short = body(0, 1, 0x2f8, 7, Some([0, 0x16, 3]));
        short.truncate(64 - 36);
        let spcr = parse(&table(*b"SPCR", 1, &short)).unwrap();
        assert_eq!((spcr.io_port(), spcr.rate, spcr.pci), (Some(0x2f8), Some(115_200), None));
    }

    #[test]
    fn a_table_without_its_fields_or_with_another_signature_is_refused() {
        let mut short = body(0, 1, 0x2f8, 7, None);
        short.truncate(62 - 36);
        assert_eq!(parse(&table(*b"SPCR", 2, &short)), Err(Error::TooShort));
        assert_eq!(
            parse(&table(*b"DBG2", 0, &body(0, 1, 0x2f8, 7, None))),
            Err(Error::BadSignature)
        );
    }
}
