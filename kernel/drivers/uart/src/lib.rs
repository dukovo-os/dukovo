// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 16550-compatible serial ports for the Radix kernel (ADRs 0360 and
//! 0480): the PC's COM2 to COM4, serial cards on PCI, such as those
//! built on the ASIX AX99100, and the Serial-over-LAN port of Intel's
//! Active Management Technology (AMT), which a vPro machine's management
//! engine carries over the network. The kernel makes each one a terminal
//! that programs open, with the name Linux gives it, and those that
//! `serial-log=` chooses carry the kernel log and `kdb`. COM1 is the
//! architecture's own, from the kernel's first instructions on.
//!
//! [`identify`] says from a PCI function's IDs and class whether it is
//! one. A function whose class is a serial controller with a
//! 16550-compatible programming interface (07h, 00h, 02h to 06h) is one,
//! with a port of eight registers at the start of BAR 0: AMT's port, the
//! AX99100's (one port per function, subsystem A000h:1000h) and QEMU's
//! `pci-serial` all show themselves that way. The AX99100's serial
//! functions are also known by their IDs, should a card maker give them
//! another class; its parallel, SPI and local bus functions are not
//! serial ports. Only ports in I/O space are used: memory-mapped UARTs
//! space their registers in more than one way, which the class does not
//! say.
//!
//! [`Uart::probe`] checks that a port answers, through its scratch
//! register, and programs the line it is given ([`line_registers`] makes
//! one from a terminal's settings or `console=`'s options), with FIFOs on
//! where it has them and interrupts off. Its other calls are what a
//! terminal's line needs: [`Uart::transmit`] fills the transmitter
//! without waiting for it, [`Uart::take_received`] empties the receiver,
//! and [`Uart::receive_interrupts`] turns its interrupt on, for a port
//! whose interrupt reaches the kernel.
//!
//! The kernel log waits in an [`Output`] ring, which keeps its newest
//! bytes, and a [`Console`] sends it to the log ports, each at its own
//! pace from its own place in the ring, a FIFO's worth at a time: from a
//! thread that comes back while they still have some to send
//! ([`Console::send_some`]), or at once for `kdb` and the panic path
//! ([`Console::send_now`]). No port waits on another: a port at 9600
//! baud falls behind and skips what gave way in the ring, while those at
//! 115200 keep up. A port busy for [`STALL_NS`] is *stalled* and skipped
//! until its transmitter is empty again, as AMT's port may be when no
//! management console has a Serial-over-LAN session open, and a card's
//! when its far end holds it up; it then picks up at the last screenful
//! of the log ([`RESUME_BYTES`]), rather than at the backlog. While a
//! program has a port open as a terminal, the log stays off it, but for
//! `kdb`'s words and a panic report.
//!
//! The crate is a compartment with no `unsafe` code; its tests run it
//! against simulated ports.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

#[cfg(test)]
mod tests;

use radix_hw::IoPorts;
use radix_tty::line::{self, Parity, Stop};

/// The PCI class and subclass of a serial controller.
pub const PCI_CLASS: (u8, u8) = (0x07, 0x00);

/// The programming interfaces of 16550-compatible serial controllers:
/// the 16550, 16650, 16750, 16850 and 16950.
pub const PROG_IF_16550: core::ops::RangeInclusive<u8> = 0x02..=0x06;

/// The ports a port's registers take.
pub const PORT_SIZE: u16 = 8;

/// The bytes a 16550's transmit FIFO holds; the bigger FIFOs of later
/// chips take as much.
pub const FIFO_BYTES: usize = 16;

/// How long a port may stay busy before it is taken as stalled: a FIFO's
/// worth takes 1.4 ms at 115200 baud.
pub const STALL_NS: u64 = 100_000_000;

/// Polls in a row of ports that stay busy, when the clock does not move,
/// before they are taken as stalled.
pub const SPIN_LIMIT: u32 = 1_000_000;

/// How much of the newest log a port that takes bytes again after a
/// stall is sent: about a screenful of lines, 0.2 s at 115200 baud.
pub const RESUME_BYTES: usize = 2048;

