// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The register layouts and texts of PCI Express error handling: the PCI
//! Express capability, Advanced Error Reporting (AER), Downstream Port
//! Containment (DPC) and the `_OSC` call that tells whether the firmware
//! or the kernel handles them.
//!
//! Nothing here touches hardware: the kernel's `pcie` code reads and
//! writes the registers through [`crate::pci::Function`] at the offsets
//! given here, and turns what it reads into the texts of its log lines
//! and of the `aer_dev_*` files in sysfs, which follow Linux's exactly
//! (`drivers/pci/pcie/aer.c`) so that its tools read them unchanged.
//! The tests in `pcie/tests.rs` check the texts byte for byte.

/// Offsets and bits in the PCI Express capability (ID 0x10; PCIe 6.0,
/// 7.5.3). Offsets are from the capability's own start.
pub mod express {
    /// The PCI Express capabilities register (16 bits).
    pub const CAPABILITIES: usize = 0x02;
    /// Device control (16 bits).
    pub const DEVICE_CONTROL: usize = 0x08;
    /// Device status (16 bits); bits 0 to 3 are write-1-to-clear.
    pub const DEVICE_STATUS: usize = 0x0a;
    /// Link capabilities (32 bits).
    pub const LINK_CAPABILITIES: usize = 0x0c;
    /// Link status (16 bits).
    pub const LINK_STATUS: usize = 0x12;
    /// Root control (16 bits; root ports and event collectors).
    pub const ROOT_CONTROL: usize = 0x1c;
    /// Device control: report correctable errors.
    pub const REPORT_CORRECTABLE: u16 = 1 << 0;
    /// Device control: report non-fatal errors.
    pub const REPORT_NONFATAL: u16 = 1 << 1;
    /// Device control: report fatal errors.
    pub const REPORT_FATAL: u16 = 1 << 2;
    /// Device control: report unsupported requests.
    pub const REPORT_UNSUPPORTED: u16 = 1 << 3;
    /// The four reporting enables of device control.
    pub const REPORTING: u16 = 0xf;
    /// Device status: a correctable error was detected (write 1 to clear).
    pub const CORRECTABLE_DETECTED: u16 = 1 << 0;
    /// Device status: a non-fatal error was detected.
    pub const NONFATAL_DETECTED: u16 = 1 << 1;
    /// Device status: a fatal error was detected.
    pub const FATAL_DETECTED: u16 = 1 << 2;
    /// Device status: an unsupported request was received.
    pub const UNSUPPORTED_DETECTED: u16 = 1 << 3;
    /// The error bits of device status, the four above.
    pub const DEVICE_STATUS_ERRORS: u16 = 0xf;
    /// Link capabilities: the port reports when its link comes up.
    pub const LINK_ACTIVE_REPORTING: u32 = 1 << 20;
    /// Link status: the link is up.
    pub const LINK_ACTIVE: u16 = 1 << 13;
    /// Root control: the three bits that turn error messages into system
    /// errors (correctable, non-fatal, fatal).
    pub const SYSTEM_ERRORS: u16 = 0x7;

    /// The MSI or MSI-X vector the capability's interrupts use (bits
    /// 13:9 of the capabilities register).
    pub fn interrupt_message(capabilities: u16) -> u8 {
        ((capabilities >> 9) & 0x1f) as u8
    }
}

/// What kind of port or endpoint a PCI Express capability says its
/// function is (bits 7:4 of the capabilities register).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortType {
    /// A PCI Express endpoint.
    Endpoint,
    /// A legacy PCI Express endpoint.
    LegacyEndpoint,
    /// A root port of a root complex.
    RootPort,
    /// The upstream port of a switch.
    Upstream,
    /// A downstream port of a switch.
    Downstream,
    /// A PCI Express to PCI or PCI-X bridge.
    PcieToPci,
    /// A PCI or PCI-X to PCI Express bridge.
    PciToPcie,
    /// An endpoint integrated in a root complex.
    Integrated,
    /// A root complex event collector.
    EventCollector,
    /// A value the specification does not define.
    Other(u8),
}

