// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Two stacks on the two ends of a tunnel (ADR 0339): IP links, whose
//! packets go as they are, with no Ethernet header, as a WireGuard
//! interface's go to its program. Their IPv6 addresses are theirs at
//! once, with no link-local one, and what they receive passes through
//! the firewall's hook as a card's frames do.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::vec::Vec;

use smoltcp::socket::tcp;
use smoltcp::wire::{IpAddress, Ipv6Address, Ipv6Cidr};

use crate::tests::tcp_memory;
use crate::{Hooks, Ipv4Address, Ipv4Cidr, Link, Medium, Memory, Stack, filter, ipv6};

type Queue = Rc<RefCell<VecDeque<Vec<u8>>>>;

/// One end of a tunnel: packets sent go into `out`, packets received
/// come from `inbox`.
pub(crate) struct Tun {
    inbox: Queue,
    out: Queue,
}

impl Link for Tun {
    fn medium(&self) -> Medium {
        Medium::Ip
    }

    fn mac(&self) -> [u8; 6] {
        [0; 6]
    }

    fn receive(&mut self, packet: &mut [u8]) -> Option<usize> {
        let got = self.inbox.borrow_mut().pop_front()?;
        let len = got.len().min(packet.len());
        packet[..len].copy_from_slice(&got[..len]);
        Some(len)
    }

    fn can_send(&mut self) -> bool {
        true
    }

    fn send(&mut self, packet: &[u8]) -> bool {
        self.out.borrow_mut().push_back(packet.to_vec());
        true
    }

    fn flush(&mut self) {}
}

fn leak<T: Clone>(value: T, len: usize) -> &'static mut [T] {
    std::boxed::Box::leak(std::vec![value; len].into_boxed_slice())
}

/// A WireGuard interface's MTU, 1420.
const MTU: usize = 1420;

fn memory() -> Memory {
    Memory {
        receive: leak(0u8, MTU),
        transmit: leak(0u8, MTU),
        sockets: std::boxed::Box::leak(std::boxed::Box::new(
            [smoltcp::iface::SocketStorage::EMPTY; 4],
        )),
    }
}

/// The two ends of a tunnel, at 10.9.0.1 and 10.9.0.2 and fd09::1 and
/// fd09::2.
pub(crate) fn tunnel() -> (Stack<Tun>, Stack<Tun>) {
    let (a_to_b, b_to_a): (Queue, Queue) = Default::default();
    let mut a = Stack::new(Tun { inbox: b_to_a.clone(), out: a_to_b.clone() }, memory(), 1, 0);
    let mut b = Stack::new(Tun { inbox: a_to_b, out: b_to_a }, memory(), 2, 0);
    let settings = ipv6::Settings { secret: [7; 16], autoconf: false, temporary: false };
    for (stack, n) in [(&mut a, 1), (&mut b, 2)] {
        stack.set_ipv4(Ipv4Cidr::new(Ipv4Address::new(10, 9, 0, n), 24), None);
        let six = Ipv6Cidr::new(Ipv6Address::new(0xfd09, 0, 0, 0, 0, 0, 0, u16::from(n)), 64);
        stack.configure_ipv6(Some(settings), &[six], None);
    }
    (a, b)
}

fn run(a: &mut Stack<Tun>, b: &mut Stack<Tun>, now: &mut u64) {
    for _ in 0..10 {
        *now += 1_000_000;
        a.poll(*now);
        b.poll(*now);
    }
}

fn sift(_: usize, _: &[u8]) -> bool {
    false
}

fn next(_: usize, _: &mut [u8]) -> Option<usize> {
    None
}

fn deny(_: usize, _: &filter::Inbound, _: u64) -> bool {
    false
}

fn allow(_: usize, _: &filter::Inbound, _: u64) -> bool {
    true
}

/// Connects a to b's port 8080 at `to`, which b listens on: whether the
/// connection was made.
fn connects(a: &mut Stack<Tun>, b: &mut Stack<Tun>, to: IpAddress, port: u16) -> bool {
    let server = b.add_socket(tcp_memory().socket()).ok().unwrap();
    b.socket_mut::<tcp::Socket<'static>>(server).unwrap().listen(8080).unwrap();
    let client = a.add_socket(tcp_memory().socket()).ok().unwrap();
    a.connect(client, (to, 8080), None, port).unwrap();
    let mut now = 0;
    run(a, b, &mut now);
    let state = a.socket_mut::<tcp::Socket<'static>>(client).unwrap().state();
    a.socket_mut::<tcp::Socket<'static>>(client).unwrap().abort();
    b.socket_mut::<tcp::Socket<'static>>(server).unwrap().abort();
    run(a, b, &mut now);
    let _ = (a.remove_tcp(client), b.remove_tcp(server));
    state == tcp::State::Established
}

#[test]
fn a_tunnels_ipv6_addresses_are_its_own_at_once_without_a_link_local_one() {
    let (a, _) = tunnel();
    let addresses: Vec<Ipv6Cidr> = a.ipv6_addresses().collect();
    assert_eq!(
        addresses,
        [Ipv6Cidr::new(Ipv6Address::new(0xfd09, 0, 0, 0, 0, 0, 0, 1), 64)],
        "given and untested, and no fe80:: address"
    );
    assert_eq!(a.mac(), None);
    assert_eq!(a.mtu(), MTU);
}

#[test]
fn tcp_goes_through_the_tunnel_by_ipv4_and_ipv6_as_the_firewall_allows() {
    let (mut a, mut b) = tunnel();
    let four = IpAddress::Ipv4(Ipv4Address::new(10, 9, 0, 2));
    let six = IpAddress::Ipv6(Ipv6Address::new(0xfd09, 0, 0, 0, 0, 0, 0, 2));
    assert!(connects(&mut a, &mut b, four, 49152));
    assert!(connects(&mut a, &mut b, six, 49153));
    // A firewall that lets nothing in keeps b's port from a, as on a card.
    b.set_hooks(Some(Hooks { number: 1, sift, next, admit: deny }));
    assert!(!connects(&mut a, &mut b, four, 49154));
    assert!(!connects(&mut a, &mut b, six, 49155));
    b.set_hooks(Some(Hooks { number: 1, sift, next, admit: allow }));
    assert!(connects(&mut a, &mut b, four, 49156));
}