/// ASIX's PCI vendor ID.
const ASIX: u16 = 0x125b;
/// The AX99100 PCIe to multi I/O controller.
const AX99100: u16 = 0x9100;
/// The subsystem of an AX99100 function set up as a serial port.
const AX99100_SERIAL: (u16, u16) = (0xa000, 0x1000);
/// Intel's PCI vendor ID.
const INTEL: u16 = 0x8086;
/// Red Hat's, whose IDs QEMU's own devices use.
const RED_HAT: u16 = 0x1b36;

/// The device IDs of the Serial-over-LAN ports of Intel's management
/// engines ("KT", keyboard and text redirection), from the ICH8's to the
/// Core Ultra's, sorted.
const AMT_SOL: [u16; 49] = [
    0x02e3, 0x06e3, 0x108f, 0x19d5, 0x1be3, 0x1c3d, 0x1d3d, 0x1e3d, 0x2977, 0x2997, 0x29a7, 0x29b7,
    0x29c7, 0x29d7, 0x29e7, 0x29f7, 0x2a07, 0x2a17, 0x2a47, 0x2a53, 0x2e07, 0x2e17, 0x2e27, 0x2e37,
    0x2e47, 0x3b67, 0x43e3, 0x51e3, 0x7773, 0x7a6b, 0x7aeb, 0x7e73, 0x7f6b, 0x8c3d, 0x8cbd, 0x8d3d,
    0x9c3d, 0x9cbd, 0x9d3d, 0x9de3, 0xa0e3, 0xa13d, 0xa1bd, 0xa23d, 0xa2bd, 0xa363, 0xa3bd, 0xa873,
    0xd373,
];

/// What a PCI function says it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ids {
    /// Vendor ID.
    pub vendor: u16,
    /// Device ID.
    pub device: u16,
    /// Subsystem vendor and subsystem IDs.
    pub subsystem: (u16, u16),
    /// Class, subclass and programming interface.
    pub class: (u8, u8, u8),
}

/// A kind of PCI serial port this crate drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A serial function of an ASIX AX99100.
    Ax99100,
    /// The Serial-over-LAN port of Intel AMT.
    AmtSol,
    /// QEMU's `pci-serial`, `pci-serial-2x` or `pci-serial-4x`, with this
    /// many ports.
    Qemu(u8),
    /// Any other function whose class says it is a 16550.
    Generic,
}

impl Kind {
    /// Its name, for the log.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Ax99100 => "ASIX AX99100 serial port",
            Kind::AmtSol => "Intel AMT Serial-over-LAN port",
            Kind::Qemu(_) => "QEMU PCI 16550A adapter",
            Kind::Generic => "PCI 16550-compatible serial port",
        }
    }

    /// How many ports its BAR 0 holds, one after another. A card of a
    /// kind unknown here gets its first port used only: cards put more
    /// ports in more than one way.
    pub fn ports(self) -> usize {
        match self {
            Kind::Qemu(ports) => usize::from(ports),
            Kind::Ax99100 | Kind::AmtSol | Kind::Generic => 1,
        }
    }
}

/// Whether the function with `ids` is a serial port this crate drives,
/// and which kind.
pub fn identify(ids: &Ids) -> Option<Kind> {
    let (class, subclass, interface) = ids.class;
    let is_16550 = (class, subclass) == PCI_CLASS && PROG_IF_16550.contains(&interface);
    match (ids.vendor, ids.device) {
        (ASIX, AX99100) if is_16550 || ids.subsystem == AX99100_SERIAL => Some(Kind::Ax99100),
        (INTEL, device) if is_16550 && AMT_SOL.binary_search(&device).is_ok() => Some(Kind::AmtSol),
        (RED_HAT, device @ 0x0002..=0x0004) if is_16550 => Some(Kind::Qemu(match device {
            0x0002 => 1,
            0x0003 => 2,
            _ => 4,
        })),
        _ => is_16550.then_some(Kind::Generic),
    }
}

/// Register offsets of a 16550.
mod reg {
    /// Receive buffer (read) and transmit holding register (write), or
    /// the divisor's low byte with DLAB set.
    pub const DATA: u16 = 0;
    /// Interrupt enable, or the divisor's high byte with DLAB set.
    pub const INTERRUPTS: u16 = 1;
    /// Interrupt identification (read).
    pub const IDENTIFY: u16 = 2;
    /// FIFO control (write), at the same offset.
    pub const FIFO: u16 = 2;
    /// Line control.
    pub const LINE: u16 = 3;
    /// Modem control.
    pub const MODEM: u16 = 4;
    /// Line status.
    pub const STATUS: u16 = 5;
    /// Scratch.
    pub const SCRATCH: u16 = 7;
}

