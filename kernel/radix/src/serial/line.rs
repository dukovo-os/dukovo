// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The `serial` thread: it carries each serial line for its terminal, as
//! a USB serial port's controller thread does, and sends the kernel log
//! to the log ports besides COM1.
//!
//! For each line, in each round ([`Carrier::carry`]):
//!
//! - As a program opens the terminal, the port drops what was typed
//!   before, as Linux's 8250 driver does, and its receive interrupt comes
//!   on; the kernel log stays off the line, which the program draws on.
//!   Once the last program closes it, the interrupt goes off and the log
//!   comes back, from its newest line.
//! - The speed and the character format follow the terminal's settings,
//!   through the 16550's divisor and line control register
//!   (`radix_uart::line_registers`).
//! - What the port received, which its interrupt or the thread's polling
//!   keeps in its `RECEIVED`, is typed on the terminal; what the terminal
//!   cannot take yet waits until a program reads, which wakes the thread.
//! - What programs write, once the terminal has processed it for the
//!   line, goes to the transmitter a FIFO's worth at a time.
//!
//! The thread then sends the log, and sleeps: 1 ms while a transmitter
//! has more to take, 10 ms while an open terminal's port is polled,
//! 100 ms while one is open at all or the log ports wait for more of the
//! log, which the sink cannot wake it for, and otherwise until a port's
//! interrupt or a terminal wakes it.
//!
//! DTR and RTS stay raised, as the log needs them, so the lines start
//! without HUPCL; and a 16550 has no flow control of its own, so CRTSCTS
//! is not honoured.

use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use radix_hal::{Devices, Timer};
use radix_klog::warn;
use radix_mm::{Flags, HEAP};
use radix_tty::line::Line;
use radix_uart::LineRegisters;

use super::{
    ABOUT, CONSOLE, HELD, LINES, LOGGING, LOST, MADE, NEW, OPEN, OUTPUT, RECEIVED, SEEN, Waiting,
};
use crate::Arch;
use crate::fs::tty::TtyRef;
use crate::sched;

/// The bytes taken at once from a terminal to send, or from a line's
/// `RECEIVED` to type.
const CHUNK: usize = 256;

/// How long the thread sleeps while a transmitter has more to take: a
/// FIFO's worth takes 1.4 ms at 115200 baud.
const BUSY_NS: u64 = 1_000_000;
/// How often an open terminal's port is polled while its typing does not
/// come by interrupt, or its interrupt has not come yet.
const POLL_NS: u64 = 10_000_000;
/// How often an open terminal's port is polled whose interrupt came,
/// should one be lost.
const CHECK_NS: u64 = 100_000_000;
/// How long the thread sleeps between sends of the log once nothing
/// came, at first; it doubles each time nothing came, up to [`CHECK_NS`].
const IDLE_NS: u64 = 10_000_000;

/// What a line's port is reached through.
#[derive(Clone, Copy, Debug)]
enum Wire {
    /// COM1, through the architecture.
    Com1,
    /// A port in `CONSOLE`, at this index.
    Port(usize),
}

impl Wire {
    /// Line `index`'s.
    fn of(index: usize) -> Wire {
        match index {
            0 => Wire::Com1,
            _ => Wire::Port(index - 1),
        }
    }

    /// The terminal was opened (`open`), or closed for the last time.
    fn open(self, index: usize, open: bool) {
        match self {
            Wire::Com1 => Arch::serial_line_open(open),
            Wire::Port(port) => {
                let interrupts = open && by_interrupt(index);
                let mut console = CONSOLE.lock_irq();
                if let Some(uart) = console.uart(port) {
                    if open {
                        uart.discard_received();
                    }
                    uart.receive_interrupts(interrupts);
                }
                console.set_open(port, open, &mut Waiting);
            }
        }
    }

    /// Sets the port's speed and character format.
    fn set(self, registers: LineRegisters) {
        match self {
            Wire::Com1 => Arch::serial_line_set(registers.divisor, registers.control),
            Wire::Port(port) => {
                if let Some(uart) = CONSOLE.lock_irq().uart(port) {
                    uart.set_line(registers);
                }
            }
        }
    }

