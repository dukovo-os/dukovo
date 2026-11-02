// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Multicast and broadcast datagrams between two stacks on one wire
//! (ADR 0336): the groups an interface joins for its sockets, the
//! sockets each datagram reaches, and the hop limit of those sent.

use std::vec::Vec;

use smoltcp::iface::SocketHandle;
use smoltcp::socket::udp;
use smoltcp::wire::{IpAddress, IpEndpoint};

use crate::tests::{End, pair};
use crate::{GROUPS, Ipv4Address, JoinError, Stack, UdpMemory};

const MDNS: Ipv4Address = Ipv4Address::new(224, 0, 0, 251);

fn leak<T: Clone>(value: T, len: usize) -> &'static mut [T] {
    std::boxed::Box::leak(std::vec![value; len].into_boxed_slice())
}

fn udp_memory() -> UdpMemory {
    UdpMemory {
        receive_meta: leak(udp::PacketMetadata::EMPTY, 4),
        receive: leak(0u8, 2048),
        transmit_meta: leak(udp::PacketMetadata::EMPTY, 4),
        transmit: leak(0u8, 2048),
    }
}

fn run(a: &mut Stack<End>, b: &mut Stack<End>, now: &mut u64) {
    for _ in 0..5 {
        *now += 1_000_000;
        a.poll(*now);
        b.poll(*now);
    }
}

/// A UDP socket on `stack` bound to any address and `port`.
fn bound(stack: &mut Stack<End>, port: u16) -> SocketHandle {
    let handle = stack.add_socket(udp_memory().socket()).ok().unwrap();
    stack.socket_mut::<udp::Socket<'static>>(handle).unwrap().bind(port).unwrap();
    handle
}

fn udp(stack: &mut Stack<End>, handle: SocketHandle) -> &mut udp::Socket<'static> {
    stack.socket_mut::<udp::Socket<'static>>(handle).unwrap()
}

/// The IGMP messages among `frames`: their type and group.
fn igmp(frames: &[Vec<u8>]) -> Vec<(u8, Ipv4Address)> {
    frames
        .iter()
        .filter(|f| f.len() >= 42 && f[12..14] == [0x08, 0x00] && f[23] == 2)
        .map(|f| {
            let at = 14 + usize::from(f[14] & 0x0f) * 4;
            (f[at], Ipv4Address::new(f[at + 4], f[at + 5], f[at + 6], f[at + 7]))
        })
        .collect()
}

/// The time to live of each UDP datagram among `frames`.
fn ttls(frames: &[Vec<u8>]) -> Vec<u8> {
    frames.iter().filter(|f| f[12..14] == [0x08, 0x00] && f[23] == 17).map(|f| f[22]).collect()
}

#[test]
fn a_group_reaches_only_the_sockets_that_joined_it() {
    let (mut a, mut b) = pair();
    let joined = bound(&mut b, 5353);
    let other = bound(&mut b, 5353);
    b.join(MDNS.into()).unwrap();
    assert!(udp(&mut b, joined).join_group(MDNS.into()).is_ok());
    let sender = bound(&mut a, 40000);
    let to = IpEndpoint::new(IpAddress::Ipv4(MDNS), 5353);
    udp(&mut a, sender).send_slice(b"query", to).unwrap();
    let mut now = 0;
    run(&mut a, &mut b, &mut now);
    // b told the routers it joined: an IGMPv2 report for the group.
    assert_eq!(igmp(&b.link().sent.borrow()), [(0x16, MDNS)]);
    let (data, meta) = udp(&mut b, joined).recv().unwrap();
    assert_eq!(data, b"query");
    assert_eq!(meta.local_address, Some(MDNS.into()));
    assert!(udp(&mut b, other).recv().is_err());

    // Left, the group's datagrams no longer come, and the routers hear so.
    assert!(udp(&mut b, joined).leave_group(MDNS.into()));
    assert!(!udp(&mut b, joined).leave_group(MDNS.into()));
    b.leave(MDNS.into());
    udp(&mut a, sender).send_slice(b"again", to).unwrap();
    run(&mut a, &mut b, &mut now);
    assert!(udp(&mut b, joined).recv().is_err());
    assert_eq!(igmp(&b.link().sent.borrow()).last(), Some(&(0x17, MDNS)));
    assert_eq!(b.groups().count(), 0);
}

