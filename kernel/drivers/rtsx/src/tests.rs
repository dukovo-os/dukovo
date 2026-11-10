// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The driver against a simulated Realtek reader and SD card: the chips
//! found by their PCI IDs; the reader set up and the card in it, its CID
//! and CSD read through the long response's bytes; blocks moved through
//! the descriptors, with CMD12 after several and CMD13 after a write
//! until the card has written them; a failed transfer and the stop
//! after it; requests refused; the card detect; and what the RTS5209 and
//! an RTS5229 of version C do differently.
//!
//! The simulated reader runs a batch when the driver next reads the
//! pending register, by when both the batch and its DMA were started, as
//! the reader runs them together.

use std::sync::{Arc, Mutex};
use std::vec::Vec;

use radix_hw::{Dma, Mmio, Registers};

use crate::*;

/// A simulated SD card.
#[derive(Clone)]
struct SimCard {
    data: Vec<u8>,
    /// It answers CMD8.
    version2: bool,
    /// SDHC: addressed in blocks.
    high: bool,
    write_protected: bool,
    /// The SCR's SD_SPEC: 0 for a card of version 1.0x, without CMD6.
    spec: u8,
    /// It has High Speed.
    high_speed: bool,
    /// It is an MMC, which answers no CMD55.
    mmc: bool,
}

/// The reader's state and its card's.
struct State {
    internal: Vec<u8>,
    commands: u32,
    command_control: u32,
    data: u32,
    data_control: u32,
    mailbox: u32,
    pending: u32,
    enabled: u32,
    /// A batch was started and has not run yet.
    batch: bool,
    card: Option<SimCard>,
    /// Addresses below 4 GiB that stand for host memory: (low, host,
    /// bytes).
    aliases: Vec<(u32, u64, u64)>,
    /// The card's state.
    app: bool,
    tries: u32,
    rca: u16,
    selected: bool,
    block_length: u32,
    function: u8,
    /// The data the last data command gives or takes: its first byte on
    /// the card, or the small block it gives.
    place: usize,
    small: Vec<u8>,
    writing: bool,
    /// CMD13s the card stays busy for after a write.
    busy_for: u32,
    busy: u32,
    /// The commands it got, in order (an ACMD as 100 + its index).
    sent: Vec<u8>,
    /// The next transfer of data fails, its CRC wrong.
    fail: bool,
    /// What the driver wrote to the reader's own registers: (offset,
    /// value).
    pokes: Vec<(usize, u32)>,
}

struct Sim(Mutex<State>);

const CID_NAME: &[u8; 5] = b"TEST1";
const RCA: u16 = 0x4a21;

/// An internal register's place: its 14 bits.
fn at(register: u16) -> usize {
    usize::from(register & 0x3fff)
}

impl State {
    fn new(card: Option<SimCard>) -> State {
        State {
            internal: std::vec![0; 0x4000],
            commands: 0,
            command_control: 0,
            data: 0,
            data_control: 0,
            mailbox: 0,
            pending: 0,
            enabled: 0,
            batch: false,
            card,
            aliases: Vec::new(),
            app: false,
            tries: 0,
            rca: 0,
            selected: false,
            block_length: 512,
            function: 0,
            place: 0,
            small: Vec::new(),
            writing: false,
            busy_for: 0,
            busy: 0,
            sent: Vec::new(),
            fail: false,
            pokes: Vec::new(),
        }
    }

    fn get(&self, register: u16) -> u8 {
        self.internal[at(register)]
    }

    fn put(&mut self, register: u16, mask: u8, value: u8) {
        let old = self.internal[at(register)];
        self.internal[at(register)] = old & !mask | value & mask;
        // The PHY is written as soon as it is asked.
        if register == chip::PHY_RWCTL {
            self.internal[at(register)] &= !chip::PHY_BUSY;
        }
    }

    /// The host memory a 32-bit address stands for.
    fn host(&self, address: u32) -> u64 {
        let within = |&&(low, _, len): &&(u32, u64, u64)| {
            (u64::from(low)..u64::from(low) + len).contains(&u64::from(address))
        };
        let (low, host, _) = self.aliases.iter().find(within).expect("an address it was lent");
        host + u64::from(address - low)
    }

