// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Isochronous TDs (4.11.2.3): the packets of a sound device, a TD for
//! each service interval.

use std::vec;

use super::*;

/// The isochronous OUT endpoint's device context index, endpoint 1.
const OUT: u8 = 2;
/// The feedback IN endpoint's, endpoint 1 IN.
const FEEDBACK: u8 = 3;

/// A USB Audio Class device at `speed_id`, as QEMU's `usb-audio` is at
/// full speed: its control interface, and its streaming interface, whose
/// setting 0 has no endpoint and setting 1 an isochronous OUT endpoint of
/// `packet` (wMaxPacketSize) every `interval` (bInterval). With
/// `feedback`, the endpoint is asynchronous and the setting has an IN
/// endpoint that says the device's rate; with `companion`, a SuperSpeed
/// companion gives the endpoint its bursts.
fn speaker(speed_id: u8, packet: u16, interval: u8, feedback: bool, companion: u8) -> SimDevice {
    let mut device = keyboard(speed_id, if speed_id == 4 { 9 } else { 64 });
    if speed_id == 4 {
        device.device[2..4].copy_from_slice(&[0x00, 0x03]);
    }
    let endpoints = if feedback { 2 } else { 1 };
    let mut config = vec![9, 2, 0, 0, 2, 1, 0, 0xc0, 50];
    config.extend([9, 4, 0, 0, 0, 1, 1, 0, 0]);
    config.extend([9, 4, 1, 0, 0, 1, 2, 0, 0]);
    config.extend([9, 4, 1, 1, endpoints, 1, 2, 0, 0]);
    let [low, high] = packet.to_le_bytes();
    // Synchronous (0x0d), as QEMU's is, or asynchronous (0x05) with its
    // feedback.
    let attributes = if feedback { 0x05 } else { 0x0d };
    config.extend([9, 5, 0x01, attributes, low, high, interval, 0, 0]);
    if companion > 0 {
        config.extend([6, descriptor::SUPERSPEED_ENDPOINT_COMPANION, companion, 0, 0, 0x10]);
    }
    if feedback {
        config.extend([9, 5, 0x81, 0x11, 4, 0, interval, 0, 0]);
    }
    let total = config.len() as u16;
    config[2..4].copy_from_slice(&total.to_le_bytes());
    device.config = config;
    device
}

/// `device` on root port `port`, configured, with its streaming
/// interface in setting 1.
fn streaming(sim: &Arc<Sim>, device: SimDevice, port: u8) -> (Xhci, TestHost, Device) {
    let setting = setting_of(&device, 1);
    sim.state().devices[usize::from(port - 1)] = Some(device);
    let (mut xhci, mut host) = start(sim);
    let mut device = attach(&mut xhci, &mut host, port).unwrap();
    let mut below = Vec::new();
    device.configure(&mut xhci, &mut host, &mut alloc(&mut below), 1, &[]).unwrap();
    device
        .set_interface(&mut xhci, &mut host, &mut alloc(&mut below), (1, 1), &[], &setting)
        .unwrap();
    host.others.clear();
    (xhci, host, device)
}

/// An Isoch TRB's Transfer Burst Count and Last Burst Packet Count.
fn bursts(control: u32) -> (u32, u32) {
    ((control >> 7) & 3, (control >> 16) & 0xf)
}

