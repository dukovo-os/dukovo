// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! ASIX's gigabit Ethernet adapters (ADR 0362): the AX88179 and AX88179A
//! on USB 3 and the AX88178A on USB 2, inside TP-Link's UE306, D-Link's
//! DUB-1312, Lenovo's, Belkin's, Samsung's and Sitecom's adapters and
//! many USB-C docks. They speak no CDC protocol of their own: one
//! interface of the vendor's class has a bulk endpoint each way and an
//! interrupt endpoint, and the chip is set up with vendor requests that
//! read and write its MAC's registers and its PHY's.
//!
//! - **Received frames come in bursts** ([`frames`]): the frames one after
//!   another, each starting on an 8-byte boundary, then a 4-byte header
//!   for each, then a 4-byte trailer that says how many headers there are
//!   and where they start. A burst can be longer than one transfer here
//!   (a page), so the kernel joins transfers until one comes short.
//! - **Each frame sent has an 8-byte header before it** ([`build`]), its
//!   length. A transfer that would end on a packet's boundary gets a
//!   second header that asks for padding, instead of a zero-length packet.
//! - **The link is the PHY's** ([`resolve`]): read over MDIO with vendor
//!   requests, as IEEE 802.3 clause 22 lays its registers out, and the
//!   MAC's medium and bulk-in settings follow it ([`medium`],
//!   [`bulk_in`]).
//!
//! The register map, the bring-up sequence and the burst formats follow
//! FreeBSD's axge(4) and OpenBSD's axen(4) (credits/asix-usb-ethernet.md);
//! no code is taken from them.

use super::{Port, Protocol};
use crate::{Configuration, DEVICE_TO_HOST, Setup, TO_DEVICE, TransferType, VENDOR, class};

/// The chips.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    /// The AX88178A, on USB 2.
    Ax88178a,
    /// The AX88179, on USB 3.
    Ax88179,
    /// The AX88179A, its successor, which is put in the AX88179's mode.
    Ax88179a,
}

impl Model {
    /// Its name, for the log and the interface's card.
    pub fn name(self) -> &'static str {
        match self {
            Model::Ax88178a => "ASIX AX88178A",
            Model::Ax88179 => "ASIX AX88179",
            Model::Ax88179a => "ASIX AX88179A",
        }
    }
}

/// The chip of the device `vendor`:`product` at release `release`, if it
/// is one of these. The AX88179 and AX88179A share an ID and differ in
/// their release: 1.00 and before, 2.00 and after.
pub fn identify(vendor: u16, product: u16, release: u16) -> Option<Model> {
    match (vendor, product) {
        (0x0b95, 0x178a) => Some(Model::Ax88178a),
        (0x0b95, 0x1790) => match release {
            ..=0x0100 => Some(Model::Ax88179),
            0x0200.. => Some(Model::Ax88179a),
            _ => None,
        },
        // Belkin B2B128, D-Link DUB-1312, Lenovo's, Sitecom LN-032 and
        // Samsung's.
        (0x050d, 0x0128)
        | (0x2001, 0x4a00)
        | (0x17ef, 0x304b)
        | (0x0df6, 0x0072)
        | (0x04e8, 0xa100) => Some(Model::Ax88179),
        _ => None,
    }
}

/// The adapter `model` in `config`: its first interface, of the vendor's
/// class, with a bulk endpoint each way.
pub fn port(config: &Configuration<'_>, model: Model) -> Option<Port> {
    let interface = config.interfaces().find(|i| {
        i.descriptor.number == 0
            && i.descriptor.alternate == 0
            && i.descriptor.class == class::VENDOR
    })?;
    let bulk = |into: bool| {
        interface.endpoints().find(|e| e.transfer_type() == TransferType::Bulk && e.is_in() == into)
    };
    let (bulk_in, bulk_out) = (bulk(true)?, bulk(false)?);
    (bulk_in.max_packet() > 0 && bulk_out.max_packet() > 0).then_some(Port {
        protocol: Protocol::Asix(model),
        control: 0,
        data: 0,
        alternate: 0,
        bulk_in,
        bulk_out,
        // Its interrupt endpoint is not read: the PHY is polled.
        notify: None,
        mac_string: 0,
        max_segment: 1514,
    })
}

/// What a vendor request reaches: its bRequest.
mod access {
    /// The MAC's registers: wValue is the register, wIndex and wLength
    /// its bytes.
    pub const MAC: u8 = 0x01;
    /// The PHY's: wValue is its MDIO address, wIndex the register.
    pub const PHY: u8 = 0x02;
    /// The AX88179A's mode: wValue picks it.
    pub const FW_MODE: u8 = 0x08;
}

