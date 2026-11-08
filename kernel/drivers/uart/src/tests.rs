// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Identification against the IDs of real and emulated functions, and
//! the port, console and output ring against simulated 16550s: one that
//! sends at once, one whose transmitter never empties, as AMT's port may
//! with no session open, and one that is gone and reads all ones.

use std::collections::VecDeque;
use std::string::String;
use std::sync::{Arc, Mutex};
use std::vec::Vec;

use radix_hw::IoPorts;

use super::*;

/// How a simulated port's transmitter behaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tx {
    /// Its FIFO empties as soon as it is read.
    Fast,
    /// It never empties.
    Stuck,
}

#[derive(Debug)]
struct State {
    tx: Tx,
    /// Whether the device answers at all; a gone one reads all ones.
    present: bool,
    scratch: u8,
    line: u8,
    interrupts: u8,
    divisor: u16,
    fifo: u8,
    modem: u8,
    /// Bytes sent, in order.
    sent: Vec<u8>,
    /// Bytes in the transmit FIFO since it last emptied.
    queued: usize,
    /// The most ever queued at once.
    most_queued: usize,
    /// Bytes waiting to be read.
    received: VecDeque<u8>,
    /// Line status reads.
    status_reads: u32,
}

/// A simulated 16550 at offset `base` of an I/O window of `len` ports.
#[derive(Clone, Debug)]
struct Fake {
    state: Arc<Mutex<State>>,
    base: u16,
    len: u16,
}

impl Fake {
    fn new(tx: Tx) -> Fake {
        Fake::at(tx, 0, 8)
    }

    fn at(tx: Tx, base: u16, len: u16) -> Fake {
        Fake {
            state: Arc::new(Mutex::new(State {
                tx,
                present: true,
                scratch: 0,
                line: 0,
                interrupts: 0xff,
                divisor: 0,
                fifo: 0,
                modem: 0,
                sent: Vec::new(),
                queued: 0,
                most_queued: 0,
                received: VecDeque::new(),
                status_reads: 0,
            })),
            base,
            len,
        }
    }

    fn sent(&self) -> String {
        String::from_utf8(self.state.lock().unwrap().sent.clone()).unwrap()
    }

    fn set_tx(&self, tx: Tx) {
        self.state.lock().unwrap().tx = tx;
    }
}

impl IoPorts for Fake {
    fn len(&self) -> u16 {
        self.len
    }

    fn read8(&self, offset: u16) -> u8 {
        let mut s = self.state.lock().unwrap();
        if !s.present {
            return 0xff;
        }
        let Some(offset) = offset.checked_sub(self.base).filter(|&o| o < 8) else {
            return 0xff;
        };
        let dlab = s.line & 0x80 != 0;
        match offset {
            0 if dlab => s.divisor as u8,
            0 => s.received.pop_front().unwrap_or(0),
            1 if dlab => (s.divisor >> 8) as u8,
            1 => s.interrupts,
            3 => s.line,
            4 => s.modem,
            5 => {
                s.status_reads += 1;
                let empty = match s.tx {
                    Tx::Fast => {
                        s.queued = 0;
                        true
                    }
                    Tx::Stuck => s.queued == 0,
                };
                (u8::from(empty) << 5) | (u8::from(empty) << 6) | u8::from(!s.received.is_empty())
            }
            7 => s.scratch,
            _ => 0,
        }
    }

    fn write8(&self, offset: u16, value: u8) {
        let mut s = self.state.lock().unwrap();
        if !s.present {
            return;
        }
        let Some(offset) = offset.checked_sub(self.base).filter(|&o| o < 8) else {
            return;
        };
        let dlab = s.line & 0x80 != 0;
        match offset {
            0 if dlab => s.divisor = (s.divisor & 0xff00) | u16::from(value),
            0 => {
                s.sent.push(value);
                s.queued += 1;
                s.most_queued = s.most_queued.max(s.queued);
            }
            1 if dlab => s.divisor = (s.divisor & 0x00ff) | (u16::from(value) << 8),
            1 => s.interrupts = value,
            2 => s.fifo = value,
            3 => s.line = value,
            4 => s.modem = value,
            7 => s.scratch = value,
            _ => {}
        }
    }

