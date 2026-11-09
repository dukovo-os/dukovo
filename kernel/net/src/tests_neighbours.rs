// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a stack remembers of its neighbours (ADR 0417): it keeps them when
//! an address changes and the network stays, forgets them when the link
//! goes down, and an answer it makes to one it does not know waits for the
//! neighbour to answer its request rather than being lost.

use std::cell::Cell;
use std::rc::Rc;

use crate::tests::{End, ends, memory, ping_memory};
use crate::{Ipv4Address, Ipv4Cidr, Link, Pinger, Stack};

/// One end of the wire, with a link that can go down.
struct Plugged {
    end: End,
    up: Rc<Cell<bool>>,
}

impl Link for Plugged {
    fn mac(&self) -> [u8; 6] {
        self.end.mac()
    }

    fn receive(&mut self, frame: &mut [u8]) -> Option<usize> {
        self.end.receive(frame)
    }

    fn can_send(&mut self) -> bool {
        self.end.can_send()
    }

    fn send(&mut self, frame: &[u8]) -> bool {
        self.end.send(frame)
    }

    fn flush(&mut self) {
        self.end.flush();
    }

    fn carrier(&mut self) -> bool {
        self.up.get()
    }
}

/// Two stacks on one wire, at 10.0.0.1/24 and 10.0.0.2/24, and the switch
/// of the first one's link.
fn plugged() -> (Stack<Plugged>, Stack<Plugged>, Rc<Cell<bool>>) {
    let (a, b) = ends(true);
    let up = Rc::new(Cell::new(true));
    let a = Plugged { end: a, up: up.clone() };
    let b = Plugged { end: b, up: Rc::new(Cell::new(true)) };
    let mut a = Stack::new(a, memory(), 1, 0);
    let mut b = Stack::new(b, memory(), 2, 0);
    a.set_ipv4(Ipv4Cidr::new(Ipv4Address::new(10, 0, 0, 1), 24), None);
    b.set_ipv4(Ipv4Cidr::new(Ipv4Address::new(10, 0, 0, 2), 24), None);
    (a, b, up)
}

/// Polls both stacks, a millisecond apart, `rounds` times.
fn run(a: &mut Stack<Plugged>, b: &mut Stack<Plugged>, now: &mut u64, rounds: usize) {
    for _ in 0..rounds {
        *now += 1_000_000;
        a.poll(*now);
        b.poll(*now);
    }
}

/// How many ARP requests `stack` sent.
fn arp_requests(stack: &mut Stack<Plugged>) -> usize {
    let sent = stack.link().end.sent.borrow();
    // The Ethernet type ARP, and the operation a request.
    sent.iter().filter(|frame| frame[12..14] == [0x08, 0x06] && frame[20..22] == [0, 1]).count()
}

/// Has `from` ping `to` once with `seq`, and says whether the reply came.
fn ping(
    from: &mut Stack<Plugged>,
    other: &mut Stack<Plugged>,
    pinger: &Pinger,
    to: Ipv4Address,
    seq: u16,
    now: &mut u64,
) -> bool {
    pinger.send(from, to, seq).unwrap();
    run(from, other, now, 5);
    pinger.reply(from) == Some((seq, to))
}

#[test]
fn keeps_its_neighbours_when_an_address_changes_on_their_network() {
    let (mut a, mut b, _) = plugged();
    let pinger = Pinger::new(&mut a, ping_memory(), 1).ok().unwrap();
    let mut now = 0;
    let to = Ipv4Address::new(10, 0, 0, 2);
    assert!(ping(&mut a, &mut b, &pinger, to, 1, &mut now));
    assert_eq!(arp_requests(&mut a), 1);

    // The same address again, as DHCP renews it, and then another on the
    // same network: the stack still knows 10.0.0.2's card.
    a.set_ipv4(Ipv4Cidr::new(Ipv4Address::new(10, 0, 0, 1), 24), None);
    assert!(ping(&mut a, &mut b, &pinger, to, 2, &mut now));
    a.configure(
        &[
            Ipv4Cidr::new(Ipv4Address::new(10, 0, 0, 1), 24),
            Ipv4Cidr::new(Ipv4Address::new(10, 0, 0, 7), 24),
        ],
        None,
    );
    assert!(ping(&mut a, &mut b, &pinger, to, 3, &mut now));
    assert_eq!(arp_requests(&mut a), 1, "no request after an address change");
}

#[test]
fn forgets_its_neighbours_on_a_network_it_left() {
    let (mut a, mut b, _) = plugged();
    let pinger = Pinger::new(&mut a, ping_memory(), 1).ok().unwrap();
    let mut now = 0;
    let to = Ipv4Address::new(10, 0, 0, 2);
    assert!(ping(&mut a, &mut b, &pinger, to, 1, &mut now));

    // Moved to another network and back: 10.0.0.2 may be another machine.
    // (smoltcp sends a request a second at most.)
    a.set_ipv4(Ipv4Cidr::new(Ipv4Address::new(10, 0, 1, 1), 24), None);
    a.set_ipv4(Ipv4Cidr::new(Ipv4Address::new(10, 0, 0, 1), 24), None);
    run(&mut a, &mut b, &mut now, 1_000);
    assert!(ping(&mut a, &mut b, &pinger, to, 2, &mut now));
    assert_eq!(arp_requests(&mut a), 2);
}

#[test]
fn forgets_its_neighbours_when_its_link_goes_down() {
    let (mut a, mut b, up) = plugged();
    let pinger = Pinger::new(&mut a, ping_memory(), 1).ok().unwrap();
    let mut now = 0;
    let to = Ipv4Address::new(10, 0, 0, 2);
    assert!(ping(&mut a, &mut b, &pinger, to, 1, &mut now));

    // A link that stays up keeps them, however long.
    run(&mut a, &mut b, &mut now, 2_500);
    assert!(ping(&mut a, &mut b, &pinger, to, 2, &mut now));
    assert_eq!(arp_requests(&mut a), 1);

    // Down for two seconds, as a cable moved to another switch: the stack
    // asks for 10.0.0.2 again once it is back.
    up.set(false);
    run(&mut a, &mut b, &mut now, 2_000);
    up.set(true);
    assert!(ping(&mut a, &mut b, &pinger, to, 3, &mut now));
    assert_eq!(arp_requests(&mut a), 2);
}

#[test]
fn an_answer_to_a_neighbour_it_forgot_waits_for_it() {
    let (mut a, mut b, up) = plugged();
    let pinger = Pinger::new(&mut b, ping_memory(), 1).ok().unwrap();
    let mut now = 0;
    let to = Ipv4Address::new(10, 0, 0, 1);
    // b knows a's card, and a b's, from b's request.
    assert!(ping(&mut b, &mut a, &pinger, to, 1, &mut now));

    // a's link goes down and comes back: a forgets b, which still knows a,
    // as a machine that was not unplugged does.
    up.set(false);
    run(&mut a, &mut b, &mut now, 1_100);
    up.set(true);
    run(&mut a, &mut b, &mut now, 1_100);
    let asked = arp_requests(&mut a);

    // b's next ping comes straight to a, whose answer waits while a asks
    // for b, and goes when b answers: the ping is not lost.
    assert!(ping(&mut b, &mut a, &pinger, to, 2, &mut now));
    assert_eq!(arp_requests(&mut a), asked + 1);
}
