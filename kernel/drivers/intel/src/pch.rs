// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Intel's Ethernet in the chipset (ADR 0361): the MAC in the platform
//! controller hub (PCH), with a PHY chip of its own beside it, joined by
//! a fast link for frames and by SMBus for when the host sleeps. Two
//! generations, FreeBSD's `pch2lan` and `pch_spt`:
//!
//! - the 82579LM and 82579V, with Intel's 6 and 7 series chipsets
//!   (Cougar Point and Panther Point), as on the Dell Precision T1650;
//! - the I219-LM and I219-V with the 100 and 200 series (Sunrise Point
//!   and Union Point), Lewisburg, and Comet Lake's H series, as on the
//!   Dell OptiPlex 7040.
//!
//! The PHY has masters besides the driver: the management engine (ME),
//! which runs Intel AMT on vPro machines and may be using it, and the
//! chipset, which configures it from the NVM after each reset. It may
//! also be where the driver cannot reach it: in ultra low power mode
//! (ULP) on the I219, or talking SMBus to the ME. So the driver, with
//! FreeBSD's e1000_ich8lan.c (Intel's shared code, BSD-3-Clause) as the
//! reference for the sequences and errata:
//!
//! 1. has the ME take the I219's PHY out of ULP, or takes it out itself
//!    when no ME runs; finds the PHY by its ID registers, forcing the MAC
//!    to SMBus or power-cycling the PHY (LANPHYPC) when it does not
//!    answer; and resets it, unless the ME blocks PHY resets, as it does
//!    while an AMT Serial-over-LAN or IDE-R session is open;
//! 2. resets the MAC and PHY together with the software flag held,
//!    which the ME, the hardware and the driver share, and does what the
//!    PHY needs after a reset: the 82579's slow MDIO and noise
//!    thresholds, and the PHY configuration in the NVM's extended region,
//!    which it reads from the chipset's flash ([`nvm`]) where the NVM
//!    says software is to write it;
//! 3. applies the settings the specification updates call for, turns
//!    the jumbo frame workaround off, and advertises every speed up to
//!    1000 Mb/s full duplex, with flow control off as across the crate;
//! 4. drives one queue pair of legacy descriptors, as the e1000 family
//!    has them ([`rings`](crate::rings)), and raises one MSI;
//! 5. watches the link while the card runs, each time it has received
//!    all there is: as the link comes up or goes down, it sets the
//!    interpacket gap and the PHY's settings for the link's speed (packet
//!    loss and hang workarounds), the K1 power states and, on the I219,
//!    the latency tolerance it reports to the platform.
//!
//! On the 82579 with the ME running, a register write can be lost while
//! the ME holds the bus they share, so every write waits until it lets
//! go, and a ring tail that did not take stops the card
//! (FreeBSD's e1000_osdep.c and [`Interlock`]).
//!
//! It stops the chip without a reset: an I219 reset with descriptors in
//! its rings can hang it until the machine resets, and the next driver
//! resets it as this one does at its start, before any ring is loaded.
//!
//! Energy Efficient Ethernet, wake on LAN, jumbo frames, the I217 and
//! I218, and the I219s of Cannon Lake and later are not done.

mod nvm;

use radix_hw::pci::Function;
use radix_hw::{Dma, Mmio};
use radix_nic::{Error, Link, wait_for};

use self::nvm::Nvm;
use crate::rings::{self, Format, Interlock, Rings};
use crate::shared::{self, CAUSES, ctrl, rctl, status, tctl};
use crate::{Card, Interrupt, Kind, Model, Ops};

/// The registers, from the start of BAR 0.
mod reg {
    pub use crate::shared::reg::{
        CTRL, CTRL_EXT, ICR, IMC, IMS, ITR, MDIC, MTA, RAH0, RAL0, RCTL, STATUS, TCTL,
    };
    pub const STRAP: usize = 0x000c;
    pub const FEXTNVM6: usize = 0x0010;
    pub const FEXTNVM4: usize = 0x0024;
    pub const FEXTNVM: usize = 0x0028;
    pub const KMRNCTRLSTA: usize = 0x0034;
    pub const FEXTNVM3: usize = 0x003c;
    pub const FEXTNVM7: usize = 0x00e4;
    pub const SVCR: usize = 0x00f0;
    pub const SVT: usize = 0x00f4;
    pub const LTRV: usize = 0x00f8;
    pub const FCTTV: usize = 0x0170;
    pub const TIPG: usize = 0x0410;
    pub const LEDCTL: usize = 0x0e00;
    pub const EXTCNF_CTRL: usize = 0x0f00;
    pub const EXTCNF_SIZE: usize = 0x0f08;
    pub const PHY_CTRL: usize = 0x0f10;
    pub const PCIEANACFG: usize = 0x0f18;
    pub const IOSFPC: usize = 0x0f28;
    pub const PBA: usize = 0x1000;
    pub const PBECCSTS: usize = 0x100c;
    pub const FCRTL: usize = 0x2160;
    pub const FCRTH: usize = 0x2168;
    pub const RDTR: usize = 0x2820;
    pub const KABGTXD: usize = 0x3004;
    pub const TIDV: usize = 0x3820;
    pub const TXDCTL0: usize = 0x3828;
    pub const TARC0: usize = 0x3840;
    pub const TXDCTL1: usize = 0x3928;
    pub const TARC1: usize = 0x3940;
    pub const RFCTL: usize = 0x5008;
    pub const WUFC: usize = 0x5808;
    pub const GCR: usize = 0x5b00;
    pub const H2ME: usize = 0x5b50;
    pub const FWSM: usize = 0x5b54;
    pub const FEXTNVM11: usize = 0x5bbc;
    pub const FFLT_DBG: usize = 0x5f04;
    pub const FCRTV_PCH: usize = 0x5f40;
    /// The I219's NVM registers, from here in BAR 0.
    pub const FLASH: usize = 0xe000;
    /// How far the 82579's registers go, and the I219's.
    pub const END_PCH2: usize = FCRTV_PCH + 4;
    pub const END_SPT: usize = FLASH + super::nvm::END;
}

