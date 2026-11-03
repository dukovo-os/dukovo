// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The debug console that `kdb` and the panic path talk on: the serial
//! port, the xHCI Debug Capability when the `dbc` flag puts the console
//! there (ADR 0160), and the serial ports on PCI cards and of Intel AMT
//! (ADR 0360). Each byte goes to all of them, and a byte typed on any is
//! read.

use radix_hal::Platform;

use crate::{Arch, uart, usb};

/// Whether there is a debug console.
pub fn present() -> bool {
    Arch::has_debug_console() || usb::dbc::console() || uart::present()
}

/// Writes `text` to the debug console.
pub fn write(text: &str) {
    Arch::console_write(text);
    usb::dbc::write(text);
    uart::write(text);
}

/// A byte typed on the debug console, if any.
pub fn read() -> Option<u8> {
    Arch::console_read().or_else(usb::dbc::read).or_else(uart::read)
}