    /// Hands the transmitter as much of `bytes` as it takes now; returns
    /// how many it took.
    fn send(self, bytes: &[u8]) -> usize {
        match self {
            Wire::Com1 => Arch::serial_line_send(bytes),
            Wire::Port(port) => {
                CONSOLE.lock_irq().uart(port).map_or(0, |uart| uart.transmit(bytes))
            }
        }
    }
}

/// Whether line `index` takes typing by an interrupt.
fn by_interrupt(index: usize) -> bool {
    ABOUT.lock_irq()[index].is_some_and(|about| about.by_interrupt())
}

/// Turns line `index`'s receive interrupt on or off while its terminal is
/// open, for `kdb` (`super::hold_for_kdb`).
pub(super) fn receive_interrupts(index: usize, on: bool) {
    match Wire::of(index) {
        Wire::Com1 => Arch::serial_line_open(on),
        Wire::Port(port) => {
            let interrupts = on && by_interrupt(index);
            if let Some(uart) = CONSOLE.lock_irq().uart(port) {
                uart.receive_interrupts(interrupts);
            }
        }
    }
}

/// What a round asks of the thread's sleep.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Need {
    /// Nothing: it may sleep until woken.
    Nothing,
    /// A look within [`CHECK_NS`].
    Check,
    /// A look within [`POLL_NS`].
    Poll,
    /// A look within [`BUSY_NS`].
    Busy,
}

/// A line, and what the thread keeps of it between rounds.
struct Carrier {
    /// Its index, COM1's 0.
    index: usize,
    wire: Wire,
    terminal: TtyRef,
    /// Whether a program has the terminal open.
    open: bool,
    /// The settings' generation last set on the port.
    generation: u64,
    /// Whether a speed the port cannot run at was reported.
    warned: bool,
    /// Bytes received that the terminal has not taken yet.
    held: [u8; CHUNK],
    held_len: usize,
    /// Bytes taken from the terminal that the transmitter has not taken
    /// yet, from `sending_at`.
    sending: [u8; CHUNK],
    sending_at: usize,
    sending_len: usize,
}

impl Carrier {
    fn new(index: usize, terminal: TtyRef) -> Carrier {
        Carrier {
            index,
            wire: Wire::of(index),
            terminal,
            open: false,
            // The settings the terminal was made with are the port's.
            generation: 0,
            warned: false,
            held: [0; CHUNK],
            held_len: 0,
            sending: [0; CHUNK],
            sending_at: 0,
            sending_len: 0,
        }
    }

    /// One round of the line; `held` while `kdb` holds the typing.
    fn carry(&mut self, held: bool) -> Need {
        if !held {
            self.opened_or_closed();
        }
        self.settings();
        let polled = self.poll(held);
        self.give();
        let busy = self.send(held);
        if busy { Need::Busy } else { polled }
    }

    /// Turns the port's interrupt on as a program opens the terminal, and
    /// off after the last one closes it, which also lets the kernel log
    /// back onto the line.
    fn opened_or_closed(&mut self) {
        let open = self.terminal.line_open();
        if open == self.open {
            return;
        }
        self.open = open;
        OPEN[self.index].store(open, Ordering::Release);
        self.wire.open(self.index, open);
        if !open {
            // Typing nobody reads goes, as on Linux.
            self.held_len = 0;
            RECEIVED[self.index].lock_irq().clear();
        }
    }

    /// Sets the port's speed and character format if the terminal's
    /// settings changed.
    fn settings(&mut self) {
        let (generation, flags, ospeed) = self.terminal.line_settings();
        if generation == self.generation {
            return;
        }
        self.generation = generation;
        let line = Line::from_cflag(flags, ospeed);
        match radix_uart::line_registers(&line) {
            Some(registers) => self.wire.set(registers),
            // B0 hangs a line up; this one's DTR stays raised for the log.
            None if line.rate == 0 => {}
            None if !self.warned => {
                self.warned = true;
                let name = super::name(self.number());
                warn!(
                    "Serial: /devices/{name} cannot run at {} baud; it keeps its speed",
                    line.rate
                );
            }
            None => {}
        }
    }

    /// Takes what the port received, if its terminal is open and its
    /// typing does not come by interrupt alone. Returns how soon it wants
    /// to be polled again.
    fn poll(&self, held: bool) -> Need {
        let Wire::Port(_) = self.wire else { return Need::Nothing };
        if !self.open || held {
            return Need::Nothing;
        }
        super::take_typing(&CONSOLE.lock_irq(), self.index, false);
        if SEEN[self.index].load(Ordering::Relaxed) { Need::Check } else { Need::Poll }
    }

