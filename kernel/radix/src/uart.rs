// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Serial ports on PCI (ADR 0360): serial cards such as those built on
//! the ASIX AX99100, Intel AMT's Serial-over-LAN port, and QEMU's
//! `pci-serial`, as consoles beside COM1, for the machines that have
//! none.
//!
//! The PCI scan (`fs::disk::init`) hands each function [`identify`] knows
//! to [`bind`], which turns on its I/O decoding, finds its ports at the
//! start of its BAR 0 (`radix_uart::Uart::probe`), and adds them to one
//! console of up to [`PORTS`]. The first port makes that console a sink
//! of the kernel log, which replays the log so far into it, and starts
//! the `serial` thread.
//!
//! The sink only copies into [`OUTPUT`], a ring of the last 64 KiB (the
//! oldest bytes give way), since the sinks run with the log's lock held
//! and interrupts off, and a port at 115200 baud takes 87 µs a byte. The
//! thread sends what waits as the ports take it, a FIFO's worth at a
//! time, and sleeps between: 1 ms while bytes wait, and longer, up to
//! [`IDLE_MAX_NS`], while none have. `kdb`, which may run while the
//! thread cannot, and the panic path, whose other CPUs are stopped, send
//! at once ([`write`], [`flush`]) through `crate::debug_console`, which
//! also reads what is typed on any port ([`read`]). A port busy for
//! `radix_uart::STALL_NS` is skipped until it takes bytes again, as AMT's
//! may be with no management console in a session.
//!
//! The ports are not terminals that programs open: they carry the log
//! and `kdb`, as COM1 does. Locks: [`CONSOLE`] before [`OUTPUT`], and
//! nothing is logged while either is held.

use core::sync::atomic::{AtomicBool, Ordering};

use radix_hal::Timer;
use radix_hw::pci::{Bar, Function, command};
use radix_klog::{Sink, Style, info, warn};
use radix_sync::SpinLock;
use radix_uart::{Console, FIFO_BYTES, Ids, Kind, Output, PORT_SIZE, Pending, Uart};

use crate::Arch;
use crate::ioports::IoWindow;
use crate::sched::{self, Options, Policy};

/// The most ports the console carries.
const PORTS: usize = 8;
/// The bytes kept for the ports.
const OUTPUT_BYTES: usize = 64 * 1024;
/// How long the thread sleeps while bytes wait: a FIFO's worth takes
/// 1.4 ms at 115200 baud.
const BUSY_NS: u64 = 1_000_000;
/// How long it sleeps once nothing waits, at first; it doubles each time
/// nothing came, up to [`IDLE_MAX_NS`].
const IDLE_NS: u64 = 10_000_000;
const IDLE_MAX_NS: u64 = 100_000_000;
/// How long one of `kdb`'s writes may take.
const WRITE_NS: u64 = 1_000_000_000;
/// How long the panic path waits for the report to go out, and the
/// newest bytes it keeps for that: 1.4 s's worth at 115200 baud.
const FLUSH_NS: u64 = 2_000_000_000;
const FLUSH_BYTES: usize = 16 * 1024;

/// Whether a port carries the console.
static PRESENT: AtomicBool = AtomicBool::new(false);
/// The ports.
static CONSOLE: SpinLock<Console<IoWindow, PORTS>> = SpinLock::new(Console::new());
/// What waits to be sent to them.
static OUTPUT: SpinLock<Output<OUTPUT_BYTES>> = SpinLock::new(Output::new());

/// The log's sink: copies the text. The log holds interrupts off around
/// its sinks.
struct SerialSink;

impl Sink for SerialSink {
    fn write(&self, text: &str, _style: Style) {
        OUTPUT.lock().push(text);
    }
}

static SINK: SerialSink = SerialSink;

/// [`OUTPUT`], locked for each chunk, so the log adds to it while the
/// ports send.
struct Waiting;

impl Pending for Waiting {
    fn peek(&mut self, chunk: &mut [u8; FIFO_BYTES]) -> (u64, usize) {
        OUTPUT.lock_irq().peek(chunk)
    }

    fn consume_until(&mut self, position: u64) {
        OUTPUT.lock_irq().consume_until(position);
    }
}

