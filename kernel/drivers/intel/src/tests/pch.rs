// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The driver of the Ethernet in the chipset against a simulated 82579
//! and I219: the MAC's registers, a PHY behind MDIC with its pages at
//! two addresses, the chipset's flash behind its registers, and a
//! management engine that may hold the PHY, block its resets, keep it in
//! ultra low power mode or hold the bus a write needs.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::vec::Vec;

use radix_hw::pci::{Address, CONFIG_SIZE, Function};
use radix_hw::{Mmio, Registers};
use radix_nic::{Error, Nic};

use super::{alloc, no_delay, read_mem, u64_at, write_mem};
use crate::pch::{self, Mac, Platform};
use crate::{Card as Running, Interrupt, Kind, Model};

const MAC: [u8; 6] = [0x00, 0x1c, 0xc0, 0x12, 0x34, 0x56];
const FALLBACK: [u8; 6] = [0x02, 0, 0, 0, 0, 1];

const CTRL: usize = 0x0000;
const STATUS: usize = 0x0008;
const STRAP: usize = 0x000c;
const CTRL_EXT: usize = 0x0018;
const MDIC: usize = 0x0020;
const FEXTNVM4: usize = 0x0024;
const FEXTNVM: usize = 0x0028;
const KMRNCTRLSTA: usize = 0x0034;
const LTRV: usize = 0x00f8;
const SVT: usize = 0x00f4;
const ICR: usize = 0x00c0;
const IMS: usize = 0x00d0;
const IMC: usize = 0x00d8;
const RCTL: usize = 0x0100;
const TCTL: usize = 0x0400;
const TIPG: usize = 0x0410;
const LEDCTL: usize = 0x0e00;
const EXTCNF_CTRL: usize = 0x0f00;
const EXTCNF_SIZE: usize = 0x0f08;
const PHY_CTRL: usize = 0x0f10;
const IOSFPC: usize = 0x0f28;
const PBA: usize = 0x1000;
const RDBAL: usize = 0x2800;
const RDLEN: usize = 0x2808;
const RDH: usize = 0x2810;
const RDT: usize = 0x2818;
const TDBAL: usize = 0x3800;
const TDLEN: usize = 0x3808;
const TDH: usize = 0x3810;
const TDT: usize = 0x3818;
const TARC0: usize = 0x3840;
const RAL0: usize = 0x5400;
const RAH0: usize = 0x5404;
const H2ME: usize = 0x5b50;
const FWSM: usize = 0x5b54;
const FEXTNVM11: usize = 0x5bbc;
const FLASH: usize = 0xe000;
const SIZE: usize = 0x2_0000;

const CTRL_GIO_MASTER_DISABLE: u32 = 1 << 2;
const CTRL_LANPHYPC_OVERRIDE: u32 = 1 << 16;
const CTRL_LANPHYPC_VALUE: u32 = 1 << 17;
const CTRL_RST: u32 = 1 << 26;
const CTRL_PHY_RST: u32 = 1 << 31;
const STATUS_LAN_INIT_DONE: u32 = 1 << 9;
const STATUS_PHYRA: u32 = 1 << 10;
const STATUS_GIO_MASTER_ENABLE: u32 = 1 << 19;
const CTRL_EXT_LPCD: u32 = 1 << 2;
const CTRL_EXT_FORCE_SMBUS: u32 = 1 << 11;
const CTRL_EXT_DRV_LOAD: u32 = 1 << 28;
const SWFLAG: u32 = 1 << 5;
const FWSM_RSPCIPHY: u32 = 1 << 6;
const FWSM_ULP_CFG_DONE: u32 = 1 << 10;
const FWSM_FW_VALID: u32 = 1 << 15;
const FWSM_PCIM2PCI: u32 = 1 << 24;
const H2ME_ULP: u32 = 1 << 11;
const H2ME_ENFORCE_SETTINGS: u32 = 1 << 12;

/// The PHY's registers, as (page, number).
const PHY_CONTROL: (u32, u32) = (0, 0);
const PHY_ANAR: (u32, u32) = (0, 4);
const PHY_1000T: (u32, u32) = (0, 9);
const PHY_CTRL_2: (u32, u32) = (0, 18);
const PHY_CFG: (u32, u32) = (0, 22);
const OEM_BITS: (u32, u32) = (768, 25);
const SMB_ADDR: (u32, u32) = (768, 26);
const LED_CONFIG: (u32, u32) = (768, 30);
const KMRN_MODE_CTRL: (u32, u32) = (769, 16);
const SMB_CTRL: (u32, u32) = (769, 23);
const KMRN_FIFO: (u32, u32) = (770, 16);
const PM_CTRL: (u32, u32) = (770, 17);
const PLL_GATE: (u32, u32) = (772, 28);
const TX_PTR_GAP: (u32, u32) = (776, 20);
const ULP_CONFIG1: (u32, u32) = (779, 16);

/// The flash registers.
const HSFSTS: usize = 0x04;
const HSFCTL: usize = 0x06;
const FADDR: usize = 0x08;
const FDATA0: usize = 0x10;
const FLCDONE: u16 = 1 << 0;
const FLCERR: u16 = 1 << 1;
const DAEL: u16 = 1 << 2;
const FLDESVALID: u16 = 1 << 14;