    /// Runs the batch started, if one was: each command in turn, what its
    /// reads and checks read written back to the buffer.
    fn run(&mut self) {
        if !std::mem::take(&mut self.batch) {
            return;
        }
        let bytes = (self.command_control & 0x00ff_ffff) as usize;
        let mut words = std::vec![0u8; bytes];
        let buffer = self.host(self.commands);
        radix_hw::fake_memory(buffer, &mut words);
        let mut out = Vec::new();
        for &word in words.as_chunks::<4>().0 {
            let word = u32::from_le_bytes(word);
            let (kind, register) = (word >> 30, (word >> 16 & 0x3fff) as u16 | 0xc000);
            let (mask, value) = ((word >> 8) as u8, word as u8);
            match kind {
                0 => out.push(self.get(register)),
                1 => {
                    self.put(register, mask, value);
                    if register == chip::SD_TRANSFER && value & chip::SD_TRANSFER_START != 0 {
                        self.transfer(value & 0x0f);
                    }
                }
                _ => {
                    let now = self.get(register);
                    out.push(now);
                    if now & mask != value {
                        self.pending |= pending::FAILED;
                        radix_hw::fake_memory_write(buffer, &out);
                        return;
                    }
                }
            }
        }
        radix_hw::fake_memory_write(buffer, &out);
        self.pending |= pending::DONE;
    }

    /// What SD_TRANSFER's `mode` asks: a command, or a command's data.
    /// It ends at once, or stays started if the card did not answer.
    fn transfer(&mut self, mode: u8) {
        let ok = match mode {
            chip::TM_CMD_RSP => self.command(),
            chip::TM_AUTO_READ3 | chip::TM_AUTO_WRITE3 => self.move_data(mode),
            _ => panic!("transfer mode {mode:#x}"),
        };
        if ok {
            let ended = chip::SD_TRANSFER_END | chip::SD_STAT_IDLE;
            self.put(chip::SD_TRANSFER, 0xff, ended);
        }
    }

    /// The card's status: transfer (4) and ready for data, or programming
    /// (7) while busy.
    fn status(&self) -> u32 {
        let state = if self.busy > 0 { 7 } else { 4 };
        state << 9 | if self.busy > 0 { 0 } else { 1 << 8 } | if self.app { 1 << 5 } else { 0 }
    }

    /// Sends the command in SD_CMD0 to SD_CMD4 to the card; false if it
    /// did not answer.
    fn command(&mut self) -> bool {
        let index = self.get(chip::SD_CMD0) & 0x3f;
        let argument =
            u32::from_be_bytes(core::array::from_fn(|i| self.get(chip::SD_CMD0 + 1 + i as u16)));
        let Some(card) = self.card.clone() else {
            return self.quiet();
        };
        let app = std::mem::take(&mut self.app);
        self.sent.push(if app { 100 + index } else { index });
        let blocks = card.data.len() / 512;
        let short = match (app, index) {
            (_, 0) => {
                (self.rca, self.selected, self.tries) = (0, false, 0);
                return true;
            }
            (false, 8) if card.version2 => argument & 0xfff,
            (false, 8) => return self.quiet(),
            (false, 55) if card.mmc => return self.quiet(),
            (false, 55) => {
                self.app = true;
                self.status() | 1 << 5
            }
            (true, 41) => {
                self.tries += 1;
                let ready = if self.tries > 2 { 1 << 31 } else { 0 };
                let high = if card.high && argument & 1 << 30 != 0 { 1 << 30 } else { 0 };
                ready | high | 0x00ff_8000
            }
            (false, 2) => return self.long(cid()),
            (false, 3) => {
                self.rca = RCA;
                u32::from(RCA) << 16
            }
            (false, 9) => return self.long(csd(&card)),
            (false, 7) => {
                self.selected = argument >> 16 == u32::from(self.rca);
                self.status()
            }
            (true, 6) => self.status(),
            (false, 16) => {
                self.block_length = argument;
                self.status()
            }
            (true, 51) => {
                self.small = std::vec![card.spec, 0, 0, 0, 0, 0, 0, 0];
                self.status()
            }
            (false, 6) => {
                let mut functions = std::vec![0u8; 64];
                functions[13] = if card.high_speed { 0b11 } else { 0b01 };
                if argument & 1 << 31 != 0 && card.high_speed {
                    self.function = (argument & 0xf) as u8;
                }
                functions[16] = if argument & 1 << 31 != 0 { self.function } else { 0 };
                self.small = functions;
                self.status()
            }
            (false, 17 | 18 | 24 | 25) => {
                let address = argument as usize * if card.high { 512 } else { 1 };
                let status = self.status();
                if address / 512 >= blocks {
                    return self.answer(index, status | 1 << 31);
                }
                self.place = address;
                self.small.clear();
                self.writing = matches!(index, 24 | 25);
                status
            }
            (false, 12) => {
                let status = self.status();
                if self.writing {
                    self.busy = self.busy_for;
                }
                status
            }
            (false, 13) => {
                let status = self.status();
                self.busy = self.busy.saturating_sub(1);
                status
            }
            _ => panic!("command {index}, app {app}"),
        };
        self.answer(index, short)
    }