impl PortType {
    /// The type the capabilities register `capabilities` names.
    pub fn of(capabilities: u16) -> PortType {
        match (capabilities >> 4) & 0xf {
            0x0 => PortType::Endpoint,
            0x1 => PortType::LegacyEndpoint,
            0x4 => PortType::RootPort,
            0x5 => PortType::Upstream,
            0x6 => PortType::Downstream,
            0x7 => PortType::PcieToPci,
            0x8 => PortType::PciToPcie,
            0x9 => PortType::Integrated,
            0xa => PortType::EventCollector,
            other => PortType::Other(other as u8),
        }
    }

    /// Whether errors of the whole hierarchy below arrive here: a root
    /// port or a root complex event collector.
    pub fn is_root(self) -> bool {
        matches!(self, PortType::RootPort | PortType::EventCollector)
    }

    /// Whether the port has a link to a device below it and may contain
    /// DPC: a root port or a switch's downstream port.
    pub fn is_downstream(self) -> bool {
        matches!(self, PortType::RootPort | PortType::Downstream)
    }
}

/// Extended capability IDs (PCIe 6.0, 7.6).
pub mod extended {
    /// Advanced Error Reporting.
    pub const AER: u16 = 0x0001;
    /// Downstream Port Containment.
    pub const DPC: u16 = 0x001d;
}

/// Advanced Error Reporting (PCIe 6.0, 7.8.4). Offsets are from the
/// capability's own start.
pub mod aer {
    use core::fmt;

    /// Uncorrectable error status (32 bits, write-1-to-clear).
    pub const UNCORRECTABLE_STATUS: usize = 0x04;
    /// Uncorrectable error mask.
    pub const UNCORRECTABLE_MASK: usize = 0x08;
    /// Uncorrectable error severity: a set bit makes the error fatal.
    pub const UNCORRECTABLE_SEVERITY: usize = 0x0c;
    /// Correctable error status (32 bits, write-1-to-clear).
    pub const CORRECTABLE_STATUS: usize = 0x10;
    /// Correctable error mask.
    pub const CORRECTABLE_MASK: usize = 0x14;
    /// Advanced error capabilities and control.
    pub const CONTROL: usize = 0x18;
    /// The header log: four dwords of the TLP that caused the error.
    pub const HEADER_LOG: usize = 0x1c;
    /// Root error command (root ports and event collectors).
    pub const ROOT_COMMAND: usize = 0x2c;
    /// Root error status.
    pub const ROOT_STATUS: usize = 0x30;
    /// The IDs of the last error messages the root received: the
    /// correctable source's in the low half, the uncorrectable's in the
    /// high half.
    pub const ERROR_SOURCE: usize = 0x34;
    /// Root error command: the three interrupt enables (correctable,
    /// non-fatal, fatal).
    pub const ROOT_COMMAND_ALL: u32 = 0x7;

    /// Bits of the root error status register.
    pub mod root {
        /// An ERR_COR message was received.
        pub const CORRECTABLE: u32 = 1 << 0;
        /// More than one ERR_COR message was received.
        pub const MULTIPLE_CORRECTABLE: u32 = 1 << 1;
        /// An ERR_FATAL or ERR_NONFATAL message was received.
        pub const UNCORRECTABLE: u32 = 1 << 2;
        /// More than one such message was received.
        pub const MULTIPLE_UNCORRECTABLE: u32 = 1 << 3;
        /// The first uncorrectable message was fatal.
        pub const FIRST_FATAL: u32 = 1 << 4;
        /// An ERR_NONFATAL message was received.
        pub const NONFATAL_RECEIVED: u32 = 1 << 5;
        /// An ERR_FATAL message was received.
        pub const FATAL_RECEIVED: u32 = 1 << 6;
        /// Every status bit of the register.
        pub const RECEIVED: u32 = 0x7f;

        /// The vector the root's AER interrupt uses (bits 31:27).
        pub fn interrupt_message(status: u32) -> u8 {
            (status >> 27) as u8
        }
    }

