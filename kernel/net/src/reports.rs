// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The ICMP and ICMPv6 errors that report on what an interface's sockets
//! sent (ADR 0345): a router's "time exceeded" when a datagram's hop
//! limit ran out on its way, a machine's "port unreachable" when no
//! socket there had its port, and the rest of RFC 792's and RFC 4443's
//! errors, which the kernel gives the sockets as Linux does
//! (IP_RECVERR, MSG_ERRQUEUE), for traceroute(8) and mtr(8) among them.
//!
//! smoltcp hands an error only to an ICMP socket bound to a UDP port,
//! and only when what it quotes holds the whole datagram, though RFC
//! 792's routers quote an IP header and 8 bytes of what came after it.
//! [`heard`] reads an error in a frame as the interface receives it,
//! whatever it quotes, as long as that holds the header of the UDP
//! datagram or echo request it is about, and says what it reports: a
//! [`Report`]. Redirects and source quenches are not errors that reach a
//! UDP socket on Linux, and are left to the stack; so are the errors
//! about what a socket of another kind sent.

use smoltcp::wire::{Icmpv4Packet, Icmpv6Packet, IpAddress, Ipv4Address, Ipv6Address};

use crate::Medium;
use crate::filter::carried_v6;

/// The most bytes of what an error quotes that a [`Report`] keeps for
/// its socket: an echo request's header and 120 bytes of its data, or a
/// datagram's 128.
pub const QUOTE: usize = 128;

/// What the packet an error reports on was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum About {
    /// A UDP datagram.
    Udp,
    /// An ICMP or ICMPv6 echo request: a ping socket's.
    Ping,
}

/// An ICMP or ICMPv6 error about a packet this machine sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report {
    /// What the packet was.
    pub about: About,
    /// Who sent the error: a router on the packet's way, or the machine
    /// it went to.
    pub offender: IpAddress,
    /// Where the error came: an address of the interface's, or so it says.
    pub to: IpAddress,
    /// Its ICMP or ICMPv6 type.
    pub kind: u8,
    /// Its code.
    pub code: u8,
    /// What it says besides, as Linux's `ee_info`: for ICMP, the next
    /// hop's MTU of a "fragmentation needed" and the pointer of a
    /// "parameter problem", 0 otherwise; for ICMPv6, the word after its
    /// checksum, the MTU of a "packet too big" and the pointer of a
    /// "parameter problem".
    pub info: u32,
    /// Where the packet came from: an address of this machine's.
    pub local: IpAddress,
    /// The UDP port it came from, or the echo request's identifier.
    pub local_port: u16,
    /// Where it went.
    pub remote: IpAddress,
    /// The UDP port it went to: 0 for an echo request.
    pub remote_port: u16,
    quote: [u8; QUOTE],
    quoted: u8,
}

impl Report {
    /// Whether it is an ICMPv6 error, about an IPv6 packet.
    pub fn is_v6(&self) -> bool {
        matches!(self.offender, IpAddress::Ipv6(_))
    }

    /// What Linux gives a socket with it: what it quotes past the UDP
    /// header, or from the echo request's header on, [`QUOTE`] bytes at
    /// most.
    pub fn data(&self) -> &[u8] {
        &self.quote[..usize::from(self.quoted)]
    }

    /// The echo request's sequence number, for a ping socket's.
    pub fn sequence(&self) -> Option<u16> {
        match self.about {
            About::Ping => Some(u16::from_be_bytes([*self.quote.get(6)?, *self.quote.get(7)?])),
            About::Udp => None,
        }
    }
}