/// The PHY's MDIO address.
const PHY_ADDRESS: u16 = 3;

/// The MAC's registers.
pub mod register {
    /// Physical link status: which speed of USB the chip is on.
    pub const PLSR: u8 = 0x02;
    /// Receive control.
    pub const RCR: u8 = 0x0b;
    /// The node ID: its MAC address.
    pub const NIDR: u8 = 0x10;
    /// Medium status: speed, duplex, flow control, receiving.
    pub const MSR: u8 = 0x22;
    /// Monitor mode: wake on LAN.
    pub const MMSR: u8 = 0x24;
    /// The PHY's power and reset.
    pub const EPPRCR: u8 = 0x26;
    /// How the bulk IN endpoint gathers frames into bursts.
    pub const BULK_IN: u8 = 0x2e;
    /// Clock select.
    pub const CLOCK: u8 = 0x33;
    /// Checksum offload of the frames received.
    pub const RX_CHECKSUM: u8 = 0x34;
    /// Checksum offload of the frames sent.
    pub const TX_CHECKSUM: u8 = 0x35;
    /// The water level at which a pause frame is sent.
    pub const PAUSE_HIGH: u8 = 0x54;
    /// The water level at which it is taken back.
    pub const PAUSE_LOW: u8 = 0x55;
}

/// PLSR's bits: the USB speed.
pub mod plsr {
    /// USB 1.1's full speed.
    pub const FULL_SPEED: u8 = 0x01;
    /// USB 2's high speed.
    pub const HIGH_SPEED: u8 = 0x02;
    /// USB 3's SuperSpeed.
    pub const SUPER_SPEED: u8 = 0x04;
}

/// RCR's bits.
mod rcr {
    pub const ALL_MULTICAST: u16 = 0x0002;
    pub const BROADCAST: u16 = 0x0008;
    pub const START: u16 = 0x0080;
    pub const DROP_CRC_ERRORS: u16 = 0x0100;
}

/// MSR's bits.
mod msr {
    pub const GIGABIT: u16 = 0x0001;
    pub const FULL_DUPLEX: u16 = 0x0002;
    pub const CLOCK_125MHZ: u16 = 0x0008;
    pub const RECEIVE: u16 = 0x0100;
    pub const FAST: u16 = 0x0200;
}

/// EPPRCR's bit that takes the PHY out of reset.
const EPPRCR_IPRL: u8 = 0x20;
/// CLOCK's bits: both of the chip's clocks on.
const CLOCK_ON: u8 = 0x03;

/// A read of `len` bytes of the MAC's register `register`.
pub fn read(register: u8, len: u16) -> Setup {
    Setup {
        request_type: DEVICE_TO_HOST | VENDOR | TO_DEVICE,
        request: access::MAC,
        value: u16::from(register),
        index: len,
        length: len,
    }
}

/// A write of `len` bytes of the MAC's register `register`, the bytes in
/// the data stage.
pub fn write(register: u8, len: u16) -> Setup {
    Setup {
        request_type: VENDOR | TO_DEVICE,
        request: access::MAC,
        value: u16::from(register),
        index: len,
        length: len,
    }
}

/// A read of the PHY's register `register`: two bytes.
pub fn phy_read(register: u8) -> Setup {
    Setup {
        request_type: DEVICE_TO_HOST | VENDOR | TO_DEVICE,
        request: access::PHY,
        value: PHY_ADDRESS,
        index: u16::from(register),
        length: 2,
    }
}

/// A write of the PHY's register `register`, its two bytes in the data
/// stage.
pub fn phy_write(register: u8) -> Setup {
    Setup {
        request_type: VENDOR | TO_DEVICE,
        request: access::PHY,
        value: PHY_ADDRESS,
        index: u16::from(register),
        length: 2,
    }
}

/// A request of the bring-up, and how long to wait after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    /// The request, a write.
    pub setup: Setup,
    /// Its data stage: the first `setup.length` bytes.
    pub data: [u8; 2],
    /// Milliseconds to wait once it is done.
    pub then_ms: u32,
}

impl Step {
    fn write(register: u8, value: &[u8], then_ms: u32) -> Step {
        let mut data = [0; 2];
        data[..value.len()].copy_from_slice(value);
        Step { setup: write(register, value.len() as u16), data, then_ms }
    }
}

