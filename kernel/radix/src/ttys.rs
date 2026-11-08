// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The PC's first serial port, COM1, as a terminal programs open (Phase
//! 14, ADR 0318): `/devices/ttyS0`, as Linux names it, so that the
//! installer, a shell or a sign-in can run on a serial console with no
//! screen attached.
//!
//! COM1 already carries the kernel log and `kdb`. [`start`] makes it the
//! line of a terminal (`fs::tty::make_line`) at the 115200 baud GRUB and
//! the log run it at, and starts the `ttyS0` thread, which carries the
//! line as a USB serial port's controller thread does:
//!
//! - The port's interrupt hands each byte it receives to [`received`],
//!   which keeps it in [`RECEIVED`] and wakes the thread; the thread
//!   types it on the terminal. What the terminal cannot take yet waits
//!   until a program reads, which wakes the thread again.
//! - What programs write, once the terminal has processed it for the
//!   line, goes to the transmitter a FIFO's worth at a time, and the
//!   thread sleeps 1 ms between, as long as more waits.
//! - The speed and the character format follow the terminal's settings,
//!   through the 16550's divisor and line control register
//!   (`radix_uart::line_registers`).
//! - While a program has the terminal open, the port's receive interrupt
//!   is on and the kernel log stays off the line, which the program draws
//!   on; the log is still kept, for `dmesg` and logd, and a panic report
//!   and `kdb` still go out on the line. Once the last program closes it,
//!   the log goes to the line again.
//!
//! DTR and RTS stay raised, since the log needs them, so the line starts
//! without HUPCL; and a 16550 has no flow control of its own, so CRTSCTS
//! is not honoured. A machine without COM1 has no `ttyS0`. While `kdb`
//! runs at a breakpoint, what is typed on COM1 is its own
//! ([`hold_for_kdb`]).

use core::sync::atomic::{AtomicBool, Ordering};

use radix_hal::Devices;
use radix_klog::{info, warn};
use radix_sync::{Once, SpinLock};
use radix_tty::Ring;
use radix_tty::line::{B115200, Line, cflag};

use crate::Arch;
use crate::fs::tty::{self, TtyRef};
use crate::sched::{self, Options, Policy, ThreadRef};

/// The terminal's name in `/devices`.
const NAME: &str = "ttyS0";

/// The control flags it starts with: B115200, CS8, CREAD and CLOCAL, the
/// line GRUB and the kernel log already run.
const CFLAG: u32 = B115200 | cflag::CSIZE | 0o200 | cflag::CLOCAL;

/// The bytes the interrupt keeps for the thread: what the terminal's own
/// input holds.
const RECEIVED_BYTES: usize = 4096;

/// The bytes the thread takes from the terminal to send at once.
const SEND_BYTES: usize = 256;

/// How long the thread sleeps while what it sends waits on the
/// transmitter: a FIFO's worth takes 1.4 ms at 115200 baud.
const BUSY_NS: u64 = 1_000_000;

/// The line's terminal.
static TTY: Once<TtyRef> = Once::new();
/// The thread that carries it.
static THREAD: Once<ThreadRef> = Once::new();
/// What the port received that the thread has not typed yet. Filled by
/// the interrupt, so only ever locked with interrupts off.
static RECEIVED: SpinLock<Ring<[u8; RECEIVED_BYTES]>> =
    SpinLock::new(Ring::new([0; RECEIVED_BYTES]));
/// Set when [`RECEIVED`] was full and a byte was lost, for the thread to
/// say so once.
static LOST: AtomicBool = AtomicBool::new(false);

/// Makes COM1 the line of `/devices/ttyS0` and starts its thread, if the
/// machine has COM1. Called once, after the device tree is made.
pub fn start() {
    let Some(port) = Arch::start_serial_line(received) else { return };
    let tty = match tty::make_line(NAME, CFLAG, wake, 0) {
        Ok(tty) => tty,
        Err(err) => {
            warn!("Serial: {port} is not a terminal: /devices/{NAME} could not be made: {err:?}");
            return;
        }
    };
    let _ = TTY.set(tty);
    match sched::spawn(NAME, run, 0, Options::new().policy(Policy::Fair { nice: -5 })) {
        Ok(thread) => {
            let _ = THREAD.set(thread);
            info!("Serial: {port} is the terminal /devices/{NAME}, at 115200 baud");
        }
        Err(err) => {
            warn!("Serial: could not start the thread that carries /devices/{NAME}: {err}");
            if let Some(tty) = TTY.get() {
                tty.line_gone();
            }
        }
    }
}

/// Turns the port's receive interrupt off while `kdb` runs at a
/// breakpoint (`held`), and back on after, if a program has the terminal
/// open: the other CPUs keep running then, and the interrupt would take
/// what is typed for `kdb`. The kernel log goes out on the line meanwhile.
/// After a panic the other CPUs are stopped, and this is not needed.
pub fn hold_for_kdb(held: bool) {
    if TTY.get().is_some_and(|tty| tty.line_open()) {
        Arch::serial_line_open(!held);
    }
}

