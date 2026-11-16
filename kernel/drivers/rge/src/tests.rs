// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The driver against a simulated chip: registers that act as OpenBSD's
//! `rge(4)` expects them to, with the MAC's and the PHY's registers, the
//! PCI Express PHY's and the configuration space behind their access
//! registers, and a wire whose frames the tests put in and take out
//! through the descriptors in (fake) DMA memory.
//!
//! The model logs every register access and wait. OpenBSD's own
//! functions, compiled against the same model with the start sequence
//! this driver follows (tools/rge-openbsd-check), logged the accesses
//! whose hashes [`openbsd_sequence`] checks: the port makes the same
//! accesses in the same order with the same values, chip by chip, from
//! registers that all start with something in them.

use std::cell::Cell;
use std::fmt::Write as _;
use std::string::String;
use std::sync::{Arc, Mutex};
use std::vec::Vec;

use radix_hw::{Dma, Mmio, Registers, fake_memory, fake_memory_write};
use radix_nic::{Error, Interrupt, Link, Nic, PAGE, SLOT};

use crate::ocp::Ocp;
use crate::reg::{self, cmd, isr, phystat, rxcfg};
use crate::{Card, Kind, Model, identify, start};

const MAC: [u8; 6] = [0x00, 0xe0, 0x4c, 0x68, 0x12, 0x34];
const FALLBACK: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
/// Where the PCI Express capability is in the simulated configuration
/// space, and what its link control register holds: ASPM L0s and L1 and
/// clock power management on.
const EXPRESS: usize = 0x70;
const LINK_CONTROL: u32 = 0x0011_0143;

const OWN: u32 = 0x8000_0000;
const EOR: u32 = 0x4000_0000;
const RX_SOF: u32 = 0x0200_0000;
const RX_EOF: u32 = 0x0100_0000;
const RX_ERROR: u32 = 0x0010_0000;
const TX_SOF: u32 = 0x2000_0000;
const TX_EOF: u32 = 0x1000_0000;

/// Where the RTL8127's EPHY page register (0x0ffe) lands among the
/// EPHY's registers, whose address is seven bits.
const EPHY_PAGE_REGISTER: usize = 0x7e;

const KINDS: [(Kind, u32); 7] = [
    (Kind::R25, 0x6090_0000),
    (Kind::R25B, 0x6410_0000),
    (Kind::R25D1, 0x6880_0000),
    (Kind::R25D2, 0x6890_0000),
    (Kind::R26_1, 0x6490_0000),
    (Kind::R26_2, 0x64a0_0000),
    (Kind::R27, 0x6c90_0000),
];

fn alloc(pages: usize) -> Option<Dma> {
    Some(Dma::fake(pages * PAGE))
}

std::thread_local! {
    static NOW: Cell<u64> = const { Cell::new(0) };
}

fn clock() -> u64 {
    NOW.with(|now| {
        // Every look at the clock takes a microsecond.
        now.set(now.get() + 1_000);
        now.get()
    })
}

fn advance(ns: u64) {
    NOW.with(|now| now.set(now.get() + ns));
}

fn read_mem(phys: u64, len: usize) -> Vec<u8> {
    let mut out = std::vec![0u8; len];
    fake_memory(phys, &mut out);
    out
}

fn u32_at(phys: u64) -> u32 {
    u32::from_le_bytes(read_mem(phys, 4).try_into().unwrap())
}

fn u64_at(phys: u64) -> u64 {
    u64::from_le_bytes(read_mem(phys, 8).try_into().unwrap())
}

struct State {
    regs: Vec<u8>,
    mac: Vec<u16>,
    phy: Vec<u16>,
    ephy: [u16; 0x80],
    /// The page the EPHY's extended address register selected, and the
    /// writes since the chip was made: page, register, value.
    ephy_page: u16,
    ephy_writes: Vec<(u16, usize, u16)>,
    /// The configuration space, in words, as the CSI registers reach it.
    csi: Vec<u32>,
    revision: u32,
    /// Every access and wait, as the C side logs them.
    log: String,
    /// The writes to the MAC's and the PHY's registers: register, value.
    mac_writes: Vec<(u16, u16)>,
    phy_writes: Vec<(u16, u16)>,
    rx_index: usize,
    tx_index: usize,
    sent: Vec<Vec<u8>>,
    /// Whether the chip ignores the transmit start register, as a hung
    /// one would.
    hung: bool,
}

/// Where the fields of a descriptor are, and the bits of a received
/// buffer's status, in the format the chip is set to.
#[derive(Clone, Copy)]
struct Layout {
    stride: u64,
    rx_address: u64,
    rx_status: u64,
    sof: u32,
    eof: u32,
    error: u32,
}

/// What a chip of the second format writes over a received buffer's
/// address.
const HASH: u64 = 0x5eed_0000_dead_beef;

struct Chip {
    state: Mutex<State>,
}

