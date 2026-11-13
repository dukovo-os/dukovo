// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Identification against the IDs of real and emulated functions, and
//! the port, console and output ring against simulated 16550s: one that
//! sends at once, one whose transmitter empties only when the test says,
//! as a slow line's or AMT's with no session open, one without FIFOs, and
//! one that is gone and reads all ones.

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
    /// It empties only when [`Fake::drain`] says so.
    Held,
}

#[derive(Debug)]
struct State {
    tx: Tx,
    /// Whether the device answers at all; a gone one reads all ones.
    present: bool,
    /// Whether it has FIFOs, as a 16550A; an 8250 or 16450 has none.
    fifos: bool,
    scratch: u8,
    line: u8,
    interrupts: u8,
    divisor: u16,
    fifo: u8,
    modem: u8,
    /// Bytes sent, in order.
    sent: Vec<u8>,
    /// Bytes in the transmitter since it last emptied.
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
                fifos: true,
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

    /// Empties a held transmitter.
    fn drain(&self) {
        self.state.lock().unwrap().queued = 0;
    }

    fn type_in(&self, text: &[u8]) {
        self.state.lock().unwrap().received.extend(text);
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
            2 => {
                let fifos = if s.fifos && s.fifo & 1 != 0 { 0xc0 } else { 0 };
                // Received data available, or none pending.
                let pending = s.interrupts & 1 != 0 && !s.received.is_empty();
                fifos | if pending { 0x04 } else { 0x01 }
            }
            3 => s.line,
            4 => s.modem,
            5 => {
                s.status_reads += 1;
                let empty = match s.tx {
                    Tx::Fast => {
                        s.queued = 0;
                        true
                    }
                    Tx::Held => s.queued == 0,
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

/// A port that answers, at 115200 baud, 8N1.
fn uart(fake: &Fake) -> Uart<Fake> {
    Uart::probe(fake.clone(), 0, LineRegisters::DEFAULT).expect("the port answers")
}

#[test]
fn probe_programs_the_line_given_with_fifos_and_no_interrupts() {
    let fake = Fake::new(Tx::Fast);
    let uart = uart(&fake);
    let s = fake.state.lock().unwrap();
    assert_eq!(s.divisor, 1);
    assert_eq!(s.line, 0x03);
    assert_eq!(s.interrupts, 0x00);
    assert_eq!(s.fifo, 0xc7);
    assert_eq!(s.modem, 0x03);
    drop(s);
    assert_eq!(uart.fifo(), FIFO_BYTES);
    assert!(uart.ready());
    // console=ttyS1,57600e7: divisor 2, 7 bits, even parity.
    let fake = Fake::new(Tx::Fast);
    let line = line_registers(&line::Line::from_console_options("57600e7").unwrap()).unwrap();
    Uart::probe(fake.clone(), 0, line).unwrap();
    let s = fake.state.lock().unwrap();
    assert_eq!((s.divisor, s.line), (2, 0x1a));
}

#[test]
fn a_uart_without_fifos_takes_a_byte_at_a_time() {
    let fake = Fake::new(Tx::Held);
    fake.state.lock().unwrap().fifos = false;
    let uart = uart(&fake);
    assert_eq!(uart.fifo(), 1);
    // Its FIFO control is left off, as a 16550 without the A needs.
    assert_eq!(fake.state.lock().unwrap().fifo, 0x00);
    assert_eq!(uart.transmit(b"ab"), 1);
    assert_eq!(uart.transmit(b"b"), 0);
    fake.drain();
    assert_eq!(uart.transmit(b"b"), 1);
    assert_eq!(fake.sent(), "ab");
    assert_eq!(fake.state.lock().unwrap().most_queued, 1);
}

#[test]
fn probe_finds_no_port_where_none_answers() {
    let gone = Fake::new(Tx::Fast);
    gone.state.lock().unwrap().present = false;
    let line = LineRegisters::DEFAULT;
    assert!(Uart::probe(gone, 0, line).is_none());
    // A window too short for the port.
    assert!(Uart::probe(Fake::at(Tx::Fast, 0, 7), 0, line).is_none());
    // A port past the window's end, and one at its very end.
    assert!(Uart::probe(Fake::at(Tx::Fast, 8, 8), 8, line).is_none());
    assert!(Uart::probe(Fake::at(Tx::Fast, 8, 16), 8, line).is_some());
    // An offset whose port would run past the end of I/O space.
    assert!(Uart::probe(Fake::at(Tx::Fast, 0, u16::MAX), u16::MAX - 3, line).is_none());
}

#[test]
fn a_line_is_set_without_touching_the_interrupts() {
    let fake = Fake::new(Tx::Fast);
    let uart = uart(&fake);
    uart.receive_interrupts(true);
    uart.set_line(LineRegisters { divisor: 12, control: 0x1b });
    let s = fake.state.lock().unwrap();
    assert_eq!((s.divisor, s.line, s.interrupts), (12, 0x1b, 0x01));
    drop(s);
    // A divisor of 0 would stop the clock: passed over.
    uart.set_line(LineRegisters { divisor: 0, control: 0x03 });
    assert_eq!(fake.state.lock().unwrap().divisor, 12);
}

#[test]
fn receive_interrupts_come_on_with_out2_and_go_off_without() {
    let fake = Fake::new(Tx::Fast);
    let uart = uart(&fake);
    fake.type_in(b"x");
    // Off, a waiting byte raises nothing.
    assert!(!uart.interrupt_pending());
    uart.receive_interrupts(true);
    assert_eq!(fake.state.lock().unwrap().modem, 0x0b);
    assert!(uart.interrupt_pending());
    let mut got = [0u8; 4];
    assert_eq!(uart.take_received(&mut got), 1);
    assert!(!uart.interrupt_pending());
    uart.receive_interrupts(false);
    let s = fake.state.lock().unwrap();
    assert_eq!((s.interrupts, s.modem), (0x00, 0x03));
}

#[test]
fn taking_what_was_received_stops_at_the_room_given() {
    let fake = Fake::new(Tx::Fast);
    let uart = uart(&fake);
    fake.type_in(b"hello");
    let mut got = [0u8; 3];
    assert_eq!(uart.take_received(&mut got), 3);
    assert_eq!(&got, b"hel");
    assert_eq!(uart.take_received(&mut got), 2);
    assert_eq!(&got[..2], b"lo");
    assert_eq!(uart.take_received(&mut got), 0);
    // Opening a port drops what was typed before.
    fake.type_in(&[b'z'; 40]);
    uart.discard_received();
    assert_eq!(uart.take_received(&mut got), 0);
}

#[test]
fn a_port_that_went_away_reads_neither_ready_nor_bytes() {
    let fake = Fake::new(Tx::Fast);
    let uart = uart(&fake);
    uart.receive_interrupts(true);
    fake.type_in(b"x");
    fake.state.lock().unwrap().present = false;
    // All ones would say "empty", "data ready" and "interrupt pending",
    // forever.
    assert!(!uart.ready());
    assert_eq!(uart.transmit(b"lost"), 0);
    let mut got = [0u8; 4];
    assert_eq!(uart.take_received(&mut got), 0);
    assert!(!uart.interrupt_pending());
    uart.discard_received();
}

#[test]
fn transmitting_fills_the_fifo_only_once_it_is_empty() {
    let fake = Fake::new(Tx::Held);
    let uart = uart(&fake);
    assert_eq!(uart.transmit(b""), 0);
    assert_eq!(uart.transmit(b"0123456789abcdef and more"), FIFO_BYTES);
    assert_eq!(uart.transmit(b" and more"), 0);
    fake.drain();
    assert_eq!(uart.transmit(b" and more"), 9);
    assert_eq!(fake.sent(), "0123456789abcdef and more");
}

/// A clock for the console that moves 1 ms each time it is read.
fn ticking() -> impl FnMut() -> u64 {
    let mut now = 0;
    move || {
        now += 1_000_000;
        now
    }
}

/// A console of the `fakes`, each a log port.
fn console<const N: usize>(fakes: &[&Fake]) -> Console<Fake, N> {
    let mut console = Console::new();
    for fake in fakes {
        console.add(uart(fake), true).expect("room for the port");
    }
    console
}

#[test]
fn the_console_writes_crlf_lines_to_every_log_port() {
    let (a, b) = (Fake::new(Tx::Fast), Fake::new(Tx::Fast));
    let mut console: Console<Fake, 4> = console(&[&a, &b]);
    assert_eq!(console.len(), 2);
    assert!(Console::<Fake, 1>::new().is_empty());
    let mut output: Output<256> = Output::new();
    output.push("Boot complete.\nkdb> ");
    assert!(console.send_now(&mut output, &mut ticking(), u64::MAX));
    assert_eq!(a.sent(), "Boot complete.\r\nkdb> ");
    assert_eq!(b.sent(), "Boot complete.\r\nkdb> ");
    // What was sent is not sent again.
    output.push("help\n");
    assert!(console.send_some(&mut output, 0));
    assert_eq!(a.sent(), "Boot complete.\r\nkdb> help\r\n");
}

#[test]
fn the_console_never_overfills_a_fifo() {
    let fake = Fake::new(Tx::Fast);
    let mut console: Console<Fake, 1> = console(&[&fake]);
    // Newlines that land on a chunk's last byte, where `\r` fills it.
    let text: String = (0..40).map(|i| if i % 16 == 15 { '\n' } else { 'a' }).collect();
    let mut output: Output<256> = Output::new();
    output.push(&text);
    assert!(console.send_now(&mut output, &mut ticking(), u64::MAX));
    let state = fake.state.lock().unwrap();
    assert!(state.most_queued <= FIFO_BYTES, "{} bytes queued", state.most_queued);
    drop(state);
    assert_eq!(fake.sent(), text.replace('\n', "\r\n"));
}

#[test]
fn a_slow_port_never_holds_up_a_fast_one() {
    let (slow, fast) = (Fake::new(Tx::Held), Fake::new(Tx::Fast));
    let mut console: Console<Fake, 2> = console(&[&slow, &fast]);
    let mut output: Output<256> = Output::new();
    let text = "0123456789abcdef".repeat(4);
    output.push(&text);
    // The fast port gets it all in one round; the slow one a FIFO's
    // worth, and the round says it still has some.
    assert!(!console.send_some(&mut output, 0));
    assert_eq!(fast.sent(), text);
    assert_eq!(slow.sent(), text[..16]);
    // Each round after its FIFO empties, it gets the next.
    for round in 2..=4 {
        slow.drain();
        let done = console.send_some(&mut output, round * 1_000_000);
        assert_eq!(slow.sent(), text[..16 * round as usize]);
        assert_eq!(done, round == 4);
    }
    assert_eq!(fast.sent(), text);
}

#[test]
fn a_port_that_falls_behind_skips_what_gave_way() {
    let (slow, fast) = (Fake::new(Tx::Held), Fake::new(Tx::Fast));
    let mut console: Console<Fake, 2> = console(&[&slow, &fast]);
    let mut output: Output<32> = Output::new();
    output.push("0123456789abcdef0123");
    console.send_some(&mut output, 0);
    assert_eq!(slow.sent(), "0123456789abcdef");
    // The log runs on past the ring's size before the slow port drains:
    // what it was to send next, from place 16, gave way.
    for (round, text) in [(1, "ABCDEFGHIJKLMNOPQRST"), (2, "UVWXYZabcdefghijklmn")] {
        output.push(text);
        console.send_some(&mut output, round * 1_000_000);
    }
    assert_eq!(fast.sent(), "0123456789abcdef0123ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmn");
    assert_eq!(output.oldest(), 60 - 32);
    slow.drain();
    console.send_some(&mut output, 3_000_000);
    // It goes on from the oldest byte the ring still holds.
    assert_eq!(slow.sent(), "0123456789abcdefIJKLMNOPQRSTUVWX");
}

#[test]
fn a_stalled_port_is_skipped_and_the_others_still_get_the_text() {
    let (stuck, fast) = (Fake::new(Tx::Held), Fake::new(Tx::Fast));
    let mut console: Console<Fake, 2> = console(&[&stuck, &fast]);
    let mut output: Output<256> = Output::new();
    output.push("0123456789abcdef and then some more\n");
    // The stuck port is waited for, as one at 115200 baud would be...
    assert!(!console.send_some(&mut output, 0));
    assert!(!console.send_some(&mut output, STALL_NS - 1));
    assert!(!console.is_stalled(0));
    // ...until it has been busy for STALL_NS, from when it was first
    // found busy.
    assert!(console.send_some(&mut output, STALL_NS));
    assert!(console.is_stalled(0));
    assert!(!console.is_stalled(1));
    assert_eq!(stuck.sent(), "0123456789abcdef");
    assert_eq!(fast.sent(), "0123456789abcdef and then some more\r\n");
    // Stalled, it costs one status read a round and gets nothing.
    output.push("next line\n");
    let before = stuck.state.lock().unwrap().status_reads;
    assert!(console.send_some(&mut output, STALL_NS + 1));
    assert_eq!(stuck.state.lock().unwrap().status_reads, before + 1);
    assert_eq!(stuck.sent(), "0123456789abcdef");
    assert!(fast.sent().ends_with("next line\r\n"));
    // Once its transmitter takes bytes again, it gets the rest, which is
    // well within the last screenful.
    stuck.set_tx(Tx::Fast);
    assert!(console.send_now(&mut output, &mut ticking(), u64::MAX));
    assert!(!console.is_stalled(0));
    assert_eq!(stuck.sent(), fast.sent());
}

#[test]
fn a_port_back_from_a_long_stall_picks_up_at_the_last_screenful() {
    let (stuck, fast) = (Fake::new(Tx::Held), Fake::new(Tx::Fast));
    let mut console: Console<Fake, 2> = console(&[&stuck, &fast]);
    let mut output: Output<{ 16 * 1024 }> = Output::new();
    output.push("The first line of the log\n");
    console.send_now(&mut output, &mut || 0, u64::MAX);
    assert!(console.is_stalled(0));
    assert_eq!(stuck.sent().len(), FIFO_BYTES);
    // 100 lines of 60 bytes come while no session is open.
    for line in 0..100 {
        output.push(&format!("line {line:03}: {}\n", "-".repeat(49)));
    }
    stuck.set_tx(Tx::Fast);
    assert!(console.send_some(&mut output, 1));
    let sent = stuck.sent();
    let resumed = &sent[FIFO_BYTES..];
    // At most RESUME_BYTES, from the start of a line.
    assert!(resumed.len() <= RESUME_BYTES, "{} bytes", resumed.len());
    assert!(resumed.len() > RESUME_BYTES - 61, "{} bytes", resumed.len());
    assert!(resumed.starts_with("line "), "{resumed:?}");
    assert!(resumed.ends_with("line 099: -------------------------------------------------\r\n"));
    assert_eq!(fast.sent().len(), output.end() as usize);
}

#[test]
fn every_port_stalled_still_ends_a_send() {
    let stuck = Fake::new(Tx::Held);
    let mut console: Console<Fake, 1> = console(&[&stuck]);
    let mut output: Output<64> = Output::new();
    output.push("0123456789abcdef and the rest\n");
    assert!(console.send_now(&mut output, &mut ticking(), u64::MAX));
    assert!(console.is_stalled(0));
    assert_eq!(stuck.sent(), "0123456789abcdef");
    // With no port at all, too.
    let mut none: Console<Fake, 1> = Console::new();
    assert!(none.send_now(&mut output, &mut ticking(), u64::MAX));
}

#[test]
fn sending_at_once_stops_at_the_deadline() {
    let stuck = Fake::new(Tx::Held);
    let mut console: Console<Fake, 1> = console(&[&stuck]);
    let mut output: Output<64> = Output::new();
    output.push("0123456789abcdef and the rest");
    // 10 ms is less than STALL_NS: the port is still waited for.
    assert!(!console.send_now(&mut output, &mut ticking(), 10_000_000));
    assert_eq!(stuck.sent(), "0123456789abcdef");
    assert!(!console.is_stalled(0));
}

#[test]
fn sending_at_once_stalls_a_busy_port_when_the_clock_stands_still() {
    let stuck = Fake::new(Tx::Held);
    let mut console: Console<Fake, 1> = console(&[&stuck]);
    let mut output: Output<64> = Output::new();
    output.push("0123456789abcdef and the rest");
    assert!(console.send_now(&mut output, &mut || 0, u64::MAX));
    assert!(console.is_stalled(0));
    assert_eq!(stuck.sent(), "0123456789abcdef");
}

#[test]
fn a_port_found_late_gets_what_the_ring_still_holds() {
    let first = Fake::new(Tx::Fast);
    let mut console: Console<Fake, 2> = console(&[&first]);
    let mut output: Output<16> = Output::new();
    output.push("Radix\n");
    console.send_some(&mut output, 0);
    output.push("PCI 00:16.3\n");
    // The ring holds the newest 16 bytes; a card the PCI scan finds now
    // gets them, then the rest as it comes.
    let late = Fake::new(Tx::Fast);
    assert_eq!(console.add(uart(&late), true), Some(1));
    console.send_some(&mut output, 1);
    assert_eq!(late.sent(), "x\r\nPCI 00:16.3\r\n");
    output.push("ok\n");
    console.send_some(&mut output, 2);
    assert_eq!(late.sent(), "x\r\nPCI 00:16.3\r\nok\r\n");
    assert_eq!(first.sent(), "Radix\r\nPCI 00:16.3\r\nok\r\n");
}

#[test]
fn an_open_port_gets_no_log_and_after_closing_only_what_came_after() {
    let (terminal, other) = (Fake::new(Tx::Fast), Fake::new(Tx::Fast));
    let mut console: Console<Fake, 2> = console(&[&terminal, &other]);
    let mut output: Output<256> = Output::new();
    output.push("before\n");
    console.send_some(&mut output, 0);
    console.set_open(0, true, &mut output);
    output.push("while open\n");
    assert!(console.send_some(&mut output, 1));
    assert_eq!(terminal.sent(), "before\r\n");
    // The program draws on the line through its UART.
    assert_eq!(console.uart(0).unwrap().transmit(b"$ "), 2);
    console.set_open(0, false, &mut output);
    output.push("after\n");
    console.send_some(&mut output, 2);
    assert_eq!(terminal.sent(), "before\r\n$ after\r\n");
    assert_eq!(other.sent(), "before\r\nwhile open\r\nafter\r\n");
}

#[test]
fn kdb_reaches_an_open_port_without_the_log_it_was_spared() {
    let terminal = Fake::new(Tx::Fast);
    let mut console: Console<Fake, 1> = console(&[&terminal]);
    let mut output: Output<256> = Output::new();
    console.set_open(0, true, &mut output);
    output.push("a log line the program is spared\n");
    let start = output.end();
    output.push("kdb> ");
    console.skip_open_to(start);
    assert!(console.send_now(&mut output, &mut ticking(), u64::MAX));
    assert_eq!(terminal.sent(), "kdb> ");
    // The thread still keeps the log off it.
    output.push("more log\n");
    assert!(console.send_some(&mut output, 0));
    assert_eq!(terminal.sent(), "kdb> ");
}

#[test]
fn a_panic_report_reaches_every_log_port_open_or_not() {
    let (terminal, behind) = (Fake::new(Tx::Fast), Fake::new(Tx::Held));
    let mut console: Console<Fake, 2> = console(&[&terminal, &behind]);
    let mut output: Output<256> = Output::new();
    console.set_open(0, true, &mut output);
    output.push(&"old log\n".repeat(10));
    console.send_some(&mut output, 0);
    behind.set_tx(Tx::Fast);
    output.push("PANIC: report\n");
    output.keep_newest("PANIC: report\r\n".len());
    assert!(console.send_now(&mut output, &mut ticking(), u64::MAX));
    assert_eq!(terminal.sent(), "PANIC: report\r\n");
    // The one that was behind skips to the report too.
    assert_eq!(behind.sent(), "old log\r\nold logPANIC: report\r\n");
}

#[test]
fn a_port_outside_the_log_gets_nothing_and_is_not_read() {
    let (log, ups) = (Fake::new(Tx::Fast), Fake::new(Tx::Fast));
    let mut console: Console<Fake, 2> = Console::new();
    assert_eq!(console.add(uart(&log), true), Some(0));
    assert_eq!(console.add(uart(&ups), false), Some(1));
    assert!(console.carries_log(0));
    assert!(!console.carries_log(1));
    let mut output: Output<256> = Output::new();
    output.push("PANIC: report\n");
    assert!(console.send_now(&mut output, &mut ticking(), u64::MAX));
    assert!(console.send_some(&mut output, 0));
    assert_eq!(ups.sent(), "");
    // A UPS's status is not kdb's command.
    ups.type_in(b"Q1\r");
    assert_eq!(console.read(), None);
    assert_eq!(ups.state.lock().unwrap().received.len(), 3);
}

#[test]
fn the_console_reads_what_is_typed_on_any_log_port() {
    let (a, b) = (Fake::new(Tx::Fast), Fake::new(Tx::Fast));
    let mut console: Console<Fake, 2> = console(&[&a, &b]);
    assert_eq!(console.read(), None);
    b.type_in(b"cpus\r");
    a.type_in(b"h");
    let mut got = Vec::new();
    while let Some(byte) = console.read() {
        got.push(byte);
    }
    assert_eq!(got, b"hcpus\r");
}

#[test]
fn a_full_console_turns_a_port_away() {
    let mut console: Console<Fake, 1> = Console::new();
    assert_eq!(console.add(uart(&Fake::new(Tx::Fast)), true), Some(0));
    assert_eq!(console.add(uart(&Fake::new(Tx::Fast)), true), None);
    assert_eq!(console.len(), 1);
    assert!(console.uart(1).is_none());
    assert!(!console.carries_log(1));
}

#[test]
fn the_output_ring_keeps_the_newest_bytes_by_their_places() {
    let mut output: Output<8> = Output::new();
    assert!(output.is_empty());
    output.push("ab\n");
    assert_eq!((output.oldest(), output.end(), output.len()), (0, 4, 4));
    let mut chunk = [0; FIFO_BYTES];
    assert_eq!(output.peek_from(0, &mut chunk), (0, 4));
    assert_eq!(&chunk[..4], b"ab\r\n");
    // From a later place, and no further than the slice.
    assert_eq!(output.peek_from(2, &mut chunk), (2, 2));
    assert_eq!(&chunk[..2], b"\r\n");
    assert_eq!(output.peek_from(0, &mut chunk[..1]), (0, 1));
    // Past its size, the oldest bytes give way, across the wrap; a place
    // before the oldest reads from the oldest, and one past the end reads
    // nothing.
    output.push("0123456789");
    assert_eq!((output.oldest(), output.end(), output.len()), (6, 14, 8));
    assert_eq!(output.peek_from(0, &mut chunk), (6, 8));
    assert_eq!(&chunk[..8], b"23456789");
    assert_eq!(output.peek_from(99, &mut chunk), (14, 0));
    assert_eq!(Pending::end(&mut output), 14);
    output.keep_newest(3);
    assert_eq!(output.peek_from(0, &mut chunk), (11, 3));
    assert_eq!(&chunk[..3], b"789");
    // Keeping more than it holds brings nothing back.
    output.keep_newest(5);
    assert_eq!(output.len(), 3);
    output.keep_newest(0);
    assert!(output.is_empty());
}

#[test]
fn a_ring_of_no_bytes_holds_none() {
    let mut output: Output<0> = Output::new();
    output.push("dropped\n");
    assert!(output.is_empty());
    assert_eq!(output.peek_from(0, &mut [0; 4]), (0, 0));
}

#[test]
fn a_line_is_set_by_its_divisor_and_line_control() {
    use radix_tty::line::{B9600, B115200, Line, Parity, Stop, cflag};
    let read = 0o200;
    // COM1's own: 115200 baud, 8N1, as GRUB and the kernel's log set it.
    let line = Line::from_cflag(B115200 | cflag::CSIZE | read | cflag::CLOCAL, 0);
    assert_eq!(line_registers(&line), Some(LineRegisters::DEFAULT));
    assert_eq!(line_registers(&Line::DEFAULT), Some(LineRegisters::DEFAULT));
    assert_eq!(LineRegisters::DEFAULT, LineRegisters { divisor: 1, control: 0x03 });
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