/// Where the extended configuration region is, in dwords, and what it
/// holds: a page select, a register on that page, then a page the
/// driver does not reach directly, which is skipped.
const EXT_POINTER: u32 = 0x40;
const EXT_CONFIG: [(u16, u16); 4] =
    [((769 << 5) as u16, 0x1f), (0x1234, 28), (1 << 5, 0x1f), (0x5555, 17)];

/// The platform's latency tolerance, scale 3 (32 768 ns) times 1000
/// (0x3e8).
const MAX_LATENCY: u16 = (3 << 10) | 0x3e8;

thread_local! {
    static NOW: Cell<u64> = const { Cell::new(0) };
}

/// A clock that moves a microsecond at each look.
fn clock() -> u64 {
    NOW.with(|now| {
        let then = now.get() + 1_000;
        now.set(then);
        then
    })
}

/// What a test sets the chip up as.
#[derive(Clone, Copy)]
struct Setup {
    mac: Mac,
    /// Whether the ME's firmware runs.
    me: bool,
    /// Whether the ME blocks PHY resets, as in an AMT SOL session.
    blocked: bool,
    /// Whether the PHY is on SMBus, reached only as the MAC forces SMBus
    /// or after LANPHYPC power-cycles it.
    smbus: bool,
    /// Whether the PHY never answers.
    dead: bool,
    /// Whether the ME holds the arbiter at every third look at FWSM.
    arbiter: bool,
    /// Whether the I219's ME left the PHY in ULP.
    ulp: bool,
}

impl Setup {
    fn new(mac: Mac) -> Setup {
        Setup {
            mac,
            me: true,
            blocked: false,
            smbus: false,
            dead: false,
            arbiter: false,
            ulp: false,
        }
    }
}

struct State {
    setup: Setup,
    regs: Vec<u32>,
    icr: u32,
    phy: HashMap<(u32, u32), u16>,
    page: u32,
    emi_address: u16,
    emi: HashMap<u16, u16>,
    kmrn: HashMap<u32, u16>,
    /// Whether the PHY answers over MDIO in PCI Express mode.
    reachable: bool,
    on_smbus: bool,
    lanphypc_low: bool,
    toggles: u32,
    phy_resets: u32,
    resets: u32,
    ulp_done: bool,
    fwsm_reads: u32,
    busy: bool,
    writes_while_busy: u32,
    lose_tail: bool,
    /// The link: its speed and whether it is full duplex.
    link: Option<(u32, bool)>,
    flash: Vec<u8>,
    hsfsts: u16,
    hsfctl: u16,
    faddr: u32,
    fdata0: u32,
    sent: Vec<Vec<u8>>,
}

struct Fake {
    state: Mutex<State>,
}

/// What the NVM loads into the MAC's registers at a reset.
fn loaded(mac: Mac) -> [(usize, u32); 6] {
    let strap = (0x48 << 17) | (2 << 12) | if mac == Mac::Spt { 1 << 1 } else { 0 };
    [
        (STRAP, strap),
        (FEXTNVM, 1 << 27),
        (EXTCNF_CTRL, EXT_POINTER << 16),
        (EXTCNF_SIZE, (EXT_CONFIG.len() as u32) << 16),
        (LEDCTL, 0x0706_8302),
        (PHY_CTRL, 1 << 1),
    ]
}

impl Fake {
    fn new(setup: Setup) -> Arc<Fake> {
        let mut phy = HashMap::new();
        let id = match setup.mac {
            Mac::Pch2 => 0x0092,
            Mac::Spt => 0x00a3,
        };
        phy.insert((0, 2), 0x0154);
        phy.insert((0, 3), id);
        // ULP's settings, sticky and on SMBus, and K1 off.
        phy.insert(ULP_CONFIG1, 0x0114);
        phy.insert(SMB_CTRL, 0x0001);
        phy.insert(TX_PTR_GAP, 0x8 << 2);
        // The NVM: two banks of 2048 words. The 82579's starts a sector
        // into the flash, with its first bank valid; the I219's at 0,
        // with its second.
        let (base, bank) = match setup.mac {
            Mac::Pch2 => (4096, 0),
            Mac::Spt => (0, 1),
        };
        let mut flash = std::vec![0xffu8; 16384];
        let mut word = |at: usize, value: u16| {
            flash[base + 2 * at..base + 2 * at + 2].copy_from_slice(&value.to_le_bytes());
        };
        for b in 0..2 {
            word(b * 2048 + 0x13, if b == bank { 0x8000 } else { 0x4000 });
        }
        let start = bank * 2048 + 2 * EXT_POINTER as usize;
        for (pair, (data, number)) in EXT_CONFIG.iter().enumerate() {
            word(start + 2 * pair, *data);
            word(start + 2 * pair + 1, *number);
        }
        let state = State {
            setup,
            regs: std::vec![0; SIZE / 4],
            icr: 0,
            phy,
            page: 0,
            emi_address: 0,
            emi: HashMap::new(),
            kmrn: HashMap::new(),
            reachable: !setup.dead && !setup.smbus,
            on_smbus: setup.smbus,
            lanphypc_low: false,
            toggles: 0,
            phy_resets: 0,
            resets: 0,
            ulp_done: setup.ulp,
            fwsm_reads: 0,
            busy: false,
            writes_while_busy: 0,
            lose_tail: false,
            link: Some((1000, true)),
            flash,
            hsfsts: 0,
            hsfctl: 0,
            faddr: 0,
            fdata0: 0,
            sent: Vec::new(),
        };
        let fake = Arc::new(Fake { state: Mutex::new(state) });
        Fake::reset(&mut fake.state.lock().unwrap());
        fake
    }

