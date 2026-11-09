// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A stack whose card has no room for what it has to send (ADR 0418): it
//! polls again at once for a millisecond, as a card that sends has room
//! again within that, then waits between polls rather than keep a CPU
//! busy, and sends what waited as soon as the card has room. An IPv6
//! address test it could not send waits the same way, and is not counted
//! as a frame lost.

use std::cell::Cell;
use std::rc::Rc;

use smoltcp::iface::SocketHandle;
use smoltcp::socket::udp;
use smoltcp::wire::{IpAddress, IpEndpoint};

use crate::ipv6::{Address, Settings};
use crate::tests::{End, ends, memory, udp_memory};
use crate::{FULL_NS, Ipv4Address, Ipv4Cidr, Link, STUCK_NS, Stack};

const MS: u64 = 1_000_000;

/// One end of the wire, with a card whose ring can be full.
struct Full {
    end: End,
    room: Rc<Cell<bool>>,
}

impl Link for Full {
    fn mac(&self) -> [u8; 6] {
        self.end.mac()
    }

    fn receive(&mut self, frame: &mut [u8]) -> Option<usize> {
        self.end.receive(frame)
    }

    fn can_send(&mut self) -> bool {
        self.room.get()
    }

    fn send(&mut self, frame: &[u8]) -> bool {
        self.room.get() && self.end.send(frame)
    }

    fn flush(&mut self) {
        self.end.flush();
    }
}

/// Two stacks on one wire, at 10.0.0.1/24 and 10.0.0.2/24, and the
/// switch that fills the first one's ring.
fn full_pair() -> (Stack<Full>, Stack<Full>, Rc<Cell<bool>>) {
    let (a, b) = ends(true);
    let room = Rc::new(Cell::new(true));
    let a = Full { end: a, room: room.clone() };
    let b = Full { end: b, room: Rc::new(Cell::new(true)) };
    let mut a = Stack::new(a, memory(), 1, 0);
    let mut b = Stack::new(b, memory(), 2, 0);
    a.set_ipv4(Ipv4Cidr::new(Ipv4Address::new(10, 0, 0, 1), 24), None);
    b.set_ipv4(Ipv4Cidr::new(Ipv4Address::new(10, 0, 0, 2), 24), None);
    (a, b, room)
}

/// A UDP socket on `stack` at `port`.
fn udp_socket(stack: &mut Stack<Full>, port: u16) -> SocketHandle {
    let handle = stack.add_socket(udp_memory().socket()).ok().unwrap();
    stack.socket_mut::<udp::Socket<'static>>(handle).unwrap().bind(port).unwrap();
    handle
}

/// Has `socket` on `stack` send `data` to the second stack's port 53.
fn send(stack: &mut Stack<Full>, socket: SocketHandle, data: &[u8]) {
    let to = IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::new(10, 0, 0, 2)), 53);
    stack.socket_mut::<udp::Socket<'static>>(socket).unwrap().send_slice(data, to).unwrap();
}

/// What `socket` on `stack` received, if anything.
fn received(stack: &mut Stack<Full>, socket: SocketHandle) -> Option<std::vec::Vec<u8>> {
    let socket = stack.socket_mut::<udp::Socket<'static>>(socket).unwrap();
    socket.recv().ok().map(|(data, _)| data.to_vec())
}

/// Polls both stacks, a millisecond apart, `rounds` times.
fn run(a: &mut Stack<Full>, b: &mut Stack<Full>, now: &mut u64, rounds: usize) {
    for _ in 0..rounds {
        *now += MS;
        a.poll(*now);
        b.poll(*now);
    }
}

#[test]
fn waits_between_polls_while_its_card_has_no_room_and_sends_once_it_has() {
    let (mut a, mut b, room) = full_pair();
    let receiver = udp_socket(&mut b, 53);
    let sender = udp_socket(&mut a, 40000);
    let mut now = 0;
    // The first datagram goes, and the neighbour is known.
    send(&mut a, sender, b"one");
    run(&mut a, &mut b, &mut now, 5);
    assert_eq!(received(&mut b, receiver).as_deref(), Some(&b"one"[..]));
    assert_eq!(a.poll_delay(now), None);

    // The card's ring is full. For a millisecond the stack polls again
    // at once, as a card that sends has room again by then.
    room.set(false);
    send(&mut a, sender, b"two");
    let full = now;
    while now < full + FULL_NS {
        a.poll(now);
        assert_eq!(a.poll_delay(now), Some(0));
        now += FULL_NS / 4;
    }
    // Then it waits between polls, for as long as the card has none.
    for _ in 0..100 {
        a.poll(now);
        assert_eq!(a.poll_delay(now), Some(STUCK_NS));
        now += STUCK_NS;
    }
    // Waiting is not losing: nothing was counted as dropped.
    assert_eq!(a.counters().send_dropped, 0);

    // Room again: the datagram goes at the next poll, and nothing waits.
    room.set(true);
    run(&mut a, &mut b, &mut now, 2);
    assert_eq!(received(&mut b, receiver).as_deref(), Some(&b"two"[..]));
    assert_eq!(a.poll_delay(now), None);

    // A ring full again polls at once again first: the millisecond is
    // counted from when the card last had room.
    room.set(false);
    send(&mut a, sender, b"three");
    a.poll(now);
    assert_eq!(a.poll_delay(now), Some(0));
}

#[test]
fn an_address_test_waits_for_room_and_is_not_counted_lost() {
    let (mut a, _, room) = full_pair();
    room.set(false);
    a.configure_ipv6(
        Some(Settings { secret: *b"0123456789abcdef", autoconf: true, temporary: false }),
        &[],
        None,
    );
    // The link-local address's test is due, but the card has no room for
    // its solicitation: the stack waits between polls after a
    // millisecond, rather than try again at once until it has.
    let mut now = 0;
    a.poll(now);
    assert_eq!(a.poll_delay(now), Some(0));
    now += FULL_NS;
    for _ in 0..100 {
        a.poll(now);
        assert_eq!(a.poll_delay(now), Some(STUCK_NS));
        now += STUCK_NS;
    }
    assert_eq!(a.counters().send_dropped, 0);
    assert!(a.link().end.sent.borrow().is_empty());
    assert!(a.ipv6().addresses().all(Address::tentative));

    // With room, the solicitation goes, and a second later the address
    // is the stack's.
    room.set(true);
    a.poll(now);
    let sent = a.link().end.sent.borrow().len();
    assert!(sent >= 1, "the address's test was sent");
    now += 1_100 * MS;
    a.poll(now);
    assert!(a.ipv6().addresses().any(|address| !address.tentative()));
}
