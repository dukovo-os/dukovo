// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The ICMP and ICMPv6 errors a stack hears about what its sockets sent
//! (ADR 0345): read from packets made here, RFC 792's short quotes among
//! them, and heard as a stack polls, from its own loopback link, from
//! the other end of a wire and of a tunnel.

use std::cell::RefCell;
use std::vec::Vec;

use smoltcp::socket::udp;
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv6Address};

use crate::reports::{self, About, QUOTE, Reason, Report};
use crate::tests::{loopback, pair, udp_memory};
use crate::{Ipv4Address, Link, Medium, Reporter, Stack};

/// The internet checksum of `bytes`.
fn checksum(bytes: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for pair in bytes.chunks(2) {
        let word = u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)]);
        sum += u32::from(word);
    }
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// An IPv4 packet from `from` to `to` carrying `body` as `protocol`.
fn ipv4(from: [u8; 4], to: [u8; 4], protocol: u8, ttl: u8, body: &[u8]) -> Vec<u8> {
    let len = u16::try_from(20 + body.len()).unwrap();
    let mut packet = std::vec![0x45, 0];
    packet.extend_from_slice(&len.to_be_bytes());
    packet.extend_from_slice(&[0, 0, 0x40, 0, ttl, protocol, 0, 0]);
    packet.extend_from_slice(&from);
    packet.extend_from_slice(&to);
    let sum = checksum(&packet);
    packet[10..12].copy_from_slice(&sum.to_be_bytes());
    packet.extend_from_slice(body);
    packet
}

/// An ICMP message of `kind` and `code`, its word after the checksum
/// `rest`, quoting `quote`.
fn icmp(kind: u8, code: u8, rest: [u8; 4], quote: &[u8]) -> Vec<u8> {
    let mut message = std::vec![kind, code, 0, 0];
    message.extend_from_slice(&rest);
    message.extend_from_slice(quote);
    let sum = checksum(&message);
    message[2..4].copy_from_slice(&sum.to_be_bytes());
    message
}

/// A UDP datagram from port `from` to `to`, carrying `data`, its
/// checksum left out, as IPv4 allows.
fn udp_datagram(from: u16, to: u16, data: &[u8]) -> Vec<u8> {
    let mut datagram = Vec::new();
    datagram.extend_from_slice(&from.to_be_bytes());
    datagram.extend_from_slice(&to.to_be_bytes());
    datagram.extend_from_slice(&u16::try_from(8 + data.len()).unwrap().to_be_bytes());
    datagram.extend_from_slice(&[0, 0]);
    datagram.extend_from_slice(data);
    datagram
}

/// An echo request of `kind` with `ident` and `sequence`, carrying
/// `data`; its checksum is not looked at.
fn echo(kind: u8, ident: u16, sequence: u16, data: &[u8]) -> Vec<u8> {
    let mut request = std::vec![kind, 0, 0, 0];
    request.extend_from_slice(&ident.to_be_bytes());
    request.extend_from_slice(&sequence.to_be_bytes());
    request.extend_from_slice(data);
    request
}

const ME: [u8; 4] = [10, 0, 2, 15];
const ROUTER: [u8; 4] = [10, 0, 2, 2];
const FAR: [u8; 4] = [192, 0, 2, 7];

fn v4(octets: [u8; 4]) -> IpAddress {
    IpAddress::Ipv4(Ipv4Address::from(octets))
}

/// The router's "time exceeded" about `sent`, which this machine sent,
/// quoting `quoted` bytes of it.
fn exceeded(sent: &[u8], quoted: usize) -> Vec<u8> {
    let message = icmp(11, 0, [0; 4], &sent[..quoted.min(sent.len())]);
    ipv4(ROUTER, ME, 1, 64, &message)
}

#[test]
fn reads_a_time_exceeded_that_quotes_8_bytes_as_rfc_792_routers_do() {
    let sent = ipv4(ME, FAR, 17, 1, &udp_datagram(40000, 33434, &[0x5a; 32]));
    let packet = exceeded(&sent, 28);
    let report = reports::heard(&packet, Medium::Ip, true).unwrap();
    assert_eq!(report.about, About::Udp);
    assert_eq!((report.offender, report.to), (v4(ROUTER), v4(ME)));
    assert_eq!((report.kind, report.code, report.info), (11, 0, 0));
    assert_eq!((report.local, report.local_port), (v4(ME), 40000));
    assert_eq!((report.remote, report.remote_port), (v4(FAR), 33434));
    assert!(report.data().is_empty(), "nothing past the UDP header");
    assert!(!report.is_v6());
    assert_eq!(report.sequence(), None);
}