impl Chip {
    fn new(revision: u32, mac: [u8; 6], scrambled: bool) -> Arc<Chip> {
        let mut state = State {
            regs: std::vec![0; reg::END],
            mac: std::vec![0; 0x8000],
            phy: std::vec![0; 0x8000],
            ephy: [0; 0x80],
            ephy_page: 0,
            ephy_writes: Vec::new(),
            csi: std::vec![0; 0x400],
            revision,
            log: String::new(),
            mac_writes: Vec::new(),
            phy_writes: Vec::new(),
            rx_index: 0,
            tx_index: 0,
            sent: Vec::new(),
            hung: false,
        };
        if scrambled {
            // Every register starts with something in it, so that a mask
            // read wrong shows.
            for (i, byte) in state.regs.iter_mut().enumerate() {
                *byte = ((i as u32).wrapping_mul(0x9e37_79b1) >> 24) as u8;
            }
            for (i, word) in state.mac.iter_mut().enumerate() {
                *word = ((i as u32).wrapping_mul(0x85eb_ca6b) >> 16) as u16;
            }
            for (i, word) in state.phy.iter_mut().enumerate() {
                *word = ((i as u32).wrapping_mul(0xc2b2_ae35) >> 16) as u16;
            }
            for (i, word) in state.ephy.iter_mut().enumerate() {
                *word = ((i as u32).wrapping_mul(0x27d4_eb2f) >> 16) as u16;
            }
            for (i, word) in state.csi.iter_mut().enumerate() {
                *word = (i as u32).wrapping_mul(0x1656_67b1);
            }
        }
        // The PHY says it is up, and no SFP+ cage is behind the MAC.
        state.phy[0xa420 >> 1] = 3;
        state.mac[0xd006 >> 1] = 0;
        Chip::put(
            &mut state,
            reg::ADDR0,
            4,
            u64::from(u32::from_le_bytes(mac[..4].try_into().unwrap())),
        );
        Chip::put(&mut state, reg::ADDR1, 2, u64::from(u16::from_le_bytes([mac[4], mac[5]])));
        state.csi[(EXPRESS + 0x10) >> 2] = LINK_CONTROL;
        Arc::new(Chip { state: Mutex::new(state) })
    }

    fn get(state: &State, at: usize, size: usize) -> u64 {
        let mut bytes = [0u8; 8];
        bytes[..size].copy_from_slice(&state.regs[at..at + size]);
        u64::from_le_bytes(bytes)
    }

    fn put(state: &mut State, at: usize, size: usize, value: u64) {
        state.regs[at..at + size].copy_from_slice(&value.to_le_bytes()[..size]);
    }

    fn phy_store(state: &mut State, at: u16, value: u16) {
        // The standard control register's reset is done at once.
        let value = if at == 0xa400 { value & !0x8000 } else { value };
        if at == 0xb820 {
            let patching = if value & 0x0010 != 0 { 0x0040 } else { 0 };
            state.phy[0xb800 >> 1] = (state.phy[0xb800 >> 1] & !0x0040) | patching;
        }
        state.phy[usize::from(at >> 1)] = value;
        state.phy_writes.push((at, value));
    }

    fn written(state: &mut State, at: usize, size: usize, value: u64) {
        let v = value as u32;
        if at == reg::ISR && size == 4 {
            let isr = Chip::get(state, at, 4) & !value;
            Chip::put(state, at, 4, isr);
            return;
        }
        Chip::put(state, at, size, value);
        match (at, size) {
            (reg::CMD, 1) => state.regs[at] &= !cmd::RESET,
            (reg::MACOCP, 4) => {
                let addr = ((v >> 16) & 0x7fff) as usize;
                if v & reg::ocp::BUSY != 0 {
                    state.mac[addr] = v as u16;
                    state.mac_writes.push(((addr << 1) as u16, v as u16));
                    Chip::put(state, at, 4, u64::from(v & !reg::ocp::BUSY));
                } else {
                    let read = (v & 0xffff_0000) | u32::from(state.mac[addr]);
                    Chip::put(state, at, 4, u64::from(read));
                }
            }
            (reg::PHYOCP, 4) => {
                let addr = ((v >> 16) & 0x7fff) as usize;
                if v & reg::ocp::BUSY != 0 {
                    Chip::phy_store(state, (addr << 1) as u16, v as u16);
                    Chip::put(state, at, 4, u64::from(v & !reg::ocp::BUSY));
                } else {
                    let read = reg::ocp::BUSY | (v & 0x7fff_0000) | u32::from(state.phy[addr]);
                    Chip::put(state, at, 4, u64::from(read));
                }
            }
            (reg::EPHYAR, 4) => {
                let addr = ((v >> 16) & 0x7f) as usize;
                if v & reg::ephyar::BUSY != 0 {
                    state.ephy[addr] = v as u16;
                    if addr == EPHY_PAGE_REGISTER {
                        state.ephy_page = v as u16;
                    } else {
                        let page = state.ephy_page;
                        state.ephy_writes.push((page, addr, v as u16));
                    }
                    Chip::put(state, at, 4, u64::from(v & !reg::ephyar::BUSY));
                } else {
                    let read = reg::ephyar::BUSY | (v & 0x7fff_0000) | u32::from(state.ephy[addr]);
                    Chip::put(state, at, 4, u64::from(read));
                }
            }
            (reg::CSIAR, 4) => {
                let addr = ((v & 0xfff) >> 2) as usize;
                if v & reg::csiar::BUSY != 0 {
                    state.csi[addr] = Chip::get(state, reg::CSIDR, 4) as u32;
                    Chip::put(state, at, 4, u64::from(v & !reg::csiar::BUSY));
                } else {
                    let data = u64::from(state.csi[addr]);
                    Chip::put(state, reg::CSIDR, 4, data);
                    Chip::put(state, at, 4, u64::from(v | reg::csiar::BUSY));
                }
            }
            (reg::TXSTART, 2) if v & 1 != 0 && !state.hung => Chip::transmit(state),
            _ => {}
        }
    }