    /// A reset: the registers as the NVM loads them, the address in the
    /// first receive address register, and the software flag free. The
    /// packet buffer's split and FEXTNVM11 stay, as drivers set them
    /// before a reset.
    fn reset(state: &mut State) {
        let kept = [PBA, FEXTNVM11].map(|at| (at, state.regs[at / 4]));
        state.regs.iter_mut().for_each(|r| *r = 0);
        for (at, value) in kept {
            state.regs[at / 4] = value;
        }
        state.resets += 1;
        for (at, value) in loaded(state.setup.mac) {
            state.regs[at / 4] = value;
        }
        state.regs[RAL0 / 4] = u32::from_le_bytes([MAC[0], MAC[1], MAC[2], MAC[3]]);
        state.regs[RAH0 / 4] = u32::from(u16::from_le_bytes([MAC[4], MAC[5]])) | (1 << 31);
        state.regs[STATUS / 4] = STATUS_LAN_INIT_DONE | STATUS_PHYRA;
    }

    fn reg(&self, at: usize) -> u32 {
        self.state.lock().unwrap().regs[at / 4]
    }

    fn phy(&self, register: (u32, u32)) -> u16 {
        *self.state.lock().unwrap().phy.get(&register).unwrap_or(&0)
    }

    fn set_link(&self, link: Option<(u32, bool)>) {
        self.state.lock().unwrap().link = link;
    }

    /// One MDIO transaction, with what MDIC holds after it.
    fn mdic(state: &mut State, value: u32) -> u32 {
        assert_ne!(state.regs[EXTCNF_CTRL / 4] & SWFLAG, 0, "MDIO with the software flag held");
        let number = (value >> 16) & 0x1f;
        let address = (value >> 21) & 0x1f;
        let op = (value >> 26) & 3;
        let data = value as u16;
        let forced = state.regs[CTRL_EXT / 4] & CTRL_EXT_FORCE_SMBUS != 0;
        let answers = !state.setup.dead && (state.reachable || (state.on_smbus && forced));
        if !answers {
            return (value & !0xffff) | (1 << 28) | (1 << 30);
        }
        let key = match address {
            1 if number == 0x1f => {
                assert_eq!(op, 1, "the page is written");
                state.page = u32::from(data) >> 5;
                return (value & !0xffff) | u32::from(data) | (1 << 28);
            }
            1 => (if state.page == 0 { 768 } else { state.page }, number),
            2 => {
                if number > 0xf {
                    assert_eq!(state.page, 0, "page 0 selected for register {number}");
                }
                (0, number)
            }
            _ => panic!("no PHY at address {address}"),
        };
        let result = match (key, op) {
            ((0, 0x10), 1) => {
                state.emi_address = data;
                data
            }
            ((0, 0x11), 1) => {
                state.emi.insert(state.emi_address, data);
                data
            }
            ((0, 26), 2) => match state.link {
                Some((mbps, _)) => {
                    let speed = match mbps {
                        1000 => 1 << 9,
                        100 => 1 << 8,
                        _ => 0,
                    };
                    (1 << 6) | (1 << 12) | speed
                }
                None => 0,
            },
            (key, 1) => {
                state.phy.insert(key, data);
                data
            }
            (key, _) => *state.phy.get(&key).unwrap_or(&0),
        };
        (value & !0xffff) | u32::from(result) | (1 << 28)
    }

    fn deliver(&self, frame: &[u8]) -> bool {
        let mut state = self.state.lock().unwrap();
        let count = state.regs[RDLEN / 4] as usize / 16;
        let head = state.regs[RDH / 4] as usize;
        if count == 0 || head == state.regs[RDT / 4] as usize {
            return false;
        }
        let base = u64::from(state.regs[RDBAL / 4]) | (u64::from(state.regs[RDBAL / 4 + 1]) << 32);
        let at = base + (head * 16) as u64;
        write_mem(u64_at(at), frame);
        let mut write_back = [0u8; 8];
        write_back[..2].copy_from_slice(&(frame.len() as u16).to_le_bytes());
        write_back[4] = 0x01 | 0x02;
        write_mem(at + 8, &write_back);
        state.regs[RDH / 4] = ((head + 1) % count) as u32;
        state.icr |= 1 << 7;
        true
    }

