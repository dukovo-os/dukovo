// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Serial ports as terminals, with Linux's names (ADRs 0318, 0360, 0480
//! and 0481): COM1 to COM4 are `/devices/ttyS0` to `ttyS3`, each if it is
//! there, and the 16550s on PCI follow from `ttyS4` in the order the PCI
//! scan finds them, from serial cards such as those built on the ASIX
//! AX99100 to Intel AMT's Serial-over-LAN port. So a BMC's
//! Serial-over-LAN on COM2 is `ttyS1`, and AMT's is `ttyS4`, on both
//! systems.
//!
//! - COM1 is the architecture's, from the kernel's first instructions,
//!   and is a terminal through `Arch::start_serial_line` and the calls
//!   after it. The other ports are a `radix_uart::Console` here:
//!   [`find_legacy`] probes COM2 to COM4 as soon as the command line is
//!   read, and the PCI scan hands each port it finds to [`pci::bind`].
//! - `console=ttyS1,57600n8` runs a port at that line from its first use,
//!   and serial-console gives each `console=` port a sign-in. With no
//!   `console=` at all, the port the firmware sends its console to, as
//!   ACPI's SPCR names it, runs at the firmware's speed and gets the
//!   sign-in (ADR 0481), unless `spcr=off`.
//!   `serial-log=` chooses the ports that carry the kernel log, `kdb` and
//!   a panic report, all of them unless it says otherwise; one left out
//!   gets nothing from the kernel and is not read by `kdb`, so that a UPS
//!   or a modem on it is never sent text or read as commands.
//! - [`start`] makes the terminals of the ports found so far and starts
//!   the `serial` thread, which carries every line ([`line`]) and sends
//!   the log to the log ports besides COM1, each at its own pace from its
//!   own place in [`OUTPUT`] (`radix_uart::Console::send_some`). A port
//!   the PCI scan finds after gets its terminal then.
//! - Typing comes by interrupt where a port's interrupt reaches the
//!   kernel: ISA interrupt 3, which COM2 and COM4 share ([`legacy`]), and
//!   MSI on PCI. Elsewhere it is polled every 10 ms while the terminal is
//!   open: on COM3, whose interrupt 4 is COM1's, on a PCI port without
//!   MSI, as the kernel routes no INTx, and on any port until its
//!   interrupt has once brought bytes. After that a port is still polled
//!   every 100 ms, should an interrupt be lost.
//! - While a program has a port open, the kernel log stays off it, as on
//!   COM1; `kdb`'s words and a panic report still go out on it if it is a
//!   log port.
//! - `kdb` and the panic path, which cannot wait for the thread, send at
//!   once ([`write`], [`flush`]) through `crate::debug_console`, which
//!   also reads what is typed on a log port ([`read`]).
//!
//! `/kernel/class/tty` shows each port, from [`ports`].
//!
//! Locks: [`CONSOLE`] before [`OUTPUT`] and before one of [`RECEIVED`],
//! which are never held together otherwise. Nothing is logged, allocated
//! or waited for while any is held. The sink takes only [`OUTPUT`], with
//! interrupts off; the interrupt handlers take [`CONSOLE`], and so does
//! everyone else, with interrupts off.

mod legacy;
mod line;
pub mod pci;

use core::fmt::{self, Write as _};
use core::sync::atomic::{AtomicBool, Ordering};

use radix_hal::cmdline::CommandLine;
use radix_hal::{Devices, FirmwareConsole, Timer};
use radix_hw::pci::Address as PciAddress;
use radix_klog::{Sink, Style, TextBuf, info, warn};
use radix_sync::{Once, SpinLock};
use radix_tty::Ring;
use radix_tty::line::{Line, Parity, Stop, cflag};
use radix_uart::{Console, FIFO_BYTES, LineRegisters, Output, Pending, Uart};

use crate::Arch;
use crate::fs::tty::{self, TtyRef};
use crate::ioports::IoWindow;
use crate::sched::{self, Options, Policy, ThreadRef};