/// A byte the port received, in its interrupt.
fn received(byte: u8) {
    if !RECEIVED.lock().push_byte(byte) {
        LOST.store(true, Ordering::Relaxed);
    }
    wake(0);
}

/// Wakes the thread: a byte came, or the terminal changed.
fn wake(_: usize) {
    if let Some(thread) = THREAD.get() {
        sched::wake(thread);
    }
}

/// What the thread keeps between its rounds.
struct Carried {
    /// Whether a program has the terminal open.
    open: bool,
    /// The settings' generation last set on the port.
    generation: u64,
    /// Whether a speed the port cannot run at was reported.
    warned: bool,
    /// Bytes received that the terminal has not taken yet.
    held: [u8; SEND_BYTES],
    held_len: usize,
    /// Bytes taken from the terminal that the transmitter has not taken
    /// yet, from `sending_at`.
    sending: [u8; SEND_BYTES],
    sending_at: usize,
    sending_len: usize,
}

/// The `ttyS0` thread: carries the line, then sleeps until the port or
/// the terminal wakes it, or for 1 ms while the transmitter is busy.
fn run(_: usize) {
    let Some(tty) = TTY.get() else { return };
    let mut carried = Carried {
        open: false,
        generation: 0,
        warned: false,
        held: [0; SEND_BYTES],
        held_len: 0,
        sending: [0; SEND_BYTES],
        sending_at: 0,
        sending_len: 0,
    };
    loop {
        opened_or_closed(tty, &mut carried);
        settings(tty, &mut carried);
        give(tty, &mut carried);
        let busy = send(tty, &mut carried);
        if busy {
            sched::block_until(sched::now_ns().saturating_add(BUSY_NS), NAME);
        } else {
            sched::block(NAME);
        }
    }
}

/// Turns the port's interrupt on as a program opens the terminal, and
/// off after the last one closes it, which also lets the kernel log back
/// onto the line.
fn opened_or_closed(tty: &TtyRef, carried: &mut Carried) {
    let open = tty.line_open();
    if open == carried.open {
        return;
    }
    carried.open = open;
    Arch::serial_line_open(open);
    if !open {
        // Typing nobody reads goes, as on Linux.
        carried.held_len = 0;
        RECEIVED.lock_irq().clear();
    }
}

/// Sets the port's speed and character format if the terminal's settings
/// changed.
fn settings(tty: &TtyRef, carried: &mut Carried) {
    let (generation, flags, ospeed) = tty.line_settings();
    if generation == carried.generation {
        return;
    }
    carried.generation = generation;
    let line = Line::from_cflag(flags, ospeed);
    match radix_uart::line_registers(&line) {
        Some(registers) => Arch::serial_line_set(registers.divisor, registers.control),
        // B0 hangs a line up; this one's DTR stays raised for the log.
        None if line.rate == 0 => {}
        None if !carried.warned => {
            carried.warned = true;
            warn!("Serial: COM1 cannot run at {} baud; /devices/{NAME} keeps its speed", line.rate);
        }
        None => {}
    }
}

/// Types what the port received on the terminal, as much as it takes.
fn give(tty: &TtyRef, carried: &mut Carried) {
    loop {
        if carried.held_len == 0 {
            carried.held_len = RECEIVED.lock_irq().pop(&mut carried.held);
            if carried.held_len == 0 {
                break;
            }
        }
        let taken = tty.line_give(&carried.held[..carried.held_len]);
        carried.held.copy_within(taken..carried.held_len, 0);
        carried.held_len -= taken;
        if taken == 0 {
            // The terminal is full: the rest waits for a program to read.
            break;
        }
    }
    if LOST.swap(false, Ordering::Relaxed) {
        warn!("Serial: /devices/{NAME} received more than its programs read; some typing was lost");
    }
}

/// Hands the transmitter what the terminal has to send, as much as it
/// takes now. Returns whether some still waits on it.
fn send(tty: &TtyRef, carried: &mut Carried) -> bool {
    loop {
        if carried.sending_len == 0 {
            carried.sending_at = 0;
            carried.sending_len = tty.line_take(&mut carried.sending);
            if carried.sending_len == 0 {
                return false;
            }
            // A program may have opened the terminal since this round
            // began, and what was just taken may be its first writes. The
            // receive interrupt goes on before they go out, not in the
            // next round: a question such as serial-console's size query
            // gets its answer at once, and the next round's opening would
            // empty the receive FIFO of that answer. A program opens the
            // terminal before it writes, so what it wrote being here means
            // its opening shows here too.
            opened_or_closed(tty, carried);
        }
        let from = carried.sending_at;
        let sent = Arch::serial_line_send(&carried.sending[from..from + carried.sending_len]);
        if sent == 0 {
            return true;
        }
        carried.sending_at += sent;
        carried.sending_len -= sent;
    }
}