    /// The card answers command `index` with `short`.
    fn answer(&mut self, index: u8, short: u32) -> bool {
        self.put(chip::SD_CMD0, 0xff, index);
        for (i, byte) in short.to_be_bytes().into_iter().enumerate() {
            self.put(chip::SD_CMD0 + 1 + i as u16, 0xff, byte);
        }
        true
    }

    /// The card answers with the 128-bit `register`: ping-pong buffer 2
    /// holds its bytes after the start bits, and SD_CMD5 something else.
    fn long(&mut self, register: u128) -> bool {
        self.put(chip::PPBUF_BASE2, 0xff, 0x3f);
        for (i, byte) in register.to_be_bytes().into_iter().enumerate().take(15) {
            self.put(chip::PPBUF_BASE2 + 1 + i as u16, 0xff, byte);
        }
        self.put(chip::SD_CMD5, 0xff, 0x5a);
        true
    }

    /// The card does not answer: no response within 80 clocks.
    fn quiet(&mut self) -> bool {
        self.put(chip::SD_STAT2, 0xff, chip::SD_RSP_80CLK_TIMEOUT);
        false
    }

    /// Moves the data of the command answered last between the card and
    /// memory, by the descriptors or into one buffer.
    fn move_data(&mut self, mode: u8) -> bool {
        if std::mem::take(&mut self.fail) {
            self.put(chip::SD_STAT1, 0xff, 0x40);
            return false;
        }
        let length =
            u16::from_le_bytes([self.get(chip::SD_BYTE_CNT_L), self.get(chip::SD_BYTE_CNT_H)]);
        let blocks =
            u16::from_le_bytes([self.get(chip::SD_BLOCK_CNT_L), self.get(chip::SD_BLOCK_CNT_H)]);
        let bytes = usize::from(length) * usize::from(blocks);
        let counted =
            u32::from_be_bytes(core::array::from_fn(|i| self.get(chip::DMATC3 - i as u16)));
        assert_eq!(counted as usize, bytes, "the DMA's count");
        assert!(self.data_control & dma::START != 0, "the DMA started with the batch");
        let reads = self.data_control & dma::READ != 0;
        assert_eq!(reads, mode == chip::TM_AUTO_READ3);
        // The memory the bytes go to or come from, in pieces.
        let mut pieces = Vec::new();
        if self.data_control & dma::DESCRIBED != 0 {
            let table = self.host(self.data);
            for i in 0.. {
                let mut word = [0u8; 8];
                radix_hw::fake_memory(table + 8 * i, &mut word);
                let word = u64::from_le_bytes(word);
                assert_eq!(word & 0x31, 0x21, "a valid descriptor of data");
                pieces.push(((word >> 32) as u32, (word >> 12 & 0xf_ffff) as usize));
                if word & DESCRIPTOR_END != 0 {
                    break;
                }
            }
        } else {
            pieces.push((self.data, (self.data_control & 0x00ff_ffff) as usize));
        }
        assert_eq!(pieces.iter().map(|&(_, len)| len).sum::<usize>(), bytes);
        let card = self.card.as_mut().unwrap();
        let mut place = self.place;
        for (address, len) in pieces {
            let host = {
                let within = |&&(low, _, all): &&(u32, u64, u64)| {
                    (u64::from(low)..u64::from(low) + all).contains(&u64::from(address))
                };
                let (low, host, _) = self.aliases.iter().find(within).unwrap();
                host + u64::from(address - low)
            };
            if !self.small.is_empty() {
                radix_hw::fake_memory_write(host, &self.small[..len]);
            } else if reads {
                radix_hw::fake_memory_write(host, &card.data[place..place + len]);
            } else {
                radix_hw::fake_memory(host, &mut card.data[place..place + len]);
            }
            place += len;
        }
        if !reads && blocks == 1 {
            self.busy = self.busy_for;
        }
        true
    }
}