#[test]
fn keeps_what_a_whole_quote_holds_past_the_udp_header() {
    let sent = ipv4(ME, FAR, 17, 1, &udp_datagram(40000, 33434, &[0x5a; 32]));
    let report = reports::heard(&exceeded(&sent, sent.len()), Medium::Ip, true).unwrap();
    assert_eq!(report.data(), &[0x5a; 32][..]);
    // A long one, as far as QUOTE.
    let sent = ipv4(ME, FAR, 17, 1, &udp_datagram(40000, 33434, &[7; 400]));
    let report = reports::heard(&exceeded(&sent, sent.len()), Medium::Ip, true).unwrap();
    assert_eq!(report.data(), &[7; QUOTE][..]);
}

#[test]
fn reads_one_in_an_ethernet_frame_padded_past_its_end() {
    let sent = ipv4(ME, FAR, 17, 1, &udp_datagram(40000, 9, b""));
    let message = icmp(3, 3, [0; 4], &sent);
    let mut frame = std::vec![0u8; 12];
    frame.extend_from_slice(&[0x08, 0x00]);
    frame.extend_from_slice(&ipv4(FAR, ME, 1, 64, &message));
    frame.resize(frame.len() + 20, 0xee);
    assert!(reports::may_be(&frame, Medium::Ethernet));
    let report = reports::heard(&frame, Medium::Ethernet, true).unwrap();
    assert_eq!((report.kind, report.code), (3, 3));
    assert_eq!(report.offender, v4(FAR));
    // An ARP frame is none.
    frame[12..14].copy_from_slice(&[0x08, 0x06]);
    assert!(!reports::may_be(&frame, Medium::Ethernet));
    assert_eq!(reports::heard(&frame, Medium::Ethernet, true), None);
}

#[test]
fn says_the_mtu_a_datagram_needs_fragmenting_for_and_a_problems_pointer() {
    let sent = ipv4(ME, FAR, 17, 64, &udp_datagram(40000, 53, b"query"));
    let message = icmp(3, 4, [0, 0, 0x05, 0x78], &sent);
    let report = reports::heard(&ipv4(ROUTER, ME, 1, 64, &message), Medium::Ip, true).unwrap();
    assert_eq!((report.kind, report.code, report.info), (3, 4, 1400));
    let message = icmp(12, 0, [9, 0, 0, 0], &sent);
    let report = reports::heard(&ipv4(ROUTER, ME, 1, 64, &message), Medium::Ip, true).unwrap();
    assert_eq!((report.kind, report.info), (12, 9));
}

#[test]
fn reads_one_about_an_echo_request_with_its_identifier_and_number() {
    let sent = ipv4(ME, FAR, 1, 2, &echo(8, 0x1234, 7, &[1; 56]));
    let report = reports::heard(&exceeded(&sent, 28), Medium::Ip, true).unwrap();
    assert_eq!(report.about, About::Ping);
    assert_eq!((report.local_port, report.remote_port), (0x1234, 0));
    assert_eq!(report.sequence(), Some(7));
    assert_eq!(report.data()[..2], [8, 0], "from the request's header on");
    assert_eq!(report.data().len(), 8);
}

#[test]
fn takes_no_other_message_and_none_it_cannot_read_ports_in() {
    let sent = ipv4(ME, FAR, 17, 1, &udp_datagram(40000, 33434, b""));
    let error = |kind: u8, quote: &[u8]| ipv4(ROUTER, ME, 1, 64, &icmp(kind, 0, [0; 4], quote));
    // Echo replies, redirects and source quenches.
    for kind in [0, 4, 5] {
        assert_eq!(reports::heard(&error(kind, &sent), Medium::Ip, true), None, "{kind}");
    }
    // A quote short of the UDP header.
    assert_eq!(reports::heard(&error(11, &sent[..27]), Medium::Ip, true), None);
    // About TCP, or an echo reply.
    let tcp = ipv4(ME, FAR, 6, 1, &[0; 20]);
    assert_eq!(reports::heard(&error(11, &tcp), Medium::Ip, true), None);
    let reply = ipv4(ME, FAR, 1, 1, &echo(0, 1, 1, b""));
    assert_eq!(reports::heard(&error(11, &reply), Medium::Ip, true), None);
    // About a fragment past the first.
    let mut fragment = sent.clone();
    fragment[6..8].copy_from_slice(&[0x00, 0x10]);
    assert_eq!(reports::heard(&error(11, &fragment), Medium::Ip, true), None);
    // In a fragment itself.
    let mut split = error(11, &sent);
    split[6] = 0x20;
    assert_eq!(reports::heard(&split, Medium::Ip, true), None);
    // Not ICMP at all.
    let datagram = ipv4(ROUTER, ME, 17, 64, &udp_datagram(53, 40000, b"answer"));
    assert!(!reports::may_be(&datagram, Medium::Ip));
    assert_eq!(reports::heard(&datagram, Medium::Ip, true), None);
}

