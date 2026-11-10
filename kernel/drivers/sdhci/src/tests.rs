// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The driver against a simulated SD host controller and card: what the
//! card says of itself, reads and writes through ADMA2's descriptors of
//! both sizes, failures and the stop after them, the card detect, and the
//! bus speed modes: High Speed, and UHS-I's after a switch to 1.8 V, tuned
//! where they must be; and MMCs and eMMCs, their bus's width, High Speed
//! and HS200.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::vec::Vec;

use radix_hw::{Dma, Mmio, Registers};

use crate::*;

/// A simulated card.
#[derive(Clone)]
struct SimCard {
    data: Vec<u8>,
    /// It answers CMD8.
    version2: bool,
    /// It is SDHC: addressed in blocks.
    high: bool,
    write_protected: bool,
    /// The SCR's SD_SPEC: 0 for a card of version 1.0x, without CMD6; an
    /// MMC's CSD's SPEC_VERS, 4 and later with an EXT_CSD.
    spec: u8,
    /// CMD6's group 1, the bus speed modes it has (bit 1 High Speed or
    /// SDR25, 2 SDR50, 3 SDR104, 4 DDR50), and group 4, its current
    /// limits (bit 1 400 mA, 2 600 mA, 3 800 mA).
    modes: u16,
    currents: u16,
    /// It switches to 1.8 V when asked (S18A).
    uhs: bool,
    /// It says it will switch, and then holds DAT0 to DAT3 low.
    switch_fails: bool,
    /// Tuning finds no sampling point.
    tuning_fails: bool,
    /// It is an MMC: CMD1 rather than ACMD41, and an EXT_CSD.
    mmc: bool,
    /// An MMC's EXT_CSD CARD_TYPE: bit 1 High Speed at 52 MHz, bit 4
    /// HS200 at 1.8 V.
    card_type: u8,
}

/// A simulated slot and its card.
struct State {
    regs: [u8; 256],
    card: Option<SimCard>,
    /// The card's state.
    app: bool,
    tries: u32,
    rca: u16,
    selected: bool,
    block_length: u32,
    /// The commands it got, in order (an ACMD as 100 + its index).
    commands: Vec<u8>,
    /// The next data command fails with these error bits.
    fail: Option<u16>,
    /// Addresses below 4 GiB that stand for host memory, for 32-bit
    /// descriptors and SDMA: (low, host, bytes).
    aliases: Vec<(u32, u64, u64)>,
    /// An SDMA transfer stopped at a boundary: the card's byte it goes on
    /// from, the bytes left, and whether it writes to the card.
    sdma: Option<(usize, usize, bool)>,
    /// What the driver wrote to Power Control and Software Reset, in
    /// order: (register, value).
    pokes: Vec<(usize, u64)>,
    /// The card detect has not settled.
    settling: bool,
    /// What the Buffer Data Port gives next, a word at a time.
    buffer: VecDeque<u32>,
    /// The card agreed to 1.8 V (S18A), and took CMD11 and switched.
    agreed: bool,
    switched: bool,
    /// CMD6's functions switched to: group 1's and group 4's.
    function: u8,
    limit: u8,
    /// CMD19s or CMD21s sent, and the last one's block size.
    tunes: u32,
    tuning_size: u16,
    /// An MMC's EXT_CSD BUS_WIDTH and HS_TIMING, and its last CMD6 was
    /// refused.
    bus_width: u8,
    timing: u8,
    switch_error: bool,
    /// The data lines the slot has wired to the card: 8, or 4 where the
    /// upper four go nowhere.
    lines: u8,
}

struct Sim(Mutex<State>);

const CID_NAME: &[u8; 5] = b"TEST1";

impl State {
    /// A slot holding `card`, its registers all 0.
    fn new(card: Option<SimCard>) -> State {
        State {
            regs: [0; 256],
            card,
            app: false,
            tries: 0,
            rca: 0,
            selected: false,
            block_length: 0,
            commands: Vec::new(),
            fail: None,
            aliases: Vec::new(),
            sdma: None,
            pokes: Vec::new(),
            settling: false,
            buffer: VecDeque::new(),
            agreed: false,
            switched: false,
            function: 0,
            limit: 0,
            tunes: 0,
            tuning_size: 0,
            bus_width: 0,
            timing: 0,
            switch_error: false,
            lines: 8,
        }
    }

    fn get(&self, at: usize, size: usize) -> u64 {
        let mut value = 0u64;
        for i in (0..size).rev() {
            value = value << 8 | u64::from(self.regs[at + i]);
        }
        value
    }

    fn put(&mut self, at: usize, size: usize, value: u64) {
        for i in 0..size {
            self.regs[at + i] = (value >> (8 * i)) as u8;
        }
    }

    fn set_status(&mut self, normal: u16, error: u16) {
        // A bit is set only while it is reported (2.2.18).
        let normal = normal & self.get(reg::NORMAL_STATUS_ENABLE, 2) as u16;
        let error = error & self.get(reg::ERROR_STATUS_ENABLE, 2) as u16;
        let normal = if error != 0 { normal | status::ERROR } else { normal };
        let now = self.get(reg::NORMAL_STATUS, 2) as u16;
        self.put(reg::NORMAL_STATUS, 2, u64::from(now | normal));
        let now = self.get(reg::ERROR_STATUS, 2) as u16;
        self.put(reg::ERROR_STATUS, 2, u64::from(now | error));
    }

    fn respond(&mut self, words: [u32; 4]) {
        for (i, word) in words.iter().enumerate() {
            self.put(reg::RESPONSE + 4 * i, 4, u64::from(*word));
        }
    }

    /// A 128-bit register as an R2 response: its bits 127 to 8.
    fn long(register: u128) -> [u32; 4] {
        let shifted = register >> 8;
        core::array::from_fn(|i| (shifted >> (32 * i)) as u32)
    }

    fn cid() -> u128 {
        let mut cid = 0x03u128 << 120 | u128::from(u16::from_be_bytes(*b"SD")) << 104;
        for (i, &c) in CID_NAME.iter().enumerate() {
            cid |= u128::from(c) << (96 - 8 * i);
        }
        cid | 0x10 << 56 | 0xdead_beef << 24 | (24 << 4 | 6) << 8 | 1
    }

    /// An MMC's CID: maker 0x15, OEM 1, "EMMC01" of revision 2.1, made in
    /// June 2024 (year 11 from 2013).
    fn mmc_cid() -> u128 {
        let mut cid = 0x15u128 << 120 | 1 << 112 | 0x01 << 104;
        for (i, &c) in b"EMMC01".iter().enumerate() {
            cid |= u128::from(c) << (96 - 8 * i);
        }
        cid | 0x21 << 48 | 0xcafe_f00d << 16 | (6 << 4 | 0xb) << 8 | 1
    }

    /// An MMC's CSD: its capacity in C_SIZE up to 2 GB, beyond which it
    /// says 2 GB and its EXT_CSD the rest.
    fn mmc_csd(card: &SimCard) -> u128 {
        let blocks = card.data.len() as u128 / 512;
        let size = if card.high { 0xfff } else { blocks * 512 / (256 * 1024) - 1 };
        3 << 126 | u128::from(card.spec) << 122 | 9 << 80 | size << 62 | 7 << 47 | 1
    }

    /// An MMC's EXT_CSD as it stands: revision 8 (5.1).
    fn ext_csd(&self, card: &SimCard) -> [u8; 512] {
        let mut ext = [0u8; 512];
        ext[183] = self.bus_width;
        ext[185] = self.timing;
        ext[192] = 8;
        ext[196] = card.card_type;
        if card.high {
            let sectors = (card.data.len() / 512) as u32;
            ext[212..216].copy_from_slice(&sectors.to_le_bytes());
        }
        ext[504] = 0x5a;
        ext
    }

    /// The host's and the MMC's bus are as wide as each other, and no
    /// wider than the lines the slot wired.
    fn widths_agree(&self) -> bool {
        let control = self.regs[reg::HOST_CONTROL];
        let host = if control & 1 << 5 != 0 {
            8
        } else if control & 1 << 1 != 0 {
            4
        } else {
            1
        };
        let card = match self.bus_width {
            2 => 8,
            1 => 4,
            _ => 1,
        };
        host == card && host <= self.lines
    }