    fn ring(state: &State, lo: usize) -> u64 {
        Chip::get(state, lo, 4) | (Chip::get(state, lo + 4, 4) << 32)
    }

    /// The descriptor format the chip's registers select: in the receive
    /// configuration and again in the MAC's register 0xeb58, which the
    /// driver must set alike.
    fn layout(state: &State) -> Layout {
        let second = Chip::get(state, reg::RXCFG, 4) as u32 & rxcfg::ENBDESCV1 == 0;
        assert_eq!(
            state.mac[0xeb58 >> 1] & 3,
            if second { 2 } else { 1 },
            "the descriptor format is selected the same way twice"
        );
        if second {
            Layout {
                stride: 16,
                rx_address: 0,
                rx_status: 12,
                sof: 0x2000_0000,
                eof: 0x1000_0000,
                error: 0x0040_0000,
            }
        } else {
            Layout {
                stride: 32,
                rx_address: 16,
                rx_status: 28,
                sof: RX_SOF,
                eof: RX_EOF,
                error: RX_ERROR,
            }
        }
    }

    fn transmit(state: &mut State) {
        let base = Chip::ring(state, reg::TXDESC_ADDR_LO);
        let layout = Chip::layout(state);
        loop {
            let at = base + (state.tx_index as u64) * layout.stride;
            let command = u32_at(at);
            if command & OWN == 0 {
                break;
            }
            assert_eq!(command & (TX_SOF | TX_EOF), TX_SOF | TX_EOF, "one buffer a frame");
            let len = (command & 0xffff) as usize;
            state.sent.push(read_mem(u64_at(at + 8), len));
            fake_memory_write(at, &(command & !OWN).to_le_bytes());
            state.tx_index = if command & EOR != 0 { 0 } else { state.tx_index + 1 };
            let raised = Chip::get(state, reg::ISR, 4) | u64::from(isr::TX_OK);
            Chip::put(state, reg::ISR, 4, raised);
        }
    }

    /// Puts `frame` on the wire to the chip, with its frame check
    /// sequence and `status` in its descriptors, in as many buffers as it
    /// needs: whether the chip had them.
    fn deliver(&self, frame: &[u8], status: u32) -> bool {
        let mut state = self.state.lock().unwrap();
        if Chip::get(&state, reg::CMD, 1) as u8 & cmd::RXENB == 0 {
            return false;
        }
        let base = Chip::ring(&state, reg::RXDESC_ADDR_LO);
        let layout = Chip::layout(&state);
        // Whatever the format calls an error.
        let error = if status & RX_ERROR != 0 { layout.error } else { 0 };
        assert_eq!(status & !RX_ERROR, 0);
        let mut data = frame.to_vec();
        data.extend_from_slice(&[0xfc, 0xfc, 0xfc, 0xfc]);
        let mut offset = 0;
        let mut first = true;
        while offset < data.len() {
            let at = base + (state.rx_index as u64) * layout.stride;
            let command = u32_at(at + layout.rx_status);
            if command & OWN == 0 {
                return false;
            }
            let room = (command & 0x3fff) as usize;
            let piece = room.min(data.len() - offset);
            fake_memory_write(u64_at(at + layout.rx_address), &data[offset..offset + piece]);
            if layout.stride == 16 {
                // The second format has the frame's hash where the buffer's
                // address was.
                fake_memory_write(at, &HASH.to_le_bytes());
            }
            offset += piece;
            let mut written = (command & EOR) | error;
            if first {
                written |= layout.sof;
            }
            written |= if offset == data.len() { layout.eof } else { 0 };
            written |= piece as u32;
            fake_memory_write(at + layout.rx_status, &written.to_le_bytes());
            state.rx_index = if command & EOR != 0 { 0 } else { state.rx_index + 1 };
            first = false;
        }
        let raised = Chip::get(&state, reg::ISR, 4) | u64::from(isr::RX_OK);
        Chip::put(&mut state, reg::ISR, 4, raised);
        true
    }

    fn sent(&self) -> Vec<Vec<u8>> {
        core::mem::take(&mut self.state.lock().unwrap().sent)
    }

    fn reg(&self, at: usize, size: usize) -> u64 {
        Chip::get(&self.state.lock().unwrap(), at, size)
    }

    fn set_reg(&self, at: usize, size: usize, value: u64) {
        Chip::put(&mut self.state.lock().unwrap(), at, size, value);
    }

    fn log(&self) -> String {
        core::mem::take(&mut self.state.lock().unwrap().log)
    }
}

impl Registers for Chip {
    fn read(&self, at: usize, size: usize) -> u64 {
        let mut state = self.state.lock().unwrap();
        let mut value = Chip::get(&state, at, size);
        match (at, size) {
            (reg::MCUCMD, 1) => value |= 0x30,
            (reg::IM, 2) => value |= 0x0103,
            (reg::TWICMD, 2) => value |= 0x0200,
            (reg::TXCFG, 4) => {
                value = (value & !u64::from(reg::TXCFG_HWREV)) | u64::from(state.revision);
            }
            _ => {}
        }
        let _ = writeln!(state.log, "r{size} {at:04x} {value:x}");
        value
    }