impl Registers for Sim {
    fn read(&self, offset: usize, _size: usize) -> u64 {
        let mut state = self.0.lock().unwrap();
        let value = match offset {
            reg::PENDING => {
                state.run();
                let mut bits = state.pending;
                if let Some(card) = &state.card {
                    bits |= pending::SD_IN;
                    if card.write_protected {
                        bits |= pending::WRITE_PROTECT;
                    }
                }
                bits
            }
            reg::INTERNAL => state.mailbox,
            reg::ENABLED => state.enabled,
            reg::COMMANDS => state.commands,
            reg::DATA => state.data,
            _ => 0,
        };
        u64::from(value)
    }

    fn write(&self, offset: usize, _size: usize, value: u64) {
        let mut state = self.0.lock().unwrap();
        let value = value as u32;
        if offset != reg::INTERNAL {
            state.pokes.push((offset, value));
        }
        match offset {
            reg::COMMANDS => state.commands = value,
            reg::COMMAND_CONTROL => {
                state.command_control = value;
                state.batch = value & START_COMMANDS != 0;
            }
            reg::DATA => state.data = value,
            reg::DATA_CONTROL => state.data_control = value,
            reg::PENDING => state.pending &= !value,
            reg::ENABLED => state.enabled = value,
            reg::INTERNAL => {
                let register = (value >> 16 & 0x3fff) as u16 | 0xc000;
                let byte = if value & INTERNAL_WRITE != 0 {
                    state.put(register, (value >> 8) as u8, value as u8);
                    // The byte written comes back, not the register.
                    value as u8
                } else {
                    state.get(register)
                };
                state.mailbox = value & !INTERNAL_BUSY & !0xff | u32::from(byte);
            }
            _ => {}
        }
    }
}

/// A card's CID: maker 3, "SD", "TEST1" of revision 1.0, serial
/// 0xdeadbeef, made in June 2024.
fn cid() -> u128 {
    let mut cid = 0x03u128 << 120 | u128::from(u16::from_be_bytes(*b"SD")) << 104;
    for (i, &c) in CID_NAME.iter().enumerate() {
        cid |= u128::from(c) << (96 - 8 * i);
    }
    cid | 0x10 << 56 | 0xdead_beef << 24 | (24 << 4 | 6) << 8 | 1
}

fn csd(card: &SimCard) -> u128 {
    let blocks = card.data.len() as u128 / 512;
    if card.high {
        1 << 126 | (blocks / 1024 - 1) << 48 | 1
    } else {
        let size = blocks * 512 / (256 * 1024) - 1;
        9 << 80 | size << 62 | 7 << 47 | 1
    }
}

/// An SDHC card of 4096 blocks with High Speed, of version 3.0x.
fn card(blocks: usize, high: bool) -> SimCard {
    SimCard {
        data: std::vec![0; blocks * 512],
        version2: true,
        high,
        write_protected: false,
        spec: 2,
        high_speed: true,
        mmc: false,
    }
}

/// A host page and the address below 4 GiB the reader is given for it.
fn lent(sim: &Sim, low: u32, len: usize) -> (Dma, u64) {
    let page = Dma::fake(len);
    sim.0.lock().unwrap().aliases.push((low, page.phys(), len as u64));
    (page, u64::from(low))
}