/// The most ports besides COM1: COM2 to COM4, and eight on PCI.
const PORTS: usize = 11;
/// The lines: COM1's, then each port's, at its index in [`CONSOLE`]
/// plus one.
const LINES: usize = PORTS + 1;
/// The bytes of the log kept for the ports.
const OUTPUT_BYTES: usize = 64 * 1024;
/// The bytes kept of what a line received for its terminal: what the
/// terminal's own input holds.
const RECEIVED_BYTES: usize = 4096;
/// The most bytes taken from a port at once, should one that went away
/// read as always holding some.
const RECEIVE_LIMIT: usize = 256;
/// How long one of `kdb`'s writes may wait on the ports.
const WRITE_NS: u64 = 1_000_000_000;
/// How long the panic path waits for the report to go out, and the
/// newest bytes it keeps for that: 1.4 s's worth at 115200 baud.
const FLUSH_NS: u64 = 2_000_000_000;
const FLUSH_BYTES: usize = 16 * 1024;

/// What a port is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// COM1 to COM4, by number.
    Com(u8),
    /// A port on PCI: what it is, its function, and which of the
    /// function's ports it is, from 0.
    Pci { kind: radix_uart::Kind, address: PciAddress, port: u8 },
}

impl Kind {
    /// Its word in `/kernel/class/tty/ttyS<n>/kind`.
    pub fn word(self) -> &'static str {
        match self {
            Kind::Com(1) => "com1",
            Kind::Com(2) => "com2",
            Kind::Com(3) => "com3",
            Kind::Com(_) => "com4",
            Kind::Pci { kind: radix_uart::Kind::AmtSol, .. } => "amt-sol",
            Kind::Pci { kind: radix_uart::Kind::Ax99100, .. } => "ax99100",
            Kind::Pci { kind: radix_uart::Kind::Qemu(_), .. } => "qemu",
            Kind::Pci { kind: radix_uart::Kind::Generic, .. } => "pci",
        }
    }
}

/// How what is typed on a port reaches the kernel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interrupt {
    /// By this ISA interrupt.
    Isa(u8),
    /// By MSI.
    Msi,
    /// By polling, while the terminal is open.
    Polled,
}

impl fmt::Display for Interrupt {
    /// As `/kernel/class/tty/ttyS<n>/interrupt` says it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Interrupt::Isa(irq) => write!(f, "isa {irq}"),
            Interrupt::Msi => f.write_str("msi"),
            Interrupt::Polled => f.write_str("polled"),
        }
    }
}

/// What a line is, for the log and `/kernel/class/tty`.
#[derive(Clone, Copy, Debug)]
pub struct About {
    /// The `n` of its name, `ttyS<n>`.
    pub number: u8,
    pub kind: Kind,
    /// Its first I/O port.
    pub base: u16,
    pub interrupt: Interrupt,
    /// Whether it carries the kernel log, `kdb` and a panic report.
    pub log: bool,
    /// The line it runs at from boot.
    pub line: Line,
    /// The line the firmware gives it, when it is the port the firmware
    /// sends its console to ([`firmware_console`]), whether or not the
    /// command line lets it be a console.
    pub firmware: Option<Line>,
}

impl About {
    /// Its name in `/devices`.
    pub fn name(&self) -> TextBuf<8> {
        name(self.number)
    }

    /// Whether its interrupt has to be told apart from polling, by
    /// whether it once came.
    fn by_interrupt(&self) -> bool {
        self.interrupt != Interrupt::Polled
    }
}

/// `ttyS<number>`.
fn name(number: u8) -> TextBuf<8> {
    let mut name = TextBuf::new();
    let _ = write!(name, "ttyS{number}");
    name
}

