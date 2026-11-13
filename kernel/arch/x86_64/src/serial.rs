// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 16550 UART driver: the debug console, and the line of a terminal.
//!
//! COM1 carries the kernel log and the kernel debugger. The Dell Inspiron
//! 15 5570 has no serial port; [`init`] notices that the UART's scratch
//! register does not hold a value and turns the console off, so writes cost
//! nothing there.
//!
//! The command line sets it up at the first look at the boot information,
//! before the kernel logs a line ([`configure`], ADR 0480):
//! `console=ttyS0,57600n8` gives it Linux's console options, and
//! `serial-log=` may leave it out of the ports that carry the log, `kdb`
//! and a panic report, for a device such as a UPS on it.
//!
//! It is also the line of the terminal `/devices/ttyS0` (ADR 0318), which
//! the kernel's `serial` thread carries: [`start_line`] routes COM1's ISA
//! interrupt 4 to a handler that hands each byte received to the driver,
//! [`line_send`] fills the transmitter's FIFO without waiting for it, and
//! [`line_set`] sets the speed and the character format a program asks
//! for. While a program has the terminal open ([`line_open`]), the port's
//! receive interrupt is on and the kernel log stays off the line, since
//! the program draws on it; `kdb` and a panic report still go out on it.
//! DTR and RTS stay raised all along, as the kernel log needs them, and
//! the line has no flow control.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use radix_hal::cmdline::CommandLine;
use radix_klog::{Sink, Style};
use radix_sync::{Once, SpinLock};
use radix_tty::line::Line;

use super::io::{inb, outb};
use crate::devices;

/// A 16550-compatible UART at a fixed I/O port base.
#[derive(Clone, Copy)]
pub struct Serial {
    base: u16,
}

/// Line-status register bit: a received byte is waiting.
const LSR_DATA_READY: u8 = 1 << 0;
/// Line-status register bit: the transmit holding register is empty, or
/// with the FIFOs on, the transmit FIFO.
const LSR_THR_EMPTY: u8 = 1 << 5;

/// Interrupt enable register bit: a received byte waits, or the receive
/// FIFO reached its threshold or has held bytes for four characters'
/// time.
const IER_RECEIVED: u8 = 1 << 0;
/// Interrupt identification register: both bits set once the FIFOs are
/// on, which only a 16550A and later have.
const IIR_FIFOS: u8 = 0xc0;
/// Modem control register: DTR and RTS, which stay raised.
const MCR_DTR_RTS: u8 = 0x03;
/// Modem control register: OUT2, which on a PC lets the port's interrupt
/// reach the interrupt controller.
const MCR_OUT2: u8 = 1 << 3;
/// Line control register: the divisor latch in place of the data and
/// interrupt enable registers.
const LCR_DLAB: u8 = 1 << 7;

/// COM1's ISA interrupt.
const IRQ_COM1: u8 = 4;
/// A 16550A's transmit FIFO; an older UART takes one byte at a time.
const FIFO_BYTES: usize = 16;
/// Bytes the interrupt handler takes at most, should a port that went
/// away read as always holding one.
const RECEIVE_LIMIT: usize = 256;

/// Polls before giving up on a byte, so a stuck UART cannot hang the kernel.
const TX_SPIN_LIMIT: u32 = 100_000;

/// The first serial port.
const COM1: Serial = Serial { base: 0x3f8 };

static PRESENT: AtomicBool = AtomicBool::new(false);
/// Whether COM1 carries the kernel log, `kdb` and a panic report, as
/// `serial-log=` says.
static LOG: AtomicBool = AtomicBool::new(true);
/// Serialises whole writes, so lines from different CPUs do not interleave.
static LOCK: SpinLock<()> = SpinLock::new(());
/// The bytes the transmitter takes at once: [`FIFO_BYTES`], or 1 without
/// FIFOs.
static TX_ROOM: AtomicUsize = AtomicUsize::new(1);
/// What [`start_line`] was given for each byte received.
static RECEIVED: Once<fn(u8)> = Once::new();
/// Whether a program has the line's terminal open, which keeps the
/// kernel log off it.
static LINE_OPEN: AtomicBool = AtomicBool::new(false);

