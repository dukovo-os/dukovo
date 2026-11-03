// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::vec::Vec;

use proptest::prelude::*;

use super::super::Protocol;
use super::{
    Medium, Model, RECEIVE, TX_HEADER, build, bulk_in, configure, frames, identify, medium, mii,
    phy_read, phy_write, plsr, port, power_up, read, register, resolve, write,
};
use crate::{Configuration, Setup};

/// The one configuration of an AX88179: an interface of the vendor's
/// class with an interrupt endpoint and a bulk endpoint each way, of
/// `packet`-byte packets.
fn configuration(class: u8, packet: u16) -> Vec<u8> {
    let mut bytes = std::vec![9, 2, 0, 0, 1, 1, 0, 0xa0, 124];
    bytes.extend([9, 4, 0, 0, 3, class, 0xff, 0, 0]);
    bytes.extend([7, 5, 0x81, 3, 8, 0, 11]);
    let [low, high] = packet.to_le_bytes();
    bytes.extend([7, 5, 0x82, 2, low, high, 0]);
    bytes.extend([7, 5, 0x03, 2, low, high, 0]);
    let len = bytes.len() as u16;
    bytes[2..4].copy_from_slice(&len.to_le_bytes());
    bytes
}

#[test]
fn knows_the_chips_by_id_and_release() {
    assert_eq!(identify(0x0b95, 0x1790, 0x0100), Some(Model::Ax88179));
    assert_eq!(identify(0x0b95, 0x1790, 0x0200), Some(Model::Ax88179a));
    assert_eq!(identify(0x0b95, 0x1790, 0x0150), None);
    assert_eq!(identify(0x0b95, 0x178a, 0x0100), Some(Model::Ax88178a));
    for (vendor, product) in
        [(0x050d, 0x0128), (0x2001, 0x4a00), (0x17ef, 0x304b), (0x0df6, 0x0072), (0x04e8, 0xa100)]
    {
        assert_eq!(identify(vendor, product, 0x0100), Some(Model::Ax88179));
    }
    // ASIX's USB 2 chips of the AX88772 family, and Realtek's RTL8153.
    assert_eq!(identify(0x0b95, 0x772b, 0x0001), None);
    assert_eq!(identify(0x0bda, 0x8153, 0x3000), None);
    assert_eq!(Protocol::Asix(Model::Ax88179).name(), "ASIX AX88179");
    assert_eq!(Protocol::Asix(Model::Ax88179a).driver(), "ax88179_178a");
}

#[test]
fn finds_the_vendor_interface_and_its_bulk_pair() {
    let bytes = configuration(0xff, 1024);
    let found = port(&Configuration::parse(&bytes).unwrap(), Model::Ax88179).unwrap();
    assert_eq!(found.protocol, Protocol::Asix(Model::Ax88179));
    assert_eq!((found.control, found.data, found.alternate), (0, 0, 0));
    assert_eq!(found.bulk_in.address, 0x82);
    assert_eq!(found.bulk_out.address, 0x03);
    assert_eq!(found.bulk_out.max_packet(), 1024);
    assert_eq!(found.notify, None);
    // Not the vendor's class: some other device.
    let bytes = configuration(0x02, 512);
    assert_eq!(port(&Configuration::parse(&bytes).unwrap(), Model::Ax88179), None);
}

#[test]
fn vendor_requests_name_the_register_and_its_length() {
    // FreeBSD's axge_read_mem: wValue the register, wIndex the length.
    let setup = read(register::NIDR, 6);
    assert_eq!(
        setup,
        Setup { request_type: 0xc0, request: 0x01, value: 0x10, index: 6, length: 6 }
    );
    let setup = write(register::MSR, 2);
    assert_eq!(
        setup,
        Setup { request_type: 0x40, request: 0x01, value: 0x22, index: 2, length: 2 }
    );
    // The PHY at MDIO address 3, the register in wIndex.
    let setup = phy_read(mii::BMSR);
    assert_eq!(setup, Setup { request_type: 0xc0, request: 0x02, value: 3, index: 1, length: 2 });
    assert_eq!(phy_write(mii::GTCR).index, 9);
    assert_eq!(phy_write(mii::GTCR).request_type, 0x40);
}