/// Which kind of PCI serial port `function` is, if it is one.
pub fn identify(function: &Function) -> Option<Kind> {
    radix_uart::identify(&Ids {
        vendor: function.vendor_id(),
        device: function.device_id(),
        subsystem: function.subsystem(),
        class: function.class(),
    })
}

/// Sets up the ports of `function`, a `kind`, and adds them to the
/// console. Kept out of the frame of the PCI scan.
#[inline(never)]
pub fn bind(function: &Function, kind: Kind) {
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
    let window = IoWindow::new(base, u16::try_from(size).unwrap_or(u16::MAX));
    function.set_command(function.command() | command::IO);
    let mut added = 0usize;
    let mut full = false;
    {
        let mut console = CONSOLE.lock_irq();
        for index in 0..kind.ports() {
            let Some(offset) = u16::try_from(index).ok().and_then(|i| i.checked_mul(PORT_SIZE))
            else {
                break;
            };
            let Some(uart) = Uart::probe(window, offset) else { continue };
            if !console.add(uart) {
                full = true;
                break;
            }
            added += 1;
        }
    }
    match added {
        0 => warn!("Serial: the {name} at {address} does not answer at I/O port {base:#x}"),
        1 => info!(
            "Serial: {name} at {address}, I/O port {base:#x}: 115200 baud, 8N1; the kernel log \
             and kdb are on it"
        ),
        _ => info!(
            "Serial: {name} at {address}, I/O ports from {base:#x}: {added} ports at 115200 \
             baud, 8N1; the kernel log and kdb are on them"
        ),
    }
    if full {
        warn!(
            "Serial: the console carries {PORTS} ports at most; the rest of {address} is not used"
        );
    }
    if added > 0 {
        start();
    }
}

/// Once the first port is in: the log's sink, with what was logged so
/// far, and the thread that sends it.
fn start() {
    if PRESENT.swap(true, Ordering::AcqRel) {
        return;
    }
    if !radix_klog::add_sink(&SINK, true) {
        warn!("Serial: the kernel log has no room for another console; only kdb is on the ports");
    }
    if let Err(err) =
        sched::spawn("serial", run, 0, Options::new().policy(Policy::Fair { nice: -5 }))
    {
        warn!(
            "Serial: could not start the thread that sends the log: {err}; only kdb and a panic \
             report reach the ports"
        );
    }
}

/// The `serial` thread: sends what waits as the ports take it.
fn run(_: usize) {
    let mut idle_ns = IDLE_NS;
    loop {
        let had = !OUTPUT.lock_irq().is_empty();
        let done = CONSOLE.lock_irq().send_some(&mut Waiting, Arch::now_ns());
        let sleep_ns = if !done {
            BUSY_NS
        } else if had {
            IDLE_NS
        } else {
            idle_ns
        };
        idle_ns = if had { IDLE_NS } else { (idle_ns * 2).min(IDLE_MAX_NS) };
        sched::sleep_ns(sleep_ns);
    }
}

/// Whether a PCI serial port carries the console.
pub fn present() -> bool {
    PRESENT.load(Ordering::Acquire)
}

/// Writes `text`, and what waits before it, to the ports at once, for
/// `kdb`.
pub fn write(text: &str) {
    if !present() {
        return;
    }
    OUTPUT.lock_irq().push(text);
    send_now(WRITE_NS);
}

/// A byte typed on a port, if any.
pub fn read() -> Option<u8> {
    if !present() {
        return None;
    }
    CONSOLE.lock_irq().read()
}

/// Sends the panic report, the newest bytes that wait, for up to 2 s.
/// Panic mode takes the locks from the stopped CPUs that may hold them.
pub fn flush() {
    if !present() {
        return;
    }
    OUTPUT.lock_irq().keep_newest(FLUSH_BYTES);
    send_now(FLUSH_NS);
}

/// Sends what waits, waiting on the ports for up to `limit_ns`.
fn send_now(limit_ns: u64) {
    let deadline = Arch::now_ns().saturating_add(limit_ns);
    CONSOLE.lock_irq().send_now(&mut Waiting, &mut Arch::now_ns, deadline);
}
