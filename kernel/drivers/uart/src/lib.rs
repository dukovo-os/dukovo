// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! 16550-compatible serial ports on PCI for the Radix kernel (ADR 0360):
//! serial cards, such as those built on the ASIX AX99100, and the
//! Serial-over-LAN port of Intel's Active Management Technology (AMT),
//! which a vPro machine's management engine carries over the network.
//! The kernel makes each one a console beside COM1, for the lab machines
//! that have no COM1 (see the *Dukovo Test Lab* design): the kernel log
//! goes to it, and `kdb` reads what is typed on it.
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
//! register, and programs it the way COM1 is: 115200 baud from the
//! 1.8432 MHz clock every one of these runs its divisor from (divisor 1),
//! 8 data bits, no parity, one stop bit, FIFOs on and interrupts off.
//! What is to be written waits in an [`Output`] ring, whose oldest bytes
//! give way when it is full, and [`Console`] sends it to every port, a
//! FIFO's worth at a time, as the ports take it: from a thread that
//! comes back when they have sent it ([`Console::try_send`]), or at once
//! for `kdb` and the panic path ([`Console::send_now`]). A port busy for
//! [`STALL_NS`] is *stalled* and skipped until its transmitter is empty
//! again: AMT's port may stall this way when no management console has
//! a Serial-over-LAN session open, and a card's may when its far end
//! holds it up, and the others must not wait on it.
//!
//! The crate is a compartment with no `unsafe` code; its tests run it
//! against simulated ports.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

#[cfg(test)]
mod tests;

use radix_hw::IoPorts;

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
    /// FIFO control (write).
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

/// Line status: a received byte is waiting.
const STATUS_DATA_READY: u8 = 1 << 0;
/// Line status: the transmitter's holding register, or with FIFOs on its
/// FIFO, is empty.
const STATUS_TX_EMPTY: u8 = 1 << 5;
/// What a port that no longer answers reads as.
const GONE: u8 = 0xff;

/// One 16550-compatible port: eight registers from `base` in `regs`.
#[derive(Debug)]
pub struct Uart<P> {
    regs: P,
    base: u16,
}

impl<P: IoPorts> Uart<P> {
    /// The port at `base` in `regs`, if one answers there, programmed for
    /// 115200 baud, 8N1, FIFOs on, interrupts off.
    pub fn probe(regs: P, base: u16) -> Option<Uart<P>> {
        if base.checked_add(PORT_SIZE)? > regs.len() {
            return None;
        }
        let uart = Uart { regs, base };
        for pattern in [0x5a, 0xa5] {
            uart.write(reg::SCRATCH, pattern);
            if uart.read(reg::SCRATCH) != pattern {
                return None;
            }
        }
        uart.write(reg::INTERRUPTS, 0x00);
        uart.write(reg::LINE, 0x80); // DLAB on
        uart.write(reg::DATA, 0x01); // divisor 1: 1.8432 MHz / 16 = 115200 baud
        uart.write(reg::INTERRUPTS, 0x00);
        uart.write(reg::LINE, 0x03); // 8N1, DLAB off
        uart.write(reg::FIFO, 0xc7); // FIFOs on and cleared, 14-byte threshold
        uart.write(reg::MODEM, 0x03); // DTR and RTS
        Some(uart)
    }

    /// Whether the transmitter takes [`FIFO_BYTES`] more bytes.
    pub fn ready(&self) -> bool {
        let status = self.read(reg::STATUS);
        status != GONE && status & STATUS_TX_EMPTY != 0
    }

    /// Hands `byte` to the transmitter; call [`Uart::ready`] first.
    pub fn send(&self, byte: u8) {
        self.write(reg::DATA, byte);
    }

    /// A received byte, if one is waiting.
    pub fn receive(&self) -> Option<u8> {
        let status = self.read(reg::STATUS);
        (status != GONE && status & STATUS_DATA_READY != 0).then(|| self.read(reg::DATA))
    }

    fn read(&self, offset: u16) -> u8 {
        self.regs.read8(self.base + offset)
    }