#[test]
fn powers_up_and_puts_an_ax88179a_in_the_older_mode() {
    let steps: Vec<_> = power_up(Model::Ax88179).collect();
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[0].setup.value, u16::from(register::EPPRCR));
    assert_eq!(&steps[0].data, &[0, 0]);
    assert_eq!(&steps[1].data, &[0x20, 0]);
    assert!(steps[1].then_ms >= 200);
    assert_eq!(steps[2].setup.value, u16::from(register::CLOCK));
    assert_eq!(steps[2].setup.length, 1);
    assert_eq!(steps[2].data[0], 0x03);
    let steps: Vec<_> = power_up(Model::Ax88179a).collect();
    assert_eq!(steps.len(), 4);
    assert_eq!((steps[3].setup.request, steps[3].setup.value, steps[3].data[0]), (0x08, 0, 0));
}

#[test]
fn configures_the_filter_and_a_gigabit_medium() {
    assert_eq!(RECEIVE, 0x018a);
    let steps = configure();
    let rcr = steps.iter().find(|s| s.setup.value == u16::from(register::RCR)).unwrap();
    assert_eq!(rcr.setup.length, 2);
    assert_eq!(u16::from_le_bytes(rcr.data), 0x018a);
    let msr = steps.iter().find(|s| s.setup.value == u16::from(register::MSR)).unwrap();
    assert_eq!(u16::from_le_bytes(msr.data), 0x0103);
    // No checksum offload either way.
    for checksum in [register::RX_CHECKSUM, register::TX_CHECKSUM] {
        let step = steps.iter().find(|s| s.setup.value == u16::from(checksum)).unwrap();
        assert_eq!((step.setup.length, step.data[0]), (1, 0));
    }
}

#[test]
fn resolves_the_link_from_both_ends() {
    let up = mii::BMSR_LINK | mii::BMSR_NEGOTIATED;
    // Down, or still negotiating.
    assert_eq!(resolve(0x7949, 0x01e1, 0x41e1, 0x0300, 0x3c00), None);
    assert_eq!(resolve(mii::BMSR_LINK, 0x01e1, 0x41e1, 0x0300, 0x3c00), None);
    // A gigabit partner.
    let gigabit = Some(Medium { mbps: 1000, full_duplex: true });
    assert_eq!(resolve(up, 0x01e1, 0xc1e1, 0x0300, 0x3c00), gigabit);
    // A partner of 100 Mb/s, full and half duplex.
    let fast = Some(Medium { mbps: 100, full_duplex: true });
    assert_eq!(resolve(up, 0x01e1, 0x41e1, 0x0300, 0x0000), fast);
    let half = Some(Medium { mbps: 100, full_duplex: false });
    assert_eq!(resolve(up, 0x01e1, 0x40a1, 0x0300, 0x0000), half);
    // Gigabit at half duplex only, if both say so.
    let gigabit_half = Some(Medium { mbps: 1000, full_duplex: false });
    assert_eq!(resolve(up, 0x01e1, 0x41e1, 0x0100, 0x0c00), gigabit_half);
    // 10 Mb/s, and a partner that advertised nothing.
    assert_eq!(resolve(up, 0x01e1, 0x0061, 0, 0), Some(Medium { mbps: 10, full_duplex: true }));
    assert_eq!(resolve(up, 0x01e1, 0x0000, 0, 0), Some(Medium { mbps: 10, full_duplex: false }));
}

#[test]
fn sets_the_medium_and_bursts_for_each_speed() {
    let at = |mbps, full_duplex| Medium { mbps, full_duplex };
    assert_eq!(medium(at(1000, true)), 0x010b);
    assert_eq!(medium(at(100, true)), 0x0302);
    assert_eq!(medium(at(100, false)), 0x0300);
    assert_eq!(medium(at(10, true)), 0x0102);
    assert_eq!(bulk_in(1000, plsr::SUPER_SPEED), [7, 0x4f, 0, 0x12, 0xff]);
    assert_eq!(bulk_in(1000, plsr::HIGH_SPEED), [7, 0x20, 3, 0x16, 0xff]);
    assert_eq!(bulk_in(1000, plsr::FULL_SPEED), [7, 0xcc, 0x4c, 0x18, 0x08]);
    assert_eq!(bulk_in(100, plsr::SUPER_SPEED), [7, 0xae, 7, 0x18, 0xff]);
    assert_eq!(bulk_in(100, plsr::FULL_SPEED), [7, 0xcc, 0x4c, 0x18, 0x08]);
    assert_eq!(bulk_in(10, plsr::SUPER_SPEED), [7, 0xcc, 0x4c, 0x18, 0x08]);
}