/// CTRL's bits beyond the shared ones.
const CTRL_LANPHYPC_OVERRIDE: u32 = 1 << 16;
const CTRL_LANPHYPC_VALUE: u32 = 1 << 17;
const CTRL_MEHE: u32 = 1 << 19;
/// STATUS's.
const STATUS_LAN_INIT_DONE: u32 = 1 << 9;
const STATUS_PHYRA: u32 = 1 << 10;
/// CTRL_EXT's.
const CTRL_EXT_LPCD: u32 = 1 << 2;
const CTRL_EXT_FORCE_SMBUS: u32 = 1 << 11;
const CTRL_EXT_RO_DIS: u32 = 1 << 17;
const CTRL_EXT_PHYPDEN: u32 = 1 << 20;
/// FWSM's: the PHY may be reset (with the ME's leave), the ME has
/// undone ULP, the ME's firmware runs, the ME holds the PCIm-to-PCI
/// arbiter.
const FWSM_RSPCIPHY: u32 = 1 << 6;
const FWSM_ULP_CFG_DONE: u32 = 1 << 10;
const FWSM_FW_VALID: u32 = 1 << 15;
const FWSM_PCIM2PCI: u32 = 1 << 24;
/// H2ME's, the host's messages to the ME.
const H2ME_ULP: u32 = 1 << 11;
const H2ME_ENFORCE_SETTINGS: u32 = 1 << 12;
/// EXTCNF_CTRL's: the software flag, the gate on the hardware's own PHY
/// configuration, and where the NVM's extended region starts (in
/// dwords).
const EXTCNF_CTRL_SWFLAG: u32 = 1 << 5;
const EXTCNF_CTRL_GATE_PHY_CFG: u32 = 1 << 7;
const EXTCNF_POINTER_SHIFT: u32 = 16;
const EXTCNF_POINTER_MASK: u32 = 0x0fff;
/// EXTCNF_SIZE's: how long the extended region is, in pairs of words.
const EXTCNF_LENGTH_SHIFT: u32 = 16;
const EXTCNF_LENGTH_MASK: u32 = 0xff;
/// FEXTNVM's: software writes the PHY's configuration.
const FEXTNVM_SW_CONFIG: u32 = 1 << 27;
/// FEXTNVM3's PHY configuration counter, set to 50 ms.
const FEXTNVM3_PHY_CFG_COUNTER_MASK: u32 = 3 << 26;
const FEXTNVM3_PHY_CFG_COUNTER_50MS: u32 = 2 << 26;
/// FEXTNVM4's beacon duration, set to 8 µs, or 16 µs on the 82579 at
/// 10 Mb/s.
const FEXTNVM4_BEACON_MASK: u32 = 0x7;
const FEXTNVM4_BEACON_8US: u32 = 0x7;
const FEXTNVM4_BEACON_16US: u32 = 0x3;
/// FEXTNVM6's, and PCIEANACFG's in the same place.
const FEXTNVM6_K1_OFF_ENABLE: u32 = 1 << 31;
/// FEXTNVM7's.
const FEXTNVM7_DISABLE_SMB_PERST: u32 = 1 << 5;
/// FEXTNVM11's.
const FEXTNVM11_DISABLE_MULR_FIX: u32 = 1 << 13;
/// PHY_CTRL's, which the OEM bits follow.
const PHY_CTRL_D0A_LPLU: u32 = 1 << 1;
const PHY_CTRL_GBE_DISABLE: u32 = 1 << 6;
/// KMRNCTRLSTA's: a Kumeran register's offset, and the read enable.
const KMRN_OFFSET_SHIFT: u32 = 16;
const KMRN_OFFSET_MASK: u32 = 0x1f << KMRN_OFFSET_SHIFT;
const KMRN_REN: u32 = 1 << 21;
/// The Kumeran registers the driver uses.
const KMRN_CTRL: u32 = 0x1;
const KMRN_TIMEOUTS: u32 = 0x4;
const KMRN_INBAND_PARAM: u32 = 0x9;
const KMRN_HD_CTRL: u32 = 0x10;
const KMRN_K0S_CTRL: u32 = 0x1e;
const KMRN_K0S_MIN_TIME_SHIFT: u32 = 4;
const KMRN_K0S_MIN_TIME_MASK: u16 = 0x7 << KMRN_K0S_MIN_TIME_SHIFT;
/// LTRV's: snoop latency in the low half, no-snoop in the high, each a
/// value and a scale; the requirement bits; and the bit that sends it.
const LTRV_VALUE_MASK: u64 = 0x3ff;
const LTRV_SCALE_SHIFT: u32 = 10;
const LTRV_SCALE_MAX: u32 = 5;
const LTRV_REQ: u32 = 1 << 15;
const LTRV_NOSNOOP_SHIFT: u32 = 16;
const LTRV_SEND: u32 = 1 << 30;
/// SVT's OBFF high water mark, and SVCR's enables.
const SVT_OFF_HWM_MASK: u32 = 0x1f;
const SVCR_OFF_EN: u32 = 1 << 0;
const SVCR_OFF_MASKINT: u32 = 1 << 12;
/// Where the platform says the most latency it tolerates, snoop then
/// no-snoop, in the function's configuration space.
const PCI_LTR_CAP: usize = 0xa8;
/// KABGTXD's band gap bias for squelch.
const KABGTXD_BGSQLBIAS: u32 = 0x0005_0000;
/// PBECCSTS's.
const PBECCSTS_ECC_ENABLE: u32 = 1 << 16;
/// RFCTL's: NFS reads and writes filtering off.
const RFCTL_NFSW_DIS: u32 = 1 << 6;
const RFCTL_NFSR_DIS: u32 = 1 << 7;
/// TXDCTL's fields: prefetch threshold 31, write-back threshold 1, in
/// descriptors.
const TXDCTL_PTHRESH: u32 = 0x3f;
const TXDCTL_WTHRESH: u32 = 0x3f << 16;
const TXDCTL_FULL_TX_DESC_WB: u32 = 0x0101_0000;
const TXDCTL_MAX_TX_DESC_PREFETCH: u32 = 0x0100_001f;
const TXDCTL_BIT_22: u32 = 1 << 22;
/// TARC0's arbitration count for the I219.
const TARC0_CB_MULTIQ_3_REQ: u32 = 3 << 28;
const TARC0_CB_MULTIQ_2_REQ: u32 = 2 << 28;
/// IOSFPC's receive descriptor minimum threshold, a sixteenth.
const IOSFPC_RDMTS_HEX: u32 = 1 << 16;
/// GCR's no-snoop bits, all of which the driver clears.
const GCR_NO_SNOOP_ALL: u32 = 0x3f;
/// The packet buffer split: 26 KiB to receive, the rest to send.
const PBA_26K: u32 = 0x1a;
/// TIPG's transmit gap.
const TIPG_IPGT_MASK: u32 = 0x3ff;

/// MDIC's fields.
mod mdic {
    pub const REG_SHIFT: u32 = 16;
    pub const PHY_SHIFT: u32 = 21;
    pub const OP_WRITE: u32 = 1 << 26;
    pub const OP_READ: u32 = 2 << 26;
    pub const READY: u32 = 1 << 28;
    pub const ERROR: u32 = 1 << 30;
}

/// A PHY register as FreeBSD's PHY_REG names one: its page, then its
/// number on the page.
const fn phy_reg(page: u32, number: u32) -> u32 {
    (page << 5) | (number & 0x1f)
}

/// The PHY's registers the driver uses.
mod phy {
    use super::phy_reg;

    pub const CONTROL: u32 = 0;
    pub const ID1: u32 = 2;
    pub const ID2: u32 = 3;
    pub const AUTONEG_ADV: u32 = 4;
    pub const GIGABIT_CONTROL: u32 = 9;
    /// The 82579's and I219's own, on page 0.
    pub const EMI_ADDR: u32 = 0x10;
    pub const EMI_DATA: u32 = 0x11;
    pub const CTRL_2: u32 = 18;
    pub const CFG: u32 = 22;
    pub const M_STATUS: u32 = 26;
    /// On the pages from 768, at PHY address 1.
    pub const OEM_BITS: u32 = phy_reg(768, 25);
    pub const SMB_ADDR: u32 = phy_reg(768, 26);
    pub const LED_CONFIG: u32 = phy_reg(768, 30);
    pub const KMRN_MODE_CTRL: u32 = phy_reg(769, 16);
    pub const PORT_GEN_CFG: u32 = phy_reg(769, 17);
    pub const RX_PATH: u32 = phy_reg(769, 20);
    pub const SMB_CTRL: u32 = phy_reg(769, 23);
    pub const PAUSE_TIME: u32 = phy_reg(769, 27);
    pub const KMRN_FIFO_CTRLSTA: u32 = phy_reg(770, 16);
    pub const PM_CTRL: u32 = phy_reg(770, 17);
    pub const PLL_CLOCK_GATE: u32 = phy_reg(772, 28);
    pub const TX_PTR_GAP: u32 = phy_reg(776, 20);
    pub const TX_CFG: u32 = phy_reg(776, 23);
    pub const ULP_CONFIG1: u32 = phy_reg(779, 16);

    /// The page select register, and the first page at address 1.
    pub const PAGE_SELECT: u32 = 0x1f;
    pub const HV_PAGES: u32 = 768;
    pub const WAKE_UP_PAGE: u32 = 800;
    /// The registers above it on a page need the page selected.
    pub const MAX_UNPAGED: u32 = 0xf;

    /// The PHY IDs: the 82579's, and the I217's, which the I219 has.
    pub const ID_82579: u32 = 0x0154_0090;
    pub const ID_I217: u32 = 0x0154_00a0;
    pub const REVISION_MASK: u32 = 0xffff_fff0;