    /// An MMC's CMD6: writes a byte of the EXT_CSD, or refuses it, as
    /// CMD13 then says.
    fn mmc_switch(&mut self, card: &SimCard, argument: u32) {
        let (access, index, value) =
            (argument >> 24 & 3, argument >> 16 & 0xff, argument >> 8 & 0xff);
        let takes = match (access, index, value) {
            (3, 183, 0..=2) => true,
            (3, 185, 0) => true,
            (3, 185, 1) => card.card_type & 1 << 1 != 0,
            (3, 185, 2) => card.card_type & 1 << 4 != 0 && !card.switch_fails,
            _ => false,
        };
        self.switch_error = !takes;
        match (takes, index) {
            (true, 183) => self.bus_width = value as u8,
            (true, 185) => self.timing = value as u8,
            _ => {}
        }
    }

    fn csd(card: &SimCard) -> u128 {
        let blocks = card.data.len() as u128 / 512;
        if card.high {
            1 << 126 | (blocks / 1024 - 1) << 48 | 1
        } else {
            // 512-byte blocks, C_SIZE_MULT 7: 256 KiB for each C_SIZE.
            let size = blocks * 512 / (256 * 1024) - 1;
            9 << 80 | size << 62 | 7 << 47 | 1
        }
    }

    /// The host memory a descriptor's address stands for.
    fn host(&self, address: u64, wide: bool) -> u64 {
        if wide {
            return address;
        }
        let within = |&&(low, _, len): &&(u32, u64, u64)| {
            (u64::from(low)..u64::from(low) + len).contains(&address)
        };
        let (low, host, _) = self.aliases.iter().find(within).unwrap();
        host + (address - u64::from(*low))
    }

    /// Moves an SDMA transfer's bytes from SDMA System Address up to the
    /// next boundary, or to its end; stopped at the boundary, the address
    /// is where it goes on from.
    fn sdma_move(&mut self) {
        let Some((place, left, writes)) = self.sdma.take() else { return };
        let address = self.get(reg::SDMA_ADDRESS, 4);
        let boundary = 4096u64 << (self.get(reg::BLOCK_SIZE, 2) >> 12 & 7);
        let len = ((address / boundary + 1) * boundary - address).min(left as u64) as usize;
        let host = self.host(address, false);
        let data = &mut self.card.as_mut().unwrap().data;
        if writes {
            radix_hw::fake_memory(host, &mut data[place..place + len]);
        } else {
            radix_hw::fake_memory_write(host, &data[place..place + len]);
        }
        if len < left {
            self.sdma = Some((place + len, left - len, writes));
            self.put(reg::SDMA_ADDRESS, 4, address + len as u64);
        }
    }

    /// Moves a data command's blocks through the descriptors.
    fn transfer(&mut self, index: u8, argument: u32) -> Result<(), u16> {
        if let Some(bits) = self.fail.take() {
            return Err(bits);
        }
        let card = self.card.clone().unwrap();
        if card.mmc && !self.widths_agree() {
            return Err(1 << 5);
        }
        if self.regs[reg::HOST_CONTROL] >> 3 & 3 == 0 {
            // SDMA: from the address, stopping at each boundary.
            let blocks = if matches!(index, 18 | 25) { self.get(reg::BLOCK_COUNT, 2) } else { 1 };
            let start = if card.high { argument as usize * 512 } else { argument as usize };
            let total = blocks as usize * 512;
            if start + total > card.data.len() {
                return Err(1 << 4);
            }
            self.sdma = Some((start, total, matches!(index, 24 | 25)));
            self.sdma_move();
            return Ok(());
        }
        let wide = self.regs[reg::HOST_CONTROL] >> 3 & 3 == 3;
        let size = if wide { 12 } else { 8 };
        let mut table = self.get(reg::ADMA_ADDRESS, 4);
        if wide {
            table |= self.get(reg::ADMA_ADDRESS + 4, 4) << 32;
        }
        let table = self.host(table, wide);
        let count = self.get(reg::BLOCK_COUNT, 2) as usize;
        let multiple = matches!(index, 18 | 25);
        let blocks = if multiple { count } else { 1 };
        let start = if card.high { argument as usize * 512 } else { argument as usize };
        let total = blocks * 512;
        if start + total > card.data.len() {
            return Err(1 << 4);
        }
        let mut done = 0;
        let mut at = table;
        loop {
            let mut entry = [0u8; 12];
            radix_hw::fake_memory(at, &mut entry[..size]);
            let attributes = u16::from_le_bytes([entry[0], entry[1]]);
            if attributes & 1 == 0 {
                return Err(1 << 9);
            }
            let len = usize::from(u16::from_le_bytes([entry[2], entry[3]]));
            let mut address = u64::from(u32::from_le_bytes(entry[4..8].try_into().unwrap()));
            if wide {
                address |= u64::from(u32::from_le_bytes(entry[8..12].try_into().unwrap())) << 32;
            }
            let host = self.host(address, wide);
            if done + len > total {
                return Err(1 << 9);
            }
            let place = start + done;
            if matches!(index, 17 | 18) {
                radix_hw::fake_memory_write(host, &card.data[place..place + len]);
            } else {
                let mut bytes = std::vec![0u8; len];
                radix_hw::fake_memory(host, &mut bytes);
                self.card.as_mut().unwrap().data[place..place + len].copy_from_slice(&bytes);
            }
            done += len;
            if attributes & 2 != 0 {
                break;
            }
            at += size as u64;
        }
        if done == total { Ok(()) } else { Err(1 << 9) }
    }

    /// Puts `bytes`, a block the card sends, in the Buffer Data Port.
    fn send_block(&mut self, bytes: &[u8]) {
        let (words, _) = bytes.as_chunks::<4>();
        self.buffer = words.iter().map(|word| u32::from_le_bytes(*word)).collect();
        self.set_status(status::BUFFER_READ_READY, 0);
    }

    /// CMD6's 64-byte status for `argument` (Physical Layer, 4.3.10), and
    /// in switch mode the switch.
    fn switch(&mut self, card: &SimCard, argument: u32) -> [u8; 64] {
        let mut out = [0u8; 64];
        out[1] = 100;
        out[6..8].copy_from_slice(&(0x8000 | card.currents | 1).to_be_bytes());
        out[12..14].copy_from_slice(&(0x8000 | card.modes | 1).to_be_bytes());
        let (group1, group4) = ((argument & 0xf) as u8, (argument >> 12 & 0xf) as u8);
        let takes = |function: u8, of: u16| function == 0xf || (of | 1) & 1 << function != 0;
        let (group1, group4) = if takes(group1, card.modes) && takes(group4, card.currents) {
            (group1, group4)
        } else {
            (0xf, 0xf)
        };
        if argument & 1 << 31 != 0 && group1 != 0xf {
            self.function = group1;
        }
        if argument & 1 << 31 != 0 && group4 != 0xf {
            self.limit = group4;
        }
        let shown = |asked: u8, now: u8| if asked == 0xf { now } else { asked };
        out[15] = shown(group4, self.limit) << 4;
        out[16] =
            if group1 == 0xf && argument & 0xf != 0xf { 0xf } else { shown(group1, self.function) };
        out
    }

    /// A tuning block (CMD19 or CMD21): the controller finds a sampling
    /// point on the fifth, or with `tuning_fails` gives up at the 40th.
    fn tune(&mut self, card: &SimCard) {
        self.respond([0x900, 0, 0, 0]);
        self.tunes += 1;
        self.tuning_size = self.get(reg::BLOCK_SIZE, 2) as u16;
        let control2 = self.get(reg::HOST_CONTROL2, 2);
        if control2 & 1 << 6 != 0 {
            if self.tunes >= 5 && !card.tuning_fails {
                self.put(reg::HOST_CONTROL2, 2, control2 & !(1 << 6) | 1 << 7);
            } else if self.tunes >= 40 {
                self.put(reg::HOST_CONTROL2, 2, control2 & !(1 << 6 | 1 << 7));
            }
        }
        self.send_block(&std::vec![0x5a; usize::from(self.tuning_size)]);
    }