/// Interrupt enable: a received byte waits, or the receive FIFO reached
/// its threshold or has held bytes for four characters' time.
const INTERRUPTS_RECEIVED: u8 = 1 << 0;
/// Interrupt identification: set while no interrupt is pending.
const IDENTIFY_NONE: u8 = 1 << 0;
/// Interrupt identification: both bits set once the FIFOs are on, which
/// only a 16550A and later have; a 16550 without the A, whose FIFOs do
/// not work, sets one.
const IDENTIFY_FIFOS: u8 = 0xc0;
/// FIFO control: FIFOs on and both cleared, a receive interrupt at 14
/// bytes.
const FIFO_ON: u8 = 0xc7;
/// Line control: the divisor latch in place of the data and interrupt
/// enable registers.
const LINE_DLAB: u8 = 1 << 7;
/// Modem control: DTR and RTS, which stay raised.
const MODEM_DTR_RTS: u8 = 0x03;
/// Modem control: OUT2, which on a PC lets a COM port's interrupt reach
/// the interrupt controller, and which Linux raises on every port whose
/// interrupt it takes.
const MODEM_OUT2: u8 = 1 << 3;
/// Line status: a received byte is waiting.
const STATUS_DATA_READY: u8 = 1 << 0;
/// Line status: the transmitter's holding register, or with FIFOs on its
/// FIFO, is empty.
const STATUS_TX_EMPTY: u8 = 1 << 5;
/// What a port that no longer answers reads as.
const GONE: u8 = 0xff;

/// The most bytes [`Uart::discard_received`] reads, should a port read as
/// always holding one.
const DISCARD_LIMIT: usize = 256;

/// The clock a 16550's divisor divides, in bits per second: its
/// 1.8432 MHz crystal over 16.
pub const BASE_RATE: u32 = 115_200;

/// How far the rate a divisor gives may be from the rate asked for, in
/// thousandths: a receiver samples each bit in its middle, and keeps in
/// step through a character up to about 3% apart.
const RATE_TOLERANCE: u32 = 20;

/// What a 16550's registers hold for a [`line::Line`]: the divisor of
/// [`BASE_RATE`] and the line control register's character format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineRegisters {
    /// The divisor latch, 1 for 115200 baud.
    pub divisor: u16,
    /// The line control register, with DLAB off: the character's size in
    /// bits 0 and 1, two stop bits (one and a half for five-bit
    /// characters) in bit 2, and the parity in bits 3 to 5.
    pub control: u8,
}

impl LineRegisters {
    /// 115200 baud, 8 data bits, no parity, one stop bit: the line GRUB
    /// and the kernel log run, and a port's own until `console=` or a
    /// program sets another.
    pub const DEFAULT: LineRegisters = LineRegisters { divisor: 1, control: 0x03 };
}

/// The registers that set a 16550's line as `line` says (ADR 0318), or
/// `None` for a rate its divisor cannot give within [`RATE_TOLERANCE`],
/// above [`BASE_RATE`], or 0, which hangs the line up and leaves it as it
/// is. The divisor is rounded to the nearest, as Linux's 8250 driver
/// rounds it. A 16550 has no flow control of its own, so `rts_cts` is
/// not set here.
pub fn line_registers(line: &line::Line) -> Option<LineRegisters> {
    if line.rate == 0 || line.rate > BASE_RATE {
        return None;
    }
    let divisor = (BASE_RATE + line.rate / 2) / line.rate;
    let divisor = u16::try_from(divisor).ok().filter(|&d| d > 0)?;
    let given = BASE_RATE / u32::from(divisor);
    if given.abs_diff(line.rate) * 1000 > line.rate * RATE_TOLERANCE {
        return None;
    }
    // Bits 0 and 1: 5 to 8 data bits as 0 to 3.
    let mut control = line.data_bits.clamp(5, 8) - 5;
    if line.stop != Stop::One {
        control |= LINE_TWO_STOP;
    }
    control |= match line.parity {
        Parity::None => 0,
        Parity::Odd => LINE_PARITY,
        Parity::Even => LINE_PARITY | LINE_EVEN,
        // A stuck parity bit is the opposite of the even bit: 1 (mark)
        // without it, 0 (space) with it.
        Parity::Mark => LINE_PARITY | LINE_STICK,
        Parity::Space => LINE_PARITY | LINE_EVEN | LINE_STICK,
    };
    Some(LineRegisters { divisor, control })
}