    fn write(&self, at: usize, size: usize, value: u64) {
        let mut state = self.state.lock().unwrap();
        // The rings' addresses differ from run to run.
        let ring = matches!(
            at,
            reg::RXDESC_ADDR_LO | reg::RXDESC_ADDR_HI | reg::TXDESC_ADDR_LO | reg::TXDESC_ADDR_HI
        );
        let logged = if ring { 0 } else { value };
        let _ = writeln!(state.log, "w{size} {at:04x} {logged:x}");
        Chip::written(&mut state, at, size, value);
    }
}

/// A chip of revision `revision`, its registers, and a delay that logs
/// its waits.
fn chip(revision: u32, scrambled: bool) -> (Arc<Chip>, Mmio) {
    let chip = Chip::new(revision, MAC, scrambled);
    let regs = Mmio::fake(chip.clone(), reg::END);
    (chip, regs)
}

fn delay_into(chip: &Arc<Chip>) -> impl FnMut(u32) + use<> {
    let chip = chip.clone();
    move |us| {
        let _ = writeln!(chip.state.lock().unwrap().log, "d {us}");
    }
}

fn started(revision: u32, interrupt: Interrupt) -> (Arc<Chip>, Card) {
    started_on(revision, interrupt, false)
}

fn started_on(revision: u32, interrupt: Interrupt, scrambled: bool) -> (Arc<Chip>, Card) {
    let (chip, regs) = chip(revision, scrambled);
    let model = identify(crate::VENDOR, 0x8125).unwrap();
    let mut delay = delay_into(&chip);
    let card =
        start(model, regs, Some(EXPRESS), interrupt, FALLBACK, &mut alloc, &mut delay, clock)
            .unwrap();
    (chip, card)
}

/// FNV-1a, 64 bits.
fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[test]
fn identifies_the_cards() {
    assert_eq!(identify(0x10ec, 0x8125), Some(Model { name: None }));
    assert_eq!(identify(0x10ec, 0x8126), Some(Model { name: None }));
    assert_eq!(identify(0x10ec, 0x3000), Some(Model { name: Some("Killer E3000") }));
    assert_eq!(identify(0x10ec, 0x8127), Some(Model { name: None }));
    assert_eq!(identify(0x10ec, 0x5000), Some(Model { name: Some("Killer E5000") }));
    // The gigabit chips are not this driver's.
    assert_eq!(identify(0x10ec, 0x8168), None);
    assert_eq!(identify(0x8086, 0x8125), None);
}

#[test]
fn tells_the_chips_apart() {
    for (kind, revision) in KINDS {
        // The revision's bits alone, whatever else the register holds.
        assert_eq!(Kind::from_txcfg(revision | 0x0300_0700), Some(kind));
    }
    // A revision the driver does not know.
    assert_eq!(Kind::from_txcfg(0x7000_0000), None);
    assert_eq!(Kind::R25.name(), "Realtek RTL8125A");
    assert_eq!(Kind::R26_2.name(), "Realtek RTL8126A");
    assert_eq!(Kind::R27.name(), "Realtek RTL8127");
}

#[test]
fn reaches_the_standard_phy_registers() {
    assert_eq!(Ocp::mii(reg::mii::BMCR), 0xa400);
    assert_eq!(Ocp::mii(reg::mii::ANAR), 0xa408);
    assert_eq!(Ocp::mii(reg::mii::GTCR), 0xa412);
}

/// The FNV-1a hashes and lengths of the logs OpenBSD's functions wrote,
/// chip by chip, against this model (`make_harness.py hash` prints them):
/// see the module's documentation.
const OPENBSD: [(Kind, u64, usize); 7] = [
    (Kind::R25, 0x717cc8f6e03ecdcc, 1239),
    (Kind::R25B, 0xb1767a06123f4be7, 751),
    (Kind::R25D1, 0xb3352f5a1d0bed78, 1267),
    (Kind::R25D2, 0xaa0d6d467740d319, 802),
    (Kind::R26_1, 0x9675b32ca4bd3f6b, 1553),
    (Kind::R26_2, 0xa140f81a2b1729dc, 1097),
    (Kind::R27, 0xfe5286f92247e5fb, 1878),
];

#[test]
fn openbsd_sequence() {
    for (kind, revision) in KINDS {
        let (chip, card) = started_on(revision, Interrupt::Msi, true);
        assert_eq!(card.kind(), kind);
        let log = chip.log();
        let (_, hash, lines) = OPENBSD.iter().find(|(k, ..)| *k == kind).unwrap();
        assert_eq!((fnv(&log), log.lines().count()), (*hash, *lines), "{kind:?}");
    }
}

impl Chip {
    fn mac_reg(&self, at: u16) -> u16 {
        self.state.lock().unwrap().mac[usize::from(at >> 1)]
    }

    fn phy_reg(&self, at: u16) -> u16 {
        self.state.lock().unwrap().phy[usize::from(at >> 1)]
    }

    fn set_mac_reg(&self, at: u16, value: u16) {
        self.state.lock().unwrap().mac[usize::from(at >> 1)] = value;
    }