    /// The command just written.
    fn command(&mut self, value: u16) {
        let index = (value >> 8 & 0x3f) as u8;
        let argument = self.get(reg::ARGUMENT, 4) as u32;
        let app = core::mem::take(&mut self.app);
        self.commands.push(if app { 100 + index } else { index });
        let powered = self.regs[reg::POWER_CONTROL] & 1 != 0;
        let clocked = self.get(reg::CLOCK_CONTROL, 2) & 4 != 0;
        let Some(card) = self.card.clone().filter(|_| powered && clocked) else {
            self.set_status(status::ERROR, status::COMMAND_TIMEOUT);
            return;
        };
        let timeout = |state: &mut State| state.set_status(status::ERROR, status::COMMAND_TIMEOUT);
        let r1 = |state: &mut State, bits: u32| state.respond([bits | 0x900, 0, 0, 0]);
        let mut busy = false;
        match (app, index) {
            (_, 0) => {
                self.tries = 0;
                self.rca = 0;
                self.selected = false;
                self.agreed = false;
                self.switched = false;
                self.function = 0;
                self.limit = 0;
                self.bus_width = 0;
                self.timing = 0;
            }
            (false, 8) if card.mmc && self.selected && card.spec >= 4 => {
                r1(self, 0);
                if !self.widths_agree() {
                    self.set_status(status::ERROR, 1 << 5);
                    return;
                }
                let ext = self.ext_csd(&card);
                self.send_block(&ext);
                return;
            }
            (false, 8) if card.version2 && !card.mmc => self.respond([argument & 0xfff, 0, 0, 0]),
            (false, 8) => return timeout(self),
            (true, 41) if card.mmc => return timeout(self),
            (false, 1) if card.mmc => {
                self.tries += 1;
                let mut ocr = 0x00ff_8080;
                if self.tries >= 3 {
                    ocr |= 1 << 31;
                    if card.high && argument & 1 << 30 != 0 {
                        ocr |= 1 << 30;
                    }
                }
                self.respond([ocr, 0, 0, 0]);
            }
            (false, 2) if card.mmc => self.respond(Self::long(Self::mmc_cid())),
            (false, 3) if card.mmc => {
                self.rca = (argument >> 16) as u16;
                r1(self, 0);
            }
            (false, 9) if card.mmc => self.respond(Self::long(Self::mmc_csd(&card))),
            (false, 6) if card.mmc && self.selected && card.spec >= 4 => {
                self.mmc_switch(&card, argument);
                r1(self, 0);
                busy = true;
            }
            (false, 13) if self.selected => {
                let error = core::mem::take(&mut self.switch_error);
                r1(self, if error { 1 << 7 } else { 0 });
            }
            (false, 55) => {
                self.app = true;
                r1(self, 1 << 5);
            }
            (true, 41) => {
                self.tries += 1;
                let mut ocr = 0x00ff_8000;
                if self.tries >= 3 {
                    ocr |= 1 << 31;
                    if card.high && argument & 1 << 30 != 0 {
                        ocr |= 1 << 30;
                    }
                    if card.uhs && argument & 1 << 24 != 0 {
                        ocr |= 1 << 24;
                        self.agreed = true;
                    }
                }
                self.respond([ocr, 0, 0, 0]);
            }
            (false, 11) if self.agreed => {
                self.switched = !card.switch_fails;
                r1(self, 0);
            }
            (true, 51) if self.selected => {
                r1(self, 0);
                let mut scr = [0u8; 8];
                scr[0] = card.spec;
                scr[1] = 0x05;
                self.send_block(&scr);
                return;
            }
            (false, 6) if self.selected && card.spec > 0 => {
                r1(self, 0);
                let status = self.switch(&card, argument);
                self.send_block(&status);
                return;
            }
            (false, 19) if self.selected && !card.mmc => return self.tune(&card),
            (false, 21) if self.selected && card.mmc && self.timing == 2 => {
                return self.tune(&card);
            }
            (false, 2) => self.respond(Self::long(Self::cid())),
            (false, 3) => {
                self.rca = 0xb368;
                self.respond([u32::from(self.rca) << 16 | 0x500, 0, 0, 0]);
            }
            (false, 9) => self.respond(Self::long(Self::csd(&card))),
            (false, 7) => {
                self.selected = argument >> 16 == u32::from(self.rca);
                r1(self, 0);
                busy = true;
            }
            (true, 6) => r1(self, 0),
            (false, 16) => {
                self.block_length = argument;
                r1(self, 0);
            }
            (false, 12) => {
                r1(self, 0);
                busy = true;
            }
            (false, 17 | 18 | 24 | 25) if self.selected => {
                let writes = matches!(index, 24 | 25);
                if writes && card.write_protected {
                    r1(self, 1 << 26);
                } else if let Err(bits) = self.transfer(index, argument) {
                    r1(self, 0);
                    self.set_status(status::COMMAND_COMPLETE | status::ERROR, bits);
                    return;
                } else {
                    r1(self, 0);
                }
                busy = true;
            }
            _ => return timeout(self),
        }
        let mut normal = status::COMMAND_COMPLETE;
        if self.sdma.is_some() {
            // Stopped at a boundary: the transfer goes on once the address
            // is written again.
            normal |= status::DMA_INTERRUPT;
            busy = false;
        }
        if busy {
            normal |= status::TRANSFER_COMPLETE;
        }
        self.set_status(normal, 0);
    }
}

impl Sim {
    /// Puts `card` in the slot, as a card detect says it.
    fn insert(&self, card: SimCard) {
        let mut state = self.0.lock().unwrap();
        state.card = Some(card);
        state.set_status(status::CARD_INSERTION, 0);
    }

    /// Takes the card out: the controller powers it off.
    fn remove(&self) {
        let mut state = self.0.lock().unwrap();
        state.card = None;
        state.regs[reg::POWER_CONTROL] = 0;
        state.set_status(status::CARD_REMOVAL, 0);
    }
}

impl Registers for Sim {
    fn read(&self, offset: usize, size: usize) -> u64 {
        let mut state = self.0.lock().unwrap();
        if offset == reg::BUFFER {
            let word = state.buffer.pop_front().unwrap_or(0);
            if state.buffer.is_empty() {
                state.set_status(status::TRANSFER_COMPLETE, 0);
            }
            return u64::from(word);
        }
        if offset == reg::PRESENT_STATE {
            let mut value = if state.settling { 0 } else { 1 << 17 };
            if let Some(card) = &state.card {
                value |= 0x5 << 16;
                if !card.write_protected {
                    value |= 1 << 19;
                }
                // A card that took CMD11 holds DAT0 to DAT3 low until its
                // clock comes back at 1.8 V; one that fails holds them on.
                let clocked = state.get(reg::CLOCK_CONTROL, 2) & 4 != 0;
                let at_1_8 = state.get(reg::HOST_CONTROL2, 2) & 1 << 3 != 0;
                let low = state.agreed
                    && state.commands.contains(&11)
                    && (!clocked || !at_1_8 || !state.switched);
                if !low {
                    value |= 0xf << 20;
                }
            }
            return value;
        }
        state.get(offset, size)
    }

    fn write(&self, offset: usize, size: usize, value: u64) {
        let mut state = self.0.lock().unwrap();
        if matches!(offset, reg::POWER_CONTROL | reg::SOFTWARE_RESET) {
            state.pokes.push((offset, value));
        }
        match offset {
            reg::NORMAL_STATUS | reg::ERROR_STATUS => {
                let now = state.get(offset, size);
                state.put(offset, size, now & !value);
            }
            reg::SOFTWARE_RESET => {
                if value & 1 != 0 {
                    let keep: Vec<u8> = state.regs[0x40..0x50].to_vec();
                    let version = state.get(reg::VERSION, 2);
                    state.regs = [0; 256];
                    state.regs[0x40..0x50].copy_from_slice(&keep);
                    state.put(reg::VERSION, 2, version);
                }
            }
            reg::CLOCK_CONTROL => {
                let stable = if value & 1 != 0 { 2 } else { 0 };
                state.put(offset, size, value | stable);
            }
            reg::COMMAND => {
                state.put(offset, size, value);
                state.command(value as u16);
            }
            reg::SDMA_ADDRESS if state.sdma.is_some() => {
                state.put(offset, size, value);
                state.sdma_move();
                let done = if state.sdma.is_some() {
                    status::DMA_INTERRUPT
                } else {
                    status::TRANSFER_COMPLETE
                };
                state.set_status(done, 0);
            }
            _ => state.put(offset, size, value),
        }
    }
}

/// A slot of a controller of `version`, `base_mhz` and, with `wide`, 64-bit
/// DMA, holding `card`.
fn slot(version: u8, base_mhz: u64, wide: bool, card: Option<SimCard>) -> (Slot, Arc<Sim>) {
    let caps = base_mhz << 8 | 1 << 19 | 1 << 21 | 1 << 24 | if wide { 1 << 28 } else { 0 };
    slot_with(version, caps, 0, card)
}