/// Line control: two stop bits.
const LINE_TWO_STOP: u8 = 1 << 2;
/// Line control: a parity bit is sent and checked.
const LINE_PARITY: u8 = 1 << 3;
/// Line control: even parity.
const LINE_EVEN: u8 = 1 << 4;
/// Line control: stick parity.
const LINE_STICK: u8 = 1 << 5;

/// One 16550-compatible port: eight registers from `base` in `regs`.
#[derive(Debug)]
pub struct Uart<P> {
    regs: P,
    base: u16,
    /// The bytes its transmitter takes at once: [`FIFO_BYTES`] with its
    /// FIFOs on, else 1.
    fifo: usize,
}

impl<P: IoPorts> Uart<P> {
    /// The port at `base` in `regs`, if one answers there, programmed for
    /// `line`, with FIFOs on if it has working ones, interrupts off, and
    /// DTR and RTS raised.
    pub fn probe(regs: P, base: u16, line: LineRegisters) -> Option<Uart<P>> {
        if base.checked_add(PORT_SIZE)? > regs.len() {
            return None;
        }
        let mut uart = Uart { regs, base, fifo: 1 };
        for pattern in [0x5a, 0xa5] {
            uart.write(reg::SCRATCH, pattern);
            if uart.read(reg::SCRATCH) != pattern {
                return None;
            }
        }
        uart.write(reg::INTERRUPTS, 0x00);
        uart.set_line(line);
        uart.write(reg::FIFO, FIFO_ON);
        if uart.read(reg::IDENTIFY) & IDENTIFY_FIFOS == IDENTIFY_FIFOS {
            uart.fifo = FIFO_BYTES;
        } else {
            // An 8250 or 16450 has none; a 16550's do not work.
            uart.write(reg::FIFO, 0x00);
        }
        uart.write(reg::MODEM, MODEM_DTR_RTS);
        Some(uart)
    }

    /// The bytes its transmitter takes at once: [`FIFO_BYTES`] with its
    /// FIFOs on, else 1.
    pub fn fifo(&self) -> usize {
        self.fifo
    }

    /// Sets its speed and character format; a divisor of 0 is passed
    /// over.
    pub fn set_line(&self, line: LineRegisters) {
        if line.divisor == 0 {
            return;
        }
        let [low, high] = line.divisor.to_le_bytes();
        self.write(reg::LINE, LINE_DLAB);
        self.write(reg::DATA, low);
        self.write(reg::INTERRUPTS, high);
        self.write(reg::LINE, line.control & !LINE_DLAB);
    }

    /// Turns its receive interrupt on or off, with OUT2, which a PC's COM
    /// port needs to pass it on.
    pub fn receive_interrupts(&self, on: bool) {
        if on {
            self.write(reg::MODEM, MODEM_DTR_RTS | MODEM_OUT2);
            self.write(reg::INTERRUPTS, INTERRUPTS_RECEIVED);
        } else {
            self.write(reg::INTERRUPTS, 0x00);
            self.write(reg::MODEM, MODEM_DTR_RTS);
        }
    }

    /// Whether an interrupt of its is pending.
    pub fn interrupt_pending(&self) -> bool {
        let identify = self.read(reg::IDENTIFY);
        identify != GONE && identify & IDENTIFY_NONE == 0
    }

    /// Whether the transmitter is empty, and takes [`Uart::fifo`] bytes.
    pub fn ready(&self) -> bool {
        let status = self.read(reg::STATUS);
        status != GONE && status & STATUS_TX_EMPTY != 0
    }

