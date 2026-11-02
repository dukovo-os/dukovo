// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the firewall looks at in the frames an interface receives from
//! other machines (ADR 0334).
//!
//! The firewall decides about the packets that start something: a TCP
//! segment that opens a connection (SYN without ACK), a UDP datagram that
//! answers nothing this interface sent, and an ICMP or ICMPv6 echo
//! request. [`inbound`] finds them in a frame and says what they are, an
//! [`Inbound`]; the rest goes on to the stack as it is: the segments of
//! connections, which the stack drops unless one of its own is theirs,
//! ARP, neighbour discovery and the ICMP errors that report on what was
//! sent, and fragments, which the stack does not put back together.
//!
//! [`Flows`] is how an interface knows a datagram's answer: each UDP
//! datagram it sends notes its two ends for [`FLOW_NS`], and one that
//! comes back from the far end to the near one is let in, as Linux's
//! connection tracking lets in replies. A datagram sent to a group of
//! machines (broadcast or multicast), as a DHCP or mDNS query is, takes
//! answers from any machine, to the port it was sent from and from the
//! port it went to.

use smoltcp::wire::{IpAddress, Ipv4Address, Ipv6Address};

/// What a packet the firewall decides about is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    /// A TCP segment that opens a connection.
    Tcp,
    /// A UDP datagram that answers nothing sent.
    Udp,
    /// An ICMP or ICMPv6 echo request.
    Ping,
}

/// A packet from another machine that the firewall decides about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inbound {
    /// What it is.
    pub protocol: Protocol,
    /// Where it came from.
    pub from: IpAddress,
    /// The port it came from: 0 for a ping.
    pub from_port: u16,
    /// Where it goes: one of the interface's addresses, or a group's.
    pub to: IpAddress,
    /// The port it goes to: 0 for a ping.
    pub port: u16,
}

/// The parts of a packet the firewall reads: its version's addresses,
/// what it carries, and where that starts.
struct Ip {
    from: IpAddress,
    to: IpAddress,
    carries: u8,
    at: usize,
}

const ETHERNET: usize = 14;
const IPV4: [u8; 2] = [0x08, 0x00];
const IPV6: [u8; 2] = [0x86, 0xdd];

/// IP's numbers for what a packet carries.
const ICMP: u8 = 1;
const TCP: u8 = 6;
const UDP: u8 = 17;
const ICMPV6: u8 = 58;
/// IPv6's extension headers that come before what a packet carries:
/// hop-by-hop options, routing, the fragment header and destination
/// options.
const HOP_BY_HOP: u8 = 0;
const ROUTING: u8 = 43;
const FRAGMENT: u8 = 44;
const DESTINATION: u8 = 60;

/// The echo requests of ICMP and ICMPv6.
const ECHO_REQUEST: u8 = 8;
const ECHO_REQUEST_V6: u8 = 128;

/// TCP's SYN and ACK flags.
const SYN: u8 = 0x02;
const ACK: u8 = 0x10;