    /// Types what the port received on the terminal, as much as it takes.
    fn give(&mut self) {
        loop {
            if self.held_len == 0 {
                self.held_len = RECEIVED[self.index].lock_irq().pop(&mut self.held);
                if self.held_len == 0 {
                    break;
                }
            }
            let taken = self.terminal.line_give(&self.held[..self.held_len]);
            self.held.copy_within(taken..self.held_len, 0);
            self.held_len -= taken;
            if taken == 0 {
                // The terminal is full: the rest waits for a program to
                // read.
                break;
            }
        }
        if LOST[self.index].swap(false, Ordering::Relaxed) {
            let name = super::name(self.number());
            warn!(
                "Serial: /devices/{name} received more than its programs read; some typing was lost"
            );
        }
    }

    /// Hands the transmitter what the terminal has to send, as much as it
    /// takes now. Returns whether some still waits on it.
    fn send(&mut self, held: bool) -> bool {
        loop {
            if self.sending_len == 0 {
                self.sending_at = 0;
                self.sending_len = self.terminal.line_take(&mut self.sending);
                if self.sending_len == 0 {
                    return false;
                }
                // A program may have opened the terminal since this round
                // began, and what was just taken may be its first writes.
                // The receive interrupt goes on before they go out, not in
                // the next round: a question such as serial-console's size
                // query gets its answer at once, and the next round's
                // opening would empty the receive FIFO of that answer. A
                // program opens the terminal before it writes, so what it
                // wrote being here means its opening shows here too.
                if !held {
                    self.opened_or_closed();
                }
            }
            let from = self.sending_at;
            let sent = self.wire.send(&self.sending[from..from + self.sending_len]);
            if sent == 0 {
                return true;
            }
            self.sending_at += sent;
            self.sending_len -= sent;
        }
    }

    /// The `n` of its name, `ttyS<n>`.
    fn number(&self) -> u8 {
        ABOUT.lock_irq()[self.index].map_or(0, |about| about.number)
    }
}

/// The thread: carries every line, sends the log, and sleeps as the
/// lines and the log need.
pub(super) fn run() {
    let mut lines = Vec::new_in(HEAP.with(Flags::MAY_SLEEP));
    if lines.try_reserve_exact(LINES).is_err() {
        warn!("Serial: out of memory for the serial lines; they are not terminals");
        return;
    }
    let mut idle_ns = IDLE_NS;
    let mut log_end = 0;
    loop {
        if NEW.swap(false, Ordering::Acquire) {
            for index in 0..LINES {
                let made = MADE.lock_irq()[index].take();
                if let Some(terminal) = made {
                    // Room for every line was reserved.
                    let _ = lines.push_within_capacity(Carrier::new(index, terminal));
                }
            }
        }
        let held = HELD.load(Ordering::Acquire);
        let mut need = Need::Nothing;
        for line in &mut lines {
            need = need.max(line.carry(held));
        }
        let logging = LOGGING.load(Ordering::Acquire);
        if logging {
            let end = OUTPUT.lock_irq().end();
            let sent_all = CONSOLE.lock_irq().send_some(&mut Waiting, Arch::now_ns());
            if !sent_all {
                need = Need::Busy;
                idle_ns = IDLE_NS;
            } else if end != log_end {
                idle_ns = IDLE_NS;
            } else {
                idle_ns = (idle_ns * 2).min(CHECK_NS);
            }
            log_end = end;
        }
        let lines_ns = match need {
            Need::Busy => Some(BUSY_NS),
            Need::Poll => Some(POLL_NS),
            Need::Check => Some(CHECK_NS),
            Need::Nothing => None,
        };
        // The sink cannot wake the thread: the log is looked for.
        let log_ns = logging.then_some(idle_ns);
        let sleep_ns = match (lines_ns, log_ns) {
            (Some(lines), Some(log)) => Some(lines.min(log)),
            (lines, log) => lines.or(log),
        };
        match sleep_ns {
            Some(ns) => sched::block_until(sched::now_ns().saturating_add(ns), "serial"),
            None => sched::block("serial"),
        }
    }
}