    /// Hands the transmitter as much of `bytes` as it takes now, without
    /// waiting: up to [`Uart::fifo`] once it is empty, else nothing.
    /// Returns how many it took.
    pub fn transmit(&self, bytes: &[u8]) -> usize {
        if bytes.is_empty() || !self.ready() {
            return 0;
        }
        let count = bytes.len().min(self.fifo);
        for &byte in &bytes[..count] {
            self.write(reg::DATA, byte);
        }
        count
    }

    /// Moves what the receiver holds into `into`, as much as fits;
    /// returns how many bytes. A port that went away reads as holding
    /// none.
    pub fn take_received(&self, into: &mut [u8]) -> usize {
        let mut count = 0;
        while count < into.len() {
            let status = self.read(reg::STATUS);
            if status == GONE || status & STATUS_DATA_READY == 0 {
                break;
            }
            into[count] = self.read(reg::DATA);
            count += 1;
        }
        count
    }

    /// Drops what the receiver holds, as Linux's 8250 driver does when a
    /// port is opened: what was typed before is nobody's.
    pub fn discard_received(&self) {
        let mut sink = [0u8; FIFO_BYTES];
        for _ in 0..DISCARD_LIMIT / FIFO_BYTES {
            if self.take_received(&mut sink) < sink.len() {
                break;
            }
        }
    }

    fn read(&self, offset: u16) -> u8 {
        self.regs.read8(self.base + offset)
    }

    fn write(&self, offset: u16, value: u8) {
        self.regs.write8(self.base + offset, value);
    }
}

/// How the last bytes a port of a [`Console`] could take went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sent {
    /// It sent all there is.
    All,
    /// Its transmitter is busy, and it is waited for.
    Busy,
    /// It is stalled, and skipped.
    Stalled,
}

/// A port of a [`Console`], and its place in the [`Output`].
#[derive(Debug)]
struct Port<P> {
    uart: Uart<P>,
    /// Whether it carries the kernel log, `kdb` and a panic report.
    log: bool,
    /// Whether a program has it open as a terminal, which keeps the log
    /// off it.
    open: bool,
    /// Where the next byte for it is in all the text ever written.
    next: u64,
    stalled: bool,
    /// When it was first found busy, since it last took bytes.
    busy_since: Option<u64>,
}

impl<P: IoPorts> Port<P> {
    /// Hands the port what waits for it in `pending`, a FIFO's worth at a
    /// time, for as long as it takes it now. `moved` is set if it took
    /// some.
    fn send(&mut self, pending: &mut impl Pending, now_ns: u64, moved: &mut bool) -> Sent {
        if self.stalled {
            if !self.uart.ready() {
                return Sent::Stalled;
            }
            self.stalled = false;
            self.next = resume_point(pending, self.next);
        }
        let mut chunk = [0u8; FIFO_BYTES];
        let room = self.uart.fifo().min(FIFO_BYTES);
        loop {
            let (from, len) = pending.peek_from(self.next, &mut chunk[..room]);
            // Bytes that gave way in the ring are skipped.
            self.next = from;
            if len == 0 {
                self.busy_since = None;
                return Sent::All;
            }
            let taken = self.uart.transmit(&chunk[..len]);
            if taken == 0 {
                let since = *self.busy_since.get_or_insert(now_ns);
                if now_ns.saturating_sub(since) < STALL_NS {
                    return Sent::Busy;
                }
                self.stalled = true;
                self.busy_since = None;
                return Sent::Stalled;
            }
            self.busy_since = None;
            self.next = from + taken as u64;
            *moved = true;
        }
    }
}

/// Where a port that stalled at `next` picks the log up again: there, if
/// it is among the newest [`RESUME_BYTES`], else at the first line that
/// starts in them.
fn resume_point(pending: &mut impl Pending, next: u64) -> u64 {
    let floor = pending.end().saturating_sub(RESUME_BYTES as u64);
    if next >= floor {
        return next;
    }
    let mut scan = [0u8; 64];
    let mut position = floor;
    while position - floor < RESUME_BYTES as u64 {
        let (from, len) = pending.peek_from(position, &mut scan);
        if len == 0 {
            return from;
        }
        if let Some(newline) = scan[..len].iter().position(|&byte| byte == b'\n') {
            return from + newline as u64 + 1;
        }
        position = from + len as u64;
    }
    floor
}

/// What a round of sending did.
struct Round {
    /// Whether a port that is not stalled still has bytes to send.
    waiting: bool,
    /// Whether any port took bytes.
    moved: bool,
}