#[test]
fn a_wrong_checksum_counts_only_where_checksums_are_checked() {
    let sent = ipv4(ME, FAR, 17, 1, &udp_datagram(40000, 33434, b""));
    let mut packet = exceeded(&sent, 28);
    packet[22] ^= 0xff;
    assert_eq!(reports::heard(&packet, Medium::Ip, true), None);
    assert!(reports::heard(&packet, Medium::Ip, false).is_some());
}

/// An IPv6 packet from `from` to `to` carrying `body` as `next`.
fn ipv6(from: Ipv6Address, to: Ipv6Address, next: u8, hops: u8, body: &[u8]) -> Vec<u8> {
    let mut packet = std::vec![0x60, 0, 0, 0];
    packet.extend_from_slice(&u16::try_from(body.len()).unwrap().to_be_bytes());
    packet.extend_from_slice(&[next, hops]);
    packet.extend_from_slice(&from.octets());
    packet.extend_from_slice(&to.octets());
    packet.extend_from_slice(body);
    packet
}

/// An ICMPv6 message from `from` to `to`, its checksum made.
fn icmpv6(from: Ipv6Address, to: Ipv6Address, kind: u8, rest: [u8; 4], quote: &[u8]) -> Vec<u8> {
    let mut message = std::vec![kind, 0, 0, 0];
    message.extend_from_slice(&rest);
    message.extend_from_slice(quote);
    let mut pseudo = Vec::new();
    pseudo.extend_from_slice(&from.octets());
    pseudo.extend_from_slice(&to.octets());
    pseudo.extend_from_slice(&u32::try_from(message.len()).unwrap().to_be_bytes());
    pseudo.extend_from_slice(&[0, 0, 0, 58]);
    pseudo.extend_from_slice(&message);
    let sum = checksum(&pseudo);
    message[2..4].copy_from_slice(&sum.to_be_bytes());
    message
}

#[test]
fn reads_icmpv6_errors_about_datagrams_and_echo_requests() {
    let me = Ipv6Address::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0x15);
    let router = Ipv6Address::new(0xfe80, 0, 0, 0, 0, 0, 0, 1);
    let far = Ipv6Address::new(0x2001, 0xdb8, 9, 0, 0, 0, 0, 7);
    let sent = ipv6(me, far, 17, 1, &udp_datagram(40000, 33434, b"probe"));
    let message = icmpv6(router, me, 3, [0; 4], &sent);
    let packet = ipv6(router, me, 58, 255, &message);
    assert!(reports::may_be(&packet, Medium::Ip));
    let report = reports::heard(&packet, Medium::Ip, true).unwrap();
    assert!(report.is_v6());
    assert_eq!(report.about, About::Udp);
    assert_eq!((report.kind, report.code, report.info), (3, 0, 0));
    assert_eq!(report.offender, IpAddress::Ipv6(router));
    assert_eq!((report.local, report.local_port), (IpAddress::Ipv6(me), 40000));
    assert_eq!((report.remote, report.remote_port), (IpAddress::Ipv6(far), 33434));
    assert_eq!(report.data(), b"probe");

    // A packet too big says its MTU.
    let message = icmpv6(router, me, 2, 1280u32.to_be_bytes(), &sent);
    let report = reports::heard(&ipv6(router, me, 58, 255, &message), Medium::Ip, true).unwrap();
    assert_eq!((report.kind, report.info), (2, 1280));

    // About an echo request.
    let sent = ipv6(me, far, 58, 2, &echo(128, 0x4321, 3, &[0; 8]));
    let message = icmpv6(router, me, 3, [0; 4], &sent);
    let report = reports::heard(&ipv6(router, me, 58, 255, &message), Medium::Ip, true).unwrap();
    assert_eq!(
        (report.about, report.local_port, report.sequence()),
        (About::Ping, 0x4321, Some(3))
    );

    // A wrong checksum, and an informational message.
    let mut wrong = ipv6(router, me, 58, 255, &message);
    wrong[42] ^= 1;
    assert_eq!(reports::heard(&wrong, Medium::Ip, true), None);
    let message = icmpv6(router, me, 129, [0; 4], &sent);
    assert_eq!(reports::heard(&ipv6(router, me, 58, 255, &message), Medium::Ip, true), None);
}

