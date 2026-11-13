// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The PC's COM2 to COM4, at the I/O ports and ISA interrupts Linux's
//! 8250 driver looks for them at: `ttyS1` at 0x2F8 on interrupt 3,
//! `ttyS2` at 0x3E8 on 4 and `ttyS3` at 0x2E8 on 3, each used only if it
//! answers. COM1, `ttyS0` at 0x3F8 on 4, is the architecture's.
//!
//! COM2 and COM4 share interrupt 3, which is edge-triggered: its handler
//! reads both until neither has an interrupt pending, so that the line
//! falls and the next byte raises it again. COM3's interrupt 4 is COM1's,
//! whose handler is the architecture's, so COM3's typing is polled.

use core::sync::atomic::Ordering;

use radix_hal::Devices;
use radix_hal::cmdline::CommandLine;
use radix_klog::{info, warn};
use radix_sync::Once;
use radix_uart::{PORT_SIZE, Uart};

use super::{ABOUT, About, CONSOLE, Interrupt, Kind, SEEN};
use crate::Arch;
use crate::ioports::IoWindow;

/// COM2 to COM4: the `n` of `ttyS<n>`, the first I/O port, and the ISA
/// interrupt.
const PORTS: [(u8, u16, u8); 3] = [(1, 0x2f8, 3), (2, 0x3e8, 4), (3, 0x2e8, 3)];

/// The interrupt COM2 and COM4 share.
const SHARED: u8 = 3;

/// The lines on the shared interrupt, and how many there are.
static SHARING: Once<([usize; 2], usize)> = Once::new();

/// Rounds of the shared interrupt's handler, should a port keep one
/// pending: Linux's 8250 driver gives up at 512 interrupts; each round
/// here takes up to 256 bytes a port.
const ROUNDS: usize = 16;

/// Probes COM2 to COM4, each at its line from boot, as `flags` says.
pub(super) fn find(flags: &CommandLine<'_>) {
    for (number, base, _) in PORTS {
        let name = super::name(number);
        let (line, registers) = super::console_line(flags, name.as_str());
        let Some(uart) = Uart::probe(IoWindow::new(base, PORT_SIZE), 0, registers) else {
            continue;
        };
        let about = About {
            number,
            kind: Kind::Com(number + 1),
            base,
            // Until its interrupt is routed, at the start.
            interrupt: Interrupt::Polled,
            log: flags.carries_log(name.as_str()),
            line,
        };
        if super::add(uart, about).is_none() {
            warn!("Serial: COM{} has no room among the serial ports; it is not used", number + 1);
        }
    }
}

/// Whether `about` is a port on the shared interrupt.
fn shares(about: &About) -> bool {
    PORTS.iter().any(|&(number, _, irq)| irq == SHARED && about.kind == Kind::Com(number + 1))
}

/// Routes interrupt 3 to COM2 and COM4, if either is there.
pub(super) fn route() {
    let mut lines = [0; 2];
    let mut count = 0;
    let mut com2 = false;
    for (index, about) in ABOUT.lock_irq().iter().enumerate() {
        let Some(about) = about.as_ref().filter(|about| shares(about)) else { continue };
        if count < lines.len() {
            lines[count] = index;
            count += 1;
            com2 |= about.kind == Kind::Com(2);
        }
    }
    let (ports, take) = match (count, com2) {
        (0, _) => return,
        (1, true) => ("COM2", "takes"),
        (1, false) => ("COM4", "takes"),
        _ => ("COM2 and COM4", "take"),
    };
    // Before the interrupt can come.
    SHARING.call_once(|| (lines, count));
    let Some((vector, gsi)) = Arch::route_isa_interrupt(SHARED, on_interrupt, 0) else {
        warn!(
            "Serial: ISA interrupt {SHARED} could not be routed for {ports}: it has another \
             handler, or no vector is free; typing is polled"
        );
        return;
    };
    for about in ABOUT.lock_irq().iter_mut().flatten().filter(|about| shares(about)) {
        about.interrupt = Interrupt::Isa(SHARED);
    }
    info!(
        "Serial: {ports} {take} typing by ISA interrupt {SHARED} (GSI {gsi}, vector {vector:#x})"
    );
}

/// Interrupt 3: takes what COM2 and COM4 received, until neither has an
/// interrupt pending.
fn on_interrupt(_: usize) {
    let Some(&(lines, count)) = SHARING.get() else { return };
    let console = CONSOLE.lock();
    for _ in 0..ROUNDS {
        let mut pending = false;
        for &index in &lines[..count] {
            super::take_typing(&console, index, true);
            let port = console.uart(index - 1);
            pending |= port.is_some_and(radix_uart::Uart::interrupt_pending);
        }
        if !pending {
            return;
        }
    }
    // A port that keeps one pending gets no more rounds, and its line no
    // edge to raise the next: the thread polls it as often as an
    // untried one.
    for &index in &lines[..count] {
        SEEN[index].store(false, Ordering::Relaxed);
    }
}