/// A burst as the chip sends it: `packets` of (length, status bits),
/// each frame filled with its index and starting on an 8-byte boundary,
/// then a header for each, then the trailer.
fn burst(packets: &[(usize, u32)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (i, &(len, _)) in packets.iter().enumerate() {
        bytes.extend(std::iter::repeat_n(i as u8, len));
        bytes.resize(bytes.len().next_multiple_of(8), 0xee);
    }
    let headers = bytes.len();
    for &(len, status) in packets {
        bytes.extend(((len as u32) << 16 | status).to_le_bytes());
    }
    bytes.extend((((headers as u32) << 16) | packets.len() as u32).to_le_bytes());
    bytes
}

#[test]
fn reads_each_frame_of_a_burst() {
    let bytes = burst(&[(60, 0x0800), (1514, 0x0800), (0, 0), (98, 0x0800)]);
    let found: Vec<_> = frames(&bytes).collect();
    assert_eq!(found, [(0, 60), (64, 1514), (1584, 98)]);
    for (i, &(at, len)) in found.iter().enumerate() {
        let expect = if i == 2 { 3 } else { i as u8 };
        assert!(bytes[at..at + len].iter().all(|&b| b == expect));
    }
}

#[test]
fn passes_over_frames_that_failed() {
    let bytes = burst(&[(60, 0x2000_0000), (70, 0x0800), (80, 0x8000_0000), (90, 0)]);
    let found: Vec<_> = frames(&bytes).collect();
    assert_eq!(found, [(64, 70), (216, 90)]);
}

#[test]
fn reads_nothing_of_a_broken_burst() {
    assert_eq!(frames(&[]).count(), 0);
    assert_eq!(frames(&[1, 2, 3]).count(), 0);
    // Headers said to start past the trailer.
    let mut bytes = burst(&[(60, 0)]);
    let at = bytes.len() - 4;
    bytes[at..].copy_from_slice(&((0xfff0u32 << 16) | 1).to_le_bytes());
    assert_eq!(frames(&bytes).count(), 0);
    // A frame said to run into the headers.
    let mut bytes = burst(&[(60, 0)]);
    let header = bytes.len() - 8;
    bytes[header..header + 4].copy_from_slice(&(100u32 << 16).to_le_bytes());
    assert_eq!(frames(&bytes).count(), 0);
    // More headers counted than there are: the trailer is not read as one.
    let mut bytes = burst(&[(60, 0)]);
    let at = bytes.len() - 4;
    bytes[at..at + 2].copy_from_slice(&40u16.to_le_bytes());
    assert_eq!(frames(&bytes).collect::<Vec<_>>(), [(0, 60)]);
}

proptest! {
    #[test]
    fn any_burst_gives_frames_inside_it(bytes in prop::collection::vec(any::<u8>(), 0..6000)) {
        for (at, len) in frames(&bytes) {
            prop_assert!(len > 0);
            prop_assert!(at + len <= bytes.len());
        }
    }
}

#[test]
fn sends_a_frame_after_its_header() {
    let frame: Vec<u8> = (0..100).collect();
    let mut out = [0u8; 4096];
    let len = build(&frame, 512, &mut out).unwrap();
    assert_eq!(len, TX_HEADER + 100);
    assert_eq!(&out[..8], &[100, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(&out[8..len], &frame[..]);
}

#[test]
fn pads_a_transfer_that_ends_on_a_packet() {
    let frame = std::vec![0x5a; 512 - TX_HEADER];
    let mut out = [0u8; 4096];
    let len = build(&frame, 512, &mut out).unwrap();
    assert_eq!(len, 512 + TX_HEADER);
    assert_eq!(&out[512..520], &[0, 0, 0, 0, 0x00, 0x80, 0x00, 0x80]);
    // Not on a SuperSpeed endpoint's 1024-byte packets.
    assert_eq!(build(&frame, 1024, &mut out), Some(512));
    // No room for the pad, or for the frame.
    assert_eq!(build(&frame, 512, &mut out[..512]), None);
    assert_eq!(build(&frame, 512, &mut out[..100]), None);
}