/// The IP packet in an Ethernet frame, if it carries one that is whole
/// and is not a fragment past the first.
fn ip(frame: &[u8]) -> Option<Ip> {
    let packet = frame.get(ETHERNET..)?;
    match frame.get(12..14)? {
        t if t == IPV4 => {
            let header = usize::from(packet.first()? & 0x0f) * 4;
            if packet.first()? >> 4 != 4 || header < 20 || packet.len() < header {
                return None;
            }
            // A fragment past the first has no ports to read.
            let offset = u16::from_be_bytes([packet[6], packet[7]]) & 0x1fff;
            if offset != 0 {
                return None;
            }
            let address = |at: usize| -> Option<IpAddress> {
                let octets: [u8; 4] = packet.get(at..at + 4)?.try_into().ok()?;
                Some(IpAddress::Ipv4(Ipv4Address::from(octets)))
            };
            Some(Ip { from: address(12)?, to: address(16)?, carries: packet[9], at: header })
        }
        t if t == IPV6 => {
            if packet.first()? >> 4 != 6 || packet.len() < 40 {
                return None;
            }
            let address = |at: usize| -> Option<IpAddress> {
                let octets: [u8; 16] = packet.get(at..at + 16)?.try_into().ok()?;
                Some(IpAddress::Ipv6(Ipv6Address::from(octets)))
            };
            let (from, to) = (address(8)?, address(24)?);
            let (mut carries, mut at) = (packet[6], 40);
            // Past a few extension headers to what it carries.
            for _ in 0..4 {
                match carries {
                    HOP_BY_HOP | ROUTING | DESTINATION => {
                        let next = *packet.get(at)?;
                        at += (usize::from(*packet.get(at + 1)?) + 1) * 8;
                        carries = next;
                    }
                    FRAGMENT => {
                        let next = *packet.get(at)?;
                        let offset =
                            u16::from_be_bytes([*packet.get(at + 2)?, *packet.get(at + 3)?]) >> 3;
                        if offset != 0 {
                            return None;
                        }
                        at += 8;
                        carries = next;
                    }
                    _ => break,
                }
            }
            Some(Ip { from, to, carries, at })
        }
        _ => None,
    }
}

/// The packet in `frame` that the firewall decides about, if it is one.
pub fn inbound(frame: &[u8]) -> Option<Inbound> {
    let ip = ip(frame)?;
    let body = frame.get(ETHERNET + ip.at..)?;
    let ports = || -> Option<(u16, u16)> {
        Some((
            u16::from_be_bytes([*body.first()?, *body.get(1)?]),
            u16::from_be_bytes([*body.get(2)?, *body.get(3)?]),
        ))
    };
    let (protocol, (from_port, port)) = match ip.carries {
        TCP => {
            let flags = *body.get(13)?;
            if flags & (SYN | ACK) != SYN {
                return None;
            }
            (Protocol::Tcp, ports()?)
        }
        UDP => (Protocol::Udp, ports()?),
        ICMP if *body.first()? == ECHO_REQUEST => (Protocol::Ping, (0, 0)),
        ICMPV6 if *body.first()? == ECHO_REQUEST_V6 => (Protocol::Ping, (0, 0)),
        _ => return None,
    };
    Some(Inbound { protocol, from: ip.from, from_port, to: ip.to, port })
}

/// A UDP datagram sent: its two ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Flow {
    port: u16,
    /// The far end, or `None` for a group, whose answers may come from
    /// any machine.
    to: Option<IpAddress>,
    to_port: u16,
    until_ns: u64,
}

/// How long a datagram's answers are let in after it was sent, as long
/// as Linux's connection tracking waits for a UDP answer: 30 seconds.
pub const FLOW_NS: u64 = 30_000_000_000;

/// The most flows an interface notes; the one that ends first makes room.
pub const FLOWS: usize = 64;

/// Whether `address` is a group's: broadcast or multicast, or for IPv4,
/// with no network at hand to tell its broadcast address by, one that
/// ends in 255, as a /24's does.
fn is_group(address: IpAddress) -> bool {
    match address {
        IpAddress::Ipv4(v4) => v4.is_broadcast() || v4.is_multicast() || v4.octets()[3] == 255,
        IpAddress::Ipv6(v6) => v6.is_multicast(),
    }
}

/// The UDP datagrams an interface sent lately.
pub struct Flows {
    flows: [Option<Flow>; FLOWS],
}

impl Default for Flows {
    fn default() -> Flows {
        Flows { flows: [None; FLOWS] }
    }
}