impl Serial {
    /// Checks that a UART answers, then programs 115200 baud, 8 data bits,
    /// no parity, one stop bit, FIFOs on and interrupts off.
    fn init(self) -> bool {
        self.write_reg(7, 0x5a);
        if self.read_reg(7) != 0x5a {
            return false;
        }
        self.write_reg(1, 0x00); // interrupt enable: none
        self.write_reg(3, 0x80); // line control: DLAB on
        self.write_reg(0, 0x01); // divisor low: 115200 / 1
        self.write_reg(1, 0x00); // divisor high
        self.write_reg(3, 0x03); // line control: 8N1, DLAB off
        self.write_reg(2, 0xc7); // FIFO control: enable, clear, 14-byte threshold
        self.write_reg(4, MCR_DTR_RTS); // modem control: DTR and RTS
        true
    }

    /// Whether its FIFOs came on: a 16550A's transmitter then takes
    /// [`FIFO_BYTES`] at once.
    fn has_fifos(self) -> bool {
        self.read_reg(2) & IIR_FIFOS == IIR_FIFOS
    }

    fn write_byte(self, byte: u8) {
        for _ in 0..TX_SPIN_LIMIT {
            if self.read_reg(5) & LSR_THR_EMPTY != 0 {
                break;
            }
            core::hint::spin_loop();
        }
        self.write_reg(0, byte);
    }

    fn read_byte(self) -> Option<u8> {
        (self.read_reg(5) & LSR_DATA_READY != 0).then(|| self.read_reg(0))
    }

    fn read_reg(self, offset: u16) -> u8 {
        // SAFETY: the eight ports from `base` belong to the UART; reading
        // them affects only the UART and touches no memory.
        unsafe { inb(self.base + offset) }
    }

    fn write_reg(self, offset: u16, value: u8) {
        // SAFETY: the eight ports from `base` belong to the UART; writing
        // them affects only the UART and touches no memory.
        unsafe { outb(self.base + offset, value) }
    }
}

/// Finds and programs COM1. Returns whether it exists.
pub fn init() -> bool {
    let present = COM1.init();
    if present && COM1.has_fifos() {
        TX_ROOM.store(FIFO_BYTES, Ordering::Relaxed);
    }
    PRESENT.store(present, Ordering::Relaxed);
    present
}

/// Whether COM1 exists.
pub fn present() -> bool {
    PRESENT.load(Ordering::Relaxed)
}

/// Sets COM1 up as the command line `flags` says (ADR 0480): the line of
/// its last `console=ttyS0,<options>`, and whether it carries the log,
/// `kdb` and a panic report (`serial-log=`). Called once, at the first
/// look at the boot information, before the kernel has logged a line, so
/// that the log goes out at the speed the far end expects. Options it
/// cannot run are passed over here; the kernel's `main` warns of them.
pub fn configure(flags: &CommandLine<'_>) {
    if !present() {
        return;
    }
    let options = flags.console_options("ttyS0").flatten();
    if let Some(registers) =
        options.and_then(Line::from_console_options).as_ref().and_then(radix_uart::line_registers)
    {
        line_set(registers.divisor, registers.control);
    }
    LOG.store(flags.carries_log("ttyS0"), Ordering::Relaxed);
}

/// Whether COM1 exists and carries the kernel log, `kdb` and a panic
/// report.
pub fn carries_log() -> bool {
    present() && LOG.load(Ordering::Relaxed)
}

/// Writes `text` to COM1, turning `\n` into `\r\n`, if it carries the
/// log.
pub fn write(text: &str) {
    if !carries_log() {
        return;
    }
    // Interrupts stay off while the lock is held, so a handler on this CPU
    // that logs cannot deadlock against it.
    let were_enabled = crate::cpu::disable_interrupts();
    let guard = LOCK.lock();
    for byte in text.bytes() {
        if byte == b'\n' {
            COM1.write_byte(b'\r');
        }
        COM1.write_byte(byte);
    }
    drop(guard);
    if were_enabled {
        crate::cpu::enable_interrupts();
    }
}

/// A byte received on COM1, if one is waiting and it carries the log:
/// `kdb`'s.
pub fn read() -> Option<u8> {
    if carries_log() { COM1.read_byte() } else { None }
}

/// The kernel log's serial console. While a program has the line's
/// terminal open the log stays off it, but for a panic's report; and none
/// goes out if `serial-log=` leaves COM1 out.
pub struct SerialSink;

impl Sink for SerialSink {
    fn write(&self, text: &str, _style: Style) {
        if LINE_OPEN.load(Ordering::Relaxed) && !radix_sync::in_panic_mode() {
            return;
        }
        write(text);
    }
}