    /// The first error pointer of the control register (bits 4:0): the
    /// uncorrectable status bit the header log belongs to.
    pub fn first_error(control: u32) -> u8 {
        (control & 0x1f) as u8
    }

    /// The uncorrectable bits for which the header log holds the TLP
    /// (Linux's `AER_LOG_TLP_MASKS`): 12, 15, 16, and 18 to 31 except 22.
    pub const HEADER_LOGGED: u32 = (1 << 12) | (1 << 15) | (1 << 16) | 0xffbc_0000;

    /// The names of the correctable status bits, as Linux prints them.
    pub const CORRECTABLE_NAMES: [Option<&str>; 16] = names([
        (0, "RxErr"),
        (6, "BadTLP"),
        (7, "BadDLLP"),
        (8, "Rollover"),
        (12, "Timeout"),
        (13, "NonFatalErr"),
        (14, "CorrIntErr"),
        (15, "HeaderOF"),
    ]);

    /// The names of the uncorrectable status bits, as Linux prints them.
    pub const UNCORRECTABLE_NAMES: [Option<&str>; 32] = names([
        (0, "Undefined"),
        (4, "DLP"),
        (5, "SDES"),
        (12, "TLP"),
        (13, "FCP"),
        (14, "CmpltTO"),
        (15, "CmpltAbrt"),
        (16, "UnxCmplt"),
        (17, "RxOF"),
        (18, "MalfTLP"),
        (19, "ECRC"),
        (20, "UnsupReq"),
        (21, "ACSViol"),
        (22, "UncorrIntErr"),
        (23, "BlockedTLP"),
        (24, "AtomicOpBlocked"),
        (25, "TLPBlockedErr"),
        (26, "PoisonTLPBlocked"),
        (27, "DMWrReqBlocked"),
        (28, "IDECheck"),
        (29, "MisIDETLP"),
        (30, "PCRC_CHECK"),
        (31, "TLPXlatBlocked"),
    ]);

    /// A names table with the given bits named and the rest not.
    const fn names<const BITS: usize, const NAMED: usize>(
        named: [(usize, &'static str); NAMED],
    ) -> [Option<&'static str>; BITS] {
        let mut table = [None; BITS];
        let mut i = 0;
        while i < NAMED {
            table[named[i].0] = Some(named[i].1);
            i += 1;
        }
        table
    }

    /// The names of the bits set in `bits`, `, ` between them, in bit
    /// order. A bit without a name is `bit <n>`; no bits is `none`. Only
    /// the low 16 bits of a correctable status are bits of the register.
    pub struct Names {
        /// The status bits.
        pub bits: u32,
        /// Whether they are a correctable status (else an uncorrectable
        /// one), which has its own names.
        pub correctable: bool,
    }

    impl fmt::Display for Names {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            let (table, bits): (&[Option<&str>], u32) = if self.correctable {
                (&CORRECTABLE_NAMES, self.bits & 0xffff)
            } else {
                (&UNCORRECTABLE_NAMES, self.bits)
            };
            if bits == 0 {
                return f.write_str("none");
            }
            let mut first = true;
            for bit in (0..32).filter(|bit| bits & (1 << bit) != 0) {
                if !first {
                    f.write_str(", ")?;
                }
                first = false;
                match table.get(bit).copied().flatten() {
                    Some(name) => f.write_str(name)?,
                    None => write!(f, "bit {bit}")?,
                }
            }
            Ok(())
        }
    }

    /// One function's error counts since boot, by status bit, and the
    /// number of reports of each kind, as Linux keeps them for its
    /// `aer_dev_*` files.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Counters {
        pub(super) correctable: [u64; 16],
        nonfatal: [u64; 32],
        fatal: [u64; 32],
        pub(super) total_correctable: u64,
        total_nonfatal: u64,
        total_fatal: u64,
    }

    impl Counters {
        /// Counters that have counted nothing.
        pub const fn new() -> Counters {
            Counters {
                correctable: [0; 16],
                nonfatal: [0; 32],
                fatal: [0; 32],
                total_correctable: 0,
                total_nonfatal: 0,
                total_fatal: 0,
            }
        }