#[test]
fn each_interval_gets_a_td_started_after_the_one_before() {
    let sim = Sim::new(State::new());
    let (mut xhci, mut host, mut device) = streaming(&sim, speaker(1, 192, 1, false, 0), 6);
    let slot = device.slot();
    {
        let s = sim.state();
        let endpoint = &s.slots[&slot].endpoints[&OUT];
        // Isochronous OUT, 192 bytes every 2^3 microframes (a frame), no
        // retries, a packet an interval.
        assert_eq!((endpoint.kind, endpoint.max_packet, endpoint.interval), (1, 192, 3));
        assert_eq!((endpoint.retries, endpoint.esit, endpoint.average), (0, 192, 192));
    }
    let memory = Dma::fake(4096);
    let samples: Vec<u8> = (0..576).map(|i| i as u8).collect();
    memory.write(0, &samples);
    let at = memory.phys();
    // Three frames' packets, the third in two pieces (where a ring of
    // samples wraps), and only the third asks for an event.
    let first = device.queue_isoch(&xhci, OUT, &[(at, 192)], false).unwrap();
    device.queue_isoch(&xhci, OUT, &[(at + 192, 192)], false).unwrap();
    let third = device.queue_isoch(&xhci, OUT, &[(at + 384, 100), (at + 484, 92)], true).unwrap();
    assert_ne!(first.last, third.last);
    // Nothing is read until the doorbell rings, once for all three.
    assert!(sim.state().devices[5].as_ref().unwrap().received.is_empty());
    device.doorbell(&xhci, OUT);
    {
        let s = sim.state();
        assert_eq!(s.doorbells, [(slot, OUT, 0)]);
        let received = &s.devices[5].as_ref().unwrap().received;
        assert_eq!(received.len(), 3);
        assert_eq!(received[0], samples[..192]);
        assert_eq!(received[2], samples[384..]);
        assert_eq!(s.isoch.len(), 3);
        for &(index, _, control) in &s.isoch {
            assert_eq!(index, OUT);
            // Each starts as soon as it can after the one before, in one
            // burst of one packet.
            assert_ne!(control & control::SIA, 0);
            assert_eq!(bursts(control), (0, 0));
        }
        // A whole packet in one TRB: no packets after it.
        assert_eq!(s.isoch[0].1, 192);
        // The third's Isoch TRB holds 100 bytes, chained to the rest of
        // its packet, and only its last TRB interrupts.
        let (_, status, control) = s.isoch[2];
        assert_eq!(status & 0x1_ffff, 100);
        assert_eq!((status >> 17) & 0x1f, 1);
        assert_ne!(control & control::CHAIN, 0);
        assert_eq!(control & control::IOC, 0);
        assert_eq!(s.isoch[0].2 & (control::IOC | control::CHAIN), 0);
    }
    // One event, for the third TD's last TRB.
    assert!(matches!(
        transfer(&mut xhci, &mut host),
        Some(Event::Transfer { trb, endpoint: OUT, code: code::SUCCESS, .. }) if trb == third.last
    ));
    assert!(transfer(&mut xhci, &mut host).is_none());
}

#[test]
fn bursts_follow_the_endpoint() {
    // High speed, three transactions of 1024 bytes a microframe: a TD of
    // 3000 bytes is three packets in one burst.
    let sim = Sim::new(State::new());
    let (xhci, _host, mut device) = streaming(&sim, speaker(3, 0x1400, 1, false, 0), 6);
    {
        let s = sim.state();
        let endpoint = &s.slots[&device.slot()].endpoints[&OUT];
        assert_eq!((endpoint.interval, endpoint.burst, endpoint.esit), (0, 2, 3072));
    }
    let memory = Dma::fake(4 * 4096);
    device.queue_isoch(&xhci, OUT, &[(memory.phys(), 3000)], false).unwrap();
    device.queue_isoch(&xhci, OUT, &[(memory.phys(), 1024)], true).unwrap();
    device.doorbell(&xhci, OUT);
    let tds: Vec<(u32, u32)> = sim.state().isoch.iter().map(|&(_, _, c)| bursts(c)).collect();
    assert_eq!(tds, [(0, 2), (0, 0)]);

    // SuperSpeed, bursts of four packets: six packets are two bursts, the
    // last of two; eight are two of four.
    let sim = Sim::new(State::new());
    let (xhci, _host, mut device) = streaming(&sim, speaker(4, 1024, 1, false, 3), 1);
    // Eight packets that no 64 KiB boundary crosses.
    let memory = Dma::fake(32 * 4096);
    let start = memory.phys() & 0xffff;
    let at = memory.phys() + if start + 8192 > 0x1_0000 { 0x1_0000 - start } else { 0 };
    device.queue_isoch(&xhci, OUT, &[(at, 6 * 1024)], false).unwrap();
    device.queue_isoch(&xhci, OUT, &[(at, 4096), (at + 4096, 4096)], true).unwrap();
    device.doorbell(&xhci, OUT);
    let s = sim.state();
    let tds: Vec<(u32, u32)> = s.isoch.iter().map(|&(_, _, c)| bursts(c)).collect();
    assert_eq!(tds, [(1, 1), (1, 3)]);
    // Four packets are left after the second TD's first TRB.
    assert_eq!((s.isoch[1].1 >> 17) & 0x1f, 4);
}