    fn transmit(state: &mut State) {
        if state.regs[TCTL / 4] & (1 << 1) == 0 {
            return;
        }
        let count = state.regs[TDLEN / 4] as usize / 16;
        let base = u64::from(state.regs[TDBAL / 4]) | (u64::from(state.regs[TDBAL / 4 + 1]) << 32);
        let mut head = state.regs[TDH / 4] as usize;
        let tail = state.regs[TDT / 4] as usize;
        while head != tail {
            let at = base + (head * 16) as u64;
            let descriptor = read_mem(at, 16);
            let len = usize::from(u16::from_le_bytes([descriptor[8], descriptor[9]]));
            state.sent.push(read_mem(u64_at(at), len));
            write_mem(at + 12, &[0x01]);
            head = (head + 1) % count;
        }
        state.regs[TDH / 4] = head as u32;
        state.icr |= 1;
    }

    /// A flash cycle, as HSFCTL starts one.
    fn cycle(state: &mut State) {
        if state.hsfctl & 1 == 0 {
            return;
        }
        state.hsfctl &= !1;
        assert_eq!(state.hsfctl & (3 << 1), 0, "a read cycle");
        let bytes = usize::from((state.hsfctl >> 8) & 3) + 1;
        let at = state.faddr as usize;
        let mut data = [0u8; 4];
        data[..bytes].copy_from_slice(&state.flash[at..at + bytes]);
        state.fdata0 = u32::from_le_bytes(data);
        state.hsfsts |= FLCDONE;
    }

    fn flash_read(state: &State, offset: usize, size: usize) -> u64 {
        let status = state.hsfsts | FLDESVALID;
        u64::from(match (offset, size) {
            (0x00, 4) => (2 << 16) | 1,
            (HSFSTS, 2) => u32::from(status),
            (HSFCTL, 2) => u32::from(state.hsfctl),
            (HSFSTS, 4) => u32::from(status) | (u32::from(state.hsfctl) << 16),
            (FADDR, 4) => state.faddr,
            (FDATA0, 4) => state.fdata0,
            _ => panic!("flash register {offset:#x}, {size} bytes"),
        })
    }

    fn flash_write(state: &mut State, offset: usize, size: usize, value: u64) {
        let clear = |state: &mut State, status: u16| {
            state.hsfsts &= !(status & (FLCDONE | FLCERR | DAEL));
        };
        match (offset, size) {
            (HSFSTS, 2) => clear(state, value as u16),
            (HSFCTL, 2) => {
                state.hsfctl = value as u16;
                Fake::cycle(state);
            }
            (HSFSTS, 4) => {
                clear(state, value as u16);
                state.hsfctl = (value >> 16) as u16;
                Fake::cycle(state);
            }
            (FADDR, 4) => state.faddr = value as u32,
            _ => panic!("flash register {offset:#x}, {size} bytes written"),
        }
    }
}

/// The 82579's flash registers, its BAR 1.
struct FlashBar(Arc<Fake>);

impl Registers for FlashBar {
    fn read(&self, offset: usize, size: usize) -> u64 {
        Fake::flash_read(&self.0.state.lock().unwrap(), offset, size)
    }

    fn write(&self, offset: usize, size: usize, value: u64) {
        Fake::flash_write(&mut self.0.state.lock().unwrap(), offset, size, value);
    }
}

impl Registers for Fake {
    fn read(&self, offset: usize, size: usize) -> u64 {
        assert_eq!(size, 4, "32-bit registers");
        let mut state = self.state.lock().unwrap();
        if offset >= FLASH {
            assert_eq!(state.setup.mac, Mac::Spt, "the 82579's flash is in BAR 1");
            return Fake::flash_read(&state, offset - FLASH, size);
        }
        let value = match offset {
            STATUS => {
                let mut status = state.regs[STATUS / 4] & (STATUS_LAN_INIT_DONE | STATUS_PHYRA);
                if state.regs[CTRL / 4] & CTRL_GIO_MASTER_DISABLE == 0 {
                    status |= STATUS_GIO_MASTER_ENABLE;
                }
                if let Some((mbps, full)) = state.link {
                    let speed = match mbps {
                        1000 => 2,
                        100 => 1,
                        _ => 0,
                    };
                    status |= 0x2 | u32::from(full) | (speed << 6);
                }
                status
            }
            FWSM => {
                state.fwsm_reads += 1;
                state.busy = state.setup.arbiter && state.fwsm_reads % 3 == 1;
                let mut fwsm = 0;
                if state.setup.me {
                    fwsm |= FWSM_FW_VALID;
                }
                if !state.setup.blocked {
                    fwsm |= FWSM_RSPCIPHY;
                }
                if state.ulp_done {
                    fwsm |= FWSM_ULP_CFG_DONE;
                }
                if state.busy {
                    fwsm |= FWSM_PCIM2PCI;
                }
                fwsm
            }
            ICR => core::mem::take(&mut state.icr),
            _ => state.regs[offset / 4],
        };
        u64::from(value)
    }