        /// Counts one correctable report: each bit set in `status` adds
        /// 1 to its count (bits 16 and up are not bits of the register
        /// and are ignored), and the total adds 1. Counts stop at the
        /// largest value instead of wrapping.
        pub fn add_correctable(&mut self, status: u32) {
            for (bit, count) in self.correctable.iter_mut().enumerate() {
                if status & (1 << bit) != 0 {
                    *count = count.saturating_add(1);
                }
            }
            self.total_correctable = self.total_correctable.saturating_add(1);
        }

        /// Counts one uncorrectable report, as a fatal or a non-fatal
        /// one, as [`Counters::add_correctable`] counts a correctable.
        pub fn add_uncorrectable(&mut self, status: u32, fatal: bool) {
            let (counts, total) = if fatal {
                (&mut self.fatal, &mut self.total_fatal)
            } else {
                (&mut self.nonfatal, &mut self.total_nonfatal)
            };
            for (bit, count) in counts.iter_mut().enumerate() {
                if status & (1 << bit) != 0 {
                    *count = count.saturating_add(1);
                }
            }
            *total = total.saturating_add(1);
        }

        /// The number of reports counted: correctable, non-fatal, fatal.
        pub fn totals(&self) -> (u64, u64, u64) {
            (self.total_correctable, self.total_nonfatal, self.total_fatal)
        }

        /// Writes the text of `aer_dev_correctable`.
        ///
        /// # Errors
        ///
        /// Whatever `out` returns.
        pub fn write_correctable(&self, out: &mut dyn fmt::Write) -> fmt::Result {
            write_counts(
                out,
                &self.correctable,
                &CORRECTABLE_NAMES,
                "dev_cor_errs_bit",
                ("TOTAL_ERR_COR", self.total_correctable),
            )
        }

        /// Writes the text of `aer_dev_nonfatal`.
        ///
        /// # Errors
        ///
        /// Whatever `out` returns.
        pub fn write_nonfatal(&self, out: &mut dyn fmt::Write) -> fmt::Result {
            write_counts(
                out,
                &self.nonfatal,
                &UNCORRECTABLE_NAMES,
                "dev_nonfatal_errs_bit",
                ("TOTAL_ERR_NONFATAL", self.total_nonfatal),
            )
        }

        /// Writes the text of `aer_dev_fatal`.
        ///
        /// # Errors
        ///
        /// Whatever `out` returns.
        pub fn write_fatal(&self, out: &mut dyn fmt::Write) -> fmt::Result {
            write_counts(
                out,
                &self.fatal,
                &UNCORRECTABLE_NAMES,
                "dev_fatal_errs_bit",
                ("TOTAL_ERR_FATAL", self.total_fatal),
            )
        }
    }

    /// Writes a line for each named bit, a line for each unnamed bit
    /// with a count, then the total's line, as Linux's `aer_stats_dev_attr`.
    fn write_counts(
        out: &mut dyn fmt::Write,
        counts: &[u64],
        names: &[Option<&str>],
        unnamed: &str,
        total: (&str, u64),
    ) -> fmt::Result {
        for (bit, (&count, name)) in counts.iter().zip(names).enumerate() {
            match name {
                Some(name) => writeln!(out, "{name} {count}")?,
                None if count != 0 => writeln!(out, "{unnamed}[{bit}] {count}")?,
                None => {}
            }
        }
        writeln!(out, "{} {}", total.0, total.1)
    }

    /// What a CPER PCIe section's 96 bytes of the AER capability (UEFI
    /// 2.10, N.2.7) report: the correctable status and the uncorrectable
    /// status, each without the bits its mask hides, and the
    /// uncorrectable severity. The registers are little-endian at the
    /// offsets above.
    pub fn from_cper(bytes: &[u8; 96]) -> (u32, u32, u32) {
        let dword = |offset: usize| {
            u32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ])
        };
        (
            dword(CORRECTABLE_STATUS) & !dword(CORRECTABLE_MASK),
            dword(UNCORRECTABLE_STATUS) & !dword(UNCORRECTABLE_MASK),
            dword(UNCORRECTABLE_SEVERITY),
        )
    }
}