    fn write(&self, offset: u16, value: u8) {
        self.regs.write8(self.base + offset, value);
    }
}

/// A port of a [`Console`]: whether it is stalled, and since when it has
/// been busy.
#[derive(Debug)]
struct Line<P> {
    uart: Uart<P>,
    stalled: bool,
    busy_since: Option<u64>,
}

/// Up to `N` ports that each get what is written, in step: a chunk goes
/// to every port that is not stalled once each can take it.
#[derive(Debug)]
pub struct Console<P, const N: usize> {
    lines: [Option<Line<P>>; N],
}

impl<P, const N: usize> Default for Console<P, N> {
    fn default() -> Self {
        Console::new()
    }
}

impl<P, const N: usize> Console<P, N> {
    /// A console with no port.
    pub const fn new() -> Self {
        Console { lines: [const { None }; N] }
    }

    /// How many ports it has.
    pub fn len(&self) -> usize {
        self.lines.iter().flatten().count()
    }

    /// Whether it has none.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Adds `uart`; false, and it is dropped, when the console is full.
    pub fn add(&mut self, uart: Uart<P>) -> bool {
        match self.lines.iter_mut().find(|line| line.is_none()) {
            Some(slot) => {
                *slot = Some(Line { uart, stalled: false, busy_since: None });
                true
            }
            None => false,
        }
    }

    /// How many ports are stalled.
    pub fn stalled(&self) -> usize {
        self.lines.iter().flatten().filter(|line| line.stalled).count()
    }
}

impl<P: IoPorts, const N: usize> Console<P, N> {
    /// Hands `chunk`, at most [`FIFO_BYTES`], to every port that is not
    /// stalled, if each can take it now; true if it went (to no port at
    /// all, when every one is stalled). A port busy for [`STALL_NS`] from
    /// when it was first found busy, by the clock `now_ns`, is stalled
    /// and skipped from then on, at the cost of one status read a chunk,
    /// until its transmitter is empty again.
    pub fn try_send(&mut self, chunk: &[u8], now_ns: u64) -> bool {
        debug_assert!(chunk.len() <= FIFO_BYTES);
        let mut ready = [false; N];
        let mut blocked = false;
        for (line, ready) in self.lines.iter_mut().zip(ready.iter_mut()) {
            let Some(line) = line else { continue };
            if line.uart.ready() {
                line.stalled = false;
                line.busy_since = None;
                *ready = true;
            } else if !line.stalled {
                let since = *line.busy_since.get_or_insert(now_ns);
                if now_ns.saturating_sub(since) >= STALL_NS {
                    line.stalled = true;
                } else {
                    blocked = true;
                }
            }
        }
        if blocked {
            return false;
        }
        for (line, ready) in self.lines.iter().zip(ready) {
            if let (Some(line), true) = (line, ready) {
                for &byte in chunk {
                    line.uart.send(byte);
                }
            }
        }
        true
    }

    /// Sends what waits in `pending` that the ports take now, and
    /// returns: for a thread that comes back. True if nothing is left.
    pub fn send_some(&mut self, pending: &mut impl Pending, now_ns: u64) -> bool {
        let mut chunk = [0u8; FIFO_BYTES];
        loop {
            let (position, len) = pending.peek(&mut chunk);
            if len == 0 {
                return true;
            }
            if !self.try_send(&chunk[..len], now_ns) {
                return false;
            }
            pending.consume_until(position + len as u64);
        }
    }

    /// Sends what waits in `pending`, as the ports take it, until nothing
    /// is left or the clock `now_ns` reaches `deadline_ns`: for `kdb` and
    /// the panic path, which cannot wait for a thread. Should the clock
    /// not move, a port still busy after [`SPIN_LIMIT`] polls in a row is
    /// stalled. True if all of it went.
    pub fn send_now(
        &mut self,
        pending: &mut impl Pending,
        now_ns: &mut impl FnMut() -> u64,
        deadline_ns: u64,
    ) -> bool {
        let mut chunk = [0u8; FIFO_BYTES];
        let mut polls = 0;
        loop {
            let (position, len) = pending.peek(&mut chunk);
            if len == 0 {
                return true;
            }
            let now = now_ns();
            if now >= deadline_ns {
                return false;
            }
            if self.try_send(&chunk[..len], now) {
                pending.consume_until(position + len as u64);
                polls = 0;
            } else {
                polls += 1;
                if polls >= SPIN_LIMIT {
                    self.stall_busy();
                    polls = 0;
                }
                core::hint::spin_loop();
            }
        }
    }

