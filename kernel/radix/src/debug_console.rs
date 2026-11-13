// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The debug console that `kdb` and the panic path talk on: COM1, the
//! xHCI Debug Capability when the `dbc` flag puts the console there (ADR
//! 0160), and the other serial ports that carry the log, COM2 to COM4 and
//! those on PCI cards and of Intel AMT (ADRs 0360 and 0480). Each byte
//! goes to all of them, and a byte typed on any is read. A serial port
//! that `serial-log=` leaves out is none of them.

use radix_hal::Platform;

use crate::{Arch, serial, usb};

/// Whether there is a debug console.
pub fn present() -> bool {
    Arch::has_debug_console() || usb::dbc::console() || serial::present()
}

/// Writes `text` to the debug console.
pub fn write(text: &str) {
    Arch::console_write(text);
    usb::dbc::write(text);
    serial::write(text);
}

/// A byte typed on the debug console, if any.
pub fn read() -> Option<u8> {
    Arch::console_read().or_else(usb::dbc::read).or_else(serial::read)
}