/// A slot of a controller of version 3.00, of 200 MHz, with 64-bit DMA,
/// High Speed, 1.8 V and the UHS-I modes and tuning `uhs` says, giving
/// `max_ma` at 3.3 V, holding `card`.
fn uhs_slot(uhs: u64, max_ma: u64, card: SimCard) -> (Slot, Arc<Sim>) {
    let caps = 200 << 8 | 1 << 19 | 1 << 21 | 1 << 24 | 1 << 26 | 1 << 28 | uhs;
    slot_with(version::V3, caps, max_ma / 4, Some(card))
}

/// A UHS-I card of 4096 blocks with every mode and current limit.
fn uhs_card() -> SimCard {
    SimCard { uhs: true, modes: 0b1_1110, currents: 0b1110, ..card(4096, true) }
}

/// The UHS-I capabilities: SDR50, SDR104, DDR50, and SDR50 tuned.
const SDR50: u64 = 1 << 32;
const SDR104: u64 = 1 << 33;
const DDR50: u64 = 1 << 34;
const TUNE_SDR50: u64 = 1 << 45;

/// A slot of a controller of `version` with capabilities `caps` and
/// Maximum Current `current`, holding `card`.
fn slot_with(version: u8, caps: u64, current: u64, card: Option<SimCard>) -> (Slot, Arc<Sim>) {
    quirky_slot(version, caps, current, card, Quirks::default())
}

/// As [`slot_with`], of a controller that needs `quirks`.
fn quirky_slot(
    version: u8,
    caps: u64,
    current: u64,
    card: Option<SimCard>,
    quirks: Quirks,
) -> (Slot, Arc<Sim>) {
    let mut state = State::new(card);
    state.put(reg::CAPABILITIES, 8, caps);
    state.put(reg::MAX_CURRENT, 8, current);
    state.put(reg::VERSION, 2, u64::from(version));
    let sim = Arc::new(Sim(Mutex::new(state)));
    let regs = Mmio::fake(sim.clone(), REGISTERS);
    // The table's fake address is a host one, above 4 GiB: its low half
    // stands for it in 32-bit descriptors.
    let mut tables = Vec::new();
    let mut alloc = |pages: usize, _low: bool| {
        let dma = Dma::fake(pages * 4096);
        tables.push(dma.phys());
        Some(dma)
    };
    let slot = Slot::new(regs, &mut alloc, &mut |_| {}, quirks).unwrap();
    let aliases = tables.iter().map(|&phys| (phys as u32, phys, 4096));
    sim.0.lock().unwrap().aliases.extend(aliases);
    (slot, sim)
}

/// A card of `blocks` blocks, SDHC with `high`, of version 3.0x with High
/// Speed and no UHS-I.
fn card(blocks: usize, high: bool) -> SimCard {
    SimCard {
        data: std::vec![0; blocks * 512],
        version2: true,
        high,
        write_protected: false,
        spec: 2,
        modes: 1 << 1,
        currents: 0,
        uhs: false,
        switch_fails: false,
        tuning_fails: false,
        mmc: false,
        card_type: 0,
    }
}

/// A pattern of `len` bytes for block `block` on.
fn pattern(block: usize, len: usize) -> Vec<u8> {
    (0..len).map(|i| ((block * 512 + i) * 7 % 251) as u8).collect()
}

#[test]
fn sdhc_card_is_set_up_and_moves_blocks_with_wide_descriptors() {
    let (mut slot, sim) = slot(version::V3, 200, true, Some(card(4096, true)));
    assert!(slot.present() && slot.reaches_64());
    assert_eq!(slot.version(), (3, 0));
    let card = slot.identify(&mut |_| {}).unwrap();
    assert!(card.high_capacity);
    assert_eq!((card.blocks, card.rca, card.width, card.read_only), (4096, 0xb368, 4, false));
    assert_eq!(card.name(), "TEST1");
    assert_eq!(card.kind(), "SDHC");
    assert_eq!((card.cid.maker, card.cid.oem, card.cid.serial), (3, *b"SD", 0xdead_beef));
    assert_eq!((card.cid.year, card.cid.month, card.cid.revision), (2024, 6, 0x10));
    // High Speed: 200 MHz over 2 × 2, after the SCR and CMD6's check and
    // switch.
    assert_eq!((card.speed, card.khz), (Speed::High, 50_000));
    assert_eq!(slot.regs.read8(reg::HOST_CONTROL) & 0b110, 0b110);
    let commands = sim.0.lock().unwrap().commands.clone();
    assert_eq!(commands, [0, 8, 55, 141, 55, 141, 55, 141, 2, 3, 9, 7, 55, 106, 55, 151, 6, 6]);
    // Three buffers of five blocks, written from block 100, then read back.
    let pages: Vec<Dma> = (0..3).map(|_| Dma::fake(4096)).collect();
    let lens = [1024u32, 512, 1024];
    for (i, page) in pages.iter().enumerate() {
        page.write(0, &pattern(100 + i * 2, lens[i] as usize));
    }
    let buffers: Vec<(u64, u32)> = pages.iter().zip(lens).map(|(p, l)| (p.phys(), l)).collect();
    slot.start(Op::Write, 100, 5, &buffers, &mut |_| {}).unwrap();
    assert_eq!(slot.poll(), Some(Ok(())));
    assert_eq!(slot.poll(), None);
    let data = sim.0.lock().unwrap().card.as_ref().unwrap().data[100 * 512..102 * 512].to_vec();
    assert_eq!(data, pattern(100, 1024));
    let back = Dma::fake(4096);
    slot.start(Op::Read, 101, 2, &[(back.phys(), 1024)], &mut |_| {}).unwrap();
    assert_eq!(slot.poll(), Some(Ok(())));
    let mut got = std::vec![0u8; 1024];
    back.read(0, &mut got);
    let mut want = pattern(101, 512);
    want.extend(pattern(102, 512));
    assert_eq!(got, want);
    // One block: CMD17.
    slot.start(Op::Read, 4095, 1, &[(back.phys(), 512)], &mut |_| {}).unwrap();
    assert_eq!(slot.poll(), Some(Ok(())));
    assert_eq!(sim.0.lock().unwrap().commands.last(), Some(&17));
}

#[test]
fn standard_capacity_cards_are_addressed_in_bytes_through_narrow_descriptors() {
    // A version 2.00 controller of 52 MHz, as QEMU's: 26 MHz is the most
    // at High Speed's 50.
    let (mut slot, sim) = slot(version::V2, 52, false, Some(card(8192, false)));
    assert!(!slot.reaches_64());
    let card = slot.identify(&mut |_| {}).unwrap();
    assert!(!card.high_capacity);
    assert_eq!((card.blocks, card.khz, card.kind()), (8192, 26_000, "SD"));
    assert_eq!(sim.0.lock().unwrap().block_length, 512);
    // Host memory above 4 GiB is refused; below, the simulator maps it.
    let page = Dma::fake(4096);
    assert_eq!(
        slot.start(Op::Write, 0, 1, &[(page.phys(), 512)], &mut |_| {}),
        if page.phys() + 512 > 1 << 32 { Err(Error::BadRequest) } else { Ok(()) }
    );
    let _ = slot.poll();
    sim.0.lock().unwrap().aliases.push((0x1000_0000, page.phys(), 4096));
    page.write(0, &pattern(7, 1024));
    slot.start(Op::Write, 7, 2, &[(0x1000_0000, 1024)], &mut |_| {}).unwrap();
    assert_eq!(slot.poll(), Some(Ok(())));
    let state = sim.0.lock().unwrap();
    assert_eq!(state.get(reg::ARGUMENT, 4), 7 * 512);
    assert_eq!(state.card.as_ref().unwrap().data[7 * 512..9 * 512], pattern(7, 1024));
}

#[test]
fn version_one_cards_do_not_answer_cmd8() {
    let mut old = card(2048, false);
    old.version2 = false;
    let (mut slot, sim) = slot(version::V3, 100, true, Some(old));
    let card = slot.identify(&mut |_| {}).unwrap();
    assert!(!card.high_capacity);
    assert_eq!(card.blocks, 2048);
    // ACMD41 without HCS.
    assert!(sim.0.lock().unwrap().commands.starts_with(&[0, 8, 55, 141]));
}