/// Makes COM1 the line of a terminal: routes its ISA interrupt to a
/// handler that calls `byte` with each byte received, once [`line_open`]
/// turns the port's interrupt on. Returns what the port is, for the log,
/// or `None` if there is no COM1, it was started already, or its
/// interrupt could not be routed.
pub fn start_line(byte: fn(u8)) -> Option<&'static str> {
    if !present() || RECEIVED.get().is_some() {
        return None;
    }
    RECEIVED.call_once(|| byte);
    let Some((vector, gsi)) = devices::route_isa(IRQ_COM1, interrupt, 0) else {
        radix_klog::warn!(
            "Serial: COM1's ISA interrupt {IRQ_COM1} could not be routed: it has another \
             handler, or no vector is free"
        );
        return None;
    };
    radix_klog::info!(
        "Serial: COM1 takes typing by interrupt {IRQ_COM1} (GSI {gsi}, vector {vector:#x}); its \
         transmitter takes {} bytes at once",
        TX_ROOM.load(Ordering::Relaxed)
    );
    Some(if TX_ROOM.load(Ordering::Relaxed) == FIFO_BYTES {
        "COM1, a 16550A at I/O port 0x3f8"
    } else {
        "COM1, a UART without FIFOs at I/O port 0x3f8"
    })
}

/// The line's terminal was opened (`open`), or closed for the last time.
/// Opening empties the receive FIFO of what came before, as Linux's 8250
/// driver does, and turns the receive interrupt on; the kernel log stays
/// off the line until it is closed.
pub fn line_open(open: bool) {
    if !present() {
        return;
    }
    let were_enabled = crate::cpu::disable_interrupts();
    let guard = LOCK.lock();
    if open {
        for _ in 0..RECEIVE_LIMIT {
            if COM1.read_byte().is_none() {
                break;
            }
        }
        COM1.write_reg(4, MCR_DTR_RTS | MCR_OUT2);
        COM1.write_reg(1, IER_RECEIVED);
    } else {
        COM1.write_reg(1, 0);
        COM1.write_reg(4, MCR_DTR_RTS);
    }
    LINE_OPEN.store(open, Ordering::Relaxed);
    drop(guard);
    if were_enabled {
        crate::cpu::enable_interrupts();
    }
}

/// Hands the transmitter as much of `bytes` as it takes now, without
/// waiting: a FIFO's worth once it is empty, else nothing. Returns how
/// many it took. The terminal has done its output processing, so `\n`
/// is not turned into `\r\n` here.
pub fn line_send(bytes: &[u8]) -> usize {
    if !present() || bytes.is_empty() {
        return 0;
    }
    let were_enabled = crate::cpu::disable_interrupts();
    let guard = LOCK.lock();
    let mut sent = 0;
    if COM1.read_reg(5) & LSR_THR_EMPTY != 0 {
        sent = bytes.len().min(TX_ROOM.load(Ordering::Relaxed));
        for &byte in &bytes[..sent] {
            COM1.write_reg(0, byte);
        }
    }
    drop(guard);
    if were_enabled {
        crate::cpu::enable_interrupts();
    }
    sent
}

/// Sets the line's speed, as the divisor of 115200 baud, and its
/// character format, as the line control register holds it with the
/// divisor latch off.
pub fn line_set(divisor: u16, control: u8) {
    if !present() || divisor == 0 {
        return;
    }
    let [low, high] = divisor.to_le_bytes();
    let were_enabled = crate::cpu::disable_interrupts();
    let guard = LOCK.lock();
    COM1.write_reg(3, LCR_DLAB);
    COM1.write_reg(0, low);
    COM1.write_reg(1, high);
    COM1.write_reg(3, control & !LCR_DLAB);
    drop(guard);
    if were_enabled {
        crate::cpu::enable_interrupts();
    }
}

/// COM1's interrupt: hands on every byte waiting, which quiets it. The
/// bytes are read a FIFO's worth at a time under the lock, since
/// [`line_set`] swaps the data register for the divisor latch while it
/// holds it, and handed on after, so that nothing `byte` does can wait on
/// the lock this CPU holds.
fn interrupt(_: usize) {
    let Some(&byte) = RECEIVED.get() else { return };
    for _ in 0..RECEIVE_LIMIT / FIFO_BYTES {
        let mut chunk = [0u8; FIFO_BYTES];
        let mut count = 0;
        {
            let _guard = LOCK.lock();
            while count < chunk.len() {
                let Some(received) = COM1.read_byte() else { break };
                chunk[count] = received;
                count += 1;
            }
        }
        for &received in &chunk[..count] {
            byte(received);
        }
        if count < chunk.len() {
            break;
        }
    }
}
