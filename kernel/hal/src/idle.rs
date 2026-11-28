// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A CPU's idle states, as the scheduler's governor and the reports see
//! them (ADR 0795).

use core::fmt;

/// One idle state of a CPU. The first of a CPU's states is the wait it
/// used before anything was known of deeper ones, and costs nothing to
/// leave.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdleState {
    /// Its name: `C1`, `C2` and so on, in order.
    pub name: &'static str,
    /// How it is entered.
    pub how: Entry,
    /// How long leaving it takes, as the firmware gives it.
    pub exit_ns: u64,
    /// How long the CPU must stay in it for it to save power.
    pub residency_ns: u64,
    /// Whether a write to the line the CPU watches ends it, so that a
    /// waker need not send an interrupt (MWAIT's states).
    pub watches: bool,
}

/// How an idle state is entered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    /// HLT.
    Halt,
    /// MWAIT with this hint.
    Mwait(u32),
    /// A read of this I/O port, which the chipset answers by putting the
    /// CPU in the state.
    Port(u16),
    /// Whatever the architecture does to wait.
    Wait,
}

impl fmt::Display for Entry {
    /// Writes the description that Linux's `cpuidle` files give an ACPI
    /// state of the kind.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Entry::Halt => f.write_str("ACPI HLT"),
            Entry::Mwait(hint) => write!(f, "ACPI FFH MWAIT {hint:#x}"),
            Entry::Port(port) => write!(f, "ACPI IOPORT {port:#x}"),
            Entry::Wait => f.write_str("CPUIDLE CORE WAIT"),
        }
    }
}

/// An idle state the firmware describes for a CPU (ACPI `_CST`), before
/// the architecture decides whether it can use it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirmwareState {
    /// How it is entered: [`Entry::Halt`], [`Entry::Mwait`] or
    /// [`Entry::Port`].
    pub how: Entry,
    /// Its ACPI type: 1 for C1, 2 for C2, 3 for C3 and deeper, which may
    /// stop the local APIC's timer and flush caches.
    pub kind: u8,
    /// How long leaving it takes, in microseconds.
    pub latency_us: u32,
}

/// The first state of a CPU whose architecture knows no others.
pub const ONLY: IdleState =
    IdleState { name: "C1", how: Entry::Wait, exit_ns: 0, residency_ns: 0, watches: false };

/// The names of idle states by their place.
pub const NAMES: [&str; 8] = ["C1", "C2", "C3", "C4", "C5", "C6", "C7", "C8"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_described_as_linux_describes_acpi_states() {
        assert_eq!(Entry::Mwait(0x20).to_string(), "ACPI FFH MWAIT 0x20");
        assert_eq!(Entry::Port(0x414).to_string(), "ACPI IOPORT 0x414");
        assert_eq!(Entry::Halt.to_string(), "ACPI HLT");
    }
}