/// What each line is, by its index: COM1's first, then each port's.
static ABOUT: SpinLock<[Option<About>; LINES]> = SpinLock::new([None; LINES]);
/// The ports besides COM1.
static CONSOLE: SpinLock<Console<IoWindow, PORTS>> = SpinLock::new(Console::new());
/// The log, for the log ports besides COM1.
static OUTPUT: SpinLock<Output<OUTPUT_BYTES>> = SpinLock::new(Output::new());
/// What each line received that the thread has not typed on its
/// terminal yet. Filled by the interrupt handlers and by the thread, with
/// [`CONSOLE`] held so that bytes keep their order.
static RECEIVED: [SpinLock<Ring<[u8; RECEIVED_BYTES]>>; LINES] =
    [const { SpinLock::new(Ring::new([0; RECEIVED_BYTES])) }; LINES];
/// Set when a line's [`RECEIVED`] was full and typing was lost, for the
/// thread to say so once.
static LOST: [AtomicBool; LINES] = [const { AtomicBool::new(false) }; LINES];
/// Set once a line's interrupt brings bytes: it is polled seldom after.
static SEEN: [AtomicBool; LINES] = [const { AtomicBool::new(false) }; LINES];
/// Whether a program has a line's terminal open, as the thread last saw.
static OPEN: [AtomicBool; LINES] = [const { AtomicBool::new(false) }; LINES];
/// The terminals made that the thread has not taken up yet.
static MADE: SpinLock<[Option<TtyRef>; LINES]> = SpinLock::new([const { None }; LINES]);
/// Whether [`MADE`] holds one.
static NEW: AtomicBool = AtomicBool::new(false);
/// The `serial` thread.
static THREAD: Once<ThreadRef> = Once::new();
/// The command line, which a port found by the PCI scan reads its line
/// and its part in the log from.
static FLAGS: Once<CommandLine<'static>> = Once::new();
/// Whether a port besides COM1 carries the log, and the sink is in.
static LOGGING: AtomicBool = AtomicBool::new(false);
/// Whether `kdb` holds the ports' typing, at a breakpoint.
static HELD: AtomicBool = AtomicBool::new(false);
/// The port the firmware sends its console to, asked of the architecture
/// once.
static FIRMWARE: Once<Option<FirmwareConsole>> = Once::new();

/// The log's sink: copies the text, as the log holds interrupts off
/// around its sinks and a port at 115200 baud takes 87 µs a byte.
struct SerialSink;

impl Sink for SerialSink {
    fn write(&self, text: &str, _style: Style) {
        OUTPUT.lock().push(text);
    }
}

static SINK: SerialSink = SerialSink;

/// [`OUTPUT`], locked for each chunk, so that the log adds to it while
/// the ports send.
struct Waiting;

impl Pending for Waiting {
    fn peek_from(&mut self, position: u64, chunk: &mut [u8]) -> (u64, usize) {
        OUTPUT.lock_irq().peek_from(position, chunk)
    }

    fn end(&mut self) -> u64 {
        OUTPUT.lock_irq().end()
    }
}

/// Warns of each `console=` whose options no port can run, and of each
/// name in `serial-log=` that is no port's. Called once, with the other
/// warnings of the command line.
pub fn check_flags(flags: &CommandLine<'_>) {
    for (_, value) in flags.flags().filter(|&(name, _)| name == "console") {
        let Some((port, Some(options))) = value
            .and_then(|value| value.split_once(','))
            .map(|(port, options)| (port, Some(options).filter(|options| !options.is_empty())))
        else {
            continue;
        };
        if port_number(port).is_none() {
            continue;
        }
        match Line::from_console_options(options) {
            None => warn!(
                "Serial: console={port},{options} is not Linux's console options, a speed then \
                 n, o or e and 7 or 8, such as 57600n8; {port} runs at 115200n8"
            ),
            Some(line) if radix_uart::line_registers(&line).is_none() => warn!(
                "Serial: {port} cannot run at {} baud, past a 16550's 115200; it runs at \
                 115200n8",
                line.rate
            ),
            Some(_) => {}
        }
    }
    if let Some(value) = flags.value("spcr").filter(|&value| value != "off") {
        warn!("Serial: spcr={value} is not a setting: spcr=off is the only one");
    }
    if let Some(ports) = flags.value("serial-log").filter(|&ports| !matches!(ports, "all" | "none"))
    {
        for port in ports.split(',').filter(|&port| port_number(port).is_none()) {
            warn!("Serial: `{port}` in serial-log= is no serial port's name, such as ttyS0");
        }
    }
}