    fn csi_reg(&self, at: usize) -> u32 {
        self.state.lock().unwrap().csi[at >> 2]
    }

    fn hang(&self) {
        self.state.lock().unwrap().hung = true;
    }
}

/// Registers that read as all ones, as a card that is gone does.
struct Gone;

impl Registers for Gone {
    fn read(&self, _at: usize, _size: usize) -> u64 {
        u64::MAX
    }

    fn write(&self, _at: usize, _size: usize, _value: u64) {}
}

fn try_start(regs: Mmio) -> Result<Card, Error> {
    let model = identify(crate::VENDOR, 0x8125).unwrap();
    start(model, regs, None, Interrupt::Msi, FALLBACK, &mut alloc, &mut |_| {}, clock)
}

fn frame(len: usize, seed: u8) -> Vec<u8> {
    let mut frame: Vec<u8> =
        (0..len).map(|i| (i as u8).wrapping_mul(7).wrapping_add(seed)).collect();
    frame[..6].copy_from_slice(&MAC);
    frame
}

#[test]
fn refuses_what_it_does_not_drive() {
    // A revision the driver does not know.
    let (_, regs) = chip(0x7000_0000, false);
    assert!(matches!(try_start(regs), Err(Error::Unsupported(_))));
    assert!(matches!(try_start(Mmio::fake(Arc::new(Gone), reg::END)), Err(Error::Device(_))));
    let (chip, _) = chip(0x6410_0000, false);
    assert!(matches!(try_start(Mmio::fake(chip, 0x1000)), Err(Error::Registers)));
}

#[test]
fn starts_with_the_address_its_bootcode_left() {
    let (chip, card) = started(0x6410_0000, Interrupt::Msi);
    let info = card.info();
    assert_eq!(info.mac, MAC);
    assert_eq!(info.model, "Realtek RTL8125B");
    assert_eq!((info.receive_slots, info.transmit_slots), (256, 255));
    let low = chip.reg(reg::MAC0, 4) as u32;
    let high = chip.reg(reg::MAC4, 2) as u16;
    assert_eq!(low.to_le_bytes(), MAC[..4]);
    assert_eq!(high.to_le_bytes(), MAC[4..]);
    // Configuration writes are locked again.
    assert_eq!(chip.reg(reg::EECMD, 1) as u8 & reg::EECMD_WRITECFG, 0);
}

#[test]
fn names_killers_cards_by_their_name() {
    let (chip, regs) = chip(0x6490_0000, false);
    let model = identify(crate::VENDOR, 0x5000).unwrap();
    let mut delay = delay_into(&chip);
    let card =
        start(model, regs, None, Interrupt::Msi, FALLBACK, &mut alloc, &mut delay, clock).unwrap();
    assert_eq!(card.info().model, "Killer E5000");
    assert_eq!(card.kind(), Kind::R26_1);
}

#[test]
fn falls_back_without_an_address() {
    let chip = Chip::new(0x6410_0000, [0; 6], false);
    let card = try_start(Mmio::fake(chip.clone(), reg::END)).unwrap();
    assert_eq!(card.info().mac, FALLBACK);
    assert_eq!(
        chip.reg(reg::MAC0, 4) as u32,
        u32::from_le_bytes(FALLBACK[..4].try_into().unwrap())
    );
}

#[test]
fn turns_the_links_power_states_off() {
    let (chip, _card) = started(0x6410_0000, Interrupt::Msi);
    assert_eq!(chip.csi_reg(EXPRESS + 0x10), LINK_CONTROL & !0x0103);
}

#[test]
fn loads_no_firmware() {
    for (kind, revision) in KINDS {
        let (chip, _card) = started(revision, Interrupt::Msi);
        let state = chip.state.lock().unwrap();
        // No MAC MCU patch: nothing in its RAM, its breakpoints off.
        assert!(
            !state.mac_writes.iter().any(|&(at, _)| (0xf800..=0xf9fe).contains(&at)),
            "{kind:?}"
        );
        assert_eq!(state.mac[0xfc26 >> 1], 0, "{kind:?}");
        assert_eq!(state.mac[0xfc48 >> 1], 0, "{kind:?}");
        // No PHY MCU patch: its RAM code version is never written.
        assert!(!state.phy_writes.windows(2).any(|w| w[0] == (0xa436, 0x801e)), "{kind:?}");
    }
}

#[test]
fn starts_receiving_and_sending() {
    for (kind, revision) in KINDS {
        let (chip, card) = started(revision, Interrupt::Msi);
        assert_eq!(chip.reg(reg::CMD, 1) as u8, cmd::TXENB | cmd::RXENB, "{kind:?}");
        let filters = chip.reg(reg::RXCFG, 4) as u32;
        let wanted = rxcfg::INDIV | rxcfg::BROAD | rxcfg::MULTI;
        assert_eq!(filters & rxcfg::FILTERS, wanted, "{kind:?}");
        assert_eq!(chip.reg(reg::MAR0, 4), 0xffff_ffff);
        assert_eq!(chip.reg(reg::MAR4, 4), 0xffff_ffff);
        let longest = if matches!(kind, Kind::R25 | Kind::R25B) { 0 } else { 0x8000 };
        assert_eq!(chip.reg(reg::RXMAXSIZE, 2), (SLOT | longest) as u64, "{kind:?}");
        assert_eq!(chip.reg(reg::IMR, 4) as u32, isr::RAISED);
        assert_ne!(chip.reg(reg::RXDESC_ADDR_LO, 4), 0);
        assert_ne!(chip.reg(reg::TXDESC_ADDR_LO, 4), 0);
        drop(card);
    }
    let (chip, _card) = started(0x6410_0000, Interrupt::Polled);
    assert_eq!(chip.reg(reg::IMR, 4), 0);
}