    fn write(&self, offset: usize, size: usize, value: u64) {
        assert_eq!(size, 4, "32-bit registers");
        let value = value as u32;
        let mut state = self.state.lock().unwrap();
        if state.busy {
            state.writes_while_busy += 1;
        }
        if offset >= FLASH {
            assert_eq!(state.setup.mac, Mac::Spt, "the 82579's flash is in BAR 1");
            return Fake::flash_write(&mut state, offset - FLASH, size, u64::from(value));
        }
        match offset {
            CTRL => {
                if value & CTRL_PHY_RST != 0 {
                    // A PHY reset brings its link to the MAC back to PCI
                    // Express.
                    state.phy_resets += 1;
                    state.page = 0;
                    state.on_smbus = false;
                    state.reachable = !state.setup.dead;
                }
                if value & CTRL_RST != 0 {
                    Fake::reset(&mut state);
                    state.regs[CTRL / 4] = value & !(CTRL_RST | CTRL_PHY_RST);
                    return;
                }
                let low = value & CTRL_LANPHYPC_OVERRIDE != 0 && value & CTRL_LANPHYPC_VALUE == 0;
                if low {
                    state.lanphypc_low = true;
                } else if state.lanphypc_low && value & CTRL_LANPHYPC_OVERRIDE == 0 {
                    // Power-cycled: the PHY is back, off SMBus.
                    state.lanphypc_low = false;
                    state.toggles += 1;
                    state.on_smbus = false;
                    state.reachable = !state.setup.dead;
                    state.regs[CTRL_EXT / 4] |= CTRL_EXT_LPCD;
                }
                state.regs[CTRL / 4] = value & !CTRL_PHY_RST;
            }
            STATUS => state.regs[STATUS / 4] = value & (STATUS_LAN_INIT_DONE | STATUS_PHYRA),
            // Read only.
            STRAP | FWSM => {}
            MDIC => {
                let result = Fake::mdic(&mut state, value);
                state.regs[MDIC / 4] = result;
            }
            KMRNCTRLSTA => {
                let at = (value >> 16) & 0x1f;
                if value & (1 << 21) != 0 {
                    let data = *state.kmrn.get(&at).unwrap_or(&0);
                    state.regs[KMRNCTRLSTA / 4] = (value & !0xffff) | u32::from(data);
                } else {
                    state.kmrn.insert(at, value as u16);
                    state.regs[KMRNCTRLSTA / 4] = value;
                }
            }
            H2ME => {
                if value & H2ME_ENFORCE_SETTINGS != 0 && value & H2ME_ULP == 0 {
                    state.ulp_done = false;
                }
                state.regs[H2ME / 4] = value;
            }
            IMC => state.regs[IMS / 4] &= !value,
            IMS => state.regs[IMS / 4] |= value,
            TDT => {
                state.regs[TDT / 4] = if state.lose_tail { value.wrapping_add(1) } else { value };
                Fake::transmit(&mut state);
            }
            _ => state.regs[offset / 4] = value,
        }
    }
}

/// The function's configuration space.
struct Config(u16);

impl Registers for Config {
    fn read(&self, offset: usize, size: usize) -> u64 {
        let value: u32 = match offset {
            0x00 => 0x8086 | (u32::from(self.0) << 16),
            0x02 => u32::from(self.0),
            0x08 => 0x0200_0000,
            0xa8 | 0xaa => u32::from(MAX_LATENCY) | (u32::from(MAX_LATENCY) << 16),
            _ => 0,
        };
        u64::from(value) & if size == 8 { u64::MAX } else { (1 << (size * 8)) - 1 }
    }

    // Its BARs read as none: the driver takes them as given.
    fn write(&self, _offset: usize, _size: usize, _value: u64) {}
}

fn model(mac: Mac) -> Model {
    let device = match mac {
        Mac::Pch2 => 0x1502,
        Mac::Spt => 0x15b7,
    };
    crate::identify(0x8086, device).unwrap()
}

fn function(mac: Mac) -> Function {
    let device = match mac {
        Mac::Pch2 => 0x1502,
        Mac::Spt => 0x15b7,
    };
    let address = Address { segment: 0, bus: 0, device: 0x1f, function: 6 };
    Function::probe(address, Mmio::fake(Arc::new(Config(device)), CONFIG_SIZE)).unwrap()
}

fn start(fake: &Arc<Fake>, interrupt: Interrupt) -> Result<Running, Error> {
    let mac = fake.state.lock().unwrap().setup.mac;
    let regs = Mmio::fake(fake.clone(), SIZE);
    let flash = (mac == Mac::Pch2).then(|| Mmio::fake(Arc::new(FlashBar(fake.clone())), 0x1000));
    let function = function(mac);
    let platform = Platform { function: &function, flash, clock };
    pch::start(model(mac), regs, platform, interrupt, FALLBACK, &mut alloc, &mut no_delay)
}

fn frame(n: usize) -> Vec<u8> {
    let mut frame = std::vec![0u8; 60 + n % 100];
    frame[..6].copy_from_slice(&[0xff; 6]);
    frame[6..12].copy_from_slice(&MAC);
    frame[14..22].copy_from_slice(&(n as u64).to_le_bytes());
    frame
}