    /// A byte typed on any port, the first port's first.
    pub fn read(&mut self) -> Option<u8> {
        self.lines.iter().flatten().find_map(|line| line.uart.receive())
    }

    /// Whether port `index` is stalled, for the tests.
    #[cfg(test)]
    fn is_stalled(&self, index: usize) -> bool {
        self.lines[index].as_ref().is_some_and(|line| line.stalled)
    }

    /// Stalls every port still waiting to take a chunk.
    fn stall_busy(&mut self) {
        for line in self.lines.iter_mut().flatten() {
            if line.busy_since.is_some() {
                line.stalled = true;
            }
        }
    }
}

/// Text waiting to be sent, which a [`Console`] takes a chunk at a time:
/// an [`Output`], or one behind a lock that is taken for each chunk.
pub trait Pending {
    /// Copies the oldest bytes waiting, up to a chunk's worth, to
    /// `chunk`; gives where the first is in all the text ever written,
    /// and how many there are. They stay until
    /// [`Pending::consume_until`].
    fn peek(&mut self, chunk: &mut [u8; FIFO_BYTES]) -> (u64, usize);

    /// Drops the bytes before `position`, now that they are sent; those
    /// that already gave way to newer ones are gone already.
    fn consume_until(&mut self, position: u64);
}

/// The text waiting to be sent: a ring of `N` bytes, with `\n` made
/// `\r\n` as a terminal wants it, whose oldest bytes give way to new ones
/// when it is full, so that the log never waits on the ports.
#[derive(Debug)]
pub struct Output<const N: usize> {
    bytes: [u8; N],
    start: usize,
    len: usize,
    /// Where the oldest byte waiting is in all the text ever written.
    position: u64,
    lost: u64,
}

impl<const N: usize> Default for Output<N> {
    fn default() -> Self {
        Output::new()
    }
}

impl<const N: usize> Output<N> {
    /// An empty ring.
    pub const fn new() -> Self {
        Output { bytes: [0; N], start: 0, len: 0, position: 0, lost: 0 }
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

    /// How many bytes wait.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether none wait.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// How many bytes gave way to newer ones, ever.
    pub fn lost(&self) -> u64 {
        self.lost
    }

    /// Keeps only the newest `count` bytes: the panic path keeps the
    /// report, which comes last, rather than a backlog it has no time
    /// for.
    pub fn keep_newest(&mut self, count: usize) {
        if self.len > count {
            let lost = self.len - count;
            self.lost += lost as u64;
            self.drop_oldest(lost);
        }
    }

    fn push_byte(&mut self, byte: u8) {
        if N == 0 {
            return;
        }
        if self.len == N {
            self.lost += 1;
            self.drop_oldest(1);
        }
        self.bytes[(self.start + self.len) % N] = byte;
        self.len += 1;
    }

    fn drop_oldest(&mut self, count: usize) {
        let count = count.min(self.len);
        self.start = (self.start + count) % N.max(1);
        self.len -= count;
        self.position += count as u64;
    }
}

impl<const N: usize> Pending for Output<N> {
    fn peek(&mut self, chunk: &mut [u8; FIFO_BYTES]) -> (u64, usize) {
        let count = self.len.min(FIFO_BYTES);
        for (i, byte) in chunk.iter_mut().take(count).enumerate() {
            *byte = self.bytes[(self.start + i) % N];
        }
        (self.position, count)
    }

    fn consume_until(&mut self, position: u64) {
        let count = position.saturating_sub(self.position);
        self.drop_oldest(usize::try_from(count).unwrap_or(usize::MAX));
    }
}