#[test]
fn advertises_every_speed_with_pause_frames() {
    let (chip, _card) = started(0x6410_0000, Interrupt::Msi);
    assert_eq!(chip.phy_reg(0xa5d4) & (reg::ADV_2500TFDX | reg::ADV_5000TFDX), reg::ADV_2500TFDX);
    let (chip, _card) = started(0x64a0_0000, Interrupt::Msi);
    let fast = reg::ADV_2500TFDX | reg::ADV_5000TFDX;
    assert_eq!(chip.phy_reg(0xa5d4) & fast, fast);
    use reg::mii;
    let anar = mii::ANAR_TX_FD | mii::ANAR_TX | mii::ANAR_10_FD | mii::ANAR_10;
    assert_eq!(chip.phy_reg(0xa408), anar | mii::ANAR_FC | mii::ANAR_PAUSE_ASYM);
    assert_eq!(chip.phy_reg(0xa412), mii::GTCR_ADV_1000TFDX | mii::GTCR_ADV_1000THDX);
    // Negotiating, its reset done.
    assert_eq!(chip.phy_reg(0xa400), mii::BMCR_AUTOEN | mii::BMCR_STARTNEG);
}

#[test]
fn says_the_link_its_phy_status_says() {
    let (chip, card) = started(0x64a0_0000, Interrupt::Msi);
    let link = |status: u16| {
        chip.set_reg(reg::PHYSTAT, 2, u64::from(status));
        card.link()
    };
    let up = |mbps, full_duplex| Link { up: true, mbps, full_duplex };
    use phystat::*;
    assert_eq!(link(LINK | FDX | MBPS_5000), up(5000, true));
    assert_eq!(link(LINK | FDX | MBPS_2500), up(2500, true));
    assert_eq!(link(LINK | FDX | MBPS_1000), up(1000, true));
    assert_eq!(link(LINK | MBPS_100), up(100, false));
    assert_eq!(link(LINK | MBPS_10), up(10, false));
    assert_eq!(link(FDX | MBPS_1000), Link::default());
    assert_eq!(link(u16::MAX), Link::default());
}

#[test]
fn receives_frames() {
    let (chip, mut card) = started(0x6410_0000, Interrupt::Msi);
    let mut buffer = [0u8; 2048];
    assert_eq!(card.receive(&mut buffer), None);
    // Around the ring more than once.
    for n in 0..600 {
        let sent = frame(60 + n % 1400, n as u8);
        assert!(chip.deliver(&sent, 0));
        assert_eq!(card.receive(&mut buffer), Some(sent.len()), "frame {n}");
        assert_eq!(&buffer[..sent.len()], &sent[..]);
    }
    assert_eq!(card.dropped(), 0);
    // One the chip found wrong, and one too long for a slot, in two.
    assert!(chip.deliver(&frame(100, 1), RX_ERROR));
    assert!(chip.deliver(&frame(3000, 2), 0));
    let good = frame(64, 3);
    assert!(chip.deliver(&good, 0));
    assert_eq!(card.receive(&mut buffer), Some(good.len()));
    assert_eq!(&buffer[..good.len()], &good[..]);
    assert_eq!(card.dropped(), 3);
}

#[test]
fn sends_frames_padded() {
    let (chip, mut card) = started(0x6410_0000, Interrupt::Msi);
    let short = frame(42, 9);
    assert!(card.can_send());
    assert!(card.send(&short));
    assert!(chip.sent().is_empty(), "not before the flush");
    card.flush();
    let sent = chip.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(&sent[0][..42], &short[..]);
    assert_eq!(&sent[0][42..], &[0u8; 18]);
    // Around the ring more than once, a few at a time.
    for round in 0..100 {
        for n in 0..5 {
            assert!(card.send(&frame(1514, round + n)), "round {round}");
        }
        card.flush();
        assert_eq!(chip.sent().len(), 5);
    }
    // Neither too short nor too long a frame.
    assert!(!card.send(&[0u8; 13]));
    assert!(!card.send(&[0u8; 1519]));
}

#[test]
fn fills_its_transmit_ring_and_no_more() {
    let (chip, mut card) = started(0x6410_0000, Interrupt::Msi);
    chip.hang();
    for n in 0..255 {
        assert!(card.send(&frame(60, n as u8)), "frame {n}");
    }
    assert!(!card.can_send());
    assert!(!card.send(&frame(60, 0)));
}