std::thread_local! {
    /// What the reporter heard on this test's thread.
    static HEARD: RefCell<Vec<(usize, Report)>> = const { RefCell::new(Vec::new()) };
}

fn hear(number: usize, report: &Report) {
    HEARD.with(|heard| heard.borrow_mut().push((number, *report)));
}

fn heard() -> Vec<(usize, Report)> {
    HEARD.with(|heard| core::mem::take(&mut *heard.borrow_mut()))
}

/// A UDP socket on `stack` bound to `port`, which sends `data` to `to`.
fn send_from<L: Link>(stack: &mut Stack<L>, port: u16, data: &[u8], to: IpEndpoint) {
    let handle = stack.add_socket(udp_memory().socket()).ok().unwrap();
    let socket = stack.socket_mut::<udp::Socket<'static>>(handle).unwrap();
    socket.bind(port).unwrap();
    socket.send_slice(data, to).unwrap();
}

#[test]
fn hears_its_own_port_unreachable_on_the_loopback_link() {
    let mut lo = loopback();
    let local = IpAddress::Ipv4(Ipv4Address::new(127, 0, 0, 1));
    send_from(&mut lo, 40000, b"anyone?", IpEndpoint::new(local, 9));
    lo.poll(1_000_000);
    assert!(heard().is_empty(), "no reporter hears it");

    lo.set_reporter(Some(Reporter { number: 0, hear }));
    send_from(&mut lo, 40001, b"anyone?", IpEndpoint::new(local, 9));
    assert!(lo.poll(2_000_000), "a report changes what a socket can do");
    let heard = heard();
    assert_eq!(heard.len(), 1);
    let (number, report) = heard[0];
    assert_eq!(number, 0);
    assert_eq!((report.kind, report.code), (3, 3), "port unreachable");
    assert_eq!((report.offender, report.to), (local, local));
    assert_eq!((report.local_port, report.remote_port), (40001, 9));
    assert_eq!(report.data(), b"anyone?");
}

#[test]
fn hears_the_other_end_of_a_wire_and_only_errors_to_its_own_addresses() {
    let (mut a, mut b) = pair();
    a.set_reporter(Some(Reporter { number: 3, hear }));
    let b_address = IpAddress::Ipv4(Ipv4Address::new(10, 0, 0, 2));
    send_from(&mut a, 40000, b"x", IpEndpoint::new(b_address, 9));
    let mut now = 0;
    for _ in 0..5 {
        now += 1_000_000;
        a.poll(now);
        b.poll(now);
    }
    let heard = heard();
    assert_eq!(heard.len(), 1);
    assert_eq!(heard[0].0, 3);
    assert_eq!(heard[0].1.offender, b_address);

    // One to an address that is not a's, and one whose checksum is
    // wrong, which a card's frames are checked for.
    let sent = ipv4([10, 0, 0, 9], FAR, 17, 1, &udp_datagram(40000, 33434, b""));
    let message = icmp(11, 0, [0; 4], &sent);
    let mut frame = std::vec![2, 0, 0, 0, 0, 1, 2, 0, 0, 0, 0, 2, 0x08, 0x00];
    frame.extend_from_slice(&ipv4([10, 0, 0, 2], [10, 0, 0, 9], 1, 64, &message));
    a.link().inbox.borrow_mut().push_back(frame.clone());
    let sent = ipv4([10, 0, 0, 1], FAR, 17, 1, &udp_datagram(40000, 33434, b""));
    let mut message = icmp(11, 0, [0; 4], &sent);
    message[4] = 1;
    frame.truncate(14);
    frame.extend_from_slice(&ipv4([10, 0, 0, 2], [10, 0, 0, 1], 1, 64, &message));
    a.link().inbox.borrow_mut().push_back(frame);
    a.poll(now + 1_000_000);
    assert!(self::heard().is_empty());
}