/// A reader of `chip` holding `card`, set up, with `version` in its
/// DUMMY_REG.
fn reader_with(
    chip: Chip,
    version: u8,
    card: Option<SimCard>,
) -> (Result<Reader, Error>, Arc<Sim>) {
    let mut state = State::new(card);
    state.internal[at(chip::DUMMY_REG)] = version;
    let sim = Arc::new(Sim(Mutex::new(state)));
    let regs = Mmio::fake(sim.clone(), REGISTERS);
    let alias = sim.clone();
    // The page stands below 4 GiB at its host address's low half, which
    // the driver gives the reader.
    let mut alloc = |pages: usize, below: bool| {
        assert!(below, "the reader reaches only the first 4 GiB");
        let page = Dma::fake(pages * PAGE);
        let phys = page.phys();
        alias.0.lock().unwrap().aliases.push((phys as u32, phys, (pages * PAGE) as u64));
        Some(page)
    };
    let reader = Reader::new(regs, chip, &mut alloc, &mut |_| {});
    (reader, sim)
}

/// An RTS5227 holding `card`, set up.
fn reader(card: Option<SimCard>) -> (Reader, Arc<Sim>) {
    let (reader, sim) = reader_with(Chip::Rts5227, 0, card);
    (reader.unwrap(), sim)
}

/// A pattern of `len` bytes for block `block` on.
fn pattern(block: usize, len: usize) -> Vec<u8> {
    (0..len).map(|i| ((block * 512 + i) * 7 % 251) as u8).collect()
}

/// Runs the request in flight to its end.
fn finish(reader: &mut Reader) -> Result<(), Error> {
    for _ in 0..100 {
        if let Some(result) = reader.poll() {
            return result;
        }
    }
    panic!("the request did not end");
}

#[test]
fn chips_are_found_by_their_device_and_class() {
    assert_eq!(Chip::of(0x10ec, 0x5227, 0xff), Some(Chip::Rts5227));
    assert_eq!(Chip::of(0x10ec, 0x5287, 0xff), Some(Chip::Rtl8411b));
    assert_eq!(Chip::of(0x10ec, 0x525a, 0xff).map(Chip::bar), Some(1));
    assert_eq!(Chip::of(0x10ec, 0x5249, 0xff).map(Chip::name), Some("RTS5249"));
    // An RTS5209 that says it is an SD host controller is one; the
    // RTS5260 is not driven; another vendor's device is not Realtek's.
    assert_eq!(Chip::of(0x10ec, 0x5209, 0x08), None);
    assert_eq!(Chip::of(0x10ec, 0x5260, 0xff), None);
    assert_eq!(Chip::of(0x8086, 0x5227, 0xff), None);
}

#[test]
fn the_reader_is_set_up_with_its_interrupts_cleared_and_on() {
    let (reader, sim) = reader(None);
    let state = sim.0.lock().unwrap();
    assert_eq!(state.enabled, pending::DONE | pending::FAILED | pending::SD_CHANGED);
    // Its pending bits are cleared by writing them; the PHY, clock and
    // LED are set; the SSC is powered.
    assert_eq!(state.get(chip::NFTS_TX_CTRL) & chip::INT_READ_CLR, 0);
    assert_eq!(state.get(chip::FPDCTL) & chip::SSC_POWER_DOWN, 0);
    assert_eq!(state.get(chip::PHY_DATA0), 0x42);
    assert_eq!(state.get(chip::PHY_DATA1), 0xba);
    assert_eq!(state.get(chip::SSC_CTL1), chip::SSC_8X_EN | chip::SSC_SEL_4M);
    assert_eq!(state.get(chip::LDO_PWR_SEL) & 0x03, chip::LDO_PWR_SEL_3V3);
    drop(state);
    assert!(!reader.present());
    assert_eq!(reader.chip(), Chip::Rts5227);
}