#[test]
fn empty_slots_and_cards_of_other_kinds() {
    let (mut slot, _) = slot(version::V3, 200, true, None);
    assert!(!slot.present());
    assert_eq!(slot.identify(&mut |_| {}), Err(Error::NoCard));
    let page = Dma::fake(4096);
    assert_eq!(slot.start(Op::Read, 0, 1, &[(page.phys(), 512)], &mut |_| {}), Err(Error::NoCard));
    // Without ADMA2 or SDMA, the slot is not driven.
    let mut state = State::new(None);
    state.put(reg::CAPABILITIES, 8, 200 << 8);
    let sim = Arc::new(Sim(Mutex::new(state)));
    let mut alloc = |_: usize, _: bool| Some(Dma::fake(4096));
    let made = Slot::new(Mmio::fake(sim, REGISTERS), &mut alloc, &mut |_| {}, Quirks::default());
    assert!(matches!(made, Err(Error::Unsupported(_))));
}

#[test]
fn bad_requests_are_refused() {
    let (mut slot, _) = slot(version::V3, 200, true, Some(card(1024, true)));
    slot.identify(&mut |_| {}).unwrap();
    let page = Dma::fake(4096);
    let p = page.phys();
    let mut no_wait = |_| {};
    // Not the blocks' bytes, past the end, nothing, too many, and a
    // second while one is in flight.
    assert_eq!(slot.start(Op::Read, 0, 2, &[(p, 512)], &mut no_wait), Err(Error::BadRequest));
    assert_eq!(slot.start(Op::Read, 1023, 2, &[(p, 1024)], &mut no_wait), Err(Error::BadRequest));
    assert_eq!(slot.start(Op::Read, 0, 0, &[], &mut no_wait), Err(Error::BadRequest));
    let many = std::vec![(p, 4); MAX_BUFFERS + 1];
    assert_eq!(slot.start(Op::Read, 0, 1, &many, &mut no_wait), Err(Error::BadRequest));
    assert_eq!(slot.start(Op::Read, 0, 1, &[(p + 2, 512)], &mut no_wait), Err(Error::BadRequest));
}

#[test]
fn failures_stop_the_card_before_the_next_request() {
    let (mut slot, sim) = slot(version::V3, 200, true, Some(card(1024, true)));
    slot.identify(&mut |_| {}).unwrap();
    let page = Dma::fake(4096);
    // A CRC error on the data.
    sim.0.lock().unwrap().fail = Some(1 << 5);
    slot.start(Op::Read, 0, 4, &[(page.phys(), 2048)], &mut |_| {}).unwrap();
    let failed = slot.poll();
    assert_eq!(failed, Some(Err(Error::Command(1 << 5))));
    assert_eq!(Error::Command(1 << 5).as_str(), "the card's data was garbled (CRC)");
    // The next request stops the card first (CMD12), and works.
    slot.start(Op::Read, 0, 4, &[(page.phys(), 2048)], &mut |_| {}).unwrap();
    assert_eq!(slot.poll(), Some(Ok(())));
    let commands = sim.0.lock().unwrap().commands.clone();
    assert_eq!(commands[commands.len() - 3..], [18, 12, 18]);
}

#[test]
fn write_protected_cards_are_read_only() {
    let mut locked = card(1024, true);
    locked.write_protected = true;
    let (mut slot, _) = slot(version::V3, 200, true, Some(locked));
    let card = slot.identify(&mut |_| {}).unwrap();
    assert!(card.read_only);
    let page = Dma::fake(4096);
    assert_eq!(
        slot.start(Op::Write, 0, 1, &[(page.phys(), 512)], &mut |_| {}),
        Err(Error::BadRequest)
    );
    slot.start(Op::Read, 0, 1, &[(page.phys(), 512)], &mut |_| {}).unwrap();
    assert_eq!(slot.poll(), Some(Ok(())));
}

#[test]
fn csd_and_cid_are_read() {
    // A 32 GB SDXC card's CSD 2.0: C_SIZE 0xEDC7 (59,592 MiB / 2 + ...).
    let csd = State::long(1 << 126 | 0xedc7u128 << 48);
    assert_eq!(Csd::parse(&csd), Csd::High((0xedc7 + 1) * 1024));
    let standard = State::csd(&card(8192, false));
    assert_eq!(Csd::parse(&State::long(standard)), Csd::Standard(8192));
    assert_eq!(Csd::parse(&State::long(2 << 126)), Csd::Ultra);
    assert_eq!(Csd::parse(&State::long(3 << 126)), Csd::Unknown);
    let cid = Cid::parse(&State::long(State::cid()));
    assert_eq!(&cid.name[..5], CID_NAME);
    let big = Card {
        family: Family::Sd,
        rca: 1,
        high_capacity: true,
        blocks: 64 * 1_000_000_000 / 512,
        cid,
        speed: Speed::Default,
        khz: 25_000,
        width: 4,
        read_only: false,
    };
    assert_eq!(big.kind(), "SDXC");
}

#[test]
fn cards_put_in_and_taken_out_are_seen_once_each() {
    let (mut slot, sim) = slot(version::V3, 200, true, None);
    let detect = slot.detect();
    assert!(slot.removable());
    assert_eq!(detect.presence(), Presence::Empty);
    assert!(!detect.events().any());
    // A card goes in: seen once, and set up.
    sim.insert(card(1024, true));
    assert_eq!(detect.events(), Events { inserted: true, removed: false });
    assert!(!detect.events().any());
    assert_eq!(detect.presence(), Presence::Card { protected: false });
    slot.identify(&mut |_| {}).unwrap();
    // Taken out and another, write-protected, put in between two looks:
    // the commands of the new card's set-up leave both events for the
    // look.
    sim.remove();
    let mut locked = card(2048, true);
    locked.write_protected = true;
    sim.insert(locked);
    let card = slot.identify(&mut |_| {}).unwrap();
    assert!(card.read_only);
    assert_eq!(detect.events(), Events { inserted: true, removed: true });
    assert_eq!(detect.presence(), Presence::Card { protected: true });
    // A card detect still settling says so.
    sim.0.lock().unwrap().settling = true;
    assert_eq!(detect.presence(), Presence::Settling);
    // Taken out: its power is off.
    sim.0.lock().unwrap().settling = false;
    sim.remove();
    detect.power_off();
    assert_eq!(detect.presence(), Presence::Empty);
    assert_eq!(sim.0.lock().unwrap().regs[reg::POWER_CONTROL], 0);
    assert_eq!(slot.identify(&mut |_| {}), Err(Error::NoCard));
}

#[test]
fn interrupts_signal_transfers_and_cards() {
    let (mut slot, sim) = slot(version::V3, 200, true, Some(card(1024, true)));
    slot.identify(&mut |_| {}).unwrap();
    let detect = slot.detect();
    // Polled: nothing is signalled, whatever happens.
    let page = Dma::fake(4096);
    slot.start(Op::Read, 0, 1, &[(page.phys(), 512)], &mut |_| {}).unwrap();
    assert!(!detect.interrupting());
    assert_eq!(slot.poll(), Some(Ok(())));
    // Interrupting: a transfer's end, until it is collected.
    detect.interrupts(true);
    slot.start(Op::Read, 0, 1, &[(page.phys(), 512)], &mut |_| {}).unwrap();
    assert!(detect.interrupting());
    assert_eq!(slot.poll(), Some(Ok(())));
    assert!(!detect.interrupting());
    // An error.
    sim.0.lock().unwrap().fail = Some(1 << 5);
    slot.start(Op::Read, 0, 1, &[(page.phys(), 512)], &mut |_| {}).unwrap();
    assert!(detect.interrupting());
    assert_eq!(slot.poll(), Some(Err(Error::Command(1 << 5))));
    assert!(!detect.interrupting());
    // The card taken out, until its watcher looks.
    sim.remove();
    assert!(detect.interrupting());
    assert!(detect.events().removed);
    assert!(!detect.interrupting());
    detect.interrupts(false);
    sim.insert(card(1024, true));
    assert!(!detect.interrupting());
}

#[test]
fn soldered_slots_are_not_removable() {
    let (slot, sim) = slot(version::V3, 200, true, Some(card(1024, true)));
    assert!(slot.removable());
    let mut state = sim.0.lock().unwrap();
    let caps = state.get(reg::CAPABILITIES, 8) | 1 << 30;
    state.put(reg::CAPABILITIES, 8, caps);
    drop(state);
    let regs = Mmio::fake(sim.clone(), REGISTERS);
    let mut alloc = |_: usize, _: bool| Some(Dma::fake(4096));
    let embedded = Slot::new(regs, &mut alloc, &mut |_| {}, Quirks::default()).unwrap();
    assert!(!embedded.removable());
}

