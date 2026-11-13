// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Serial ports on PCI (ADR 0360): serial cards such as those built on
//! the ASIX AX99100, Intel AMT's Serial-over-LAN port, and QEMU's
//! `pci-serial`. The PCI scan (`fs::disk::init`) hands each function
//! [`identify`] knows to [`bind`], which turns on its I/O decoding, finds
//! its ports at the start of its BAR 0 and names them from `ttyS4`, in
//! the order they are found, as Linux names them (ADR 0480).
//!
//! A function with MSI takes typing by interrupt, one message for all its
//! ports. The kernel routes no INTx yet, so the ports of one without are
//! polled while their terminals are open.

use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use radix_hal::Devices;
use radix_hal::cmdline::CommandLine;
use radix_hw::pci::{Bar, Function, Msi, command};
use radix_klog::{info, warn};
use radix_uart::{Ids, PORT_SIZE, Uart};

use super::{ABOUT, About, CONSOLE, FLAGS, Interrupt, Kind, LINES, THREAD};
use crate::Arch;
use crate::ioports::IoWindow;

/// The `n` of the next port's name, `ttyS<n>`: from `ttyS4`, after the
/// names COM1 to COM4 keep whether or not they are there.
static NEXT: AtomicU8 = AtomicU8::new(4);

/// For the line of a function's first port, how many lines its ports
/// are: what its interrupt reads.
static FUNCTION_LINES: [AtomicUsize; LINES] = [const { AtomicUsize::new(0) }; LINES];

/// The most ports a function has: QEMU's `pci-serial-4x`.
const MOST_PORTS: usize = 4;

/// Which kind of PCI serial port `function` is, if it is one.
pub fn identify(function: &Function) -> Option<radix_uart::Kind> {
    radix_uart::identify(&Ids {
        vendor: function.vendor_id(),
        device: function.device_id(),
        subsystem: function.subsystem(),
        class: function.class(),
    })
}

/// Sets up the ports of `function`, a `kind`: each one that answers gets
/// the next name, the line its `console=` gives, its part in the log as
/// `serial-log=` says, and a terminal. Kept out of the frame of the PCI
/// scan.
#[inline(never)]
pub fn bind(function: &Function, kind: radix_uart::Kind) {
    let address = function.address();
    let name = kind.name();
    let Some(Bar::Io { port, size }) = function.bar(0) else {
        warn!("Serial: the {name} at {address} has no I/O ports in BAR 0; it is not used");
        return;
    };
    let Ok(base) = u16::try_from(port) else {
        warn!("Serial: the {name} at {address} has its ports past the end of I/O space");
        return;
    };
    let legacy = ABOUT.lock_irq().iter().flatten().find(|about| about.base == base).copied();
    if let Some(about) = legacy {
        info!(
            "Serial: the {name} at {address} is the port at I/O port {base:#x}, which is \
             /devices/{} already",
            about.name()
        );
        return;
    }
    let window = IoWindow::new(base, u16::try_from(size).unwrap_or(u16::MAX));
    function.set_command(function.command() | command::IO);
    let flags = FLAGS.get().copied().unwrap_or(CommandLine::new(""));
    let mut lines = [0usize; MOST_PORTS];
    let mut count = 0;
    for port in 0..kind.ports().min(MOST_PORTS) {
        let Some(offset) = u16::try_from(port).ok().and_then(|port| port.checked_mul(PORT_SIZE))
        else {
            break;
        };
        let number = NEXT.load(Ordering::Relaxed);
        let port_name = super::name(number);
        let (line, registers) = super::console_line(&flags, port_name.as_str());
        let Some(uart) = Uart::probe(window, offset, registers) else { continue };
        let about = About {
            number,
            kind: Kind::Pci { kind, address, port: port as u8 },
            base: base + offset,
            interrupt: Interrupt::Polled,
            log: flags.carries_log(port_name.as_str()),
            line,
        };
        let Some(index) = super::add(uart, about) else {
            warn!("Serial: no room for more serial ports; the rest of {address} is not used");
            break;
        };
        NEXT.store(number + 1, Ordering::Relaxed);
        lines[count] = index;
        count += 1;
    }
    if count == 0 {
        warn!("Serial: the {name} at {address} does not answer at I/O port {base:#x}");
        return;
    }
    let lines = &lines[..count];
    take_interrupt(function, lines);
    if THREAD.get().is_some() {
        for &index in lines {
            let about = ABOUT.lock_irq()[index];
            if let Some(about) = about {
                super::make_terminal(index, &about, None);
            }
        }
    }
}

/// Gives `function`'s ports, `lines`, its MSI, if it has it. Their
/// receive interrupts stay off until a terminal is opened.
fn take_interrupt(function: &Function, lines: &[usize]) {
    let Some(msi) = Msi::find(function) else { return };
    let first = lines[0];
    FUNCTION_LINES[first].store(lines.len(), Ordering::Relaxed);
    let Some((_, message)) = Arch::allocate_interrupt(function.address(), on_interrupt, first)
    else {
        warn!(
            "Serial: no interrupt vector is free for the serial port at {}; its typing is polled",
            function.address()
        );
        return;
    };
    {
        let mut about = ABOUT.lock_irq();
        for &index in lines {
            if let Some(about) = &mut about[index] {
                about.interrupt = Interrupt::Msi;
            }
        }
    }
    // MSI is a write to memory, which only a bus master makes.
    function.set_command(function.command() | command::BUS_MASTER | command::INTX_DISABLE);
    msi.enable(function, message);
}

/// A function's MSI: takes what each of its ports, the lines from
/// `first`, received.
fn on_interrupt(first: usize) {
    let count = FUNCTION_LINES[first].load(Ordering::Relaxed);
    let console = CONSOLE.lock();
    for index in first..(first + count).min(LINES) {
        super::take_typing(&console, index, true);
    }
}