    pub const CONTROL_RESTART_AUTONEG: u16 = 1 << 9;
    pub const CONTROL_AUTONEG: u16 = 1 << 12;
    pub const ADV_10_HALF: u16 = 1 << 5;
    pub const ADV_10_FULL: u16 = 1 << 6;
    pub const ADV_100_HALF: u16 = 1 << 7;
    pub const ADV_100_FULL: u16 = 1 << 8;
    pub const ADV_PAUSE: u16 = 1 << 10;
    pub const ADV_ASM_DIR: u16 = 1 << 11;
    pub const GIGABIT_HALF: u16 = 1 << 8;
    pub const GIGABIT_FULL: u16 = 1 << 9;
    pub const CFG_ASSERT_CRS_ON_TX: u16 = 1 << 15;
    pub const CFG_ENABLE_DOWNSHIFT: u16 = 3 << 10;
    pub const CTRL_2_MDIX_MASK: u16 = 3 << 9;
    pub const CTRL_2_AUTO_MDIX: u16 = 1 << 10;
    pub const M_STATUS_LINK_UP: u16 = 1 << 6;
    pub const M_STATUS_SPEED_100: u16 = 1 << 8;
    pub const M_STATUS_SPEED_1000: u16 = 1 << 9;
    pub const M_STATUS_AUTONEG_COMPLETE: u16 = 1 << 12;
    pub const OEM_BITS_LPLU: u16 = 1 << 2;
    pub const OEM_BITS_GBE_DIS: u16 = 1 << 6;
    pub const OEM_BITS_RESTART_AN: u16 = 1 << 10;
    pub const SMB_ADDR_MASK: u16 = 0x7f;
    pub const SMB_ADDR_VALID: u16 = 1 << 7;
    pub const SMB_ADDR_PEC_EN: u16 = 1 << 9;
    pub const SMB_ADDR_FREQ_MASK: u16 = 0x1100;
    pub const SMB_ADDR_FREQ_LOW_SHIFT: u32 = 8;
    pub const SMB_ADDR_FREQ_HIGH_SHIFT: u32 = 12;
    pub const KMRN_MDIO_SLOW: u16 = 1 << 10;
    pub const PORT_GEN_CFG_HOST_WU: u16 = 1 << 4;
    pub const SMB_CTRL_FORCE_SMBUS: u16 = 1 << 0;
    pub const FIFO_PREAMBLE_MASK: u16 = 0x7 << 12;
    pub const FIFO_PREAMBLE_ONE: u16 = 1 << 12;
    pub const PM_CTRL_K1_CLK_REQ: u16 = 1 << 9;
    pub const PM_CTRL_K1_ENABLE: u16 = 1 << 14;
    pub const PLL_CLOCK_GATE_MASK: u16 = 0x7ff;
    pub const ULP_CONFIG1_START: u16 = 1 << 0;
    /// What ULP_CONFIG1 has cleared to leave ULP: the power-up
    /// indication, sticky ULP, reset to SMBus, wake on LAN to the host,
    /// in-band exit, ULP on LANPHYPC, and the PERST# settings.
    pub const ULP_CONFIG1_CLEAR: u16 =
        (1 << 2) | (1 << 4) | (1 << 8) | (1 << 6) | (1 << 5) | (1 << 10) | (1 << 11) | (1 << 12);

    /// The EMI registers, through EMI_ADDR and EMI_DATA.
    pub const EMI_82579_MSE_THRESHOLD: u16 = 0x084f;
    pub const EMI_82579_MSE_LINK_DOWN: u16 = 0x2411;
    pub const EMI_82579_RX_CONFIG: u16 = 0x3412;
    pub const EMI_82579_LPI_UPDATE_TIMER: u16 = 0x4805;
    pub const EMI_I217_RX_CONFIG: u16 = 0xb20c;
}

/// How long things may take.
const MDIC_US: u32 = 96_000;
const FLAG_FREE_US: u32 = 100_000;
const FLAG_TAKE_US: u32 = 1_000_000;
const RESET_BLOCK_US: u32 = 300_000;
const LAN_INIT_US: u32 = 150_000;
const ULP_US: u32 = 2_500_000;
const MASTER_US: u32 = 80_000;
const LPCD_US: u32 = 100_000;

/// The pause time and flow control refresh FreeBSD sets for these
/// chips: unused, with flow control off, but the PHY takes it too.
const PAUSE_TIME: u16 = 0xffff;
/// The largest frame, with its frame check sequence: what the latency
/// tolerance is worked out from.
const MAX_FRAME_FCS: i64 = 1518;

/// The descriptors in each ring, and the interrupt throttle: as
/// [`em`](crate::em) has them.
const RECEIVE: usize = 256;
const TRANSMIT: usize = 256;
const ITR: u32 = 1_000_000_000 / (20_000 * 256);
const TIPG: u32 = 8 | (8 << 10) | (6 << 20);

/// Which generation a chip is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mac {
    /// The 82579, FreeBSD's pch2lan.
    Pch2,
    /// The I219 of Sunrise Point and the chipsets that have it as
    /// Sunrise Point does, FreeBSD's pch_spt.
    Spt,
}

impl Mac {
    /// How far its registers go.
    fn end(self) -> usize {
        match self {
            Mac::Pch2 => reg::END_PCH2,
            Mac::Spt => reg::END_SPT,
        }
    }

    /// Whether its NVM is in a flash BAR of its own, BAR 1.
    pub fn flash_bar(self) -> bool {
        self == Mac::Pch2
    }
}

/// The chips, by device ID.
pub fn identify(device: u16) -> Option<(&'static str, Mac)> {
    Some(match device {
        0x1502 => ("82579LM", Mac::Pch2),
        0x1503 => ("82579V", Mac::Pch2),
        0x156f | 0x15b7 | 0x15b9 | 0x15d7 | 0x15e3 | 0x0d53 => ("I219-LM", Mac::Spt),
        0x1570 | 0x15b8 | 0x15d8 | 0x15d6 | 0x0d55 => ("I219-V", Mac::Spt),
        _ => return None,
    })
}

/// What a chip in the chipset needs from the machine beyond its first
/// BAR.
pub struct Platform<'a> {
    /// Its PCI function, whose configuration space the reset and the
    /// latency tolerance read.
    pub function: &'a Function,
    /// The 82579's flash registers, its BAR 1 ([`Mac::flash_bar`]).
    pub flash: Option<Mmio>,
    /// The time in nanoseconds, by which the running chip waits, as it
    /// cannot sleep.
    pub clock: fn() -> u64,
}

/// Resets the chip in the chipset `model`, whose registers are `regs`,
/// and sets it running, as [`crate::start`] does the other cards, with
/// what it needs of the machine in `platform`.
pub fn start(
    model: Model,
    regs: Mmio,
    platform: Platform<'_>,
    interrupt: Interrupt,
    fallback_mac: [u8; 6],
    alloc: &mut dyn FnMut(usize) -> Option<Dma>,
    delay: &mut dyn FnMut(u32),
) -> Result<Card, Error> {
    let Kind::Pch(mac) = model.kind else {
        return Err(Error::Unsupported("a card not in the chipset"));
    };
    if regs.len() < mac.end() {
        return Err(Error::Registers);
    }
    if regs.read32(reg::STATUS) == u32::MAX {
        return Err(Error::Device("its registers read as all ones"));
    }
    let Platform { function, flash, clock } = platform;
    let interlock =
        (mac == Mac::Pch2 && regs.read32(reg::FWSM) & FWSM_FW_VALID != 0).then_some(Interlock {
            busy_at: reg::FWSM,
            busy: FWSM_PCIM2PCI,
            clock,
            receive_enable: (reg::RCTL, rctl::EN),
            transmit_enable: (reg::TCTL, tctl::EN),
        });
    let csr = Csr { regs: regs.clone(), interlock, clock };
    let nvm = Nvm::new(mac, &regs, flash);
    let mut pch = Pch { mac, csr, function: function.clone(), nvm, link: Link::default() };
    pch.wake_phy(delay)?;
    let csr = &pch.csr;
    csr.write(reg::PBA, PBA_26K);
    if mac == Mac::Spt {
        csr.update(reg::FEXTNVM11, FEXTNVM11_DISABLE_MULR_FIX, 0);
    }
    pch.reset(delay)?;
    let csr = &pch.csr;
    csr.write(reg::WUFC, 0);
    pch.init(delay);
    let address = shared::registers_mac(&regs).unwrap_or(fallback_mac);
    pch.set_address(address);
    let sizes = (RECEIVE, TRANSMIT);
    let mut rings =
        Rings::interlocked(regs.clone(), rings::EM, Format::Legacy, sizes, interlock, alloc)?;
    rings.hand_over();
    let csr = &pch.csr;
    // No delays on receive or transmit interrupts: the throttle spaces
    // them.
    csr.write(reg::RDTR, 0);
    csr.write(reg::TIDV, 0);
    csr.write(reg::ITR, ITR);
    csr.write(reg::TIPG, TIPG);
    csr.write(reg::TCTL, tctl::EN | tctl::PSP | tctl::CT | tctl::COLD | tctl::RTLC | tctl::MULR);
    if mac == Mac::Spt {
        // The I219's errata: a receive threshold, and its transmit
        // arbitration (i218-i219 specification update 1.5.4.5).
        csr.update(reg::IOSFPC, IOSFPC_RDMTS_HEX, 0);
        csr.update(reg::TARC0, TARC0_CB_MULTIQ_2_REQ, TARC0_CB_MULTIQ_3_REQ);
    }
    pch.jumbo_off(delay);
    let csr = &pch.csr;
    csr.write(reg::RCTL, rctl::EN | rctl::BAM | rctl::MPE | rctl::SECRC);
    let _ = csr.read(reg::ICR);
    if interrupt == Interrupt::Msi {
        csr.write(reg::IMS, CAUSES);
    }
    // The ME leaves the chip to the driver from here.
    csr.update(reg::CTRL_EXT, shared::CTRL_EXT_DRV_LOAD, 0);
    let link = shared::link(&regs);
    pch.link = link;
    pch.on_link(link, delay);
    let mut card = Card::new(model.name, address, regs, rings, interrupt, &OPS);
    card.pch = Some(pch);
    Ok(card)
}