#[test]
fn acknowledges_its_interrupt_but_a_pci_error() {
    let (chip, mut card) = started(0x6410_0000, Interrupt::Msi);
    let mut buffer = [0u8; 2048];
    chip.set_reg(reg::ISR, 4, u64::from(isr::TX_OK | isr::LINKCHG));
    assert_eq!(card.receive(&mut buffer), None);
    assert_eq!(chip.reg(reg::ISR, 4), 0);
    assert!(!card.failed());
    chip.set_reg(reg::ISR, 4, u64::from(isr::SYSTEM_ERR | isr::RX_OK));
    assert_eq!(card.receive(&mut buffer), None);
    assert_eq!(chip.reg(reg::ISR, 4) as u32, isr::SYSTEM_ERR);
    assert!(card.failed());
    // Polled, the causes are left alone.
    let (chip, mut card) = started(0x6410_0000, Interrupt::Polled);
    chip.set_reg(reg::ISR, 4, u64::from(isr::TX_OK));
    assert_eq!(card.receive(&mut buffer), None);
    assert_eq!(chip.reg(reg::ISR, 4) as u32, isr::TX_OK);
}

#[test]
fn has_failed_when_gone() {
    let (chip, card) = started(0x6410_0000, Interrupt::Msi);
    assert!(!card.failed());
    chip.set_reg(reg::ISR, 4, 0xffff_ffff);
    assert!(card.failed());
}

#[test]
fn has_failed_when_it_sends_nothing_for_five_seconds() {
    let (chip, mut card) = started(0x6410_0000, Interrupt::Msi);
    let mut buffer = [0u8; 2048];
    // Idle, it has not.
    let _ = card.receive(&mut buffer);
    advance(6_000_000_000);
    let _ = card.receive(&mut buffer);
    assert!(!card.failed());
    // Frames that wait.
    chip.hang();
    assert!(card.send(&frame(60, 1)));
    card.flush();
    let _ = card.receive(&mut buffer);
    advance(4_000_000_000);
    let _ = card.receive(&mut buffer);
    assert!(!card.failed());
    advance(1_000_000_000);
    let _ = card.receive(&mut buffer);
    assert!(card.failed());
}

#[test]
fn stops() {
    let (chip, mut card) = started(0x6410_0000, Interrupt::Msi);
    let _ = chip.log();
    card.stop();
    let log = chip.log();
    assert!(log.contains("w1 0037 10\n"), "reset");
    assert_eq!(chip.reg(reg::RXCFG, 4) as u32 & rxcfg::FILTERS, 0);
    assert_eq!(chip.reg(reg::IMR, 4), 0);
    assert_eq!(chip.mac_reg(0xc0ac) & 0x1f80, 0);
    let mut buffer = [0u8; 2048];
    // Its receiver is off.
    assert!(!chip.deliver(&frame(60, 1), 0));
    assert_eq!(card.receive(&mut buffer), None);
    assert!(!card.send(&frame(60, 1)));
    // Once.
    card.stop();
    assert!(chip.log().is_empty());
    drop(card);
    assert!(chip.log().is_empty());
}

#[test]
fn stops_when_dropped() {
    let (chip, card) = started(0x6410_0000, Interrupt::Msi);
    let _ = chip.log();
    drop(card);
    assert!(chip.log().contains("w1 0037 10\n"));
}

#[test]
fn refuses_the_rtl8127_with_an_sfp_cage() {
    let (chip, regs) = chip(0x6c90_0000, false);
    chip.set_mac_reg(0xd006, 0x0007);
    let refused = try_start(regs);
    assert!(
        matches!(refused, Err(Error::Unsupported(what)) if what.starts_with("the RTL8127ATF")),
        "{:?}",
        refused.err()
    );
    // It is left as it was found: not reset, no rings given.
    assert_eq!(chip.reg(reg::CMD, 1), 0);
    assert_eq!(chip.reg(reg::RXDESC_ADDR_LO, 4), 0);
}

#[test]
fn starts_the_rtl8127_without_a_cage() {
    let (_, card) = started(0x6c90_0000, Interrupt::Msi);
    assert_eq!(card.kind(), Kind::R27);
    assert_eq!(card.info().model, "Realtek RTL8127");
    assert_eq!((card.info().receive_slots, card.info().transmit_slots), (256, 255));
}

#[test]
fn selects_the_second_descriptor_format_on_the_rtl8127_alone() {
    for (kind, revision) in KINDS {
        let (chip, _card) = started_on(revision, Interrupt::Msi, true);
        let second = kind == Kind::R27;
        assert_eq!(chip.reg(reg::RXCFG, 4) as u32 & rxcfg::ENBDESCV1 == 0, second, "{kind:?}");
        if second {
            assert_eq!(chip.mac_reg(0xeb58) & 3, 2);
        } else {
            assert_eq!(chip.mac_reg(0xeb58) & 1, 1, "{kind:?}");
        }
        if matches!(kind, Kind::R26_1 | Kind::R26_2 | Kind::R27) {
            assert_eq!(chip.reg(0xd8, 1) & 2 != 0, second, "{kind:?}");
        }
    }
}

#[test]
fn sets_the_rtl8127_up_for_one_vector() {
    let (chip, _card) = started_on(0x6c90_0000, Interrupt::Msi, true);
    let off = u64::from(reg::int_cfg0::EN | reg::int_cfg0::AUTO_CLEAR_IMR);
    assert_eq!(chip.reg(reg::INT_CFG0, 1) & (off | u64::from(reg::int_cfg0::AVOID_MISS_INTR)), 0);
    // Of the FIFO protection register's eight bits, only the
    // lowest is cleared, though OpenBSD asks for more.
    assert_eq!(chip.reg(reg::RADMFIFO_PROTECT, 1) & 1, 0);
    assert_eq!(chip.mac_reg(0xe614) & 0x0f00, 0x0f00);
    assert_eq!(chip.mac_reg(0xd40c) & 0xe038, 0x8020);
    assert_eq!(chip.mac_reg(0xe00c) & 0x1000, 0);
    assert_eq!(chip.mac_reg(0xc0c2) & 0x0040, 0);
    assert_eq!(chip.mac_reg(0xea1c) & 0x0304, 0);
}