/// Why a socket's send or receive fails for an error: what Linux's errno
/// for it says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// ENETUNREACH.
    NetworkUnreachable,
    /// EHOSTUNREACH: a "host unreachable", or a "time exceeded".
    HostUnreachable,
    /// ENOPROTOOPT: a "protocol unreachable".
    ProtocolUnreachable,
    /// ECONNREFUSED: a "port unreachable".
    Refused,
    /// EMSGSIZE: a "fragmentation needed" or "packet too big".
    MessageTooLong,
    /// EOPNOTSUPP: a "source route failed".
    NotSupported,
    /// EHOSTDOWN: a "destination host unknown".
    HostDown,
    /// ENONET: a "source host isolated".
    NoNetwork,
    /// EACCES: ICMPv6's "administratively prohibited" and its kin.
    Prohibited,
    /// EPROTO: a "parameter problem".
    ProtocolError,
}

/// What each of ICMP's "destination unreachable" codes says, and whether
/// it is a hard error, as Linux's `icmp_err_convert`.
const UNREACHABLE_REASONS: [(Reason, bool); 16] = [
    (Reason::NetworkUnreachable, false),
    (Reason::HostUnreachable, false),
    (Reason::ProtocolUnreachable, true),
    (Reason::Refused, true),
    (Reason::MessageTooLong, false),
    (Reason::NotSupported, false),
    (Reason::NetworkUnreachable, true),
    (Reason::HostDown, true),
    (Reason::NoNetwork, true),
    (Reason::NetworkUnreachable, true),
    (Reason::HostUnreachable, true),
    (Reason::NetworkUnreachable, false),
    (Reason::HostUnreachable, false),
    (Reason::HostUnreachable, true),
    (Reason::HostUnreachable, true),
    (Reason::HostUnreachable, true),
];

/// ICMPv6's, as Linux's `icmpv6_err_convert`.
const UNREACHABLE_REASONS_V6: [(Reason, bool); 7] = [
    (Reason::NetworkUnreachable, false),
    (Reason::Prohibited, true),
    (Reason::HostUnreachable, false),
    (Reason::HostUnreachable, false),
    (Reason::Refused, true),
    (Reason::Prohibited, true),
    (Reason::Prohibited, true),
];

impl Report {
    /// Why the socket's send or receive fails for it, and whether it is a
    /// hard error, one that says the other end cannot be reached at all,
    /// which a connected socket hears even when it did not ask for errors:
    /// as Linux's UDP and ping sockets say. A datagram too big for the
    /// way is a hard error, as Linux's sockets look for the path's MTU;
    /// for a ping socket, an ICMPv6 one is not, as on Linux.
    pub fn reason(&self) -> (Reason, bool) {
        let code = usize::from(self.code);
        if self.is_v6() {
            match self.kind {
                1 => UNREACHABLE_REASONS_V6
                    .get(code)
                    .copied()
                    .unwrap_or((Reason::ProtocolError, true)),
                2 => (Reason::MessageTooLong, self.about == About::Udp),
                3 => (Reason::HostUnreachable, false),
                _ => (Reason::ProtocolError, true),
            }
        } else {
            match self.kind {
                UNREACHABLE if self.code == FRAGMENTATION_NEEDED => (Reason::MessageTooLong, true),
                UNREACHABLE => UNREACHABLE_REASONS
                    .get(code)
                    .copied()
                    .unwrap_or((Reason::HostUnreachable, false)),
                TIME_EXCEEDED => (Reason::HostUnreachable, false),
                _ => (Reason::ProtocolError, true),
            }
        }
    }
}

/// IP's numbers for what a packet carries.
const ICMP: u8 = 1;
const UDP: u8 = 17;
const ICMPV6: u8 = 58;

/// ICMP's errors that reach a UDP or ping socket, and the code of a
/// "destination unreachable" that says the packet needs fragmenting.
const UNREACHABLE: u8 = 3;
const FRAGMENTATION_NEEDED: u8 = 4;
const TIME_EXCEEDED: u8 = 11;
const PARAMETER_PROBLEM: u8 = 12;

/// ICMPv6's: destination unreachable, packet too big, time exceeded and
/// parameter problem.
const ERRORS_V6: core::ops::RangeInclusive<u8> = 1..=4;