/// Has the driver look at the link, as it does with nothing to receive.
fn poll(card: &mut Running) {
    let mut buffer = [0u8; 2048];
    while card.receive(&mut buffer).is_some() {}
}

#[test]
fn the_chips_are_known_by_their_device_ids() {
    assert_eq!(model(Mac::Spt), Model { name: "I219-LM", kind: Kind::Pch(Mac::Spt) });
    assert_eq!(model(Mac::Pch2), Model { name: "82579LM", kind: Kind::Pch(Mac::Pch2) });
    assert_eq!(crate::identify(0x8086, 0x15b8).map(|m| m.name), Some("I219-V"));
    assert_eq!(crate::identify(0x8086, 0x1503).map(|m| m.name), Some("82579V"));
    assert!(model(Mac::Spt).msi());
    // Cannon Lake's I219 is not one of them yet.
    assert_eq!(crate::identify(0x8086, 0x15bb), None);
    assert!(Mac::Pch2.flash_bar() && !Mac::Spt.flash_bar());
}

#[test]
fn the_crate_wide_start_leaves_them_to_their_own() {
    let fake = Fake::new(Setup::new(Mac::Spt));
    let regs = Mmio::fake(fake.clone(), SIZE);
    let started =
        crate::start(model(Mac::Spt), regs, Interrupt::Polled, FALLBACK, &mut alloc, &mut no_delay);
    assert!(matches!(started, Err(Error::Unsupported(_))));
}

#[test]
fn an_i219_with_its_me_starts_from_its_nvm() {
    let mut setup = Setup::new(Mac::Spt);
    setup.ulp = true;
    let fake = Fake::new(setup);
    let card = start(&fake, Interrupt::Msi).unwrap();
    assert_eq!(card.info().mac, MAC);
    assert_eq!(card.info().model, "I219-LM");
    // The ME took the PHY out of ULP, and was let be.
    let h2me = fake.reg(H2ME);
    assert_eq!(h2me & (H2ME_ULP | H2ME_ENFORCE_SETTINGS), 0);
    assert!(!fake.state.lock().unwrap().ulp_done);
    // Reset with the PHY, once.
    let state = fake.state.lock().unwrap();
    assert_eq!(state.resets, 2, "the simulator's own and the driver's");
    assert!(state.phy_resets >= 2, "at its start, and with the MAC");
    drop(state);
    // The NVM's extended configuration, from its second bank, with the
    // page the driver does not reach directly skipped.
    assert_eq!(fake.phy((769, 28)), 0x1234);
    assert_eq!(fake.phy((769, 17)), 0, "the PHY's own HOST_WU cleared, not the skipped write");
    // SMBus address and clock from STRAP, the LEDs, the OEM bits.
    let smb = fake.phy(SMB_ADDR);
    assert_eq!(smb & 0x7f, 0x48);
    assert_eq!(smb & ((1 << 9) | (1 << 7)), (1 << 9) | (1 << 7), "PEC and valid");
    assert_eq!(smb & 0x1100, 0x0100, "the second frequency");
    assert_eq!(fake.phy(LED_CONFIG), 0x8302);
    assert_eq!(fake.phy(OEM_BITS) & 0x0444, 0x0404, "LPLU and restart, gigabit allowed");
    // Every speed to 1000 full, no pause, auto crossover, downshift.
    assert_eq!(fake.phy(PHY_ANAR) & 0x0de0, 0x01e0);
    assert_eq!(fake.phy(PHY_1000T) & 0x0300, 0x0200);
    assert_eq!(fake.phy(PHY_CONTROL) & 0x1200, 0x1200);
    assert_eq!(fake.phy(PHY_CFG) & 0x8c00, 0x8c00);
    assert_eq!(fake.phy(PHY_CTRL_2) & 0x0600, 0x0400);
    // The MAC's errata and settings.
    assert_eq!(fake.reg(PBA), 0x1a);
    assert_ne!(fake.reg(FEXTNVM11) & (1 << 13), 0);
    assert_ne!(fake.reg(IOSFPC) & (1 << 16), 0);
    assert_eq!(fake.reg(TARC0) & (3 << 28), 2 << 28);
    let rctl = fake.reg(RCTL);
    assert_eq!(
        rctl & ((1 << 1) | (1 << 4) | (1 << 15) | (1 << 26)),
        (1 << 1) | (1 << 4) | (1 << 15) | (1 << 26)
    );
    assert_ne!(fake.reg(TCTL) & ((1 << 1) | (1 << 28)), 0);
    assert_ne!(fake.reg(IMS) & (1 << 7), 0, "an MSI for received frames");
    assert_ne!(fake.reg(CTRL_EXT) & CTRL_EXT_DRV_LOAD, 0, "the driver has it");
    assert_eq!(fake.reg(EXTCNF_CTRL) & SWFLAG, 0, "the flag given back");
    assert_eq!(fake.reg(RDT), 255);
    // The link it came up with: 1000 full.
    assert_eq!(fake.reg(TIPG) & 0x3ff, 0x08);
    assert_eq!(fake.phy(PLL_GATE) & 0x7ff, 0xfa);
    assert_ne!(fake.phy(PM_CTRL) & (1 << 9), 0, "K1 clock request at 1000");
    assert_eq!(fake.reg(FEXTNVM4) & 7, 7, "an 8 µs beacon");
    let link = card.link();
    assert!(link.up && link.full_duplex);
    assert_eq!(link.mbps, 1000);
}