/// Up to `N` ports, each the line of a terminal, and those that carry the
/// log sent it from their own place in an [`Output`].
#[derive(Debug)]
pub struct Console<P, const N: usize> {
    ports: [Option<Port<P>>; N],
}

impl<P, const N: usize> Default for Console<P, N> {
    fn default() -> Self {
        Console::new()
    }
}

impl<P, const N: usize> Console<P, N> {
    /// A console with no port.
    pub const fn new() -> Self {
        Console { ports: [const { None }; N] }
    }

    /// How many ports it has.
    pub fn len(&self) -> usize {
        self.ports.iter().flatten().count()
    }

    /// Whether it has none.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Adds `uart`, which carries the log if `log`; gives its index, or
    /// `None`, and it is dropped, when the console is full. It starts at
    /// the oldest byte the ring holds, so that a port found late in the
    /// boot gets the log from as near the start as there is.
    pub fn add(&mut self, uart: Uart<P>, log: bool) -> Option<usize> {
        let index = self.ports.iter().position(Option::is_none)?;
        self.ports[index] =
            Some(Port { uart, log, open: false, next: 0, stalled: false, busy_since: None });
        Some(index)
    }

    /// Port `index`'s UART, for its terminal's line.
    pub fn uart(&self, index: usize) -> Option<&Uart<P>> {
        self.port(index).map(|port| &port.uart)
    }

    /// Whether port `index` carries the log.
    pub fn carries_log(&self, index: usize) -> bool {
        self.port(index).is_some_and(|port| port.log)
    }

    /// Moves the log ports that a program has open on to `position`, so
    /// that [`Console::send_now`] gives them only what is written from
    /// there: `kdb`'s words, not the log they were spared.
    pub fn skip_open_to(&mut self, position: u64) {
        for port in self.ports.iter_mut().flatten().filter(|port| port.open) {
            port.next = port.next.max(position);
        }
    }

    fn port(&self, index: usize) -> Option<&Port<P>> {
        self.ports.get(index)?.as_ref()
    }
}

impl<P: IoPorts, const N: usize> Console<P, N> {
    /// Port `index` was opened as a terminal (`open`), or closed for the
    /// last time. An open port gets no log; a closed one takes the log
    /// again from the newest byte in `pending`, not the backlog.
    pub fn set_open(&mut self, index: usize, open: bool, pending: &mut impl Pending) {
        let Some(port) = self.ports.get_mut(index).and_then(Option::as_mut) else { return };
        port.open = open;
        if !open {
            port.next = pending.end();
            port.stalled = false;
            port.busy_since = None;
        }
    }

    /// Sends each log port that no program has open what waits for it in
    /// `pending`, as much as it takes now, and returns: for a thread that
    /// comes back. True if none still has some, but those stalled.
    pub fn send_some(&mut self, pending: &mut impl Pending, now_ns: u64) -> bool {
        !self.round(pending, now_ns, false).waiting
    }

    /// Sends every log port, open ones too, what waits for it in
    /// `pending`, as the ports take it, until each has sent all or is
    /// stalled, or the clock `now_ns` reaches `deadline_ns`: for `kdb` and
    /// the panic path, which cannot wait for a thread. Should the clock
    /// not move, ports still busy after [`SPIN_LIMIT`] polls in a row in
    /// which none took a byte are stalled. True if all of it went.
    pub fn send_now(
        &mut self,
        pending: &mut impl Pending,
        now_ns: &mut impl FnMut() -> u64,
        deadline_ns: u64,
    ) -> bool {
        let mut polls = 0;
        loop {
            let now = now_ns();
            let round = self.round(pending, now, true);
            if !round.waiting {
                return true;
            }
            if now >= deadline_ns {
                return false;
            }
            if round.moved {
                polls = 0;
                continue;
            }
            polls += 1;
            if polls >= SPIN_LIMIT {
                self.stall_busy();
                polls = 0;
            }
            core::hint::spin_loop();
        }
    }

    /// A byte typed on a log port, the first port's first: `kdb`'s.
    pub fn read(&mut self) -> Option<u8> {
        let mut byte = [0u8];
        self.ports
            .iter()
            .flatten()
            .filter(|port| port.log)
            .find_map(|port| (port.uart.take_received(&mut byte) == 1).then_some(byte[0]))
    }