#[test]
fn hears_an_icmpv6_port_unreachable_through_a_tunnel() {
    let (mut a, mut b) = crate::tests_tunnel::tunnel();
    a.set_reporter(Some(Reporter { number: 5, hear }));
    let b_address = IpAddress::Ipv6(Ipv6Address::new(0xfd09, 0, 0, 0, 0, 0, 0, 2));
    send_from(&mut a, 40000, b"six", IpEndpoint::new(b_address, 9));
    let mut now = 0;
    for _ in 0..5 {
        now += 1_000_000;
        a.poll(now);
        b.poll(now);
    }
    let heard = heard();
    assert_eq!(heard.len(), 1);
    let report = heard[0].1;
    assert!(report.is_v6());
    assert_eq!((report.kind, report.code), (1, 4), "port unreachable");
    assert_eq!(report.offender, b_address);
    assert_eq!(report.data(), b"six");
}

#[test]
fn says_why_as_linux_does_and_which_errors_are_hard() {
    let sent = ipv4(ME, FAR, 17, 1, &udp_datagram(40000, 33434, b""));
    let reason = |kind: u8, code: u8| {
        let message = icmp(kind, code, [0; 4], &sent);
        reports::heard(&ipv4(ROUTER, ME, 1, 64, &message), Medium::Ip, true).unwrap().reason()
    };
    assert_eq!(reason(11, 0), (Reason::HostUnreachable, false));
    assert_eq!(reason(3, 0), (Reason::NetworkUnreachable, false));
    assert_eq!(reason(3, 1), (Reason::HostUnreachable, false));
    assert_eq!(reason(3, 2), (Reason::ProtocolUnreachable, true));
    assert_eq!(reason(3, 3), (Reason::Refused, true));
    assert_eq!(reason(3, 4), (Reason::MessageTooLong, true));
    assert_eq!(reason(3, 7), (Reason::HostDown, true));
    assert_eq!(reason(3, 8), (Reason::NoNetwork, true));
    assert_eq!(reason(3, 13), (Reason::HostUnreachable, true));
    assert_eq!(reason(3, 99), (Reason::HostUnreachable, false));
    assert_eq!(reason(12, 0), (Reason::ProtocolError, true));

    let me = Ipv6Address::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0x15);
    let router = Ipv6Address::new(0xfe80, 0, 0, 0, 0, 0, 0, 1);
    let udp = ipv6(me, me, 17, 1, &udp_datagram(40000, 33434, b""));
    let ping = ipv6(me, me, 58, 1, &echo(128, 1, 1, b""));
    let reason = |sent: &[u8], kind: u8, code: u8| {
        let mut message = icmpv6(router, me, kind, [0; 4], sent);
        message[1] = code;
        message[2..4].copy_from_slice(&[0, 0]);
        let mut pseudo = Vec::new();
        pseudo.extend_from_slice(&router.octets());
        pseudo.extend_from_slice(&me.octets());
        pseudo.extend_from_slice(&u32::try_from(message.len()).unwrap().to_be_bytes());
        pseudo.extend_from_slice(&[0, 0, 0, 58]);
        pseudo.extend_from_slice(&message);
        let sum = checksum(&pseudo);
        message[2..4].copy_from_slice(&sum.to_be_bytes());
        reports::heard(&ipv6(router, me, 58, 255, &message), Medium::Ip, true).unwrap().reason()
    };
    assert_eq!(reason(&udp, 1, 4), (Reason::Refused, true));
    assert_eq!(reason(&udp, 1, 1), (Reason::Prohibited, true));
    assert_eq!(reason(&udp, 1, 3), (Reason::HostUnreachable, false));
    assert_eq!(reason(&udp, 1, 9), (Reason::ProtocolError, true));
    assert_eq!(reason(&udp, 2, 0), (Reason::MessageTooLong, true));
    assert_eq!(reason(&ping, 2, 0), (Reason::MessageTooLong, false));
    assert_eq!(reason(&udp, 3, 0), (Reason::HostUnreachable, false));
    assert_eq!(reason(&udp, 4, 0), (Reason::ProtocolError, true));
}