#[test]
fn an_sdhc_card_is_set_up_at_high_speed_and_moves_blocks() {
    let (mut reader, sim) = reader(Some(card(4096, true)));
    assert!(reader.present());
    let card = reader.identify(&mut |_| {}).unwrap();
    assert!(card.high_capacity);
    assert_eq!((card.blocks, card.rca, card.width, card.read_only), (4096, RCA, 4, false));
    assert_eq!((card.name(), card.kind()), ("TEST1", "SDHC"));
    assert_eq!((card.cid.maker, card.cid.oem, card.cid.serial), (3, *b"SD", 0xdead_beef));
    assert_eq!((card.cid.year, card.cid.month, card.cid.revision), (2024, 6, 0x10));
    assert_eq!((card.speed, card.khz), (Speed::High, 50_000));
    {
        let state = sim.0.lock().unwrap();
        assert_eq!(
            state.sent,
            [0, 8, 55, 141, 55, 141, 55, 141, 2, 3, 9, 7, 55, 106, 55, 151, 6, 6]
        );
        // Four bits, SD 2.0 mode, the clock at 50 MHz's divider.
        assert_eq!(state.get(chip::SD_CFG1) & chip::BUS_WIDTH_MASK, chip::BUS_WIDTH_4);
        assert_eq!(state.get(chip::CLK_DIV), 0x27);
        assert_eq!(state.get(chip::SSC_DIV_N_0), 100);
        assert_eq!(state.get(chip::CARD_OE), chip::SD_OUTPUT_EN);
        assert_eq!(state.get(chip::PWR_GATE_CTRL), chip::LDO3318_VCC1 | chip::LDO3318_VCC2);
        assert_eq!(state.function, 1);
    }
    // Three buffers of five blocks, written from block 100: CMD25, the
    // data, CMD12, and CMD13 until the card has written them.
    sim.0.lock().unwrap().busy_for = 3;
    let lens = [1024u32, 512, 1024];
    let mut buffers = Vec::new();
    let mut pages = Vec::new();
    for (i, len) in lens.into_iter().enumerate() {
        let (page, low) = lent(&sim, 0x2000_0000 + 0x1_0000 * i as u32, 4096);
        page.write(0, &pattern(100 + i * 2, len as usize));
        buffers.push((low, len));
        pages.push(page);
    }
    reader.start(Op::Write, 100, 5, &buffers, &mut |_| {}).unwrap();
    assert_eq!(finish(&mut reader), Ok(()));
    assert_eq!(reader.poll(), None);
    {
        let state = sim.0.lock().unwrap();
        let written = &state.card.as_ref().unwrap().data[100 * 512..105 * 512];
        let mut want = pattern(100, 1024);
        want.extend(pattern(102, 512));
        want.extend(pattern(104, 1024));
        assert_eq!(written, &want[..]);
        assert!(state.sent.ends_with(&[25, 12, 13, 13, 13, 13]), "{:?}", state.sent);
    }
    // Read back: CMD18, the data, CMD12, and no CMD13.
    let (back, low) = lent(&sim, 0x3000_0000, 4096);
    reader.start(Op::Read, 101, 2, &[(low, 1024)], &mut |_| {}).unwrap();
    assert_eq!(finish(&mut reader), Ok(()));
    let mut got = std::vec![0u8; 1024];
    back.read(0, &mut got);
    let mut want = pattern(101, 512);
    want.extend(pattern(102, 512));
    assert_eq!(got, want);
    assert!(sim.0.lock().unwrap().sent.ends_with(&[18, 12]));
    // One block, the card's last: CMD17 alone.
    reader.start(Op::Read, 4095, 1, &[(low, 512)], &mut |_| {}).unwrap();
    assert_eq!(finish(&mut reader), Ok(()));
    assert_eq!(sim.0.lock().unwrap().sent.last(), Some(&17));
}

#[test]
fn a_standard_capacity_card_is_addressed_in_bytes_at_the_default_speed() {
    // A card of version 1.0x: no CMD8 and no CMD6.
    let old = SimCard { version2: false, spec: 0, high_speed: false, ..card(8192, false) };
    let (mut reader, sim) = reader(Some(old));
    let card = reader.identify(&mut |_| {}).unwrap();
    assert!(!card.high_capacity);
    assert_eq!(
        (card.blocks, card.kind(), card.speed, card.khz),
        (8192, "SD", Speed::Default, 25_000)
    );
    assert_eq!(sim.0.lock().unwrap().get(chip::CLK_DIV), 0x37);
    assert!(sim.0.lock().unwrap().sent.contains(&16));
    // A single write waits for the card to have written it.
    sim.0.lock().unwrap().busy_for = 2;
    let (page, low) = lent(&sim, 0x2000_0000, 4096);
    page.write(0, &pattern(7, 512));
    reader.start(Op::Write, 7, 1, &[(low, 512)], &mut |_| {}).unwrap();
    assert_eq!(finish(&mut reader), Ok(()));
    let state = sim.0.lock().unwrap();
    assert_eq!(&state.card.as_ref().unwrap().data[7 * 512..8 * 512], &pattern(7, 512)[..]);
    assert!(state.sent.ends_with(&[24, 13, 13, 13]), "{:?}", state.sent);
}