#[test]
fn an_i219_reports_the_latency_its_buffer_tolerates() {
    let fake = Fake::new(Setup::new(Mac::Spt));
    let mut card = start(&fake, Interrupt::Polled).unwrap();
    // 26 KiB less two frames, at 1000 Mb/s: 188 704 ns, 185 (0xb9) at
    // scale 2; the buffer's high water mark 3 KiB.
    let encoded = (2 << 10) | 0xb9;
    let request = (1 << 30) | (1 << 15) | (1 << 31);
    assert_eq!(fake.reg(LTRV), request | encoded | (encoded << 16));
    assert_eq!(fake.reg(SVT) & 0x1f, 3);
    fake.set_link(Some((100, true)));
    poll(&mut card);
    // Ten times as long at 100 Mb/s: 58 (0x3a) at scale 3.
    let encoded = (3 << 10) | 0x3a;
    assert_eq!(fake.reg(LTRV), request | encoded | (encoded << 16));
    assert_eq!(fake.reg(TIPG) & 0x3ff, 0x0c, "100 full on the I219");
    assert_eq!(fake.phy(PLL_GATE) & 0x7ff, 0x3e8);
    assert_eq!(fake.phy(TX_PTR_GAP), 0xc023);
    fake.set_link(None);
    poll(&mut card);
    assert_eq!(fake.reg(LTRV), 1 << 30, "no requirement without a link");
}

#[test]
fn an_i219_without_an_me_leaves_ulp_itself() {
    let mut setup = Setup::new(Mac::Spt);
    setup.me = false;
    setup.smbus = true;
    let fake = Fake::new(setup);
    start(&fake, Interrupt::Polled).unwrap();
    let state = fake.state.lock().unwrap();
    assert!(state.toggles >= 1, "LANPHYPC power-cycled the PHY");
    drop(state);
    let ulp = fake.phy(ULP_CONFIG1);
    assert_eq!(ulp & 0x1d74, 0, "ULP's settings cleared");
    assert_ne!(ulp & 1, 0, "and committed");
    assert_eq!(fake.phy(SMB_CTRL) & 1, 0, "the PHY off SMBus");
    assert_eq!(fake.reg(CTRL_EXT) & CTRL_EXT_FORCE_SMBUS, 0, "and the MAC");
    assert_ne!(fake.phy(PM_CTRL) & (1 << 14), 0, "K1 back on");
}

#[test]
fn an_i219_on_smbus_is_reached_by_forcing_the_mac_to_it() {
    let mut setup = Setup::new(Mac::Spt);
    setup.smbus = true;
    let fake = Fake::new(setup);
    start(&fake, Interrupt::Polled).unwrap();
    // Found over SMBus without power-cycling it; the reset with the MAC
    // brought it back to PCI Express, where the driver set it up.
    assert_eq!(fake.state.lock().unwrap().toggles, 0);
    assert_eq!(fake.phy(PHY_ANAR) & 0x01e0, 0x01e0);
}

#[test]
fn a_phy_that_never_answers_fails_the_start() {
    let mut setup = Setup::new(Mac::Spt);
    setup.dead = true;
    let fake = Fake::new(setup);
    assert!(matches!(start(&fake, Interrupt::Polled), Err(Error::Device(_))));
    let mut setup = Setup::new(Mac::Pch2);
    setup.dead = true;
    let fake = Fake::new(setup);
    assert!(matches!(start(&fake, Interrupt::Polled), Err(Error::Device(_))));
}

#[test]
fn an_me_that_blocks_phy_resets_keeps_the_link_it_set_up() {
    let mut setup = Setup::new(Mac::Spt);
    setup.blocked = true;
    let fake = Fake::new(setup);
    start(&fake, Interrupt::Polled).unwrap();
    let state = fake.state.lock().unwrap();
    assert_eq!(state.phy_resets, 0, "no PHY reset");
    assert_eq!(state.resets, 2, "the MAC alone reset");
    drop(state);
    assert_eq!(fake.phy(PHY_ANAR), 0, "the PHY left as the ME set it up");
    assert_eq!(fake.phy((769, 28)), 0, "nor configured from the NVM");
}

#[test]
fn an_82579_with_its_me_waits_for_the_arbiter_before_each_write() {
    let mut setup = Setup::new(Mac::Pch2);
    setup.arbiter = true;
    let fake = Fake::new(setup);
    let mut card = start(&fake, Interrupt::Polled).unwrap();
    for n in 0..20 {
        assert!(card.send(&frame(n)));
        card.flush();
    }
    card.stop();
    let state = fake.state.lock().unwrap();
    assert!(state.fwsm_reads > 100);
    assert_eq!(state.writes_while_busy, 0);
    assert_eq!(state.sent.len(), 20);
}

