// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a stack's [`Tap`] sees for a packet capture (ADR 0346): each
//! frame both ways on a wire while it is watched and none while it is
//! not, a loopback packet once, and a loopback connection's bytes in
//! packets rather than across.

use std::boxed::Box;
use std::cell::RefCell;
use std::vec::Vec;

use core::sync::atomic::{AtomicBool, Ordering};

use smoltcp::socket::{tcp, udp};
use smoltcp::wire::{IpAddress, IpEndpoint};

use crate::tests::{loopback, pair, tcp_memory, udp_memory};
use crate::{Across, Ipv4Address, Medium, OtherEnd, Tap};

/// A frame a tap saw: its interface's number, its bytes, its medium, and
/// whether it went out.
type Seen = (usize, Vec<u8>, Medium, bool);

std::thread_local! {
    static SEEN: RefCell<Vec<Seen>> = const { RefCell::new(Vec::new()) };
}

fn see(number: usize, frame: &[u8], medium: Medium, outgoing: bool) {
    SEEN.with(|seen| seen.borrow_mut().push((number, frame.to_vec(), medium, outgoing)));
}

/// What the taps saw since the last time.
fn seen() -> Vec<Seen> {
    SEEN.with(|seen| core::mem::take(&mut *seen.borrow_mut()))
}

/// A tap for interface `number`, and the flag that has it look.
fn tap(number: usize) -> (Tap, &'static AtomicBool) {
    let watched: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(true)));
    (Tap { number, watched, see }, watched)
}

/// The Ethernet type of `frame`.
fn ethertype(frame: &[u8]) -> u16 {
    u16::from_be_bytes([frame[12], frame[13]])
}

#[test]
fn a_watched_stack_sees_its_frames_both_ways() {
    let (mut a, mut b) = pair();
    let (tap, watched) = tap(3);
    a.set_tap(Some(tap));
    assert!(a.watched());
    let receiver = b.add_socket(udp_memory().socket()).ok().unwrap();
    b.socket_mut::<udp::Socket<'static>>(receiver).unwrap().bind(53).unwrap();
    let sender = a.add_socket(udp_memory().socket()).ok().unwrap();
    let socket = a.socket_mut::<udp::Socket<'static>>(sender).unwrap();
    socket.bind(40000).unwrap();
    let to = IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::new(10, 0, 0, 2)), 53);
    socket.send_slice(b"query", to).unwrap();
    let mut now = 0;
    for _ in 0..5 {
        now += 1_000_000;
        a.poll(now);
        b.poll(now);
    }
    let frames = seen();
    // The ARP request and its answer, then the datagram, each as it went.
    let kinds: Vec<(u16, bool)> =
        frames.iter().map(|(_, frame, _, out)| (ethertype(frame), *out)).collect();
    assert_eq!(kinds, [(0x0806, true), (0x0806, false), (0x0800, true)]);
    assert!(
        frames.iter().all(|(number, _, medium, _)| *number == 3 && *medium == Medium::Ethernet)
    );
    let datagram = &frames[2].1;
    assert!(datagram.ends_with(b"query"));
    assert_eq!(a.link().sent.borrow().last().unwrap(), datagram);

    // Not while no capture looks.
    watched.store(false, Ordering::Relaxed);
    assert!(!a.watched());
    let socket = a.socket_mut::<udp::Socket<'static>>(sender).unwrap();
    socket.send_slice(b"again", to).unwrap();
    for _ in 0..3 {
        now += 1_000_000;
        a.poll(now);
        b.poll(now);
    }
    assert!(seen().is_empty());
    let socket = b.socket_mut::<udp::Socket<'static>>(receiver).unwrap();
    assert_eq!(socket.recv().unwrap().0, b"query");
    assert_eq!(socket.recv().unwrap().0, b"again");
}

#[test]
fn a_loopback_packet_is_seen_once() {
    let mut lo = loopback();
    let (tap, _) = tap(0);
    lo.set_tap(Some(tap));
    let local = Ipv4Address::new(127, 0, 0, 1);
    let udp = lo.add_socket(udp_memory().socket()).ok().unwrap();
    let socket = lo.socket_mut::<udp::Socket<'static>>(udp).unwrap();
    socket.bind(5353).unwrap();
    socket.send_slice(b"to myself", IpEndpoint::new(IpAddress::Ipv4(local), 5353)).unwrap();
    lo.poll(1_000_000);
    let frames = seen();
    assert_eq!(frames.len(), 1);
    let (number, packet, medium, outgoing) = &frames[0];
    assert_eq!((*number, *medium, *outgoing), (0, Medium::Ip, false));
    assert_eq!(packet[0] >> 4, 4);
    assert!(packet.ends_with(b"to myself"));
    assert_eq!(lo.socket_mut::<udp::Socket<'static>>(udp).unwrap().recv().unwrap().0, b"to myself");
}

#[test]
fn a_watched_loopback_connection_sends_packets() {
    let mut lo = loopback();
    let local = Ipv4Address::new(127, 0, 0, 1);
    let server = lo.add_socket(tcp_memory().socket()).ok().unwrap();
    lo.socket_mut::<tcp::Socket<'static>>(server).unwrap().listen(80).unwrap();
    let client = lo.add_socket(tcp_memory().socket()).ok().unwrap();
    lo.connect(client, (local.into(), 80), None, 49152).unwrap();
    lo.poll(1_000_000);
    let (tap, watched) = tap(0);
    lo.set_tap(Some(tap));
    let mut other = OtherEnd::default();
    let write = |room: &mut [u8]| {
        room[..5].copy_from_slice(b"hello");
        (5, ())
    };
    assert_eq!(lo.send_across(client, &mut other, 2_000_000, write), Across::Packets);
    lo.socket_mut::<tcp::Socket<'static>>(client).unwrap().send_slice(b"hello").unwrap();
    lo.poll(2_000_000);
    let frames = seen();
    assert!(frames.iter().any(|(_, packet, _, _)| packet.ends_with(b"hello")));
    let mut got = [0u8; 16];
    let len = lo.socket_mut::<tcp::Socket<'static>>(server).unwrap().recv_slice(&mut got).unwrap();
    assert_eq!(&got[..len], b"hello");
    // Across again once no capture looks, and the server's ACK has gone.
    watched.store(false, Ordering::Relaxed);
    lo.poll(50_000_000);
    assert_eq!(lo.send_across(client, &mut other, 50_000_000, write), Across::Passed((), 5));
}