/// The chips stop through [`Pch::halt`]; this is for what [`Card`] asks
/// of every family.
static OPS: Ops = Ops {
    icr: reg::ICR,
    link: shared::link,
    halt: |regs| {
        regs.write32(reg::IMC, u32::MAX);
        regs.write32(reg::RCTL, 0);
        regs.write32(reg::TCTL, tctl::PSP);
    },
};

/// The MAC's registers, with the 82579's write interlock when its ME
/// runs: a write the host makes while the ME holds the PCIm-to-PCI
/// arbiter can be lost, so each write waits until it lets go, for up to
/// 100 ms (FreeBSD's e1000_pcim2pci_write).
struct Csr {
    regs: Mmio,
    interlock: Option<Interlock>,
    clock: fn() -> u64,
}

impl Csr {
    fn read(&self, at: usize) -> u32 {
        self.regs.read32(at)
    }

    fn write(&self, at: usize, value: u32) {
        match &self.interlock {
            Some(interlock) => interlock.write(&self.regs, at, value),
            None => self.regs.write32(at, value),
        }
    }

    /// Sets `bits` in the register at `at`, and clears `clear`.
    fn update(&self, at: usize, bits: u32, clear: u32) {
        let value = self.read(at);
        self.write(at, (value & !clear) | bits);
    }

    /// Lets the writes before it reach the chip.
    fn flush(&self) {
        let _ = self.read(reg::STATUS);
    }
}

/// Waits by the clock, for a running card, which cannot sleep.
fn spin(clock: fn() -> u64) -> impl FnMut(u32) {
    move |us| {
        let until = clock().saturating_add(u64::from(us) * 1_000);
        while clock() < until {
            core::hint::spin_loop();
        }
    }
}

/// Whether the PHY's page `page` is reached as [`Pch::phy`] reaches it:
/// page 0, and those from 768 but the wake-up registers' 800.
fn direct_page(page: u32) -> bool {
    page == 0 || (page >= phy::HV_PAGES && page != phy::WAKE_UP_PAGE)
}

/// What the driver keeps of a chip: what it was set up with, and the
/// link as it last saw it.
pub(crate) struct Pch {
    mac: Mac,
    csr: Csr,
    function: Function,
    nvm: Option<Nvm>,
    /// The link as the driver last saw it.
    link: Link,
}

impl Pch {
    /// Whether the ME's firmware runs.
    fn me(&self) -> bool {
        self.csr.read(reg::FWSM) & FWSM_FW_VALID != 0
    }

    /// Whether the ME blocks a reset of the PHY, as during an AMT
    /// Serial-over-LAN or IDE-R session: waits up to 300 ms for it to
    /// stop.
    fn reset_blocked(&self, delay: &mut dyn FnMut(u32)) -> bool {
        !wait_for(delay, RESET_BLOCK_US, 10_000, || self.csr.read(reg::FWSM) & FWSM_RSPCIPHY != 0)
    }

    /// Takes the software flag, which the ME, the hardware and the
    /// driver share for the PHY and the MAC's registers about it:
    /// whether it was had.
    fn acquire(&self, delay: &mut dyn FnMut(u32)) -> bool {
        let csr = &self.csr;
        if !wait_for(delay, FLAG_FREE_US, 1_000, || {
            csr.read(reg::EXTCNF_CTRL) & EXTCNF_CTRL_SWFLAG == 0
        }) {
            return false;
        }
        csr.update(reg::EXTCNF_CTRL, EXTCNF_CTRL_SWFLAG, 0);
        if wait_for(delay, FLAG_TAKE_US, 1_000, || {
            csr.read(reg::EXTCNF_CTRL) & EXTCNF_CTRL_SWFLAG != 0
        }) {
            return true;
        }
        csr.update(reg::EXTCNF_CTRL, 0, EXTCNF_CTRL_SWFLAG);
        false
    }

    fn release(&self) {
        if self.csr.read(reg::EXTCNF_CTRL) & EXTCNF_CTRL_SWFLAG != 0 {
            self.csr.update(reg::EXTCNF_CTRL, 0, EXTCNF_CTRL_SWFLAG);
        }
    }

    /// Gates the hardware's configuration of the PHY from the NVM, or
    /// lets it.
    fn gate_phy_config(&self, gate: bool) {
        if gate {
            self.csr.update(reg::EXTCNF_CTRL, EXTCNF_CTRL_GATE_PHY_CFG, 0);
        } else {
            self.csr.update(reg::EXTCNF_CTRL, 0, EXTCNF_CTRL_GATE_PHY_CFG);
        }
    }

    /// One MDIO transaction with the PHY at `address`: reads register
    /// `number`, or writes `write` to it.
    fn mdic(
        &self,
        address: u32,
        number: u32,
        write: Option<u16>,
        delay: &mut dyn FnMut(u32),
    ) -> Option<u16> {
        let op = match write {
            Some(data) => mdic::OP_WRITE | u32::from(data),
            None => mdic::OP_READ,
        };
        let command = op | (number << mdic::REG_SHIFT) | (address << mdic::PHY_SHIFT);
        self.csr.write(reg::MDIC, command);
        let mut value = 0;
        let done = wait_for(delay, MDIC_US, 50, || {
            value = self.csr.read(reg::MDIC);
            value & mdic::READY != 0
        });
        // The 82579 can hand the next transaction this one's data.
        if self.mac == Mac::Pch2 {
            delay(100);
        }
        let answered = (value >> mdic::REG_SHIFT) & 0x1f == number;
        (done && value & mdic::ERROR == 0 && answered).then_some(value as u16)
    }

    /// Reads or writes a PHY register (a [`phy_reg`]), selecting its page
    /// first where it needs one: the registers of page 0 are at PHY
    /// address 2, those of the pages from 768 at address 1, with 768
    /// selected as page 0. The software flag is held. The pages between,
    /// and the wake-up page, are reached otherwise, which the driver does
    /// not: none, for them.
    fn phy(&self, register: u32, write: Option<u16>, delay: &mut dyn FnMut(u32)) -> Option<u16> {
        let page = register >> 5;
        let number = register & 0x1f;
        if !direct_page(page) {
            return None;
        }
        let address = if page >= phy::HV_PAGES { 1 } else { 2 };
        if number > phy::MAX_UNPAGED {
            let page = if page == phy::HV_PAGES { 0 } else { page };
            self.mdic(1, phy::PAGE_SELECT, Some((page << 5) as u16), delay)?;
        }
        self.mdic(address, number, write, delay)
    }

    fn phy_read(&self, register: u32, delay: &mut dyn FnMut(u32)) -> Option<u16> {
        self.phy(register, None, delay)
    }

    fn phy_write(&self, register: u32, value: u16, delay: &mut dyn FnMut(u32)) -> bool {
        self.phy(register, Some(value), delay).is_some()
    }

    /// Sets `bits` in a PHY register and clears `clear`: whether it could.
    fn phy_update(&self, register: u32, bits: u16, clear: u16, delay: &mut dyn FnMut(u32)) -> bool {
        match self.phy_read(register, delay) {
            Some(value) => self.phy_write(register, (value & !clear) | bits, delay),
            None => false,
        }
    }

    /// Writes an EMI register, through the PHY's EMI address and data.
    fn emi_write(&self, address: u16, value: u16, delay: &mut dyn FnMut(u32)) -> bool {
        self.phy_write(phy::EMI_ADDR, address, delay) && self.phy_write(phy::EMI_DATA, value, delay)
    }