#[test]
fn cards_and_slots_without_high_speed_stay_at_the_default_speed() {
    // A card of version 1.0x has no CMD6.
    let old = SimCard { spec: 0, ..card(1024, true) };
    let (mut slot_a, sim) = slot(version::V3, 200, true, Some(old));
    let card_a = slot_a.identify(&mut |_| {}).unwrap();
    assert_eq!((card_a.speed, card_a.khz), (Speed::Default, 25_000));
    assert!(!sim.0.lock().unwrap().commands.contains(&6));
    // A card without High Speed is asked, and left.
    let plain = SimCard { modes: 0, ..card(1024, true) };
    let (mut slot_b, sim) = slot(version::V3, 200, true, Some(plain));
    assert_eq!(slot_b.identify(&mut |_| {}).unwrap().speed, Speed::Default);
    assert_eq!(sim.0.lock().unwrap().commands.iter().filter(|&&c| c == 6).count(), 1);
    assert_eq!(slot_b.regs.read8(reg::HOST_CONTROL) & 0b100, 0);
    // A slot without High Speed does not switch a card that has it.
    let caps = 200 << 8 | 1 << 19 | 1 << 24 | 1 << 28;
    let (mut slot_c, sim) = slot_with(version::V3, caps, 0, Some(card(1024, true)));
    assert_eq!(slot_c.identify(&mut |_| {}).unwrap().speed, Speed::Default);
    assert_eq!(sim.0.lock().unwrap().function, 0);
}

#[test]
fn uhs_cards_switch_to_1_8_volts_and_tune_for_sdr104() {
    let (mut slot, sim) = uhs_slot(SDR50 | SDR104 | DDR50, 800, uhs_card());
    let card = slot.identify(&mut |_| {}).unwrap();
    // 200 MHz is the most at SDR104's 208, with 800 mA asked for.
    assert_eq!((card.speed, card.khz), (Speed::Sdr104, 200_000));
    let state = sim.0.lock().unwrap();
    assert!(state.switched);
    assert_eq!((state.function, state.limit), (3, 3));
    assert!(state.tunes >= 5);
    let control2 = state.get(reg::HOST_CONTROL2, 2);
    // 1.8 V, SDR104, tuning done with a sampling point found.
    assert_eq!(control2 & 0xff, 1 << 7 | 1 << 3 | 3);
    assert_eq!(state.regs[reg::HOST_CONTROL] & 0b110, 0b110);
    let at = state.commands.iter().position(|&c| c == 11).unwrap();
    assert_eq!(state.commands[at - 1], 141, "CMD11 follows ACMD41");
    drop(state);
    // It moves blocks.
    let page = Dma::fake(4096);
    slot.start(Op::Read, 8, 2, &[(page.phys(), 1024)], &mut |_| {}).unwrap();
    assert_eq!(slot.poll(), Some(Ok(())));
}

#[test]
fn a_mode_that_does_not_tune_gives_way_to_the_next() {
    let card = SimCard { tuning_fails: true, ..uhs_card() };
    let (mut slot, sim) = uhs_slot(SDR50 | SDR104 | DDR50, 800, card);
    let card = slot.identify(&mut |_| {}).unwrap();
    // SDR104 did not tune; DDR50 needs none.
    assert_eq!((card.speed, card.khz), (Speed::Ddr50, 50_000));
    let state = sim.0.lock().unwrap();
    assert_eq!(state.function, 4);
    assert_eq!(state.get(reg::HOST_CONTROL2, 2) & 0xc7, 4);
}

#[test]
fn sdr50_is_tuned_only_where_the_controller_asks() {
    let (mut slot, sim) = uhs_slot(SDR50, 800, uhs_card());
    let card = slot.identify(&mut |_| {}).unwrap();
    assert_eq!((card.speed, card.khz), (Speed::Sdr50, 100_000));
    assert_eq!(sim.0.lock().unwrap().tunes, 0);
    let (mut slot, sim) = uhs_slot(SDR50 | TUNE_SDR50, 800, uhs_card());
    assert_eq!(slot.identify(&mut |_| {}).unwrap().speed, Speed::Sdr50);
    assert!(sim.0.lock().unwrap().tunes >= 5);
    // A controller with none of the card's faster modes runs it at SDR25.
    let card = SimCard { modes: 0b1_0010, ..uhs_card() };
    let (mut slot, _) = uhs_slot(SDR104, 800, card);
    let card = slot.identify(&mut |_| {}).unwrap();
    assert_eq!((card.speed, card.khz), (Speed::Sdr25, 50_000));
}

#[test]
fn the_current_limit_is_what_both_the_card_and_slot_take() {
    let limit = |max_ma: u64, currents: u16| {
        let card = SimCard { currents, ..uhs_card() };
        let (mut slot, sim) = uhs_slot(SDR104, max_ma, card);
        slot.identify(&mut |_| {}).unwrap();
        sim.0.lock().unwrap().limit
    };
    assert_eq!(limit(800, 0b1110), 3);
    assert_eq!(limit(600, 0b1110), 2);
    assert_eq!(limit(800, 0b0010), 1);
    // A slot that does not say leaves the card's 200 mA.
    assert_eq!(limit(0, 0b1110), 0);
}

#[test]
fn a_card_that_does_not_switch_to_1_8_volts_is_set_up_again_at_3_3() {
    let card = SimCard { switch_fails: true, ..uhs_card() };
    let (mut slot, sim) = uhs_slot(SDR50 | SDR104 | DDR50, 800, card);
    let card = slot.identify(&mut |_| {}).unwrap();
    assert_eq!((card.speed, card.khz), (Speed::High, 50_000));
    let state = sim.0.lock().unwrap();
    // Powered off and set up from CMD0 again, without asking for 1.8 V.
    assert_eq!(state.commands.iter().filter(|&&c| c == 0).count(), 2);
    assert!(!state.agreed);
    assert_eq!(state.get(reg::HOST_CONTROL2, 2) & 1 << 3, 0);
}

#[test]
fn garbled_data_in_a_tuned_mode_tunes_the_slot_again() {
    let (mut slot, sim) = uhs_slot(SDR104, 800, uhs_card());
    assert_eq!(slot.identify(&mut |_| {}).unwrap().speed, Speed::Sdr104);
    let page = Dma::fake(4096);
    let read = [(page.phys(), 512)];
    let tunes = sim.0.lock().unwrap().tunes;
    sim.0.lock().unwrap().fail = Some(1 << 5);
    slot.start(Op::Read, 0, 1, &read, &mut |_| {}).unwrap();
    assert_eq!(slot.poll(), Some(Err(Error::Command(1 << 5))));
    // The next transfer tunes first.
    slot.start(Op::Read, 0, 1, &read, &mut |_| {}).unwrap();
    assert_eq!(slot.poll(), Some(Ok(())));
    assert!(sim.0.lock().unwrap().tunes > tunes);
    // A timeout is not the sampling point's: no tuning.
    let tunes = sim.0.lock().unwrap().tunes;
    sim.0.lock().unwrap().fail = Some(1 << 4);
    slot.start(Op::Read, 0, 1, &read, &mut |_| {}).unwrap();
    assert!(matches!(slot.poll(), Some(Err(_))));
    slot.start(Op::Read, 0, 1, &read, &mut |_| {}).unwrap();
    assert_eq!(slot.poll(), Some(Ok(())));
    assert_eq!(sim.0.lock().unwrap().tunes, tunes);
    // Garbled again, and the mode no longer tunes: SDR25.
    {
        let mut state = sim.0.lock().unwrap();
        state.fail = Some(1 << 5);
        state.card.as_mut().unwrap().tuning_fails = true;
        state.tunes = 0;
    }
    slot.start(Op::Read, 0, 1, &read, &mut |_| {}).unwrap();
    assert!(matches!(slot.poll(), Some(Err(_))));
    slot.start(Op::Read, 0, 1, &read, &mut |_| {}).unwrap();
    assert_eq!(slot.poll(), Some(Ok(())));
    let card = slot.card().unwrap();
    assert_eq!((card.speed, card.khz), (Speed::Sdr25, 50_000));
    assert_eq!(sim.0.lock().unwrap().function, 1);
}

/// An MMC of `blocks` blocks, addressed in sectors with `high`, of
/// version 4 or later, with the speeds `card_type` gives.
fn mmc(blocks: usize, high: bool, card_type: u8) -> SimCard {
    SimCard { mmc: true, version2: false, spec: 4, modes: 0, card_type, ..card(blocks, high) }
}