    fn read16(&self, _offset: u16) -> u16 {
        unreachable!("a 16550's registers are bytes")
    }

    fn write16(&self, _offset: u16, _value: u16) {
        unreachable!("a 16550's registers are bytes")
    }

    fn read32(&self, _offset: u16) -> u32 {
        unreachable!("a 16550's registers are bytes")
    }

    fn write32(&self, _offset: u16, _value: u32) {
        unreachable!("a 16550's registers are bytes")
    }
}

fn ids(vendor: u16, device: u16, subsystem: (u16, u16), class: (u8, u8, u8)) -> Ids {
    Ids { vendor, device, subsystem, class }
}

const SERIAL_16550: (u8, u8, u8) = (0x07, 0x00, 0x02);

#[test]
fn the_amt_list_is_sorted_for_its_binary_search() {
    assert!(AMT_SOL.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn identifies_the_ax99100s_serial_functions_and_no_other() {
    // As the BZIZU card's ports show themselves: 125b:9100, A000h:1000h.
    let serial = ids(0x125b, 0x9100, (0xa000, 0x1000), SERIAL_16550);
    assert_eq!(identify(&serial), Some(Kind::Ax99100));
    assert_eq!(Kind::Ax99100.ports(), 1);
    // Its subsystem says serial even if a card maker changed the class.
    let other_class = ids(0x125b, 0x9100, (0xa000, 0x1000), (0x07, 0x80, 0x00));
    assert_eq!(identify(&other_class), Some(Kind::Ax99100));
    // A card maker's own subsystem, with the 16550 class.
    let own_subsystem = ids(0x125b, 0x9100, (0x1d6b, 0x0001), SERIAL_16550);
    assert_eq!(identify(&own_subsystem), Some(Kind::Ax99100));
    // The parallel port, SPI and local bus functions are not serial ports.
    for (subsystem, class) in [
        ((0xa000, 0x2000), (0x07, 0x01, 0x03)),
        ((0xa000, 0x6000), (0x0c, 0x80, 0x00)),
        ((0xa000, 0x7000), (0x06, 0x80, 0x00)),
    ] {
        assert_eq!(identify(&ids(0x125b, 0x9100, subsystem, class)), None);
    }
}

#[test]
fn identifies_amt_serial_over_lan() {
    // The OptiPlex 7040's: the 100 Series chipset's KT port.
    let kt = ids(0x8086, 0xa13d, (0x1028, 0x06b9), SERIAL_16550);
    assert_eq!(identify(&kt), Some(Kind::AmtSol));
    assert_eq!(Kind::AmtSol.name(), "Intel AMT Serial-over-LAN port");
    // The OptiPlex 990's Q67 and an Alder Lake one.
    for device in [0x1c3d, 0x51e3] {
        assert_eq!(identify(&ids(0x8086, device, (0, 0), SERIAL_16550)), Some(Kind::AmtSol));
    }
    // Another Intel 16550 is a generic one; a non-16550 Intel function,
    // such as the management engine's interface, is none.
    assert_eq!(identify(&ids(0x8086, 0x1234, (0, 0), SERIAL_16550)), Some(Kind::Generic));
    assert_eq!(identify(&ids(0x8086, 0xa13a, (0, 0), (0x07, 0x80, 0x00))), None);
}

#[test]
fn identifies_qemus_cards_with_their_port_counts() {
    for (device, ports) in [(0x0002, 1), (0x0003, 2), (0x0004, 4)] {
        let kind = identify(&ids(0x1b36, device, (0x1af4, 0x1100), SERIAL_16550));
        assert_eq!(kind, Some(Kind::Qemu(ports)));
        assert_eq!(kind.unwrap().ports(), usize::from(ports));
    }
    // QEMU's NVMe controller is no serial port.
    assert_eq!(identify(&ids(0x1b36, 0x0010, (0x1af4, 0x1100), (0x01, 0x08, 0x02))), None);
}

#[test]
fn takes_every_16550_programming_interface_and_no_other() {
    for interface in 0x02..=0x06 {
        let found = identify(&ids(0x1415, 0xc158, (0, 0), (0x07, 0x00, interface)));
        assert_eq!(found, Some(Kind::Generic));
    }
    // An 8250 or 16450 has no FIFO; a modem or a parallel port is no
    // serial port.
    for class in [(0x07, 0x00, 0x00), (0x07, 0x00, 0x01), (0x07, 0x03, 0x02), (0x07, 0x01, 0x02)] {
        assert_eq!(identify(&ids(0x1415, 0xc158, (0, 0), class)), None);
    }
}

#[test]
fn probe_programs_115200_8n1_with_fifos_and_no_interrupts() {
    let fake = Fake::new(Tx::Fast);
    let uart = Uart::probe(fake.clone(), 0).expect("the port answers");
    let s = fake.state.lock().unwrap();
    assert_eq!(s.divisor, 1);
    assert_eq!(s.line, 0x03);
    assert_eq!(s.interrupts, 0x00);
    assert_eq!(s.fifo, 0xc7);
    assert_eq!(s.modem, 0x03);
    drop(s);
    assert!(uart.ready());
}

#[test]
fn probe_finds_no_port_where_none_answers() {
    let gone = Fake::new(Tx::Fast);
    gone.state.lock().unwrap().present = false;
    assert!(Uart::probe(gone, 0).is_none());
    // A window too short for the port.
    assert!(Uart::probe(Fake::at(Tx::Fast, 0, 7), 0).is_none());
    // A port past the window's end, and one at its very end.
    assert!(Uart::probe(Fake::at(Tx::Fast, 8, 8), 8).is_none());
    assert!(Uart::probe(Fake::at(Tx::Fast, 8, 16), 8).is_some());
    // An offset whose port would run past the end of I/O space.
    assert!(Uart::probe(Fake::at(Tx::Fast, 0, u16::MAX), u16::MAX - 3).is_none());
}

#[test]
fn a_port_that_went_away_reads_neither_ready_nor_bytes() {
    let fake = Fake::new(Tx::Fast);
    let uart = Uart::probe(fake.clone(), 0).unwrap();
    fake.state.lock().unwrap().received.push_back(b'x');
    fake.state.lock().unwrap().present = false;
    // All ones would say both "empty" and "data ready", forever.
    assert!(!uart.ready());
    assert_eq!(uart.receive(), None);
}

/// A clock for the console that moves 1 ms each time it is read.
fn ticking() -> impl FnMut() -> u64 {
    let mut now = 0;
    move || {
        now += 1_000_000;
        now
    }
}

/// Sends all of `text` through `console`, as `kdb` does.
fn send<const N: usize>(console: &mut Console<Fake, N>, text: &str) {
    let mut output: Output<256> = Output::new();
    output.push(text);
    assert!(console.send_now(&mut output, &mut ticking(), u64::MAX));
    assert!(output.is_empty());
}

#[test]
fn the_console_writes_crlf_lines_to_every_port() {
    let (a, b) = (Fake::new(Tx::Fast), Fake::new(Tx::Fast));
    let mut console: Console<Fake, 4> = Console::new();
    assert!(console.is_empty());
    assert!(console.add(Uart::probe(a.clone(), 0).unwrap()));
    assert!(console.add(Uart::probe(b.clone(), 0).unwrap()));
    assert_eq!(console.len(), 2);
    send(&mut console, "Boot complete.\nkdb> ");
    assert_eq!(a.sent(), "Boot complete.\r\nkdb> ");
    assert_eq!(b.sent(), "Boot complete.\r\nkdb> ");
}

#[test]
fn the_console_never_overfills_a_fifo() {
    let fake = Fake::new(Tx::Fast);
    let mut console: Console<Fake, 1> = Console::new();
    console.add(Uart::probe(fake.clone(), 0).unwrap());
    // Newlines that land on a chunk's last byte, where `\r` fills it.
    let text: String = (0..40).map(|i| if i % 16 == 15 { '\n' } else { 'a' }).collect();
    send(&mut console, &text);
    let state = fake.state.lock().unwrap();
    assert!(state.most_queued <= FIFO_BYTES, "{} bytes queued", state.most_queued);
    drop(state);
    assert_eq!(fake.sent(), text.replace('\n', "\r\n"));
}

#[test]
fn a_busy_port_holds_the_chunk_until_it_has_stalled() {
    let (stuck, fast) = (Fake::new(Tx::Stuck), Fake::new(Tx::Fast));
    let mut console: Console<Fake, 2> = Console::new();
    console.add(Uart::probe(stuck.clone(), 0).unwrap());
    console.add(Uart::probe(fast.clone(), 0).unwrap());
    // The stuck port takes its first FIFO's worth, then never empties.
    assert!(console.try_send(b"0123456789abcdef", 0));
    // The next chunk waits for it, as it would for a port sending at
    // 115200 baud...
    assert!(!console.try_send(b"more", 1_000));
    assert!(!console.try_send(b"more", STALL_NS - 1));
    assert_eq!(fast.sent(), "0123456789abcdef");
    // ...until it has been busy for STALL_NS, from when it was first
    // found busy.
    assert!(console.try_send(b"more", STALL_NS + 1_000));
    assert!(console.is_stalled(0));
    assert!(!console.is_stalled(1));
    assert_eq!(console.stalled(), 1);
    assert_eq!(stuck.sent(), "0123456789abcdef");
    assert_eq!(fast.sent(), "0123456789abcdefmore");
}

#[test]
fn a_stalled_port_is_skipped_and_the_others_still_get_the_text() {
    let (stuck, fast) = (Fake::new(Tx::Stuck), Fake::new(Tx::Fast));
    let mut console: Console<Fake, 2> = Console::new();
    console.add(Uart::probe(stuck.clone(), 0).unwrap());
    console.add(Uart::probe(fast.clone(), 0).unwrap());
    send(&mut console, "0123456789abcdef and then some more\n");
    assert!(console.is_stalled(0));
    assert_eq!(stuck.sent(), "0123456789abcdef");
    assert_eq!(fast.sent(), "0123456789abcdef and then some more\r\n");
    // Stalled, it costs one status read a chunk and gets nothing.
    let before = stuck.state.lock().unwrap().status_reads;
    assert!(console.try_send(b"next line\r\n", 0));
    assert_eq!(stuck.state.lock().unwrap().status_reads, before + 1);
    assert_eq!(stuck.sent(), "0123456789abcdef");
    assert!(fast.sent().ends_with("next line\r\n"));
    // Once its transmitter empties, it gets the text again.
    stuck.set_tx(Tx::Fast);
    send(&mut console, "back\n");
    assert!(!console.is_stalled(0));
    assert_eq!(console.stalled(), 0);
    assert_eq!(stuck.sent(), "0123456789abcdefback\r\n");
}

#[test]
fn every_port_stalled_drops_the_text() {
    let stuck = Fake::new(Tx::Stuck);
    let mut console: Console<Fake, 1> = Console::new();
    console.add(Uart::probe(stuck.clone(), 0).unwrap());
    send(&mut console, "0123456789abcdef and the rest\n");
    assert!(console.is_stalled(0));
    assert!(console.try_send(b"gone", 0));
    assert_eq!(stuck.sent(), "0123456789abcdef");
    // With no port at all, too.
    let mut none: Console<Fake, 1> = Console::new();
    assert!(none.try_send(b"nowhere", 0));
}

#[test]
fn sending_at_once_stops_at_the_deadline() {
    let stuck = Fake::new(Tx::Stuck);
    let mut console: Console<Fake, 1> = Console::new();
    console.add(Uart::probe(stuck.clone(), 0).unwrap());
    let mut output: Output<64> = Output::new();
    output.push("0123456789abcdef and the rest");
    // 10 ms is less than STALL_NS: the port is still waited for.
    assert!(!console.send_now(&mut output, &mut ticking(), 10_000_000));
    assert_eq!(output.len(), " and the rest".len());
    assert!(!console.is_stalled(0));
}

#[test]
fn sending_at_once_stalls_a_busy_port_when_the_clock_stands_still() {
    let stuck = Fake::new(Tx::Stuck);
    let mut console: Console<Fake, 1> = Console::new();
    console.add(Uart::probe(stuck.clone(), 0).unwrap());
    let mut output: Output<64> = Output::new();
    output.push("0123456789abcdef and the rest");
    assert!(console.send_now(&mut output, &mut || 0, u64::MAX));
    assert!(console.is_stalled(0));
    assert_eq!(stuck.sent(), "0123456789abcdef");
}

#[test]
fn the_output_ring_keeps_the_newest_bytes() {
    let mut output: Output<8> = Output::new();
    assert!(output.is_empty());
    output.push("ab\n");
    assert_eq!(output.len(), 4);
    let mut chunk = [0; FIFO_BYTES];
    assert_eq!(output.peek(&mut chunk), (0, 4));
    assert_eq!(&chunk[..4], b"ab\r\n");
    // Peeking takes nothing; consuming does.
    output.consume_until(2);
    assert_eq!(output.peek(&mut chunk), (2, 2));
    assert_eq!(&chunk[..2], b"\r\n");
    // Past its size, the oldest bytes give way, across the wrap.
    output.push("0123456789");
    assert_eq!(output.len(), 8);
    assert_eq!(output.lost(), 4);
    assert_eq!(output.peek(&mut chunk), (6, 8));
    assert_eq!(&chunk[..8], b"23456789");
    output.keep_newest(3);
    assert_eq!(output.lost(), 9);
    assert_eq!(output.peek(&mut chunk), (11, 3));
    assert_eq!(&chunk[..3], b"789");
    output.keep_newest(5);
    assert_eq!(output.len(), 3);
    output.consume_until(100);
    assert!(output.is_empty());
    assert_eq!(output.peek(&mut chunk), (14, 0));
}

#[test]
fn bytes_that_gave_way_while_a_chunk_went_out_are_not_dropped_twice() {
    let mut output: Output<8> = Output::new();
    output.push("abcdefgh");
    let mut chunk = [0; FIFO_BYTES];
    let (position, len) = output.peek(&mut chunk);
    assert_eq!((position, len), (0, 8));
    // While the chunk is sent, the log adds 3 bytes, and the 3 oldest
    // give way.
    output.push("xyz");
    output.consume_until(position + len as u64);
    // What was added after the chunk is still there.
    assert_eq!(output.peek(&mut chunk), (8, 3));
    assert_eq!(&chunk[..3], b"xyz");
}

#[test]
fn sending_some_stops_at_the_first_busy_port_and_resumes() {
    let stuck = Fake::new(Tx::Stuck);
    let mut console: Console<Fake, 1> = Console::new();
    console.add(Uart::probe(stuck.clone(), 0).unwrap());
    let mut output: Output<64> = Output::new();
    output.push("0123456789abcdefXY");
    assert!(!console.send_some(&mut output, 0));
    assert_eq!(stuck.sent(), "0123456789abcdef");
    assert_eq!(output.len(), 2);
    stuck.state.lock().unwrap().queued = 0;
    assert!(console.send_some(&mut output, 1_000_000));
    assert_eq!(stuck.sent(), "0123456789abcdefXY");
    assert!(output.is_empty());
}

#[test]
fn a_chunk_is_at_most_a_fifo() {
    let mut output: Output<64> = Output::new();
    output.push(&"x".repeat(40));
    let mut chunk = [0; FIFO_BYTES];
    assert_eq!(output.peek(&mut chunk), (0, FIFO_BYTES));
}

#[test]
fn the_console_reads_what_is_typed_on_any_port() {
    let (a, b) = (Fake::new(Tx::Fast), Fake::new(Tx::Fast));
    let mut console: Console<Fake, 2> = Console::new();
    console.add(Uart::probe(a.clone(), 0).unwrap());
    console.add(Uart::probe(b.clone(), 0).unwrap());
    assert_eq!(console.read(), None);
    b.state.lock().unwrap().received.extend(*b"cpus\r");
    a.state.lock().unwrap().received.push_back(b'h');
    let mut got = Vec::new();
    while let Some(byte) = console.read() {
        got.push(byte);
    }
    assert_eq!(got, b"hcpus\r");
}

#[test]
fn a_full_console_turns_a_port_away() {
    let mut console: Console<Fake, 1> = Console::new();
    assert!(console.add(Uart::probe(Fake::new(Tx::Fast), 0).unwrap()));
    assert!(!console.add(Uart::probe(Fake::new(Tx::Fast), 0).unwrap()));
    assert_eq!(console.len(), 1);
}

#[test]
fn a_line_is_set_by_its_divisor_and_line_control() {
    use radix_tty::line::{B9600, B115200, Line, Parity, Stop, cflag};
    let read = 0o200;
    // COM1's own: 115200 baud, 8N1, as GRUB and the kernel's log set it.
    let line = Line::from_cflag(B115200 | cflag::CSIZE | read | cflag::CLOCAL, 0);
    assert_eq!(line_registers(&line), Some(LineRegisters { divisor: 1, control: 0x03 }));
    // A serial port's usual start, 9600 baud: divisor 12.
    let line = Line::from_cflag(B9600 | cflag::CSIZE | read, 0);
    assert_eq!(line_registers(&line), Some(LineRegisters { divisor: 12, control: 0x03 }));
    // 7E2 at 38400 (code 0o17): divisor 3, 7 bits, two stop bits, even.
    let line = Line::from_cflag(0o17 | 0o40 | cflag::PARENB | cflag::CSTOPB, 0);
    assert_eq!(line_registers(&line), Some(LineRegisters { divisor: 3, control: 0x1e }));
    // Odd, mark and space parity, and five bits with one and a half stop
    // bits, which the same bit gives.
    let with =
        |data_bits, parity, stop| Line { rate: 115_200, data_bits, parity, stop, rts_cts: false };
    assert_eq!(line_registers(&with(8, Parity::Odd, Stop::One)).map(|r| r.control), Some(0x0b));
    assert_eq!(line_registers(&with(8, Parity::Mark, Stop::One)).map(|r| r.control), Some(0x2b));
    assert_eq!(line_registers(&with(8, Parity::Space, Stop::One)).map(|r| r.control), Some(0x3b));
    assert_eq!(
        line_registers(&with(5, Parity::None, Stop::OneAndHalf)).map(|r| r.control),
        Some(0x04)
    );
    // 110 baud rounds to divisor 1047, 0.03% off; 50 baud's 2304 is exact.
    let slow = |rate| Line { rate, ..with(8, Parity::None, Stop::One) };
    assert_eq!(line_registers(&slow(110)).map(|r| r.divisor), Some(1047));
    assert_eq!(line_registers(&slow(50)).map(|r| r.divisor), Some(2304));
    // Past the base clock, a rate no divisor comes near, and B0, which
    // hangs up: the line stays as it is.
    assert_eq!(line_registers(&slow(230_400)), None);
    assert_eq!(line_registers(&slow(80_000)), None);
    assert_eq!(line_registers(&slow(0)), None);
    // Rates a divisor gives within 2%: 56000 is 2 (57600, 2.9% off), so
    // not; 57600 is.
    assert_eq!(line_registers(&slow(56_000)), None);
    assert_eq!(line_registers(&slow(57_600)).map(|r| r.divisor), Some(2));
}