#[test]
fn a_failed_transfer_stops_the_reader_and_the_card_before_the_next() {
    let (mut reader, sim) = reader(Some(card(4096, true)));
    reader.identify(&mut |_| {}).unwrap();
    let (_page, low) = lent(&sim, 0x2000_0000, 4096);
    sim.0.lock().unwrap().fail = true;
    reader.start(Op::Read, 0, 2, &[(low, 1024)], &mut |_| {}).unwrap();
    assert_eq!(finish(&mut reader), Err(Error::Command(failed::DATA_CRC)));
    {
        let mut state = sim.0.lock().unwrap();
        // Stopped: the command buffer, the card's transfer, the DMA.
        assert!(state.pokes.contains(&(reg::COMMAND_CONTROL, STOP_COMMANDS)));
        assert!(state.pokes.contains(&(reg::DATA_CONTROL, dma::STOP)));
        assert_eq!(state.get(chip::RBCTL) & chip::RB_FLUSH, chip::RB_FLUSH);
        state.sent.clear();
    }
    // The next request stops the card first.
    reader.start(Op::Read, 0, 2, &[(low, 1024)], &mut |_| {}).unwrap();
    assert_eq!(finish(&mut reader), Ok(()));
    assert_eq!(sim.0.lock().unwrap().sent, [12, 18, 12]);
    // A request beyond the card's end is refused before it goes; one the
    // card says is out of range fails with its status.
    assert_eq!(
        reader.start(Op::Read, 4095, 2, &[(low, 1024)], &mut |_| {}),
        Err(Error::BadRequest)
    );
}

#[test]
fn requests_the_reader_cannot_make_are_refused() {
    let (mut reader, sim) = reader(Some(SimCard { write_protected: true, ..card(4096, true) }));
    let mut delay = |_| {};
    assert_eq!(reader.start(Op::Read, 0, 1, &[(0x2000_0000, 512)], &mut delay), Err(Error::NoCard));
    let card = reader.identify(&mut delay).unwrap();
    assert!(card.read_only);
    let (_page, low) = lent(&sim, 0x2000_0000, 4096);
    // Above 4 GiB, a byte count that is not the blocks', a write to a
    // write-protected card, nothing to move.
    assert_eq!(reader.start(Op::Read, 0, 1, &[(1 << 32, 512)], &mut delay), Err(Error::BadRequest));
    assert_eq!(reader.start(Op::Read, 0, 2, &[(low, 512)], &mut delay), Err(Error::BadRequest));
    assert_eq!(reader.start(Op::Write, 0, 1, &[(low, 512)], &mut delay), Err(Error::BadRequest));
    assert_eq!(reader.start(Op::Read, 0, 0, &[], &mut delay), Err(Error::BadRequest));
    // One at a time.
    reader.start(Op::Read, 0, 1, &[(low, 512)], &mut delay).unwrap();
    assert_eq!(reader.start(Op::Read, 1, 1, &[(low, 512)], &mut delay), Err(Error::BadRequest));
    assert_eq!(finish(&mut reader), Ok(()));
}

#[test]
fn an_mmc_and_an_empty_reader_are_not_set_up() {
    let (mut reader, sim) = reader(Some(SimCard { mmc: true, ..card(4096, true) }));
    assert!(matches!(reader.identify(&mut |_| {}), Err(Error::Unsupported(_))));
    assert!(reader.card().is_none());
    sim.0.lock().unwrap().card = None;
    assert_eq!(reader.identify(&mut |_| {}), Err(Error::NoCard));
}