/// CARD_TYPE's High Speed at 52 MHz and HS200 at 1.8 V.
const HS_52: u8 = 1 << 1;
const HS200: u8 = 1 << 4;

/// An eMMC's slot: version 3.00 at 200 MHz with eight lines, 64-bit DMA,
/// High Speed, 3.3 and 1.8 V and SDR104, soldered to the board.
const EMMC_SLOT: u64 =
    200 << 8 | 1 << 18 | 1 << 19 | 1 << 21 | 1 << 24 | 1 << 26 | 1 << 28 | 1 << 30 | SDR104;

#[test]
fn an_emmc_is_set_up_on_eight_bits_at_hs200() {
    let emmc = mmc(8192, true, HS_52 | HS200);
    let (mut slot, sim) = slot_with(version::V3, EMMC_SLOT, 0, Some(emmc));
    assert!(!slot.removable());
    let card = slot.identify(&mut |_| {}).unwrap();
    assert_eq!((card.family, card.kind(), card.rca), (Family::Emmc, "eMMC", 1));
    // Its capacity is the EXT_CSD's SEC_COUNT, not the CSD's 2 GB.
    assert_eq!((card.blocks, card.high_capacity, card.read_only), (8192, true, false));
    assert_eq!((card.width, card.speed, card.khz), (8, Speed::Hs200, 200_000));
    assert_eq!(card.name(), "EMMC01");
    assert_eq!((card.cid.maker, card.cid.oem, card.cid.serial), (0x15, [0, 1], 0xcafe_f00d));
    assert_eq!((card.cid.year, card.cid.month, card.cid.revision), (2024, 6, 0x21));
    let state = sim.0.lock().unwrap();
    // No answer to CMD8 or ACMD41: CMD1 until ready, the EXT_CSD read on
    // one bit and again on eight, HS200, and CMD21 until tuned.
    let set_up = [0, 8, 55, 141, 0, 1, 1, 1, 2, 3, 9, 7, 8, 6, 13, 8, 6, 13];
    assert_eq!(state.commands[..18], set_up);
    assert_eq!(state.commands[18..], [21; 5]);
    assert_eq!((state.bus_width, state.timing, state.rca, state.tuning_size), (2, 2, 1, 128));
    assert_eq!(state.regs[reg::HOST_CONTROL] & (1 << 5 | 1 << 2), 1 << 5 | 1 << 2);
    // 1.8 V, SDR104's timing, tuned.
    assert_eq!(state.get(reg::HOST_CONTROL2, 2) & 0xff, 1 << 7 | 1 << 3 | 3);
    drop(state);
    // It moves blocks on eight bits, addressed in sectors.
    let page = Dma::fake(4096);
    page.write(0, &pattern(7, 1024));
    slot.start(Op::Write, 7, 2, &[(page.phys(), 1024)], &mut |_| {}).unwrap();
    assert_eq!(slot.poll(), Some(Ok(())));
    let data = sim.0.lock().unwrap().card.as_ref().unwrap().data[7 * 512..9 * 512].to_vec();
    assert_eq!(data, pattern(7, 1024));
}

#[test]
fn a_slot_with_four_lines_wired_runs_the_bus_at_four_bits() {
    let emmc = mmc(8192, true, HS_52 | HS200);
    let (mut slot, sim) = slot_with(version::V3, EMMC_SLOT, 0, Some(emmc));
    sim.0.lock().unwrap().lines = 4;
    let card = slot.identify(&mut |_| {}).unwrap();
    assert_eq!((card.width, card.speed), (4, Speed::Hs200));
    let state = sim.0.lock().unwrap();
    assert_eq!((state.bus_width, state.tuning_size), (1, 64));
    assert_eq!(state.regs[reg::HOST_CONTROL] & (1 << 5 | 1 << 1), 1 << 1);
    // The EXT_CSD was read on one bit, garbled on eight, and read on four
    // (after the SD card's CMD8 that went unanswered).
    assert_eq!(state.commands.iter().filter(|&&c| c == 8).count(), 4);
}

#[test]
fn an_mmc_in_a_removable_slot_runs_at_high_speed() {
    // A slot of four lines without 1.8 V: no HS200.
    let (mut reader, sim) = slot(version::V3, 200, true, Some(mmc(4096, true, HS_52 | HS200)));
    let card = reader.identify(&mut |_| {}).unwrap();
    assert_eq!((card.family, card.kind()), (Family::Mmc, "MMC"));
    // 52 MHz is 50 from 200 MHz.
    assert_eq!((card.width, card.speed, card.khz), (4, Speed::MmcHigh, 50_000));
    let state = sim.0.lock().unwrap();
    assert_eq!((state.timing, state.tunes), (1, 0));
    assert_eq!(state.regs[reg::HOST_CONTROL] & 0b110, 0b110);
    drop(state);
    // A device without High Speed stays at the default speed.
    let (mut reader, sim) = slot(version::V3, 200, true, Some(mmc(4096, true, 0)));
    let card = reader.identify(&mut |_| {}).unwrap();
    assert_eq!((card.speed, card.khz), (Speed::Default, 25_000));
    assert_eq!(sim.0.lock().unwrap().timing, 0);
}

#[test]
fn small_and_old_mmcs_are_addressed_in_bytes() {
    // 2 GB or less: addressed in bytes, its capacity in the CSD.
    let (mut reader, sim) = slot(version::V3, 200, true, Some(mmc(8192, false, 0)));
    let card = reader.identify(&mut |_| {}).unwrap();
    assert!(!card.high_capacity);
    assert_eq!((card.blocks, card.speed, card.khz, card.width), (8192, Speed::Default, 25_000, 4));
    assert!(sim.0.lock().unwrap().commands.contains(&16));
    let page = Dma::fake(4096);
    page.write(0, &pattern(3, 512));
    reader.start(Op::Write, 3, 1, &[(page.phys(), 512)], &mut |_| {}).unwrap();
    assert_eq!(reader.poll(), Some(Ok(())));
    let data = sim.0.lock().unwrap().card.as_ref().unwrap().data[3 * 512..4 * 512].to_vec();
    assert_eq!(data, pattern(3, 512));
    // Before version 4: no EXT_CSD and no CMD6, one bit at 20 MHz.
    let old = SimCard { spec: 3, ..mmc(8192, false, 0) };
    let (mut reader, sim) = slot(version::V3, 200, true, Some(old));
    let card = reader.identify(&mut |_| {}).unwrap();
    assert_eq!((card.blocks, card.width, card.khz), (8192, 1, 20_000));
    let commands = sim.0.lock().unwrap().commands.clone();
    assert_eq!(commands[4..], [0, 1, 1, 1, 2, 3, 9, 7, 16]);
}

#[test]
fn hs200_refused_or_untuned_gives_way_to_high_speed() {
    let tuning_fails = SimCard { tuning_fails: true, ..mmc(8192, true, HS_52 | HS200) };
    let refused = SimCard { switch_fails: true, ..mmc(8192, true, HS_52 | HS200) };
    for emmc in [tuning_fails, refused] {
        let (mut slot, sim) = slot_with(version::V3, EMMC_SLOT, 0, Some(emmc));
        let card = slot.identify(&mut |_| {}).unwrap();
        assert_eq!((card.speed, card.khz, card.width), (Speed::MmcHigh, 50_000, 8));
        let state = sim.0.lock().unwrap();
        assert_eq!(state.timing, 1);
        // The slot's timing back to the default's, untuned, its I/O
        // still at 1.8 V.
        assert_eq!(state.get(reg::HOST_CONTROL2, 2) & 0xcf, 1 << 3);
    }
}