#[test]
fn writes_the_rtl8127s_ephy_through_its_pages() {
    let (chip, _card) = started(0x6c90_0000, Interrupt::Msi);
    let state = chip.state.lock().unwrap();
    assert_eq!(state.ephy_writes.len(), 36);
    // 0x8088 and 0x9c1c: the top four bits are the page, and the EPHY
    // keeps seven bits of the rest.
    assert_eq!(state.ephy_writes[0], (8, 0x08, 0x0064));
    assert_eq!(state.ephy_writes[35], (9, 0x1c, 0x0140));
    assert_eq!(state.ephy_page, 0);
    // The chips before it have no EPHY pages.
    drop(state);
    let (chip, _card) = started(0x6410_0000, Interrupt::Msi);
    assert!(chip.state.lock().unwrap().ephy_writes.iter().all(|&(page, ..)| page == 0));
}

#[test]
fn advertises_ten_gigabits_on_the_rtl8127() {
    let (chip, _card) = started(0x6c90_0000, Interrupt::Msi);
    let fast = reg::ADV_2500TFDX | reg::ADV_5000TFDX | reg::ADV_10000TFDX;
    assert_eq!(chip.phy_reg(0xa5d4) & fast, fast);
    // Its energy efficient Ethernet is off at 10 Gb/s too.
    assert_eq!(chip.phy_reg(0xa5d0) & 0x000e, 0);
    assert_eq!(chip.phy_reg(0xa6d4) & 0x0003, 0);
    // The chips before it do not.
    let (chip, _card) = started(0x64a0_0000, Interrupt::Msi);
    assert_eq!(chip.phy_reg(0xa5d4) & reg::ADV_10000TFDX, 0);
}

#[test]
fn says_a_ten_gigabit_link_on_the_rtl8127() {
    let (chip, card) = started(0x6c90_0000, Interrupt::Msi);
    let link = |status: u16| {
        chip.set_reg(reg::PHYSTAT, 2, u64::from(status));
        card.link()
    };
    let up = |mbps, full_duplex| Link { up: true, mbps, full_duplex };
    use phystat::*;
    assert_eq!(link(LINK | FDX | MBPS_10000), up(10000, true));
    assert_eq!(link(LINK | FDX | MBPS_5000), up(5000, true));
    assert_eq!(link(LINK | FDX | MBPS_2500), up(2500, true));
    // The fastest speed the status holds wins.
    assert_eq!(link(LINK | FDX | MBPS_10000 | MBPS_5000), up(10000, true));
    assert_eq!(link(FDX | MBPS_10000), Link::default());
}

#[test]
fn receives_frames_through_the_rtl8127s_rings() {
    let (chip, mut card) = started(0x6c90_0000, Interrupt::Msi);
    let mut buffer = [0u8; 2048];
    assert_eq!(card.receive(&mut buffer), None);
    // Around the ring more than once, each frame whole in a buffer, the
    // chip having written its hash over the buffer's address each time.
    for n in 0..600 {
        let sent = frame(60 + n % 1400, n as u8);
        assert!(chip.deliver(&sent, 0));
        assert_eq!(card.receive(&mut buffer), Some(sent.len()), "frame {n}");
        assert_eq!(&buffer[..sent.len()], &sent[..]);
    }
    assert_eq!(card.dropped(), 0);
    // One the chip found wrong, and one too long for a slot, in two.
    assert!(chip.deliver(&frame(100, 1), RX_ERROR));
    assert!(chip.deliver(&frame(3000, 2), 0));
    let good = frame(64, 3);
    assert!(chip.deliver(&good, 0));
    assert_eq!(card.receive(&mut buffer), Some(good.len()));
    assert_eq!(&buffer[..good.len()], &good[..]);
    assert_eq!(card.dropped(), 3);
}

#[test]
fn sends_frames_through_the_rtl8127s_ring() {
    let (chip, mut card) = started(0x6c90_0000, Interrupt::Msi);
    let short = frame(42, 9);
    assert!(card.send(&short));
    assert!(chip.sent().is_empty(), "not before the flush");
    card.flush();
    let sent = chip.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(&sent[0][..42], &short[..]);
    assert_eq!(&sent[0][42..], &[0u8; 18]);
    // Around the ring more than once, a few at a time.
    for round in 0..100 {
        let frames: Vec<Vec<u8>> = (0..5).map(|n| frame(1514, round + n)).collect();
        for frame in &frames {
            assert!(card.send(frame), "round {round}");
        }
        card.flush();
        assert_eq!(chip.sent(), frames);
    }
    // The ring holds one less than its descriptors.
    chip.hang();
    for n in 0..255 {
        assert!(card.send(&frame(60, n as u8)), "frame {n}");
    }
    assert!(!card.can_send());
}