impl Flows {
    /// Notes the datagram in `frame`, which the interface sends at
    /// `now_ns`, if it is one.
    pub fn sent(&mut self, frame: &[u8], now_ns: u64) {
        let Some(ip) = ip(frame) else { return };
        if ip.carries != UDP {
            return;
        }
        let Some(body) = frame.get(ETHERNET + ip.at..ETHERNET + ip.at + 4) else { return };
        let port = u16::from_be_bytes([body[0], body[1]]);
        let to_port = u16::from_be_bytes([body[2], body[3]]);
        let to = (!is_group(ip.to)).then_some(ip.to);
        let flow = Flow { port, to, to_port, until_ns: now_ns.saturating_add(FLOW_NS) };
        let same = |kept: &Flow| kept.port == port && kept.to == to && kept.to_port == to_port;
        let slot = self
            .flows
            .iter()
            .position(|kept| kept.as_ref().is_some_and(same))
            .or_else(|| self.flows.iter().position(Option::is_none))
            .unwrap_or_else(|| {
                let ends = |at: &usize| self.flows[*at].map_or(0, |flow| flow.until_ns);
                (0..FLOWS).min_by_key(ends).unwrap_or(0)
            });
        self.flows[slot] = Some(flow);
    }

    /// Whether `packet`, at `now_ns`, answers a datagram sent.
    pub fn answers(&self, packet: &Inbound, now_ns: u64) -> bool {
        packet.protocol == Protocol::Udp
            && self.flows.iter().flatten().any(|flow| {
                flow.until_ns > now_ns
                    && flow.port == packet.port
                    && match flow.to {
                        Some(to) => to == packet.from && flow.to_port == packet.from_port,
                        // A group's answer comes from the port the query
                        // went to (a DHCP or DNS server's), or, as mDNS's
                        // do, from the one it came from.
                        None => packet.from_port == flow.to_port || packet.from_port == flow.port,
                    }
            })
    }
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::*;

    /// An Ethernet frame of IPv4 from `from` to `to` carrying `carries`,
    /// whose body is `body`.
    fn v4(from: [u8; 4], to: [u8; 4], carries: u8, body: &[u8]) -> Vec<u8> {
        let mut frame = std::vec![0u8; 12];
        frame.extend_from_slice(&IPV4);
        let mut header = std::vec![0x45, 0, 0, 0, 0, 0, 0x40, 0, 64, carries, 0, 0];
        header.extend_from_slice(&from);
        header.extend_from_slice(&to);
        frame.extend_from_slice(&header);
        frame.extend_from_slice(body);
        frame
    }

    /// One of IPv6, with `extensions` before the body.
    fn v6(from: &str, to: &str, carries: u8, extensions: &[u8], body: &[u8]) -> Vec<u8> {
        let mut frame = std::vec![0u8; 12];
        frame.extend_from_slice(&IPV6);
        frame.extend_from_slice(&[0x60, 0, 0, 0, 0, 0, carries, 64]);
        frame.extend_from_slice(&from.parse::<Ipv6Address>().unwrap().octets());
        frame.extend_from_slice(&to.parse::<Ipv6Address>().unwrap().octets());
        frame.extend_from_slice(extensions);
        frame.extend_from_slice(body);
        frame
    }

    /// A TCP header from `from` to `to` with `flags`.
    fn tcp(from: u16, to: u16, flags: u8) -> Vec<u8> {
        let mut body = std::vec![0u8; 20];
        body[..2].copy_from_slice(&from.to_be_bytes());
        body[2..4].copy_from_slice(&to.to_be_bytes());
        body[12] = 0x50;
        body[13] = flags;
        body
    }

    fn udp(from: u16, to: u16) -> Vec<u8> {
        let mut body = std::vec![0u8; 8];
        body[..2].copy_from_slice(&from.to_be_bytes());
        body[2..4].copy_from_slice(&to.to_be_bytes());
        body
    }