/// The first steps, before the MAC address is read: the PHY powered and
/// out of reset, the clocks on, and an AX88179A in the AX88179's mode,
/// whose bursts and headers [`frames`] reads.
pub fn power_up(model: Model) -> impl Iterator<Item = Step> {
    let steps = [
        Step::write(register::EPPRCR, &[0, 0], 0),
        Step::write(register::EPPRCR, &[EPPRCR_IPRL, 0], 250),
        Step::write(register::CLOCK, &[CLOCK_ON], 100),
    ];
    let compatible = Step {
        setup: Setup {
            request_type: VENDOR | TO_DEVICE,
            request: access::FW_MODE,
            value: 0,
            index: 1,
            length: 1,
        },
        data: [0; 2],
        then_ms: 0,
    };
    steps.into_iter().chain((model == Model::Ax88179a).then_some(compatible))
}

/// Whether `mac`, as the chip's node ID register holds it, is an address
/// a card can have: not all zeros or ones, and not a group's.
pub fn usable(mac: &[u8; 6]) -> bool {
    *mac != [0; 6] && *mac != [0xff; 6] && mac[0] & 0x01 == 0
}

/// The receive control register's value: the adapter's own frames,
/// broadcasts and every multicast frame, without those that fail their
/// CRC, and no padding before a frame.
pub const RECEIVE: u16 = rcr::START | rcr::DROP_CRC_ERRORS | rcr::BROADCAST | rcr::ALL_MULTICAST;

/// The steps after the MAC address is read: pause frames' water levels,
/// no checksum offload either way, the receive filter, no wake on LAN,
/// and a gigabit, full-duplex medium until the PHY says otherwise.
pub fn configure() -> [Step; 7] {
    let le = u16::to_le_bytes;
    [
        Step::write(register::PAUSE_LOW, &[0x34], 0),
        Step::write(register::PAUSE_HIGH, &[0x52], 0),
        Step::write(register::TX_CHECKSUM, &[0], 0),
        Step::write(register::RX_CHECKSUM, &[0], 0),
        Step::write(register::RCR, &le(RECEIVE), 0),
        Step::write(register::MMSR, &[0], 0),
        Step::write(register::MSR, &le(msr::GIGABIT | msr::FULL_DUPLEX | msr::RECEIVE), 0),
    ]
}

/// The PHY's registers (IEEE 802.3 clause 22) and their bits used here.
pub mod mii {
    /// Control.
    pub const BMCR: u8 = 0;
    /// BMCR's reset, which clears itself when done.
    pub const BMCR_RESET: u16 = 0x8000;
    /// BMCR's autonegotiation on and restarted.
    pub const BMCR_AUTONEGOTIATE: u16 = 0x1200;
    /// Status.
    pub const BMSR: u8 = 1;
    /// BMSR's link, which holds a loss until read.
    pub const BMSR_LINK: u16 = 0x0004;
    /// BMSR's autonegotiation done.
    pub const BMSR_NEGOTIATED: u16 = 0x0020;
    /// What it advertises at 10 and 100 Mb/s.
    pub const ANAR: u8 = 4;
    /// What its partner advertises at 10 and 100 Mb/s.
    pub const ANLPAR: u8 = 5;
    /// 10 and 100 Mb/s, half and full duplex, and the IEEE 802.3 selector;
    /// no pause frames.
    pub const ANAR_ALL: u16 = 0x01e1;
    /// 1000BASE-T control: what it advertises at 1000 Mb/s.
    pub const GTCR: u8 = 9;
    /// 1000 Mb/s, full and half duplex.
    pub const GTCR_ALL: u16 = 0x0300;
    /// 1000BASE-T status: what its partner advertises at 1000 Mb/s.
    pub const GTSR: u8 = 10;
}

/// A link's speed and duplex.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Medium {
    /// Its speed, in Mb/s.
    pub mbps: u32,
    /// Whether it is full duplex.
    pub full_duplex: bool,
}

/// The link the PHY's registers say it has, `None` if it is down: the
/// best of what both ends advertise, as IEEE 802.3's priority resolution
/// (annex 28B.3) has it.
pub fn resolve(bmsr: u16, anar: u16, anlpar: u16, gtcr: u16, gtsr: u16) -> Option<Medium> {
    if bmsr & mii::BMSR_LINK == 0 || bmsr & mii::BMSR_NEGOTIATED == 0 {
        return None;
    }
    let medium = |mbps, full_duplex| Some(Medium { mbps, full_duplex });
    // GTSR's partner bits are GTCR's own, two higher.
    let gigabit = gtcr & (gtsr >> 2);
    let common = anar & anlpar;
    if gigabit & 0x0200 != 0 {
        medium(1000, true)
    } else if gigabit & 0x0100 != 0 {
        medium(1000, false)
    } else if common & 0x0100 != 0 {
        medium(100, true)
    } else if common & 0x0080 != 0 {
        medium(100, false)
    } else if common & 0x0040 != 0 {
        medium(10, true)
    } else {
        // 10 Mb/s half duplex, or a partner found by parallel detection
        // that says nothing: the least each end can do.
        medium(10, false)
    }
}