#[test]
fn an_82579_starts_with_its_workarounds_and_its_flash_bar() {
    let mut setup = Setup::new(Mac::Pch2);
    setup.me = false;
    let fake = Fake::new(setup);
    let mut card = start(&fake, Interrupt::Polled).unwrap();
    assert_eq!(card.info().model, "82579LM");
    assert_eq!(card.info().mac, MAC);
    // Slow MDIO, the noise thresholds, the LPI timer.
    assert_ne!(fake.phy(KMRN_MODE_CTRL) & (1 << 10), 0);
    let state = fake.state.lock().unwrap();
    assert_eq!(state.emi.get(&0x084f), Some(&0x0034));
    assert_eq!(state.emi.get(&0x2411), Some(&0x0005));
    assert_eq!(state.emi.get(&0x4805), Some(&0x1387));
    assert_eq!(state.emi.get(&0x3412), Some(&1), "receive configuration at 1000");
    // The longest Kumeran timeouts, for 10 Mb/s.
    assert_eq!(state.kmrn.get(&4), Some(&0xffff));
    drop(state);
    // The NVM's configuration from the flash BAR's first bank, with the
    // 82579's SMBus address but not the I219's clock.
    assert_eq!(fake.phy((769, 28)), 0x1234);
    assert_eq!(fake.phy(SMB_ADDR) & 0x11ff, 0x48 | 0x80);
    // Its hardware configures the PHY again after the driver's own.
    assert_eq!(fake.reg(EXTCNF_CTRL) & (1 << 7), 0, "ungated");
    // At 1000 Mb/s, K1 off.
    assert_eq!(fake.phy(PM_CTRL) & (1 << 14), 0);
    // At 10 half: the widest gap, a 16 µs beacon, a preamble dropped.
    fake.set_link(Some((10, false)));
    poll(&mut card);
    assert_eq!(fake.reg(TIPG) & 0x3ff, 0xff);
    assert_eq!(fake.reg(FEXTNVM4) & 7, 3);
    assert_eq!(fake.phy(KMRN_FIFO) & 0x7000, 0x1000);
    assert_eq!(fake.state.lock().unwrap().emi.get(&0x3412), Some(&0));
    fake.set_link(Some((100, true)));
    poll(&mut card);
    assert_eq!(fake.reg(TIPG) & 0x3ff, 0x08, "the 82579's gap at 100 full");
    assert_eq!(fake.phy(KMRN_FIFO) & 0x7000, 0);
}

#[test]
fn a_tail_the_me_lost_stops_an_82579() {
    let fake = Fake::new(Setup::new(Mac::Pch2));
    let mut card = start(&fake, Interrupt::Polled).unwrap();
    assert!(card.send(&frame(0)));
    card.flush();
    assert!(!card.failed());
    fake.state.lock().unwrap().lose_tail = true;
    assert!(card.send(&frame(1)));
    card.flush();
    assert!(card.failed());
    assert_eq!(fake.reg(TCTL) & (1 << 1), 0, "the transmitter off");
}

#[test]
fn frames_move_both_ways_and_the_chip_stops_without_a_reset() {
    let fake = Fake::new(Setup::new(Mac::Spt));
    let mut card = start(&fake, Interrupt::Msi).unwrap();
    for n in 0..300 {
        assert!(card.can_send());
        assert!(card.send(&frame(n)));
        card.flush();
    }
    let sent = std::mem::take(&mut fake.state.lock().unwrap().sent);
    assert_eq!(sent.len(), 300);
    assert_eq!(sent[299], frame(299));
    let mut buffer = [0u8; 2048];
    for n in 0..10 {
        assert!(fake.deliver(&frame(n)));
        assert_eq!(card.receive(&mut buffer), Some(frame(n).len()));
        assert_eq!(&buffer[..frame(n).len()], &frame(n)[..]);
    }
    let resets = fake.state.lock().unwrap().resets;
    card.stop();
    assert_eq!(fake.reg(RCTL), 0);
    assert_eq!(fake.reg(TCTL) & (1 << 1), 0);
    assert_eq!(fake.reg(IMS), 0);
    assert_eq!(fake.reg(CTRL_EXT) & CTRL_EXT_DRV_LOAD, 0, "given back to the ME");
    assert_eq!(fake.state.lock().unwrap().resets, resets);
    assert_eq!(card.receive(&mut buffer), None);
}

#[test]
fn an_82579_out_of_reach_is_power_cycled_and_read_after_its_reset() {
    let mut setup = Setup::new(Mac::Pch2);
    setup.me = false;
    setup.smbus = true;
    let fake = Fake::new(setup);
    let card = start(&fake, Interrupt::Polled).unwrap();
    assert_eq!(card.info().model, "82579LM");
    assert_eq!(fake.state.lock().unwrap().toggles, 1);
    // With the ME running, power-cycling it is the ME's to allow.
    let mut setup = Setup::new(Mac::Pch2);
    setup.smbus = true;
    setup.blocked = true;
    let fake = Fake::new(setup);
    assert!(matches!(start(&fake, Interrupt::Polled), Err(Error::Device(_))));
    assert_eq!(fake.state.lock().unwrap().toggles, 0);
}