    /// Reads a Kumeran register, of the MAC-PHY link.
    fn kmrn_read(&self, offset: u32, delay: &mut dyn FnMut(u32)) -> u16 {
        let at = (offset << KMRN_OFFSET_SHIFT) & KMRN_OFFSET_MASK;
        self.csr.write(reg::KMRNCTRLSTA, at | KMRN_REN);
        self.csr.flush();
        delay(2);
        self.csr.read(reg::KMRNCTRLSTA) as u16
    }

    fn kmrn_write(&self, offset: u32, data: u16, delay: &mut dyn FnMut(u32)) {
        let at = (offset << KMRN_OFFSET_SHIFT) & KMRN_OFFSET_MASK;
        self.csr.write(reg::KMRNCTRLSTA, at | u32::from(data));
        self.csr.flush();
        delay(2);
    }

    /// The PHY's ID, if it answers on MDIO: two tries, as the PHY may
    /// not answer while its link to the MAC changes. The flag is held.
    fn phy_id(&self, delay: &mut dyn FnMut(u32)) -> Option<u32> {
        for _ in 0..2 {
            let Some(high) = self.phy_read(phy::ID1, delay).filter(|&id| id != 0xffff) else {
                continue;
            };
            let Some(low) = self.phy_read(phy::ID2, delay).filter(|&id| id != 0xffff) else {
                continue;
            };
            let id = (u32::from(high) << 16) | (u32::from(low) & phy::REVISION_MASK);
            if id != 0 {
                return Some(id);
            }
        }
        None
    }

    /// The PHY's ID, if it answers, with the flag held; on the 82579,
    /// after putting MDIO in slow mode if it does not at first. An I219
    /// that answers, with no ME, is taken off SMBus in the PHY and the
    /// MAC.
    fn phy_answers(&self, delay: &mut dyn FnMut(u32)) -> Option<u32> {
        let id = self.phy_id(delay).or_else(|| {
            let slow = self.mac == Mac::Pch2 && self.set_mdio_slow(delay);
            slow.then(|| self.phy_id(delay)).flatten()
        });
        if id.is_some() && self.mac == Mac::Spt && !self.me() {
            let _ = self.phy_update(phy::SMB_CTRL, 0, phy::SMB_CTRL_FORCE_SMBUS, delay);
            self.csr.update(reg::CTRL_EXT, 0, CTRL_EXT_FORCE_SMBUS);
        }
        id
    }

    /// Puts the 82579's MDIO in slow mode, which some of its PHYs need:
    /// whether it could. The flag is held.
    fn set_mdio_slow(&self, delay: &mut dyn FnMut(u32)) -> bool {
        self.phy_update(phy::KMRN_MODE_CTRL, phy::KMRN_MDIO_SLOW, 0, delay)
    }

    /// Power-cycles the PHY by its LANPHYPC pin, which also brings its
    /// link to the MAC back to PCI Express from SMBus.
    fn toggle_lanphypc(&self, delay: &mut dyn FnMut(u32)) {
        let csr = &self.csr;
        csr.update(reg::FEXTNVM3, FEXTNVM3_PHY_CFG_COUNTER_50MS, FEXTNVM3_PHY_CFG_COUNTER_MASK);
        let control = (csr.read(reg::CTRL) | CTRL_LANPHYPC_OVERRIDE) & !CTRL_LANPHYPC_VALUE;
        csr.write(reg::CTRL, control);
        csr.flush();
        delay(1_000);
        csr.write(reg::CTRL, control & !CTRL_LANPHYPC_OVERRIDE);
        csr.flush();
        match self.mac {
            Mac::Pch2 => delay(50_000),
            Mac::Spt => {
                wait_for(delay, LPCD_US, 5_000, || csr.read(reg::CTRL_EXT) & CTRL_EXT_LPCD != 0);
                delay(30_000);
            }
        }
    }

    /// Takes the I219's PHY out of ultra low power mode: by asking the
    /// ME, which runs it then, or with no ME by power-cycling it and
    /// clearing its ULP settings, then resetting it. FreeBSD's
    /// e1000_disable_ulp_lpt_lp, forced.
    fn leave_ulp(&self, delay: &mut dyn FnMut(u32)) {
        let csr = &self.csr;
        if self.me() {
            csr.update(reg::H2ME, H2ME_ENFORCE_SETTINGS, H2ME_ULP);
            // The ME clears ULP_CFG_DONE once the PHY is out of ULP; one
            // that does not is left the request, as FreeBSD leaves it.
            if wait_for(delay, ULP_US, 10_000, || csr.read(reg::FWSM) & FWSM_ULP_CFG_DONE == 0) {
                csr.update(reg::H2ME, 0, H2ME_ENFORCE_SETTINGS);
            }
            return;
        }
        if !self.acquire(delay) {
            return;
        }
        self.toggle_lanphypc(delay);
        let _ = (|| {
            // The MAC may be in PCI Express mode with the PHY on SMBus.
            let smb = match self.phy_read(phy::SMB_CTRL, delay) {
                Some(smb) => smb,
                None => {
                    csr.update(reg::CTRL_EXT, CTRL_EXT_FORCE_SMBUS, 0);
                    delay(50_000);
                    self.phy_read(phy::SMB_CTRL, delay)?
                }
            };
            self.phy_write(phy::SMB_CTRL, smb & !phy::SMB_CTRL_FORCE_SMBUS, delay);
            csr.update(reg::CTRL_EXT, 0, CTRL_EXT_FORCE_SMBUS);
            // ULP turned K1 off; it comes back on.
            let k1 = self.phy_read(phy::PM_CTRL, delay)?;
            self.phy_write(phy::PM_CTRL, k1 | phy::PM_CTRL_K1_ENABLE, delay);
            let ulp = self.phy_read(phy::ULP_CONFIG1, delay)? & !phy::ULP_CONFIG1_CLEAR;
            self.phy_write(phy::ULP_CONFIG1, ulp, delay);
            // Committed by starting the PHY's own ULP configuration.
            self.phy_write(phy::ULP_CONFIG1, ulp | phy::ULP_CONFIG1_START, delay);
            csr.update(reg::FEXTNVM7, 0, FEXTNVM7_DISABLE_SMB_PERST);
            Some(())
        })();
        self.release();
        self.reset_phy(delay);
        delay(50_000);
    }

    /// Makes the PHY answer and resets it, before the driver touches it
    /// further: FreeBSD's e1000_init_phy_workarounds_pchlan, with the
    /// check of the PHY's ID e1000_init_phy_params_pchlan makes.
    fn wake_phy(&self, delay: &mut dyn FnMut(u32)) -> Result<(), Error> {
        let me = self.me();
        self.gate_phy_config(true);
        if self.mac == Mac::Spt {
            self.leave_ulp(delay);
        }
        let result = self.reach_phy(delay);
        if result.is_ok() && !self.reset_blocked(delay) {
            self.reset_phy_only(delay);
        }
        if self.mac == Mac::Pch2 && !me {
            delay(10_000);
            self.gate_phy_config(false);
        }
        let id = match result? {
            Some(id) => Some(id),
            None => {
                if !self.acquire(delay) {
                    return Err(Error::Timeout("taking the PHY from the ME"));
                }
                let id = self
                    .phy_id(delay)
                    .or_else(|| self.set_mdio_slow(delay).then(|| self.phy_id(delay)).flatten());
                self.release();
                id
            }
        };
        let expected = match self.mac {
            Mac::Pch2 => phy::ID_82579,
            Mac::Spt => phy::ID_I217,
        };
        match id {
            Some(id) if id == expected => Ok(()),
            Some(_) => Err(Error::Device("its PHY is not the one its MAC goes with")),
            None => Err(Error::Device("its PHY does not answer")),
        }
    }