    /// Whether port `index` is stalled, for the tests.
    #[cfg(test)]
    fn is_stalled(&self, index: usize) -> bool {
        self.port(index).is_some_and(|port| port.stalled)
    }

    /// Sends each log port, open ones too if `with_open`, what it takes
    /// now.
    fn round(&mut self, pending: &mut impl Pending, now_ns: u64, with_open: bool) -> Round {
        let mut round = Round { waiting: false, moved: false };
        for port in self.ports.iter_mut().flatten() {
            if port.log && (with_open || !port.open) {
                let sent = port.send(pending, now_ns, &mut round.moved);
                round.waiting |= sent == Sent::Busy;
            }
        }
        round
    }

    /// Stalls every port still waiting to take bytes.
    fn stall_busy(&mut self) {
        for port in self.ports.iter_mut().flatten() {
            if port.busy_since.take().is_some() {
                port.stalled = true;
            }
        }
    }
}

/// The text written for the ports, which a [`Console`] reads a chunk at a
/// time by its place in all the text ever written: an [`Output`], or one
/// behind a lock that is taken for each chunk, so that the log adds to it
/// while the ports send.
pub trait Pending {
    /// Copies what was written from `position` on, or from the oldest
    /// byte still held if that is later, to `chunk`, as much as fits;
    /// gives where the copy starts and how many bytes it holds.
    fn peek_from(&mut self, position: u64, chunk: &mut [u8]) -> (u64, usize);

    /// The place after the newest byte.
    fn end(&mut self) -> u64;
}

/// The text written for the ports: a ring of its newest `N` bytes, with
/// `\n` made `\r\n` as a terminal wants it, whose oldest bytes give way
/// to new ones, so that the log never waits on the ports. Each byte has
/// its place in all the text ever written, which the ports keep theirs
/// by.
#[derive(Debug)]
pub struct Output<const N: usize> {
    bytes: [u8; N],
    /// The place of the oldest byte held.
    oldest: u64,
    /// The place after the newest.
    end: u64,
}

impl<const N: usize> Default for Output<N> {
    fn default() -> Self {
        Output::new()
    }
}

impl<const N: usize> Output<N> {
    /// An empty ring.
    pub const fn new() -> Self {
        Output { bytes: [0; N], oldest: 0, end: 0 }
    }

    /// Adds `text`.
    pub fn push(&mut self, text: &str) {
        for byte in text.bytes() {
            if byte == b'\n' {
                self.push_byte(b'\r');
            }
            self.push_byte(byte);
        }
    }

    /// The place of the oldest byte held.
    pub fn oldest(&self) -> u64 {
        self.oldest
    }

    /// The place after the newest byte.
    pub fn end(&self) -> u64 {
        self.end
    }

    /// How many bytes it holds.
    pub fn len(&self) -> usize {
        // At most N.
        (self.end - self.oldest) as usize
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.end == self.oldest
    }

    /// Keeps only the newest `count` bytes: the panic path keeps the
    /// report, which comes last, rather than a backlog it has no time
    /// for.
    pub fn keep_newest(&mut self, count: usize) {
        self.oldest = self.oldest.max(self.end.saturating_sub(count as u64));
    }

    fn push_byte(&mut self, byte: u8) {
        if N == 0 {
            return;
        }
        if self.len() == N {
            self.oldest += 1;
        }
        self.bytes[Self::index(self.end)] = byte;
        self.end += 1;
    }

    /// Where the byte at `position` is in the ring.
    fn index(position: u64) -> usize {
        // The remainder is below N, a usize.
        (position % N as u64) as usize
    }
}

impl<const N: usize> Pending for Output<N> {
    fn peek_from(&mut self, position: u64, chunk: &mut [u8]) -> (u64, usize) {
        let from = position.clamp(self.oldest, self.end);
        // At most N.
        let count = ((self.end - from) as usize).min(chunk.len());
        for (offset, byte) in chunk[..count].iter_mut().enumerate() {
            *byte = self.bytes[Self::index(from + offset as u64)];
        }
        (from, count)
    }

    fn end(&mut self) -> u64 {
        self.end
    }
}