#[test]
fn garbled_data_at_hs200_tunes_again_or_falls_back_to_high_speed() {
    let emmc = mmc(8192, true, HS_52 | HS200);
    let (mut slot, sim) = slot_with(version::V3, EMMC_SLOT, 0, Some(emmc));
    assert_eq!(slot.identify(&mut |_| {}).unwrap().speed, Speed::Hs200);
    let page = Dma::fake(4096);
    let read = [(page.phys(), 512)];
    let tunes = sim.0.lock().unwrap().tunes;
    sim.0.lock().unwrap().fail = Some(1 << 5);
    slot.start(Op::Read, 0, 1, &read, &mut |_| {}).unwrap();
    assert!(matches!(slot.poll(), Some(Err(_))));
    assert!(slot.retuning());
    // The next transfer tunes first, with CMD21.
    slot.start(Op::Read, 0, 1, &read, &mut |_| {}).unwrap();
    assert_eq!(slot.poll(), Some(Ok(())));
    assert!(!slot.retuning());
    assert!(sim.0.lock().unwrap().tunes > tunes);
    // Garbled again, and HS200 no longer tunes: High Speed.
    {
        let mut state = sim.0.lock().unwrap();
        state.fail = Some(1 << 5);
        state.card.as_mut().unwrap().tuning_fails = true;
        state.tunes = 0;
    }
    slot.start(Op::Read, 0, 1, &read, &mut |_| {}).unwrap();
    assert!(matches!(slot.poll(), Some(Err(_))));
    slot.start(Op::Read, 0, 1, &read, &mut |_| {}).unwrap();
    assert_eq!(slot.poll(), Some(Ok(())));
    let card = slot.card().unwrap();
    assert_eq!((card.speed, card.khz), (Speed::MmcHigh, 50_000));
    assert_eq!(sim.0.lock().unwrap().timing, 1);
}

/// A controller of version 2.00 and 52 MHz with SDMA, High Speed and
/// 3.3 V, and ADMA2 only with `adma2`.
fn sdma_caps(adma2: bool) -> u64 {
    52 << 8 | 1 << 21 | 1 << 22 | 1 << 24 | if adma2 { 1 << 19 } else { 0 }
}

/// Polls `slot` until its request ends: how it ended, and how many polls
/// found it still going.
fn finish(slot: &mut Slot) -> (Result<(), Error>, usize) {
    for going in 0..100 {
        if let Some(result) = slot.poll() {
            return (result, going);
        }
    }
    panic!("the request never ended");
}

#[test]
fn controllers_without_adma2_move_one_buffer_by_sdma() {
    let (mut slot, sim) = slot_with(version::V2, sdma_caps(false), 0, Some(card(4096, true)));
    assert!(slot.contiguous() && !slot.reaches_64());
    slot.identify(&mut |_| {}).unwrap();
    // 1 MiB from 64 KiB below a 512 KiB boundary: it stops there and at
    // the next, and goes on each time the address is written back.
    let page = Dma::fake(1 << 20);
    sim.0.lock().unwrap().aliases.push((0x2007_0000, page.phys(), 1 << 20));
    page.write(0, &pattern(0, 1 << 20));
    // In buffers of 32 KiB, one after another, as the kernel hands it over.
    let buffers: Vec<(u64, u32)> = (0..32).map(|i| (0x2007_0000 + i * 0x8000, 0x8000)).collect();
    slot.start(Op::Write, 0, 2048, &buffers, &mut |_| {}).unwrap();
    {
        let state = sim.0.lock().unwrap();
        assert_eq!(state.regs[reg::HOST_CONTROL] >> 3 & 3, 0);
        assert_eq!(state.get(reg::BLOCK_SIZE, 2), 7 << 12 | 512);
    }
    assert_eq!(finish(&mut slot), (Ok(()), 2));
    assert_eq!(sim.0.lock().unwrap().card.as_ref().unwrap().data[..1 << 20], pattern(0, 1 << 20));
    page.write(0, &std::vec![0; 1 << 20]);
    slot.start(Op::Read, 0, 2048, &buffers, &mut |_| {}).unwrap();
    assert_eq!(finish(&mut slot), (Ok(()), 2));
    let mut back = std::vec![0u8; 1 << 20];
    page.read(0, &mut back);
    assert_eq!(back, pattern(0, 1 << 20));
    // Buffers one after another are one buffer; apart, they are refused.
    let apart = [(0x2007_0000, 512), (0x2007_1000, 512)];
    assert_eq!(slot.start(Op::Read, 0, 2, &apart, &mut |_| {}), Err(Error::BadRequest));
    let together = [(0x2007_0000, 512), (0x2007_0200, 512)];
    slot.start(Op::Read, 0, 2, &together, &mut |_| {}).unwrap();
    assert_eq!(finish(&mut slot), (Ok(()), 0));
}

#[test]
fn quirks_are_looked_up_by_vendor_and_device() {
    let r5u823 = crate::quirks::quirks(0x1180, 0xe823);
    assert_eq!(r5u823.name, Some("Ricoh R5U823"));
    assert!(r5u823.broken_adma && r5u823.single_power_write && r5u823.ricoh_mode);
    assert!(!r5u823.reset_after_request);
    let r5c822 = crate::quirks::quirks(0x1180, 0x0822);
    assert!(r5c822.broken_adma && !r5c822.ricoh_mode);
    let jmb38x = crate::quirks::quirks(0x197b, 0x2381);
    assert!(jmb38x.reset_after_request && !jmb38x.broken_adma);
    // An O2 Micro, and anything else not in the table, needs nothing.
    assert_eq!(crate::quirks::quirks(0x1217, 0x8621), Quirks::default());
    assert_eq!(crate::quirks::quirks(0x1b36, 0x0007), Quirks::default());
}

#[test]
fn a_controller_whose_adma2_is_broken_uses_sdma() {
    let broken = Quirks { broken_adma: true, ..Quirks::default() };
    let wide = sdma_caps(true) | 1 << 28;
    let (mut slot, sim) = quirky_slot(version::V3, wide, 0, Some(card(4096, true)), broken);
    assert!(slot.contiguous() && !slot.reaches_64());
    slot.identify(&mut |_| {}).unwrap();
    let page = Dma::fake(4096);
    sim.0.lock().unwrap().aliases.push((0x1000_0000, page.phys(), 4096));
    page.write(0, &pattern(3, 1024));
    slot.start(Op::Write, 3, 2, &[(0x1000_0000, 1024)], &mut |_| {}).unwrap();
    assert_eq!(finish(&mut slot), (Ok(()), 0));
    assert_eq!(
        sim.0.lock().unwrap().card.as_ref().unwrap().data[3 * 512..5 * 512],
        pattern(3, 1024)
    );
    // Without SDMA either, nothing moves data.
    let mut state = State::new(None);
    state.put(reg::CAPABILITIES, 8, sdma_caps(true) & !(1 << 22));
    let sim = Arc::new(Sim(Mutex::new(state)));
    let mut alloc = |_: usize, _: bool| Some(Dma::fake(4096));
    let made = Slot::new(Mmio::fake(sim, REGISTERS), &mut alloc, &mut |_| {}, broken);
    assert!(matches!(made, Err(Error::Unsupported(_))));
}

#[test]
fn power_comes_on_in_one_write_where_the_controller_needs_it() {
    let power = |quirks: Quirks| {
        let (mut slot, sim) =
            quirky_slot(version::V2, sdma_caps(true), 0, Some(card(4096, true)), quirks);
        slot.identify(&mut |_| {}).unwrap();
        let pokes = sim.0.lock().unwrap().pokes.clone();
        pokes
            .into_iter()
            .filter(|&(at, value)| at == reg::POWER_CONTROL && value != 0)
            .collect::<Vec<_>>()
    };
    // 3.3 V (7 << 1), then the power bit with it.
    let at = reg::POWER_CONTROL;
    assert_eq!(power(Quirks::default()), [(at, 0x0e), (at, 0x0f)]);
    let single = Quirks { single_power_write: true, ..Quirks::default() };
    assert_eq!(power(single), [(at, 0x0f)]);
}

#[test]
fn the_lines_are_reset_after_each_request_where_the_controller_needs_it() {
    let resets = |quirks: Quirks| {
        let (mut slot, sim) =
            quirky_slot(version::V2, sdma_caps(true), 0, Some(card(4096, true)), quirks);
        slot.identify(&mut |_| {}).unwrap();
        let page = Dma::fake(4096);
        sim.0.lock().unwrap().aliases.push((0x1000_0000, page.phys(), 4096));
        sim.0.lock().unwrap().pokes.clear();
        for block in [0, 1] {
            slot.start(Op::Read, block, 1, &[(0x1000_0000, 512)], &mut |_| {}).unwrap();
            assert_eq!(finish(&mut slot), (Ok(()), 0));
        }
        sim.0.lock().unwrap().pokes.clone()
    };
    assert_eq!(resets(Quirks::default()), []);
    let jmicron = Quirks { reset_after_request: true, ..Quirks::default() };
    assert_eq!(resets(jmicron), [(reg::SOFTWARE_RESET, 6), (reg::SOFTWARE_RESET, 6)]);
}