/// The `n` of a port named `ttyS<n>`.
fn port_number(name: &str) -> Option<u8> {
    let digits = name.strip_prefix("ttyS")?;
    let leading_zero = digits.len() > 1 && digits.starts_with('0');
    if leading_zero || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The port the firmware sends its console to, as ACPI's SPCR names it
/// on a PC, when the kernel drives it (ADR 0481).
fn firmware_console() -> Option<FirmwareConsole> {
    *FIRMWARE.call_once(Arch::firmware_console)
}

/// The line the firmware gives the port at I/O port `base`, if it is the
/// one it sends its console to: its speed, 115200 baud when it leaves the
/// speed as it was, as GRUB takes it; 8N1, as GRUB runs it too.
fn firmware_line(base: u16) -> Option<Line> {
    let console = firmware_console().filter(|console| console.base == base)?;
    Some(Line { rate: console.rate.unwrap_or(Line::DEFAULT.rate), ..Line::DEFAULT })
}

/// The line port `name` at I/O port `base` runs at from boot: its
/// `console=` options; the firmware's line, when no `console=` is given
/// and it is the firmware's console ([`firmware_line`]); otherwise, or
/// when no 16550 runs the line asked for ([`check_flags`] says so),
/// 115200n8.
fn console_line(flags: &CommandLine<'_>, name: &str, base: u16) -> (Line, LineRegisters) {
    let registers = |line: Line| Some((line, radix_uart::line_registers(&line)?));
    flags
        .console_options(name)
        .flatten()
        .and_then(Line::from_console_options)
        .and_then(registers)
        .or_else(|| firmware_line(base).filter(|_| flags.firmware_console()).and_then(registers))
        .unwrap_or((Line::DEFAULT, LineRegisters::DEFAULT))
}

/// Runs COM1 at the firmware's line when it is the firmware's console
/// and no `console=` is given, finds COM2 to COM4, each at its line from
/// boot, and puts the log's sink in, with what was logged so far, if one
/// carries the log. Called once, as soon as the command line `flags` is
/// read, before the kernel's first line: the architecture ran COM1 as its
/// `console=` says from the start.
pub fn find_legacy(flags: CommandLine<'static>) {
    let flags = FLAGS.call_once(|| flags);
    if flags.firmware_console() && firmware_line(COM1.1).is_some() {
        let (_, registers) = console_line(flags, COM1.0, COM1.1);
        Arch::serial_line_set(registers.divisor, registers.control);
    }
    legacy::find(flags);
}

/// COM1's name and first I/O port, which the architecture drives.
const COM1: (&str, u16) = ("ttyS0", 0x3f8);

/// Adds `uart`, a port that is `about`, to [`CONSOLE`]. Returns its line,
/// or `None` if there is no room for it.
fn add(uart: Uart<IoWindow>, about: About) -> Option<usize> {
    let index = CONSOLE.lock_irq().add(uart, about.log)? + 1;
    ABOUT.lock_irq()[index] = Some(about);
    if about.log && !LOGGING.swap(true, Ordering::AcqRel) && !radix_klog::add_sink(&SINK, true) {
        warn!(
            "Serial: the kernel log has no room for another console; only kdb and a panic report \
             reach the serial ports besides COM1"
        );
    }
    Some(index)
}

/// Makes COM1 and the ports found so far terminals, and starts the
/// `serial` thread that carries them and sends the log. Called once,
/// before the PCI scan, whose ports get theirs as they are found.
pub fn start() {
    let flags = FLAGS.get().copied().unwrap_or(CommandLine::new(""));
    let (name, base) = COM1;
    let (line, _) = console_line(&flags, name, base);
    if let Some(port) = Arch::start_serial_line(com1_received) {
        let about = About {
            number: 0,
            kind: Kind::Com(1),
            base,
            interrupt: Interrupt::Isa(4),
            log: flags.carries_log(name),
            line,
            firmware: firmware_line(base),
        };
        ABOUT.lock_irq()[0] = Some(about);
        // Its interrupt is trusted from the start: the architecture's.
        SEEN[0].store(true, Ordering::Relaxed);
        make_terminal(0, &about, Some(port));
    }
    legacy::route();
    let options = Options::new().policy(Policy::Fair { nice: -5 });
    match sched::spawn("serial", run, 0, options) {
        Ok(thread) => {
            let _ = THREAD.set(thread);
        }
        Err(err) => {
            warn!(
                "Serial: could not start the thread that carries the serial lines: {err}; they \
                 are not terminals, and only kdb and a panic report reach the ports besides COM1"
            );
            return;
        }
    }
    for index in 1..LINES {
        let about = ABOUT.lock_irq()[index];
        if let Some(about) = about {
            make_terminal(index, &about, None);
        }
    }
}

/// Makes line `index`'s terminal, `/devices/ttyS<n>`, and hands it to
/// the thread; logs what the line is. `com1` is COM1's own description.
fn make_terminal(index: usize, about: &About, com1: Option<&str>) {
    let name = about.name();
    let flags = about.line.cflag().unwrap_or(tty::LINE_CFLAG) | cflag::CREAD | cflag::CLOCAL;
    let terminal = match tty::make_line(name.as_str(), flags, wake, index) {
        Ok(terminal) => terminal,
        Err(err) => {
            warn!("Serial: /devices/{name} could not be made: {err:?}; its port is no terminal");
            return;
        }
    };
    MADE.lock_irq()[index] = Some(terminal);
    NEW.store(true, Ordering::Release);
    wake(index);
    let mut text = TextBuf::<256>::new();
    let _ = describe(&mut text, about, com1);
    info!("{text}");
}

/// The log's line for a terminal made: "Serial: COM2 at I/O port 0x2f8
/// is the terminal /devices/ttyS1, at 57600 baud, 8N1; ...".
fn describe(text: &mut TextBuf<256>, about: &About, com1: Option<&str>) -> fmt::Result {
    let line = &about.line;
    match (com1, about.kind) {
        // As COM1's has said since ADR 0318; boot tests and the lab read it.
        (Some(port), _) => {
            write!(text, "Serial: {port} is the terminal /devices/ttyS0, at {} baud", line.rate)?;
        }
        (None, Kind::Com(number)) => write!(
            text,
            "Serial: COM{number} at I/O port {:#x} is the terminal /devices/{}, at {} baud",
            about.base,
            about.name(),
            line.rate
        )?,
        (None, Kind::Pci { kind, address, .. }) => write!(
            text,
            "Serial: the {} at {address}, I/O port {:#x}, is the terminal /devices/{}, at {} baud",
            kind.name(),
            about.base,
            about.name(),
            line.rate
        )?,
    }
    let parity = match line.parity {
        Parity::None => 'N',
        Parity::Odd => 'O',
        Parity::Even => 'E',
        Parity::Mark => 'M',
        Parity::Space => 'S',
    };
    let stop = match line.stop {
        Stop::One => "1",
        Stop::OneAndHalf => "1.5",
        Stop::Two => "2",
    };
    write!(text, ", {}{parity}{stop}", line.data_bits)?;
    if about.firmware.is_some() && FLAGS.get().is_some_and(CommandLine::firmware_console) {
        text.write_str(", the firmware's console (ACPI SPCR)")?;
    }
    text.write_str(if about.log {
        "; the kernel log and kdb are on it"
    } else {
        "; the kernel log is not on it"
    })?;
    match about.interrupt {
        _ if com1.is_some() => Ok(()),
        Interrupt::Isa(irq) => write!(text, "; typing by ISA interrupt {irq}"),
        Interrupt::Msi => text.write_str("; typing by MSI"),
        Interrupt::Polled => text.write_str("; typing polled every 10 ms while it is open"),
    }
}

/// Wakes the thread: a line received bytes, or its terminal changed.
fn wake(_: usize) {
    if let Some(thread) = THREAD.get() {
        sched::wake(thread);
    }
}

/// A byte COM1 received, in its interrupt.
fn com1_received(byte: u8) {
    if !RECEIVED[0].lock().push_byte(byte) {
        LOST[0].store(true, Ordering::Relaxed);
    }
    wake(0);
}

/// Moves what port `index` (a line's, so not COM1) received into its
/// [`RECEIVED`], with [`CONSOLE`] held as `console`, if its terminal is
/// open; by its interrupt if `by_interrupt`. Returns whether it held any;
/// wakes the thread if so. What is typed on a port no program has open
/// stays there, for `kdb`.
fn take_typing(console: &Console<IoWindow, PORTS>, index: usize, by_interrupt: bool) -> bool {
    if !OPEN.get(index).is_some_and(|open| open.load(Ordering::Acquire)) {
        return false;
    }
    let Some(uart) = index.checked_sub(1).and_then(|port| console.uart(port)) else {
        return false;
    };
    let mut chunk = [0u8; FIFO_BYTES];
    let mut taken = 0;
    while taken < RECEIVE_LIMIT {
        let count = uart.take_received(&mut chunk);
        if count == 0 {
            break;
        }
        taken += count;
        if RECEIVED[index].lock_irq().push(&chunk[..count]) < count {
            LOST[index].store(true, Ordering::Relaxed);
        }
    }
    if taken == 0 {
        return false;
    }
    if by_interrupt {
        SEEN[index].store(true, Ordering::Relaxed);
    }
    wake(index);
    true
}

/// The `serial` thread: see [`line::run`].
fn run(_: usize) {
    line::run();
}

/// Whether a port besides COM1 carries the log, `kdb` and a panic
/// report.
pub fn present() -> bool {
    LOGGING.load(Ordering::Acquire)
}

/// Writes `text`, and what waits before it, to the log ports at once,
/// for `kdb`: to those a program has open too, but not the log they were
/// spared.
pub fn write(text: &str) {
    if !present() {
        return;
    }
    let deadline = Arch::now_ns().saturating_add(WRITE_NS);
    let mut console = CONSOLE.lock_irq();
    let start = {
        let mut output = OUTPUT.lock_irq();
        let start = output.end();
        output.push(text);
        start
    };
    console.skip_open_to(start);
    console.send_now(&mut Waiting, &mut Arch::now_ns, deadline);
}

/// A byte typed on a log port, if any: `kdb`'s.
pub fn read() -> Option<u8> {
    if !present() {
        return None;
    }
    CONSOLE.lock_irq().read()
}

/// Sends the panic report, the newest bytes of the log, to every log
/// port, for up to 2 s: at a panic, and as the machine powers off or
/// restarts. Panic mode takes the locks from the stopped CPUs that may
/// hold them.
pub fn flush() {
    if !present() {
        return;
    }
    OUTPUT.lock_irq().keep_newest(FLUSH_BYTES);
    let deadline = Arch::now_ns().saturating_add(FLUSH_NS);
    CONSOLE.lock_irq().send_now(&mut Waiting, &mut Arch::now_ns, deadline);
}

/// Holds what is typed on the ports for `kdb` while it runs at a
/// breakpoint (`held`), and lets it go to the terminals after: the other
/// CPUs keep running, and the ports' interrupts and the thread would
/// take it for the terminals that programs have open. The log goes out on
/// the lines meanwhile. After a panic the other CPUs are stopped, and
/// this is not needed.
pub fn hold_for_kdb(held: bool) {
    HELD.store(held, Ordering::Release);
    for index in (0..LINES).filter(|&index| OPEN[index].load(Ordering::Acquire)) {
        line::receive_interrupts(index, !held);
    }
}

/// What each line is, for `/kernel/class/tty`.
pub fn ports() -> [Option<About>; LINES] {
    *ABOUT.lock_irq()
}