/// Echo requests' types.
const ECHO_REQUEST: u8 = 8;
const ECHO_REQUEST_V6: u8 = 128;

/// The bytes of a UDP header, and of an ICMP or ICMPv6 header with its
/// word of the rest.
const UDP_HEADER: usize = 8;
const ICMP_HEADER: usize = 8;

const ETHERNET: usize = 14;
const IPV6_HEADER: usize = 40;

/// Whether `frame`, which a link of `medium` received, may hold an ICMP
/// or ICMPv6 error: what a frame must be for [`heard`] to read it, looked
/// at without reading further.
#[inline]
pub fn may_be(frame: &[u8], medium: Medium) -> bool {
    let packet = match medium {
        Medium::Ethernet => match frame.get(12..14) {
            Some([0x08, 0x00] | [0x86, 0xdd]) => frame.get(ETHERNET..).unwrap_or_default(),
            _ => return false,
        },
        Medium::Ip => frame,
    };
    match packet.first().map(|first| first >> 4) {
        Some(4) => packet.get(9) == Some(&ICMP),
        // ICMPv6, or an extension header before what it carries.
        Some(6) => packet.get(6).is_some_and(|&next| matches!(next, ICMPV6 | 0 | 43 | 44 | 60)),
        _ => false,
    }
}

/// The error in `frame`, which a link of `medium` received, if it holds
/// one about a UDP datagram or an echo request; with `verify`, only if
/// its checksum is right.
pub fn heard(frame: &[u8], medium: Medium, verify: bool) -> Option<Report> {
    let packet = match medium {
        Medium::Ethernet => match frame.get(12..14)? {
            [0x08, 0x00] | [0x86, 0xdd] => frame.get(ETHERNET..)?,
            _ => return None,
        },
        Medium::Ip => frame,
    };
    match packet.first()? >> 4 {
        4 => heard_v4(packet, verify),
        6 => heard_v6(packet, verify),
        _ => None,
    }
}

fn be16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]))
}

fn v4_at(bytes: &[u8], at: usize) -> Option<IpAddress> {
    let octets: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
    Some(IpAddress::Ipv4(Ipv4Address::from(octets)))
}

fn v6_at(bytes: &[u8], at: usize) -> Option<Ipv6Address> {
    let octets: [u8; 16] = bytes.get(at..at + 16)?.try_into().ok()?;
    Some(Ipv6Address::from(octets))
}

/// What a packet an error quotes was, from the header after its IP
/// header on: its ports or identifier, and what Linux gives a socket
/// with it.
fn quoted(carries: u8, carried: &[u8], v6: bool) -> Option<(About, u16, u16, &[u8])> {
    let echo = if v6 { (ICMPV6, ECHO_REQUEST_V6) } else { (ICMP, ECHO_REQUEST) };
    if carries == UDP {
        let data = carried.get(UDP_HEADER..)?;
        Some((About::Udp, be16(carried, 0)?, be16(carried, 2)?, data))
    } else if carries == echo.0 && *carried.first()? == echo.1 && carried.len() >= ICMP_HEADER {
        Some((About::Ping, be16(carried, 4)?, 0, carried))
    } else {
        None
    }
}