#[test]
fn a_feedback_endpoint_says_the_rate() {
    let sim = Sim::new(State::new());
    let (mut xhci, mut host, mut device) = streaming(&sim, speaker(1, 200, 1, true, 0), 6);
    // 48.25 frames a millisecond, as 10.14 in three bytes.
    let feedback = (48 << 14) + (1 << 12);
    sim.state().devices[5].as_mut().unwrap().reports.push_back(vec![
        feedback as u8,
        (feedback >> 8) as u8,
        (feedback >> 16) as u8,
    ]);
    let memory = Dma::fake(4096);
    let td = device.queue_isoch(&xhci, FEEDBACK, &[(memory.phys(), 4)], true).unwrap();
    device.doorbell(&xhci, FEEDBACK);
    // Three bytes of four: short, which the TRB asked to hear of.
    assert!(matches!(
        transfer(&mut xhci, &mut host),
        Some(Event::Transfer { trb, endpoint: FEEDBACK, code: code::SHORT_PACKET, residual: 1, .. })
            if trb == td.last
    ));
    let mut got = [0u8; 3];
    memory.read(0, &mut got);
    assert_eq!(u32::from_le_bytes([got[0], got[1], got[2], 0]), feedback);
    let s = sim.state();
    assert_eq!(s.slots[&device.slot()].endpoints[&FEEDBACK].kind, 5);
    assert_ne!(s.isoch[0].2 & control::ISP, 0);
}

#[test]
fn isochronous_tds_out_of_bounds_are_refused() {
    let sim = Sim::new(State::new());
    let (xhci, _host, mut device) = streaming(&sim, speaker(1, 192, 1, false, 0), 6);
    let memory = Dma::fake(4096);
    let at = memory.phys();
    let mut queue =
        |index: u8, buffers: &[(u64, u32)]| device.queue_isoch(&xhci, index, buffers, true);
    // Endpoint 0, an endpoint it does not have, no buffers, too many, an
    // empty one, and one across 64 KiB.
    assert_eq!(queue(1, &[(at, 8)]).err(), Some(Error::BadRequest));
    assert_eq!(queue(5, &[(at, 8)]).err(), Some(Error::BadRequest));
    assert_eq!(queue(OUT, &[]).err(), Some(Error::BadRequest));
    let four = [(at, 8); 4];
    assert_eq!(queue(OUT, &four).err(), Some(Error::BadRequest));
    assert_eq!(queue(OUT, &[(at, 0)]).err(), Some(Error::BadRequest));
    assert_eq!(queue(OUT, &[(0x1_ffc0, 0x80)]).err(), Some(Error::BadRequest));
    assert!(sim.state().isoch.is_empty());

    // A bulk endpoint takes Normal TDs, not these.
    let sim = Sim::new(State::new());
    let (xhci, _host, mut device) = configured(&sim, bulk(3));
    let memory = Dma::fake(4096);
    assert_eq!(
        device.queue_isoch(&xhci, 3, &[(memory.phys(), 512)], true).err(),
        Some(Error::BadRequest)
    );
}