/// The medium status register's value for `medium`.
pub fn medium(medium: Medium) -> u16 {
    let mut value = msr::RECEIVE;
    if medium.full_duplex {
        value |= msr::FULL_DUPLEX;
    }
    match medium.mbps {
        1000 => value | msr::GIGABIT | msr::CLOCK_125MHZ,
        100 => value | msr::FAST,
        _ => value,
    }
}

/// The bulk-in settings, the 5 bytes of [`register::BULK_IN`], for a link
/// of `mbps` on the USB speed `plsr` says: how long and how much the chip
/// gathers before it ends a burst. These are FreeBSD's values.
pub fn bulk_in(mbps: u32, plsr: u8) -> [u8; 5] {
    const SETTINGS: [[u8; 5]; 4] = [
        [7, 0x4f, 0x00, 0x12, 0xff],
        [7, 0x20, 0x03, 0x16, 0xff],
        [7, 0xae, 0x07, 0x18, 0xff],
        [7, 0xcc, 0x4c, 0x18, 0x08],
    ];
    let fast_bus = plsr & (plsr::SUPER_SPEED | plsr::HIGH_SPEED) != 0;
    let at = match mbps {
        1000 if plsr & plsr::SUPER_SPEED != 0 => 0,
        1000 if plsr & plsr::HIGH_SPEED != 0 => 1,
        100 if fast_bus => 2,
        _ => 3,
    };
    SETTINGS[at]
}

/// A received frame's header bits: failed its CRC, or dropped.
const RX_CRC_ERROR: u32 = 0x2000_0000;
const RX_DROPPED: u32 = 0x8000_0000;
/// The most headers read from one burst.
const MAX_HEADERS: usize = 512;

/// The frames in the burst `burst`, as (offset, length) in it: only those
/// wholly before the headers, and none that failed. Headers of no length,
/// which newer firmware puts between the others, are passed over.
pub fn frames(burst: &[u8]) -> impl Iterator<Item = (usize, usize)> + '_ {
    let word = |at: usize| -> Option<u32> {
        burst.get(at..at.checked_add(4)?).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let trailer = burst.len().checked_sub(4).and_then(word).unwrap_or(0);
    let count = (trailer & 0xffff) as usize;
    let headers = (trailer >> 16) as usize;
    // The headers sit between the frames and the trailer.
    let end = if headers <= burst.len().saturating_sub(4) { headers } else { 0 };
    let last = burst.len().saturating_sub(4);
    let mut header = end;
    let mut at = 0usize;
    let mut left = if end == 0 { 0 } else { count.min(MAX_HEADERS) };
    core::iter::from_fn(move || {
        while left > 0 {
            left -= 1;
            if header + 4 > last {
                return None;
            }
            let status = word(header)?;
            header += 4;
            let len = ((status >> 16) & 0x1fff) as usize;
            if len == 0 {
                continue;
            }
            if at + len > end {
                left = 0;
                return None;
            }
            let frame = at;
            at += len.next_multiple_of(8);
            if status & (RX_CRC_ERROR | RX_DROPPED) == 0 {
                return Some((frame, len));
            }
        }
        None
    })
}

/// The bytes of the header before each frame sent.
pub const TX_HEADER: usize = 8;
/// The second header's padding bits, in its second word.
const TX_PADDING: u32 = 0x8000_8000;

/// Writes into `out` the transfer that sends `frame` on a bulk endpoint
/// of `packet`-byte packets, and says how long it is: the frame with its
/// header, and a second header asking for padding if the two would end on
/// a packet's boundary. `None` if it does not fit in `out`.
pub fn build(frame: &[u8], packet: u16, out: &mut [u8]) -> Option<usize> {
    let mut len = TX_HEADER + frame.len();
    if len > out.len() || frame.len() > 0x1_ffff {
        return None;
    }
    out[0..4].copy_from_slice(&(frame.len() as u32).to_le_bytes());
    out[4..8].fill(0);
    out[TX_HEADER..len].copy_from_slice(frame);
    if packet > 0 && len.is_multiple_of(usize::from(packet)) {
        let pad = out.get_mut(len..len + TX_HEADER)?;
        pad[0..4].fill(0);
        pad[4..8].copy_from_slice(&TX_PADDING.to_le_bytes());
        len += TX_HEADER;
    }
    Some(len)
}

#[cfg(test)]
mod tests;