#[test]
fn the_card_detect_says_what_changed() {
    let (reader, sim) = reader(None);
    let detect = reader.detect();
    assert_eq!(detect.presence(), Presence::Empty);
    assert_eq!(detect.events(), Events::default());
    assert!(!detect.interrupting());
    // A write-protected card goes in: it is said to have, and a card in
    // the reader before it to have come out.
    {
        let mut state = sim.0.lock().unwrap();
        state.card = Some(SimCard { write_protected: true, ..card(4096, true) });
        state.pending |= pending::SD_CHANGED;
    }
    assert!(detect.interrupting());
    assert_eq!(detect.presence(), Presence::Card { protected: true });
    assert_eq!(detect.events(), Events { inserted: true, removed: true });
    assert_eq!(detect.events(), Events::default());
    assert!(!detect.interrupting());
    // Taken out.
    {
        let mut state = sim.0.lock().unwrap();
        state.card = None;
        state.pending |= pending::SD_CHANGED;
    }
    assert_eq!(detect.events(), Events { inserted: false, removed: true });
    // Its power and LED go off.
    {
        let mut state = sim.0.lock().unwrap();
        state.put(chip::PWR_GATE_CTRL, 0xff, 0x06);
        state.put(chip::CARD_OE, 0xff, chip::SD_OUTPUT_EN);
        state.put(chip::GPIO_CTL, 0xff, chip::GPIO_LED_ON);
    }
    detect.power_off();
    let state = sim.0.lock().unwrap();
    assert_eq!(state.get(chip::PWR_GATE_CTRL), 0);
    assert_eq!(state.get(chip::CARD_OE), 0);
    assert_eq!(state.get(chip::CARD_PWR_CTL) & chip::SD_PWR_OFF, chip::SD_PWR_OFF);
    assert_eq!(state.get(chip::GPIO_CTL) & chip::GPIO_LED_ON, 0);
}

#[test]
fn the_rts5209_gates_its_power_the_other_way() {
    let (reader, sim) = reader_with(Chip::Rts5209, 0, Some(card(4096, true)));
    let mut reader = reader.unwrap();
    assert_eq!(sim.0.lock().unwrap().get(chip::PHY_DATA1), 0xb9);
    reader.identify(&mut |_| {}).unwrap();
    // Its gates are a mask of those off: all on.
    assert_eq!(sim.0.lock().unwrap().get(chip::PWR_GATE_CTRL), 0);
    reader.detect().power_off();
    assert_eq!(sim.0.lock().unwrap().get(chip::PWR_GATE_CTRL), chip::LDO3318_OFF);
}

#[test]
fn an_rts5229_of_version_c_has_other_pull_ups() {
    let (reader, sim) = reader_with(Chip::Rts5229, 2, Some(card(4096, true)));
    let mut reader = reader.unwrap();
    reader.identify(&mut |_| {}).unwrap();
    assert_eq!(sim.0.lock().unwrap().get(chip::CARD_PULL_CTL3), chip::PULL_CTL_ENABLE3_TYPE_C);
    reader.detect().power_off();
    assert_eq!(sim.0.lock().unwrap().get(chip::CARD_PULL_CTL3), chip::PULL_CTL_DISABLE3_TYPE_C);
    // One of a version not known is refused.
    let (refused, _) = reader_with(Chip::Rts5249, 7, None);
    assert!(matches!(refused, Err(Error::Unsupported(_))));
    // The RTS525A's regulator is set to 3.3 V as its card powers on.
    let (reader, sim) = reader_with(Chip::Rts525a, 0, Some(card(4096, true)));
    reader.unwrap().identify(&mut |_| {}).unwrap();
    let ldo = sim.0.lock().unwrap().get(chip::LDO_VCC_CFG1);
    assert_eq!(ldo & chip::LDO_VCC_TUNE_MASK, chip::LDO_VCC_3V3);
}

#[test]
fn a_request_given_up_stops_the_reader() {
    let (mut reader, sim) = reader(Some(card(4096, true)));
    reader.identify(&mut |_| {}).unwrap();
    let (_page, low) = lent(&sim, 0x2000_0000, 4096);
    reader.start(Op::Read, 0, 1, &[(low, 512)], &mut |_| {}).unwrap();
    reader.abandon(&mut |_| {});
    assert_eq!(reader.poll(), None);
    assert!(sim.0.lock().unwrap().pokes.contains(&(reg::COMMAND_CONTROL, STOP_COMMANDS)));
    sim.0.lock().unwrap().sent.clear();
    reader.start(Op::Read, 0, 1, &[(low, 512)], &mut |_| {}).unwrap();
    assert_eq!(finish(&mut reader), Ok(()));
    assert_eq!(sim.0.lock().unwrap().sent, [12, 17]);
}