    /// Reaches the PHY: if it does not answer, on the I219 by forcing
    /// the MAC to SMBus, then on either by power-cycling the PHY, which
    /// the ME may block. Its ID, as it answered; none for an 82579 that
    /// was power-cycled, which is read after its reset, as FreeBSD reads
    /// it.
    fn reach_phy(&self, delay: &mut dyn FnMut(u32)) -> Result<Option<u32>, Error> {
        if !self.acquire(delay) {
            return Err(Error::Timeout("taking the PHY from the ME"));
        }
        let result = (|| {
            if let Some(id) = self.phy_answers(delay) {
                return Ok(Some(id));
            }
            if self.mac == Mac::Spt {
                self.csr.update(reg::CTRL_EXT, CTRL_EXT_FORCE_SMBUS, 0);
                // The MAC may still be retrying earlier reads.
                delay(50_000);
                if let Some(id) = self.phy_answers(delay) {
                    return Ok(Some(id));
                }
            }
            if self.reset_blocked(delay) {
                return Err(Error::Device("the ME blocks power-cycling its PHY"));
            }
            self.toggle_lanphypc(delay);
            if self.mac == Mac::Pch2 {
                return Ok(None);
            }
            if let Some(id) = self.phy_answers(delay) {
                return Ok(Some(id));
            }
            // The PHY came off SMBus; so does the MAC.
            self.csr.update(reg::CTRL_EXT, 0, CTRL_EXT_FORCE_SMBUS);
            self.phy_answers(delay).map(Some).ok_or(Error::Device("its PHY does not answer"))
        })();
        self.release();
        result
    }

    /// Resets the PHY alone, by CTRL's PHY reset, and waits for the
    /// chip to configure it: FreeBSD's e1000_phy_hw_reset_generic.
    fn reset_phy_only(&self, delay: &mut dyn FnMut(u32)) {
        if self.reset_blocked(delay) || !self.acquire(delay) {
            return;
        }
        let control = self.csr.read(reg::CTRL);
        self.csr.write(reg::CTRL, control | ctrl::PHY_RST);
        self.csr.flush();
        delay(100);
        self.csr.write(reg::CTRL, control);
        self.csr.flush();
        delay(150);
        self.release();
        self.configured(delay);
    }

    /// Resets the PHY and does what it needs after: FreeBSD's
    /// e1000_phy_hw_reset_ich8lan.
    fn reset_phy(&self, delay: &mut dyn FnMut(u32)) {
        if self.mac == Mac::Pch2 && !self.me() {
            self.gate_phy_config(true);
        }
        self.reset_phy_only(delay);
        self.after_phy_reset(delay);
    }

    /// Waits for the chip to configure the PHY after a reset, and clears
    /// what says it did: FreeBSD's e1000_get_cfg_done_ich8lan.
    fn configured(&self, delay: &mut dyn FnMut(u32)) {
        let csr = &self.csr;
        delay(10_000);
        wait_for(delay, LAN_INIT_US, 100, || csr.read(reg::STATUS) & STATUS_LAN_INIT_DONE != 0);
        csr.update(reg::STATUS, 0, STATUS_LAN_INIT_DONE);
        let status = csr.read(reg::STATUS);
        if status & STATUS_PHYRA != 0 {
            csr.write(reg::STATUS, status & !STATUS_PHYRA);
        }
    }

    /// What the PHY needs after a reset: FreeBSD's
    /// e1000_post_phy_reset_ich8lan.
    fn after_phy_reset(&self, delay: &mut dyn FnMut(u32)) {
        if self.reset_blocked(delay) {
            return;
        }
        delay(10_000);
        if !self.acquire(delay) {
            return;
        }
        if self.mac == Mac::Pch2 {
            // Slow MDIO first; then links stay up through more noise,
            // and go down after five times the threshold.
            self.set_mdio_slow(delay);
            self.emi_write(phy::EMI_82579_MSE_THRESHOLD, 0x0034, delay);
            self.emi_write(phy::EMI_82579_MSE_LINK_DOWN, 0x0005, delay);
        }
        // No wake-up for the host: the PHY was reset.
        self.phy_update(phy::PORT_GEN_CFG, 0, phy::PORT_GEN_CFG_HOST_WU, delay);
        self.configure_phy_from_nvm(delay);
        self.oem_bits(delay);
        self.release();
        if self.mac == Mac::Pch2 {
            if !self.me() {
                delay(10_000);
                self.gate_phy_config(false);
            }
            // EEE's LPI update timer, 200 µs.
            if self.acquire(delay) {
                self.emi_write(phy::EMI_82579_LPI_UPDATE_TIMER, 0x1387, delay);
                self.release();
            }
        }
    }

    /// Writes the PHY the configuration in the NVM's extended region,
    /// where the NVM leaves that to software, with its SMBus address and
    /// LED settings first: FreeBSD's e1000_sw_lcd_config_ich8lan. The
    /// flag is held.
    fn configure_phy_from_nvm(&self, delay: &mut dyn FnMut(u32)) {
        let csr = &self.csr;
        if csr.read(reg::FEXTNVM) & FEXTNVM_SW_CONFIG == 0 {
            return;
        }
        let words = (csr.read(reg::EXTCNF_SIZE) >> EXTCNF_LENGTH_SHIFT) & EXTCNF_LENGTH_MASK;
        if words == 0 {
            return;
        }
        let start =
            ((csr.read(reg::EXTCNF_CTRL) >> EXTCNF_POINTER_SHIFT) & EXTCNF_POINTER_MASK) << 1;
        if !self.write_smbus_address(delay) {
            return;
        }
        if !self.phy_write(phy::LED_CONFIG, csr.read(reg::LEDCTL) as u16, delay) {
            return;
        }
        let Some(nvm) = &self.nvm else { return };
        let bank = nvm.bank(delay);
        let mut page = 0u32;
        for pair in 0..words {
            let at = bank + start + pair * 2;
            let (Some(data), Some(number)) = (nvm.word(at, delay), nvm.word(at + 1, delay)) else {
                return;
            };
            if u32::from(number) == phy::PAGE_SELECT {
                // The page, as the PHY's page select takes it: times 32.
                page = u32::from(data);
                continue;
            }
            let register = (u32::from(number) & 0x1f) | page;
            // A page reached otherwise is left as it is.
            if direct_page(register >> 5) && !self.phy_write(register, data, delay) {
                return;
            }
        }
    }

    /// Tells the PHY its SMBus address and clock, from STRAP. The flag
    /// is held.
    fn write_smbus_address(&self, delay: &mut dyn FnMut(u32)) -> bool {
        let strap = self.csr.read(reg::STRAP);
        let Some(mut value) = self.phy_read(phy::SMB_ADDR, delay) else { return false };
        value &= !phy::SMB_ADDR_MASK;
        value |= ((strap >> 17) & 0x7f) as u16;
        value |= phy::SMB_ADDR_PEC_EN | phy::SMB_ADDR_VALID;
        if self.mac == Mac::Spt {
            let frequency = (strap >> 12) & 0x3;
            if let Some(frequency) = frequency.checked_sub(1) {
                value &= !phy::SMB_ADDR_FREQ_MASK;
                value |= ((frequency & 1) << phy::SMB_ADDR_FREQ_LOW_SHIFT) as u16;
                value |= ((frequency & 2) << (phy::SMB_ADDR_FREQ_HIGH_SHIFT - 1)) as u16;
            }
        }
        self.phy_write(phy::SMB_ADDR, value, delay)
    }

    /// Sets the PHY's gigabit disable and low power link up as the MAC's
    /// PHY_CTRL has them, where the NVM leaves that to software, and has
    /// it negotiate again: FreeBSD's e1000_oem_bits_config_ich8lan, for
    /// a running host. The flag is held.
    fn oem_bits(&self, delay: &mut dyn FnMut(u32)) {
        let csr = &self.csr;
        if csr.read(reg::FEXTNVM) & FEXTNVM_SW_CONFIG == 0 {
            return;
        }
        let control = csr.read(reg::PHY_CTRL);
        let Some(mut bits) = self.phy_read(phy::OEM_BITS, delay) else { return };
        bits &= !(phy::OEM_BITS_GBE_DIS | phy::OEM_BITS_LPLU);
        if control & PHY_CTRL_GBE_DISABLE != 0 {
            bits |= phy::OEM_BITS_GBE_DIS;
        }
        if control & PHY_CTRL_D0A_LPLU != 0 {
            bits |= phy::OEM_BITS_LPLU;
        }
        if !self.reset_blocked(delay) {
            bits |= phy::OEM_BITS_RESTART_AN;
        }
        self.phy_write(phy::OEM_BITS, bits, delay);
    }