/// Downstream Port Containment (PCIe 6.0, 7.9.14). Offsets are from the
/// capability's own start.
pub mod dpc {
    use core::fmt;

    /// The DPC capability register (16 bits).
    pub const CAPABILITY: usize = 0x04;
    /// DPC control (16 bits).
    pub const CONTROL: usize = 0x06;
    /// DPC status (16 bits).
    pub const STATUS: usize = 0x08;
    /// The ID of the function that caused the containment (16 bits).
    pub const SOURCE: usize = 0x0a;
    /// Capability: the root port has the PIO error registers.
    pub const RP_EXTENSIONS: u16 = 1 << 5;
    /// Status: the port has contained an error and holds its link down.
    pub const TRIGGERED: u16 = 1 << 0;
    /// Status: the interrupt for a containment is pending.
    pub const INTERRUPT: u16 = 1 << 3;
    /// Status: the root port is busy with a PIO error.
    pub const RP_BUSY: u16 = 1 << 4;

    /// The vector the port's DPC interrupt uses (capability bits 4:0).
    pub fn interrupt_message(capability: u16) -> u8 {
        (capability & 0x1f) as u8
    }

    /// `control` with containment on ERR_FATAL and the interrupt on, and
    /// the other trigger bits (non-fatal, ERR_COR and software) off:
    /// bits 0, 1, 3, 4 and 6 are replaced, the rest is kept.
    pub fn control_for_fatal(control: u16) -> u16 {
        const REPLACED: u16 = 0b101_1011;
        const TRIGGER_FATAL: u16 = 1 << 0;
        (control & !REPLACED) | TRIGGER_FATAL | INTERRUPT_ENABLE
    }

    /// Control: the port interrupts when it contains an error.
    pub const INTERRUPT_ENABLE: u16 = 1 << 3;

    /// Why a port contained an error.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Reason {
        /// An unmasked uncorrectable error below it.
        Unmasked,
        /// An ERR_NONFATAL message.
        Nonfatal,
        /// An ERR_FATAL message.
        Fatal,
        /// A root port programmed I/O error.
        RpPio,
        /// Software wrote the trigger bit.
        Software,
        /// A reason this code does not name.
        Other,
    }

    /// The reason a DPC status gives: bits 2:1, and when they are 3,
    /// bits 6:5 (0 a root port PIO error, 1 a software trigger).
    pub fn reason(status: u16) -> Reason {
        match (status >> 1) & 0x3 {
            0 => Reason::Unmasked,
            1 => Reason::Nonfatal,
            2 => Reason::Fatal,
            _ => match (status >> 5) & 0x3 {
                0 => Reason::RpPio,
                1 => Reason::Software,
                _ => Reason::Other,
            },
        }
    }

    impl fmt::Display for Reason {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(match self {
                Reason::Unmasked => "an unmasked uncorrectable error",
                Reason::Nonfatal => "an ERR_NONFATAL message",
                Reason::Fatal => "an ERR_FATAL message",
                Reason::RpPio => "a root port I/O error",
                Reason::Software => "a software trigger",
                Reason::Other => "a reason the port does not name",
            })
        }
    }
}

/// The PCI host bridge's `_OSC` method (PCI Firmware Specification 3.3,
/// 4.5), through which the firmware hands the kernel control of AER, the
/// PCIe capability and DPC, or keeps them.
pub mod osc {
    use core::fmt;