/// An ICMP error in `packet`, an IPv4 packet.
fn heard_v4(packet: &[u8], verify: bool) -> Option<Report> {
    if *packet.get(9)? != ICMP {
        return None;
    }
    let header = usize::from(packet[0] & 0x0f) * 4;
    let total = usize::from(be16(packet, 2)?);
    // Whole, and not in fragments, which the stack does not put back
    // together: an error is never longer than 576 bytes.
    let fragment = be16(packet, 6)? & 0x3fff;
    if header < 20 || total < header + ICMP_HEADER || total > packet.len() || fragment != 0 {
        return None;
    }
    // An Ethernet frame may be padded past the packet's end.
    let packet = &packet[..total];
    let icmp = &packet[header..];
    let (kind, code) = (icmp[0], icmp[1]);
    let info = match kind {
        UNREACHABLE if code == FRAGMENTATION_NEEDED => u32::from(be16(icmp, 6)?),
        UNREACHABLE | TIME_EXCEEDED => 0,
        PARAMETER_PROBLEM => u32::from(icmp[4]),
        _ => return None,
    };
    if verify && !Icmpv4Packet::new_unchecked(icmp).verify_checksum() {
        return None;
    }
    // What it quotes: the IP header of what was sent, and at least 8
    // bytes of what came after it, the header of what it carried.
    let inner = &icmp[ICMP_HEADER..];
    if inner.first()? >> 4 != 4 {
        return None;
    }
    let inner_header = usize::from(inner[0] & 0x0f) * 4;
    if inner_header < 20 || inner.len() < inner_header + ICMP_HEADER {
        return None;
    }
    // A fragment past the first has no ports to read.
    if be16(inner, 6)? & 0x1fff != 0 {
        return None;
    }
    let carried = &inner[inner_header..];
    let (about, local_port, remote_port, data) = quoted(inner[9], carried, false)?;
    Some(report(
        about,
        (v4_at(packet, 12)?, v4_at(packet, 16)?),
        (kind, code, info),
        (v4_at(inner, 12)?, local_port),
        (v4_at(inner, 16)?, remote_port),
        data,
    ))
}

/// An ICMPv6 error in `packet`, an IPv6 packet.
fn heard_v6(packet: &[u8], verify: bool) -> Option<Report> {
    if packet.len() < IPV6_HEADER {
        return None;
    }
    let total = IPV6_HEADER + usize::from(be16(packet, 4)?);
    if total > packet.len() {
        return None;
    }
    let packet = &packet[..total];
    // An error fits IPv6's least MTU, and comes whole: one in fragments
    // fails its checksum.
    let (carries, at) = carried_v6(packet)?;
    if carries != ICMPV6 {
        return None;
    }
    let icmp = packet.get(at..)?;
    if icmp.len() < ICMP_HEADER || !ERRORS_V6.contains(&icmp[0]) {
        return None;
    }
    let (kind, code) = (icmp[0], icmp[1]);
    let info = u32::from_be_bytes([icmp[4], icmp[5], icmp[6], icmp[7]]);
    let (offender, to) = (v6_at(packet, 8)?, v6_at(packet, 24)?);
    if verify && !Icmpv6Packet::new_unchecked(icmp).verify_checksum(&offender, &to) {
        return None;
    }
    let inner = &icmp[ICMP_HEADER..];
    if inner.len() < IPV6_HEADER || inner[0] >> 4 != 6 {
        return None;
    }
    let (carries, at) = carried_v6(inner)?;
    let carried = inner.get(at..)?;
    let (about, local_port, remote_port, data) = quoted(carries, carried, true)?;
    Some(report(
        about,
        (IpAddress::Ipv6(offender), IpAddress::Ipv6(to)),
        (kind, code, info),
        (IpAddress::Ipv6(v6_at(inner, 8)?), local_port),
        (IpAddress::Ipv6(v6_at(inner, 24)?), remote_port),
        data,
    ))
}

/// A report of these, keeping [`QUOTE`] bytes of `data`.
fn report(
    about: About,
    (offender, to): (IpAddress, IpAddress),
    (kind, code, info): (u8, u8, u32),
    (local, local_port): (IpAddress, u16),
    (remote, remote_port): (IpAddress, u16),
    data: &[u8],
) -> Report {
    let mut quote = [0; QUOTE];
    let kept = data.len().min(QUOTE);
    quote[..kept].copy_from_slice(&data[..kept]);
    Report {
        about,
        offender,
        to,
        kind,
        code,
        info,
        local,
        local_port,
        remote,
        remote_port,
        quote,
        // At most QUOTE, 128.
        quoted: u8::try_from(kept).unwrap_or(u8::MAX),
    }
}