    /// Resets the MAC and PHY together, with the flag held, and does
    /// what the PHY needs after: FreeBSD's e1000_reset_hw_ich8lan.
    fn reset(&self, delay: &mut dyn FnMut(u32)) -> Result<(), Error> {
        let csr = &self.csr;
        // A reset with a bus request in flight can hang the bus. A chip
        // that does not stop is reset all the same, as FreeBSD does.
        csr.update(reg::CTRL, ctrl::GIO_MASTER_DISABLE, 0);
        wait_for(delay, MASTER_US, 100, || csr.read(reg::STATUS) & status::GIO_MASTER_ENABLE == 0);
        csr.write(reg::IMC, u32::MAX);
        csr.write(reg::RCTL, 0);
        csr.write(reg::TCTL, tctl::PSP);
        csr.flush();
        delay(10_000);
        let mut control = csr.read(reg::CTRL);
        if !self.reset_blocked(delay) {
            // Both at once, so that their link is reset too.
            control |= ctrl::PHY_RST;
            if self.mac == Mac::Pch2 && !self.me() {
                self.gate_phy_config(true);
            }
        }
        let _ = self.acquire(delay);
        // A configuration read before the reset, and one after, give the
        // chip the time a register read during the reset would take from
        // it, which could hang the machine.
        csr.write(reg::STRAP, u32::from(self.function.read16(0)));
        csr.write(reg::CTRL, control | ctrl::RST);
        // No read here: it could hang the chip.
        delay(20_000);
        csr.write(reg::STRAP, u32::from(self.function.read16(0)));
        if csr.read(reg::CTRL) & ctrl::RST != 0 {
            return Err(Error::Timeout("a reset"));
        }
        if self.mac == Mac::Pch2 {
            csr.update(reg::FEXTNVM3, FEXTNVM3_PHY_CFG_COUNTER_50MS, FEXTNVM3_PHY_CFG_COUNTER_MASK);
        }
        if control & ctrl::PHY_RST != 0 {
            self.configured(delay);
            self.after_phy_reset(delay);
        }
        csr.write(reg::IMC, u32::MAX);
        let _ = csr.read(reg::ICR);
        csr.update(reg::KABGTXD, KABGTXD_BGSQLBIAS, 0);
        Ok(())
    }

    /// Sets the chip up after a reset: FreeBSD's e1000_init_hw_ich8lan,
    /// with e1000_initialize_hw_bits_ich8lan's errata settings and the
    /// link set up as e1000_setup_link_ich8lan does, flow control off.
    fn init(&self, delay: &mut dyn FnMut(u32)) {
        let csr = &self.csr;
        csr.update(reg::CTRL_EXT, (1 << 22) | CTRL_EXT_PHYPDEN, 0);
        csr.update(reg::TXDCTL0, TXDCTL_BIT_22, 0);
        csr.update(reg::TXDCTL1, TXDCTL_BIT_22, 0);
        csr.update(reg::TARC0, (1 << 23) | (1 << 24) | (1 << 26) | (1 << 27), 0);
        // Bit 28 goes with the transmitter's MULR, which is not on yet.
        let tarc1 = if csr.read(reg::TCTL) & tctl::MULR != 0 { (0, 1 << 28) } else { (1 << 28, 0) };
        csr.update(reg::TARC1, tarc1.0 | (1 << 24) | (1 << 26) | (1 << 30), tarc1.1);
        // NFS filtering can corrupt descriptors.
        csr.update(reg::RFCTL, RFCTL_NFSW_DIS | RFCTL_NFSR_DIS, 0);
        if self.mac == Mac::Spt {
            csr.update(reg::PBECCSTS, PBECCSTS_ECC_ENABLE, 0);
            csr.update(reg::CTRL, CTRL_MEHE, 0);
        }
        for word in 0..32 {
            csr.write(reg::MTA + 4 * word, 0);
        }
        // The link is the ME's while it blocks PHY resets: left as it is.
        if !self.reset_blocked(delay) {
            self.set_up_copper(delay);
            csr.write(reg::FCTTV, u32::from(PAUSE_TIME));
            csr.write(reg::FCRTV_PCH, u32::from(PAUSE_TIME));
            if self.acquire(delay) {
                self.phy_write(phy::PAUSE_TIME, PAUSE_TIME, delay);
                self.release();
            }
            // No pause frames sent: no thresholds.
            csr.write(reg::FCRTL, 0);
            csr.write(reg::FCRTH, 0);
        }
        for at in [reg::TXDCTL0, reg::TXDCTL1] {
            let value = csr.read(at);
            let value = (value & !TXDCTL_WTHRESH) | TXDCTL_FULL_TX_DESC_WB;
            csr.write(at, (value & !TXDCTL_PTHRESH) | TXDCTL_MAX_TX_DESC_PREFETCH);
        }
        // Snooping on for every kind of access: GCR is written as
        // Intel's shared code writes it, every bit but the no-snoop ones
        // set.
        let gcr = csr.read(reg::GCR) & !GCR_NO_SNOOP_ALL;
        csr.write(reg::GCR, gcr | !GCR_NO_SNOOP_ALL);
        csr.update(reg::CTRL_EXT, CTRL_EXT_RO_DIS, 0);
    }

    /// Sets the copper link up: the MAC takes its speed from the PHY,
    /// which advertises every speed to 1000 Mb/s full duplex and no
    /// pause, with automatic crossover and downshift, and negotiates.
    fn set_up_copper(&self, delay: &mut dyn FnMut(u32)) {
        let csr = &self.csr;
        csr.update(reg::CTRL, ctrl::SLU, ctrl::FRCSPD | ctrl::FRCDPX | ctrl::RFCE | ctrl::TFCE);
        if !self.acquire(delay) {
            return;
        }
        if self.mac == Mac::Pch2 {
            // The longest waits between polls of the PHY, which keeps
            // them from timing out at 10 Mb/s.
            self.kmrn_write(KMRN_TIMEOUTS, 0xffff, delay);
            let inband = self.kmrn_read(KMRN_INBAND_PARAM, delay);
            self.kmrn_write(KMRN_INBAND_PARAM, inband | 0x3f, delay);
        }
        self.phy_update(phy::CFG, phy::CFG_ASSERT_CRS_ON_TX | phy::CFG_ENABLE_DOWNSHIFT, 0, delay);
        self.phy_update(phy::CTRL_2, phy::CTRL_2_AUTO_MDIX, phy::CTRL_2_MDIX_MASK, delay);
        self.phy_update(
            phy::AUTONEG_ADV,
            phy::ADV_10_HALF | phy::ADV_10_FULL | phy::ADV_100_HALF | phy::ADV_100_FULL,
            phy::ADV_PAUSE | phy::ADV_ASM_DIR,
            delay,
        );
        self.phy_update(phy::GIGABIT_CONTROL, phy::GIGABIT_FULL, phy::GIGABIT_HALF, delay);
        self.phy_update(
            phy::CONTROL,
            phy::CONTROL_AUTONEG | phy::CONTROL_RESTART_AUTONEG,
            0,
            delay,
        );
        self.release();
    }

    /// Puts the receive path back as it is for frames of 1500 bytes:
    /// FreeBSD's e1000_lv_jumbo_workaround_ich8lan, turned off, which
    /// FreeBSD runs each time it sets the receiver up. RCTL is written
    /// after.
    fn jumbo_off(&self, delay: &mut dyn FnMut(u32)) {
        if !self.acquire(delay) {
            return;
        }
        let csr = &self.csr;
        // The receive path off meanwhile.
        let path = self.phy_read(phy::RX_PATH, delay).unwrap_or(0);
        if self.phy_write(phy::RX_PATH, path | (1 << 14), delay) {
            csr.update(reg::FFLT_DBG, 0, 0xf << 14);
            csr.update(reg::RCTL, 0, rctl::SECRC);
            let control = self.kmrn_read(KMRN_CTRL, delay);
            self.kmrn_write(KMRN_CTRL, control & !1, delay);
            let hd = self.kmrn_read(KMRN_HD_CTRL, delay);
            self.kmrn_write(KMRN_HD_CTRL, (hd & !(0xf << 8)) | (0xb << 8), delay);
            self.phy_update(phy::SMB_CTRL, 0, 0x7f << 5, delay);
            self.phy_update(phy::KMRN_MODE_CTRL, 1 << 13, 0, delay);
            self.phy_update(phy::TX_PTR_GAP, 0x8 << 2, 0x3ff << 2, delay);
            self.phy_write(phy::TX_CFG, 0x7e00, delay);
            self.phy_update(phy::PM_CTRL, 0, 1 << 10, delay);
            self.phy_write(phy::RX_PATH, path & !(1 << 14), delay);
        }
        self.release();
    }