    #[test]
    fn what_starts_something_is_found_and_the_rest_passes() {
        let syn = v4([10, 0, 2, 2], [10, 0, 2, 15], TCP, &tcp(41000, 22, SYN));
        let found = inbound(&syn).unwrap();
        assert_eq!(found.protocol, Protocol::Tcp);
        assert_eq!((found.from_port, found.port), (41000, 22));
        assert_eq!(found.from, IpAddress::Ipv4(Ipv4Address::new(10, 0, 2, 2)));
        // A connection's segments, and its SYN-ACK, are not.
        assert_eq!(inbound(&v4([10, 0, 2, 2], [10, 0, 2, 15], TCP, &tcp(22, 41000, ACK))), None);
        assert_eq!(
            inbound(&v4([10, 0, 2, 2], [10, 0, 2, 15], TCP, &tcp(22, 41000, SYN | ACK))),
            None
        );
        let ping = v4([10, 0, 2, 2], [10, 0, 2, 15], ICMP, &[ECHO_REQUEST, 0, 0, 0]);
        assert_eq!(inbound(&ping).unwrap().protocol, Protocol::Ping);
        assert_eq!(inbound(&v4([10, 0, 2, 2], [10, 0, 2, 15], ICMP, &[0, 0, 0, 0])), None);
        // A fragment past the first; ARP.
        let mut fragment = v4([10, 0, 2, 2], [10, 0, 2, 15], UDP, &udp(5, 6));
        fragment[ETHERNET + 6] = 0x20;
        fragment[ETHERNET + 7] = 0x10;
        assert_eq!(inbound(&fragment), None);
        let mut arp = std::vec![0u8; 42];
        arp[12..14].copy_from_slice(&[0x08, 0x06]);
        assert_eq!(inbound(&arp), None);
        // IPv6, through a hop-by-hop header, and its echo request;
        // neighbour discovery passes.
        let hop = [TCP, 0, 1, 4, 0, 0, 0, 0];
        let syn6 = v6("fec0::2", "fec0::15", HOP_BY_HOP, &hop, &tcp(41000, 8080, SYN));
        assert_eq!(inbound(&syn6).unwrap().port, 8080);
        let ping6 = v6("fe80::1", "fe80::2", ICMPV6, &[], &[ECHO_REQUEST_V6, 0, 0, 0]);
        assert_eq!(inbound(&ping6).unwrap().protocol, Protocol::Ping);
        assert_eq!(inbound(&v6("fe80::1", "ff02::1", ICMPV6, &[], &[134, 0, 0, 0])), None);
        // Cut short.
        assert_eq!(inbound(&syn[..syn.len() - 10]), None);
    }

    #[test]
    fn answers_to_datagrams_sent_are_let_in_for_a_while() {
        let mut flows = Flows::default();
        // A DNS query, and a DHCP discovery to every machine.
        flows.sent(&v4([10, 0, 2, 15], [10, 0, 2, 3], UDP, &udp(50000, 53)), 0);
        flows.sent(&v4([0, 0, 0, 0], [255, 255, 255, 255], UDP, &udp(68, 67)), 0);
        let answer = |from: [u8; 4], from_port: u16, port: u16| {
            inbound(&v4(from, [10, 0, 2, 15], UDP, &udp(from_port, port))).unwrap()
        };
        assert!(flows.answers(&answer([10, 0, 2, 3], 53, 50000), 1));
        assert!(!flows.answers(&answer([10, 0, 2, 4], 53, 50000), 1));
        assert!(!flows.answers(&answer([10, 0, 2, 3], 54, 50000), 1));
        assert!(flows.answers(&answer([10, 0, 2, 2], 67, 68), 1));
        assert!(!flows.answers(&answer([10, 0, 2, 2], 99, 68), 1));
        assert!(!flows.answers(&answer([10, 0, 2, 3], 53, 50000), FLOW_NS));
        // A TCP SYN is never an answer.
        let syn = inbound(&v4([10, 0, 2, 3], [10, 0, 2, 15], TCP, &tcp(53, 50000, SYN))).unwrap();
        assert!(!flows.answers(&syn, 1));
        // DHCPv6's question goes to a group; its answer comes from a server.
        flows.sent(&v6("fe80::5", "ff02::1:2", UDP, &[], &udp(546, 547)), 0);
        let reply = inbound(&v6("fec0::2", "fe80::5", UDP, &[], &udp(547, 546))).unwrap();
        assert!(flows.answers(&reply, 1));
        // More than it holds: the one ending first goes.
        for n in 0..FLOWS as u16 {
            flows.sent(&v4([10, 0, 2, 15], [10, 0, 2, 9], UDP, &udp(1000 + n, 7)), 5);
        }
        assert!(!flows.answers(&answer([10, 0, 2, 3], 53, 50000), 1));
    }
}