    /// The UUID 33DB4D5B-1FF7-401C-9657-7441C03DD766 of the PCI host
    /// bridge `_OSC`, as the bytes of the call's buffer.
    pub const UUID: [u8; 16] = [
        0x5b, 0x4d, 0xdb, 0x33, 0xf7, 0x1f, 0x1c, 0x40, 0x96, 0x57, 0x74, 0x41, 0xc0, 0x3d, 0xd7,
        0x66,
    ];
    /// The revision of the call.
    pub const REVISION: u64 = 1;
    /// First dword: the call only asks what the firmware would grant.
    pub const QUERY: u32 = 1 << 0;
    /// First dword, in the answer: the call failed.
    pub const FAILED: u32 = 1 << 1;
    /// First dword, in the answer: the firmware does not know the UUID.
    pub const UNKNOWN_UUID: u32 = 1 << 2;
    /// First dword, in the answer: the firmware does not know the revision.
    pub const UNKNOWN_REVISION: u32 = 1 << 3;
    /// First dword, in the answer: the firmware masked some bits of the
    /// other dwords.
    pub const MASKED: u32 = 1 << 4;
    /// Support: the kernel reaches extended configuration space.
    pub const EXTENDED_CONFIG: u32 = 1 << 0;
    /// Support: ASPM.
    pub const ASPM: u32 = 1 << 1;
    /// Support: clock power management.
    pub const CLOCK_PM: u32 = 1 << 2;
    /// Support: PCI segments.
    pub const SEGMENTS: u32 = 1 << 3;
    /// Support: message-signalled interrupts.
    pub const MSI: u32 = 1 << 4;
    /// What the kernel supports: extended configuration space, segments
    /// and MSI.
    pub const SUPPORT: u32 = EXTENDED_CONFIG | SEGMENTS | MSI;
    /// Control: native PCIe hot-plug.
    pub const HOTPLUG: u32 = 1 << 0;
    /// Control: SHPC hot-plug.
    pub const SHPC: u32 = 1 << 1;
    /// Control: power management events.
    pub const PME: u32 = 1 << 2;
    /// Control: Advanced Error Reporting.
    pub const AER: u32 = 1 << 3;
    /// Control: the PCI Express capability.
    pub const CAPABILITY: u32 = 1 << 4;
    /// Control: latency tolerance reporting.
    pub const LTR: u32 = 1 << 5;
    /// Control: Downstream Port Containment.
    pub const DPC: u32 = 1 << 7;
    /// What the kernel asks for: AER, the PCIe capability and DPC.
    pub const ASKED: u32 = AER | CAPABILITY | DPC;

    /// The three dwords of a call: the query flag, what the kernel
    /// supports, and the `control` bits it asks for.
    pub fn dwords(query: bool, control: u32) -> [u32; 3] {
        [if query { QUERY } else { 0 }, SUPPORT, control]
    }

    /// What to ask for in the call that sets control after a query
    /// offered `offered`: what the kernel asks for of it, but not AER
    /// without the PCIe capability (AER needs it to find its registers)
    /// and not DPC without AER (containment hands its errors to AER).
    pub fn to_take(offered: u32) -> u32 {
        let mut take = offered & ASKED;
        if take & CAPABILITY == 0 {
            take &= !AER;
        }
        if take & AER == 0 {
            take &= !DPC;
        }
        take
    }

    /// Control bits as text, in the order AER, DPC, the PCIe capability,
    /// PME, LTR, native hot-plug, SHPC hot-plug, joined as "A", "A and
    /// B" or "A, B and C"; no bits is "nothing".
    pub struct Granted(pub u32);

    impl fmt::Display for Granted {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            const TEXTS: [(u32, &str); 7] = [
                (AER, "AER"),
                (DPC, "DPC"),
                (CAPABILITY, "the PCIe capability"),
                (PME, "PME"),
                (LTR, "LTR"),
                (HOTPLUG, "native hot-plug"),
                (SHPC, "SHPC hot-plug"),
            ];
            let granted = TEXTS.iter().filter(|(bit, _)| self.0 & bit != 0);
            let count = granted.clone().count();
            if count == 0 {
                return f.write_str("nothing");
            }
            for (index, (_, text)) in granted.enumerate() {
                match index {
                    0 => {}
                    last if last + 1 == count => f.write_str(" and ")?,
                    _ => f.write_str(", ")?,
                }
                f.write_str(text)?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;