    /// Makes `address` the chip's one unicast address, in RAL0 and RAH0.
    /// The shared receive addresses after them are left as they are: the
    /// ME may have them.
    fn set_address(&self, address: [u8; 6]) {
        let low = u32::from_le_bytes([address[0], address[1], address[2], address[3]]);
        let high = u32::from(u16::from_le_bytes([address[4], address[5]])) | (1 << 31);
        self.csr.write(reg::RAL0, low);
        self.csr.flush();
        self.csr.write(reg::RAH0, high);
        self.csr.flush();
    }

    /// Looks at the link, and sets the chip up for it if it changed.
    pub(crate) fn look(&mut self) {
        let link = shared::link(&self.csr.regs);
        if link != self.link {
            self.link = link;
            let mut delay = spin(self.csr.clock);
            self.on_link(link, &mut delay);
        }
    }

    /// Sets the chip up for the link it has: FreeBSD's
    /// e1000_check_for_copper_link_ich8lan, without EEE.
    fn on_link(&self, link: Link, delay: &mut dyn FnMut(u32)) {
        let csr = &self.csr;
        if link.up {
            let half_10 = link.mbps == 10 && !link.full_duplex;
            let (gap, emi) = if half_10 {
                // Fewer collisions, and less receive latency in the PHY.
                (0xff, 0)
            } else if self.mac == Mac::Spt && link.full_duplex && link.mbps != 1000 {
                (0xc, 1)
            } else {
                (0x08, 1)
            };
            csr.update(reg::TIPG, gap, TIPG_IPGT_MASK);
            if self.acquire(delay) {
                let rx_config = match self.mac {
                    Mac::Pch2 => phy::EMI_82579_RX_CONFIG,
                    Mac::Spt => phy::EMI_I217_RX_CONFIG,
                };
                self.emi_write(rx_config, emi, delay);
                if self.mac == Mac::Spt {
                    let gate = if link.mbps == 1000 { 0xfa } else { 0x3e8 };
                    self.phy_update(phy::PLL_CLOCK_GATE, gate, phy::PLL_CLOCK_GATE_MASK, delay);
                    if link.mbps == 1000 {
                        self.phy_update(phy::PM_CTRL, phy::PM_CTRL_K1_CLK_REQ, 0, delay);
                        // At least 0x18 between the transmit pointers.
                        if let Some(gap) = self.phy_read(phy::TX_PTR_GAP, delay)
                            && (gap >> 2) & 0x3ff < 0x18
                        {
                            self.phy_write(
                                phy::TX_PTR_GAP,
                                (gap & !(0x3ff << 2)) | (0x18 << 2),
                                delay,
                            );
                        }
                    } else {
                        self.phy_write(phy::TX_PTR_GAP, 0xc023, delay);
                    }
                }
                self.release();
            }
        }
        if self.mac == Mac::Spt {
            // I217 packet loss: an 8 µs beacon.
            csr.update(reg::FEXTNVM4, FEXTNVM4_BEACON_8US, FEXTNVM4_BEACON_MASK);
            self.report_latency(link);
            // K1 off as the analog configuration has it, and K0s's
            // shortest time.
            let k1_off = csr.read(reg::PCIEANACFG) & FEXTNVM6_K1_OFF_ENABLE;
            csr.update(reg::FEXTNVM6, k1_off, FEXTNVM6_K1_OFF_ENABLE);
            if self.acquire(delay) {
                let k0s = self.kmrn_read(KMRN_K0S_CTRL, delay);
                let k0s = (k0s & !KMRN_K0S_MIN_TIME_MASK) | (1 << KMRN_K0S_MIN_TIME_SHIFT);
                self.kmrn_write(KMRN_K0S_CTRL, k0s, delay);
                self.release();
            }
        }
        if link.up && self.mac == Mac::Pch2 && self.acquire(delay) {
            // K1 off at 1000 and 100 Mb/s, where it drops frames; a longer
            // beacon at 10.
            if let Some(status) = self.phy_read(phy::M_STATUS, delay) {
                let up = phy::M_STATUS_LINK_UP | phy::M_STATUS_AUTONEG_COMPLETE;
                if status & up == up {
                    if status & (phy::M_STATUS_SPEED_1000 | phy::M_STATUS_SPEED_100) != 0 {
                        self.phy_update(phy::PM_CTRL, 0, phy::PM_CTRL_K1_ENABLE, delay);
                    } else {
                        csr.update(reg::FEXTNVM4, FEXTNVM4_BEACON_16US, FEXTNVM4_BEACON_MASK);
                    }
                }
            }
            // At half duplex, one preamble taken off between the PHY and
            // the MAC, so that the MAC reads the frame's type right.
            let preamble = if link.full_duplex { 0 } else { phy::FIFO_PREAMBLE_ONE };
            self.phy_update(phy::KMRN_FIFO_CTRLSTA, preamble, phy::FIFO_PREAMBLE_MASK, delay);
            self.release();
        }
    }

    /// Tells the platform how long the I219 can wait for memory before
    /// its receive buffer fills, so that it is not put to sleep longer,
    /// and when to fill and flush its buffer (OBFF): FreeBSD's
    /// e1000_platform_pm_pch_lpt.
    fn report_latency(&self, link: Link) {
        let csr = &self.csr;
        let mut ltrv = LTRV_SEND;
        let mut obff = 0u32;
        let mut encoded = 0u32;
        if link.up && link.mbps != 0 {
            let speed = i64::from(link.mbps);
            let buffer = i64::from(csr.read(reg::PBA) & 0xffff);
            let mut latency = ((buffer * 1024 - 2 * MAX_FRAME_FCS) * 8 * 1000).max(0) / speed;
            let (mut value, mut scale) = (latency as u64, 0u32);
            while value > LTRV_VALUE_MASK {
                scale += 1;
                value = value.div_ceil(32);
            }
            if scale > LTRV_SCALE_MAX {
                return;
            }
            encoded = (scale << LTRV_SCALE_SHIFT) | value as u32;
            let snoop = self.function.read16(PCI_LTR_CAP);
            let no_snoop = self.function.read16(PCI_LTR_CAP + 2);
            let most = u32::from(snoop.max(no_snoop));
            if latency_ns(encoded) > latency_ns(most) {
                encoded = most;
                latency = latency_ns(most) as i64;
            }
            let mut mark = 0i64;
            if latency != 0 {
                mark = buffer - latency * speed * 1000 / 8 / 1_000_000_000;
            }
            if !(0..=i64::from(SVT_OFF_HWM_MASK)).contains(&mark) {
                return;
            }
            obff = mark as u32;
            ltrv |= LTRV_REQ | (LTRV_REQ << LTRV_NOSNOOP_SHIFT);
        }
        csr.write(reg::LTRV, ltrv | encoded | (encoded << LTRV_NOSNOOP_SHIFT));
        csr.update(reg::SVT, obff, SVT_OFF_HWM_MASK);
        // Interrupts reach the CPU even in OBFF, so that a little
        // traffic is not held back.
        csr.update(reg::SVCR, SVCR_OFF_EN | SVCR_OFF_MASKINT, 0);
    }

    /// Stops the chip: interrupts off, then the receiver and transmitter,
    /// then its DMA, and gives it back to its firmware. It is not reset:
    /// an I219 reset with its rings loaded can hang it, and the next
    /// driver resets it as this one does.
    pub(crate) fn halt(&self) {
        let csr = &self.csr;
        let mut delay = spin(csr.clock);
        csr.write(reg::IMC, u32::MAX);
        csr.write(reg::RCTL, 0);
        csr.write(reg::TCTL, tctl::PSP);
        csr.flush();
        delay(10_000);
        csr.update(reg::CTRL, ctrl::GIO_MASTER_DISABLE, 0);
        wait_for(&mut delay, MASTER_US, 100, || {
            csr.read(reg::STATUS) & status::GIO_MASTER_ENABLE == 0
        });
        csr.update(reg::CTRL_EXT, 0, shared::CTRL_EXT_DRV_LOAD);
    }
}

/// A latency tolerance as LTRV encodes it, in nanoseconds: a 10-bit
/// value times 32 to the power of its scale.
fn latency_ns(encoded: u32) -> u64 {
    let value = u64::from(encoded) & LTRV_VALUE_MASK;
    let scale = (encoded >> LTRV_SCALE_SHIFT) & 0x7;
    value << (scale * 5).min(63)
}