#[test]
fn an_interface_stays_in_a_group_while_any_socket_has_joined_it() {
    let (_a, mut b) = pair();
    b.join(MDNS.into()).unwrap();
    b.join(MDNS.into()).unwrap();
    assert_eq!(b.groups().collect::<Vec<_>>(), [(IpAddress::Ipv4(MDNS), 2)]);
    b.leave(MDNS.into());
    assert_eq!(b.groups().collect::<Vec<_>>(), [(IpAddress::Ipv4(MDNS), 1)]);
    b.leave(MDNS.into());
    // A group not joined is left without harm.
    b.leave(MDNS.into());
    assert_eq!(b.groups().count(), 0);
    assert_eq!(b.join(Ipv4Address::new(10, 0, 0, 9).into()), Err(JoinError::NotMulticast));
    for n in 0..GROUPS {
        b.join(Ipv4Address::new(239, 1, 0, n as u8).into()).unwrap();
    }
    assert_eq!(b.join(Ipv4Address::new(239, 2, 0, 0).into()), Err(JoinError::Full));
}

#[test]
fn a_broadcast_reaches_every_socket_on_its_port_and_an_address_one() {
    let (mut a, mut b) = pair();
    let first = bound(&mut b, 9);
    let second = bound(&mut b, 9);
    let sender = bound(&mut a, 40001);
    let broadcast = IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::new(10, 0, 0, 255)), 9);
    udp(&mut a, sender).send_slice(b"all", broadcast).unwrap();
    let mut now = 0;
    run(&mut a, &mut b, &mut now);
    assert_eq!(udp(&mut b, first).recv().unwrap().0, b"all");
    assert_eq!(udp(&mut b, second).recv().unwrap().0, b"all");
    let unicast = IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::new(10, 0, 0, 2)), 9);
    udp(&mut a, sender).send_slice(b"one", unicast).unwrap();
    run(&mut a, &mut b, &mut now);
    let got = [udp(&mut b, first).recv().is_ok(), udp(&mut b, second).recv().is_ok()];
    assert_eq!(got.iter().filter(|&&g| g).count(), 1);
}

#[test]
fn datagrams_to_a_group_have_its_hop_limit() {
    let (mut a, mut b) = pair();
    let sender = bound(&mut a, 40002);
    let socket = udp(&mut a, sender);
    socket.set_multicast_hop_limit(Some(1));
    let group = IpEndpoint::new(IpAddress::Ipv4(MDNS), 5353);
    let unicast = IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::new(10, 0, 0, 2)), 5353);
    socket.send_slice(b"group", group).unwrap();
    socket.send_slice(b"one", unicast).unwrap();
    let mut now = 0;
    run(&mut a, &mut b, &mut now);
    let socket = udp(&mut a, sender);
    socket.set_multicast_hop_limit(None);
    socket.set_hop_limit(Some(9));
    socket.send_slice(b"group", group).unwrap();
    run(&mut a, &mut b, &mut now);
    assert_eq!(ttls(&a.link().sent.borrow()), [1, 64, 9]);
}

/// The Internet checksum of `bytes`.
fn checksum(bytes: &[u8]) -> u16 {
    let mut sum: u32 = bytes
        .chunks(2)
        .map(|pair| u32::from(u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)])))
        .sum();
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// An IGMPv2 general query from 10.0.0.1 to every machine (224.0.0.1),
/// asking for reports within `tenths` tenths of a second.
fn general_query(tenths: u8) -> Vec<u8> {
    let mut frame = std::vec![0x01, 0x00, 0x5e, 0x00, 0x00, 0x01, 2, 0, 0, 0, 0, 1, 0x08, 0x00];
    let mut ip = std::vec![0x45, 0, 0, 28, 0, 0, 0, 0, 1, 2, 0, 0, 10, 0, 0, 1, 224, 0, 0, 1];
    let sum = checksum(&ip);
    ip[10..12].copy_from_slice(&sum.to_be_bytes());
    let mut igmp = std::vec![0x11, tenths, 0, 0, 0, 0, 0, 0];
    let sum = checksum(&igmp);
    igmp[2..4].copy_from_slice(&sum.to_be_bytes());
    frame.extend(ip);
    frame.extend(igmp);
    frame
}

#[test]
fn an_interface_in_a_group_answers_the_routers_query() {
    let (mut a, mut b) = pair();
    b.join(MDNS.into()).unwrap();
    let mut now = 0;
    run(&mut a, &mut b, &mut now);
    assert_eq!(igmp(&b.link().sent.borrow()), [(0x16, MDNS)]);
    // With nothing to tell, polls send nothing more.
    run(&mut a, &mut b, &mut now);
    assert_eq!(igmp(&b.link().sent.borrow()).len(), 1);
    // A router asks which groups are wanted, within a second: b reports
    // its own again, half a second on, as one group spreads them so.
    b.link().inbox.borrow_mut().push_back(general_query(10));
    for _ in 0..20 {
        now += 100_000_000;
        b.poll(now);
    }
    assert_eq!(igmp(&b.link().sent.borrow()), [(0x16, MDNS), (0x16, MDNS)]);
}
