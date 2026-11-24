// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What linuxd answers a request with: the RTM_NEWLINK, RTM_NEWADDR and
//! RTM_NEWROUTE messages of a dump, the NLMSG_ERROR of a refusal or an
//! acknowledgement, and the NLMSG_DONE that ends a list.
//!
//! The layouts are Linux's (`rtnetlink(7)`, `linux/rtnetlink.h`), so
//! getifaddrs(3), Go's `syscall.NetlinkRIB` and iproute2 read them as
//! they read the kernel's. A dump is encoded whole when it starts and
//! given out in datagrams as the program receives them, as Linux's dump
//! continuation does; [`Dump`] holds what is left. Each answer carries
//! the request's sequence number and the socket's own number as its
//! sender.

use std::collections::VecDeque;
use std::net::IpAddr;

use super::machine::{Address, Link, Machine, Route};
use super::wire::{
    self, Action, Builder, Lookup, NLM_F_ACK, NLM_F_MULTI, RTM_NEWADDR, RTM_NEWLINK, RTM_NEWROUTE,
    Table,
};

/// AF_UNSPEC, AF_INET and AF_INET6.
const AF_UNSPEC: u8 = 0;
const AF_INET: u8 = 2;
const AF_INET6: u8 = 10;

/// The attributes of an interface: IFLA_ADDRESS, IFLA_BROADCAST,
/// IFLA_IFNAME, IFLA_MTU, IFLA_STATS, IFLA_TXQLEN, IFLA_OPERSTATE and
/// IFLA_STATS64.
const IFLA_ADDRESS: u16 = 1;
const IFLA_BROADCAST: u16 = 2;
const IFLA_IFNAME: u16 = 3;
const IFLA_MTU: u16 = 4;
const IFLA_STATS: u16 = 7;
const IFLA_TXQLEN: u16 = 13;
const IFLA_OPERSTATE: u16 = 16;
const IFLA_STATS64: u16 = 23;

/// The attributes of an address: IFA_ADDRESS, IFA_LOCAL, IFA_LABEL,
/// IFA_BROADCAST, IFA_CACHEINFO and IFA_FLAGS.
const IFA_ADDRESS: u16 = 1;
const IFA_LOCAL: u16 = 2;
const IFA_LABEL: u16 = 3;
const IFA_BROADCAST: u16 = 4;
const IFA_CACHEINFO: u16 = 6;
const IFA_FLAGS: u16 = 8;

/// The attributes of a route: RTA_DST, RTA_OIF, RTA_GATEWAY,
/// RTA_PRIORITY, RTA_PREFSRC and RTA_TABLE.
const RTA_DST: u16 = 1;
const RTA_OIF: u16 = 4;
const RTA_GATEWAY: u16 = 5;
const RTA_PRIORITY: u16 = 6;
const RTA_PREFSRC: u16 = 7;
const RTA_TABLE: u16 = 15;

/// RT_TABLE_MAIN, the one table listed, and RTN_UNICAST, the type of
/// every route in it.
const RT_TABLE_MAIN: u8 = 254;
const RTN_UNICAST: u8 = 1;

/// The length of an interface's send queue, which Linux gives `lo` and a
/// card alike.
const TX_QUEUE_LEN: u32 = 1000;

/// The RTM_NEWLINK for `link`: `flags` is NLM_F_MULTI in a list, 0 for
/// the one interface a request asked about.
pub(super) fn link_message(link: &Link, flags: u16, seq: u32, pid: u32) -> Vec<u8> {
    let mut message = Builder::new(RTM_NEWLINK, flags, seq, pid);
    // struct ifinfomsg: family, padding, type, index, flags, change. A
    // list reports no change.
    message.put(&[AF_UNSPEC, 0]);
    message.put(&link.kind.to_ne_bytes());
    message.put(&link.index.to_ne_bytes());
    message.put(&link.flags.to_ne_bytes());
    message.put(&0u32.to_ne_bytes());
    message.attribute(IFLA_IFNAME, &c_string(&link.name));
    message.number(IFLA_TXQLEN, TX_QUEUE_LEN);
    message.attribute(IFLA_OPERSTATE, &[link.operstate]);
    message.number(IFLA_MTU, link.mtu);
    if !link.address.is_empty() {
        message.attribute(IFLA_ADDRESS, &link.address);
        message.attribute(IFLA_BROADCAST, &link.broadcast);
    }
    let counters = link.stats.counters();
    let wide: Vec<u8> = counters.iter().flat_map(|count| count.to_ne_bytes()).collect();
    message.attribute(IFLA_STATS64, &wide);
    // struct rtnl_link_stats has 32-bit counters, which wrap as Linux's
    // do.
    let narrow: Vec<u8> = counters.iter().flat_map(|&count| (count as u32).to_ne_bytes()).collect();
    message.attribute(IFLA_STATS, &narrow);
    message.finish()
}

/// `text` with the NUL a string attribute ends in.
fn c_string(text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() + 1);
    bytes.extend_from_slice(text.as_bytes());
    bytes.push(0);
    bytes
}

/// The bytes of `address` in network order.
fn octets(address: IpAddr) -> Vec<u8> {
    match address {
        IpAddr::V4(address) => address.octets().to_vec(),
        IpAddr::V6(address) => address.octets().to_vec(),
    }
}

/// The address family of `address`.
fn family_of(address: IpAddr) -> u8 {
    if address.is_ipv4() { AF_INET } else { AF_INET6 }
}

/// The RTM_NEWADDR for `address`, in a list.
pub(super) fn address_message(address: &Address, seq: u32, pid: u32) -> Vec<u8> {
    let mut message = Builder::new(RTM_NEWADDR, NLM_F_MULTI, seq, pid);
    // struct ifaddrmsg: family, prefix length, flags, scope, interface.
    // Its `ifa_flags` is the low byte; IFA_FLAGS has them all.
    let low_flags = (address.flags & 0xff) as u8;
    message.put(&[family_of(address.address), address.prefix_len, low_flags, address.scope]);
    message.put(&address.index.to_ne_bytes());
    message.attribute(IFA_ADDRESS, &octets(address.address));
    // An IPv4 address that is not point to point is its own local one.
    if address.address.is_ipv4() {
        message.attribute(IFA_LOCAL, &octets(address.address));
    }
    if let Some(broadcast) = address.broadcast {
        message.attribute(IFA_BROADCAST, &broadcast.octets());
    }
    if let Some(label) = &address.label {
        message.attribute(IFA_LABEL, &c_string(label));
    }
    message.number(IFA_FLAGS, address.flags);
    // struct ifa_cacheinfo: preferred and valid lifetimes, then when the
    // address was made and last changed, which Radix does not keep.
    let mut lifetimes = Vec::with_capacity(16);
    for word in [address.preferred, address.valid, 0, 0] {
        lifetimes.extend_from_slice(&word.to_ne_bytes());
    }
    message.attribute(IFA_CACHEINFO, &lifetimes);
    message.finish()
}

/// The RTM_NEWROUTE for `route`, in a list.
pub(super) fn route_message(route: &Route, seq: u32, pid: u32) -> Vec<u8> {
    let mut message = Builder::new(RTM_NEWROUTE, NLM_F_MULTI, seq, pid);
    // struct rtmsg: family, destination and source prefix lengths, TOS,
    // table, protocol, scope, type, flags.
    let table = RT_TABLE_MAIN;
    message.put(&[family_of(route.destination), route.prefix_len, 0, 0]);
    message.put(&[table, route.protocol, route.scope, RTN_UNICAST]);
    message.put(&0u32.to_ne_bytes());
    message.number(RTA_TABLE, u32::from(table));
    if route.prefix_len > 0 {
        message.attribute(RTA_DST, &octets(route.destination));
    }
    if let Some(metric) = route.metric {
        message.number(RTA_PRIORITY, metric);
    }
    if let Some(source) = route.source {
        message.attribute(RTA_PREFSRC, &octets(source));
    }
    if let Some(gateway) = route.gateway {
        message.attribute(RTA_GATEWAY, &octets(gateway));
    }
    message.number(RTA_OIF, route.index);
    message.finish()
}

/// The rest of a list that is being given out: whole messages in the
/// order they are listed, and the NLMSG_DONE that follows the last.
pub(super) struct Dump {
    pending: VecDeque<Vec<u8>>,
    done: Option<Vec<u8>>,
}

impl Dump {
    /// A list of `messages` that ends with the NLMSG_DONE for `seq`.
    fn new(messages: VecDeque<Vec<u8>>, seq: u32, pid: u32) -> Dump {
        Dump { pending: messages, done: Some(wire::done(seq, pid)) }
    }

    /// The next datagram: as many whole messages as fit in `limit`
    /// bytes, and always at least one. The NLMSG_DONE is a datagram of
    /// its own, after the last message, as Linux sends it. `None` once
    /// that has been given.
    pub fn chunk(&mut self, limit: usize) -> Option<Vec<u8>> {
        let mut chunk = Vec::new();
        while let Some(next) = self.pending.front() {
            if !chunk.is_empty() && chunk.len() + next.len() > limit {
                break;
            }
            chunk.extend_from_slice(next);
            self.pending.pop_front();
        }
        if chunk.is_empty() { self.done.take() } else { Some(chunk) }
    }

    /// Whether the NLMSG_DONE has been given.
    pub fn is_finished(&self) -> bool {
        self.pending.is_empty() && self.done.is_none()
    }
}

/// The address families a request for `family` lists: all of them for
/// AF_UNSPEC, none that Radix has for any other family.
fn families(family: u8) -> &'static [u8] {
    match family {
        AF_UNSPEC => &[AF_INET, AF_INET6],
        AF_INET => &[AF_INET],
        AF_INET6 => &[AF_INET6],
        _ => &[],
    }
}

/// The list `table` and `family` ask for: interfaces by number, then
/// addresses or routes of each family in turn, as Linux walks its
/// families.
fn dump(table: Table, family: u8, machine: &dyn Machine, seq: u32, pid: u32) -> Dump {
    let messages = match table {
        Table::Links => {
            machine.links().iter().map(|link| link_message(link, NLM_F_MULTI, seq, pid)).collect()
        }
        Table::Addresses => {
            let addresses = machine.addresses();
            families(family)
                .iter()
                .flat_map(|&each| addresses.iter().filter(move |a| family_of(a.address) == each))
                .map(|address| address_message(address, seq, pid))
                .collect()
        }
        Table::Routes => {
            let routes = machine.routes();
            families(family)
                .iter()
                .flat_map(|&each| routes.iter().filter(move |r| family_of(r.destination) == each))
                .map(|route| route_message(route, seq, pid))
                .collect()
        }
    };
    Dump::new(messages, seq, pid)
}

/// The interface a request names.
fn find(links: Vec<Link>, lookup: Lookup<'_>) -> Option<Link> {
    links.into_iter().find(|link| match lookup {
        Lookup::Index(index) => link.index == index,
        Lookup::Name(name) => link.name.as_bytes() == name,
    })
}

/// Who a datagram's requests are answered to, and what is known of their
/// socket.
pub(super) struct Sender {
    /// The socket's own number, which its answers come addressed to.
    pub pid: u32,
    /// Whether the caller may change the network.
    pub root: bool,
    /// Whether the socket asked for NETLINK_CAP_ACK.
    pub cap_ack: bool,
    /// Whether a list is already being given out on the socket.
    pub dump_running: bool,
}

/// One answer to a request.
pub(super) enum Reply {
    /// A datagram to queue.
    Datagram(Vec<u8>),
    /// A list to start, and what is queued instead if one is already
    /// running by then.
    Dump { dump: Dump, busy: Vec<u8> },
}

/// The answers to the requests in `datagram`, in order.
pub(super) fn replies(datagram: &[u8], sender: &Sender, machine: &dyn Machine) -> Vec<Reply> {
    let mut replies = Vec::new();
    let mut dumping = sender.dump_running;
    for (message, action) in wire::requests(datagram, sender.root) {
        let fail = |errno| wire::error(errno, &message, sender.pid, sender.cap_ack);
        match action {
            Action::Ignore => {}
            Action::Refuse(errno) => {
                replies.push(Reply::Datagram(fail(errno)));
                continue;
            }
            Action::Link(lookup) => match find(machine.links(), lookup) {
                Some(link) => {
                    replies.push(Reply::Datagram(link_message(&link, 0, message.seq, sender.pid)));
                }
                None => {
                    replies.push(Reply::Datagram(fail(libc::ENODEV)));
                    continue;
                }
            },
            // Linux takes a second list on a socket only after the first
            // is finished, and does not acknowledge a list at all.
            Action::Dump { .. } if dumping => {
                replies.push(Reply::Datagram(fail(libc::EBUSY)));
                continue;
            }
            Action::Dump { table, family } => {
                dumping = true;
                let dump = dump(table, family, machine, message.seq, sender.pid);
                replies.push(Reply::Dump { dump, busy: fail(libc::EBUSY) });
                continue;
            }
        }
        if message.flags & NLM_F_ACK != 0 {
            replies.push(Reply::Datagram(fail(0)));
        }
    }
    replies
}

#[cfg(test)]
mod tests {
    use super::super::machine::Fixed;
    use super::*;

    const SEQ: u32 = 1;
    const PID: u32 = 4242;

    fn sender() -> Sender {
        Sender { pid: PID, root: false, cap_ack: false, dump_running: false }
    }

    #[test]
    fn a_link_is_laid_out_as_linux_lays_out_lo() {
        let machine = Fixed::qemu();
        let message = link_message(&machine.links[0], NLM_F_MULTI, SEQ, PID);
        let golden: [u8; 88] = [
            // nlmsghdr: 384 bytes, RTM_NEWLINK, NLM_F_MULTI, seq 1, pid 4242.
            0x80, 0x01, 0x00, 0x00, 0x10, 0x00, 0x02, 0x00, 0x01, 0x00, 0x00, 0x00, 0x92, 0x10,
            0x00, 0x00,
            // ifinfomsg: AF_UNSPEC, ARPHRD_LOOPBACK, index 1, flags UP |
            // LOOPBACK | RUNNING | LOWER_UP, no change.
            0x00, 0x00, 0x04, 0x03, 0x01, 0x00, 0x00, 0x00, 0x49, 0x00, 0x01, 0x00, 0x00, 0x00,
            0x00, 0x00,
            // IFLA_IFNAME "lo", IFLA_TXQLEN 1000, IFLA_OPERSTATE unknown,
            // IFLA_MTU 65536.
            0x07, 0x00, 0x03, 0x00, 0x6c, 0x6f, 0x00, 0x00, 0x08, 0x00, 0x0d, 0x00, 0xe8, 0x03,
            0x00, 0x00, 0x05, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00, 0x04, 0x00,
            0x00, 0x00, 0x01, 0x00, // IFLA_ADDRESS and IFLA_BROADCAST, all zeros.
            0x0a, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0a, 0x00,
            0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        assert_eq!(message[..88], golden);
        // IFLA_STATS64 (192 bytes) and IFLA_STATS (96 bytes) follow.
        assert_eq!(message[88..92], [0xc4, 0x00, 0x17, 0x00]);
        assert_eq!(message[284..288], [0x64, 0x00, 0x07, 0x00]);
        assert_eq!(message.len(), 384);
    }

    /// The value of the first attribute of `kind` in an interface,
    /// address or route message with a body of `body` bytes.
    fn attribute(message: &[u8], body: usize, kind: u16) -> Option<Vec<u8>> {
        let attributes = wire::attributes(&message[wire::HEADER + body..]);
        attributes.into_iter().find(|&(found, _)| found == kind).map(|(_, value)| value.to_vec())
    }

    #[test]
    fn a_links_counters_are_in_the_order_of_rtnl_link_stats64() {
        let machine = Fixed::qemu();
        let message = link_message(&machine.links[1], NLM_F_MULTI, SEQ, PID);
        let wide = attribute(&message, 16, IFLA_STATS64).unwrap();
        assert_eq!(wide.len(), 192);
        let word = |at: usize| u64::from_ne_bytes(wide[at..at + 8].try_into().unwrap());
        assert_eq!(word(0), 10, "rx_packets");
        assert_eq!(word(8), 20, "tx_packets");
        assert_eq!(word(16), 1000, "rx_bytes");
        assert_eq!(word(24), 2000, "tx_bytes");
        assert_eq!(word(56), 3, "tx_dropped");
        let narrow = attribute(&message, 16, IFLA_STATS).unwrap();
        assert_eq!(narrow.len(), 96);
        let small = |at: usize| u32::from_ne_bytes(narrow[at..at + 4].try_into().unwrap());
        assert_eq!((small(0), small(4), small(8), small(12)), (10, 20, 1000, 2000));
    }

    #[test]
    fn a_link_with_a_hardware_address_names_it_and_its_broadcast() {
        let machine = Fixed::qemu();
        let message = link_message(&machine.links[1], NLM_F_MULTI, SEQ, PID);
        assert_eq!(
            attribute(&message, 16, IFLA_ADDRESS).unwrap(),
            [0x52, 0x54, 0, 0x12, 0x34, 0x56]
        );
        assert_eq!(attribute(&message, 16, IFLA_BROADCAST).unwrap(), [0xff; 6]);
        assert_eq!(attribute(&message, 16, IFLA_IFNAME).unwrap(), b"net0\0");
        assert_eq!(attribute(&message, 16, IFLA_OPERSTATE).unwrap(), [6]);
    }

    #[test]
    fn a_link_without_a_hardware_address_has_none_to_show() {
        let mut machine = Fixed::qemu();
        machine.links[1].address.clear();
        machine.links[1].broadcast.clear();
        let message = link_message(&machine.links[1], 0, SEQ, PID);
        assert_eq!(attribute(&message, 16, IFLA_ADDRESS), None);
        assert_eq!(attribute(&message, 16, IFLA_BROADCAST), None);
        assert!(attribute(&message, 16, IFLA_STATS64).is_some());
    }

    #[test]
    fn an_ipv4_address_is_laid_out_as_linux_lays_out_one() {
        let machine = Fixed::qemu();
        let address = machine.addresses.iter().find(|a| a.index == 2 && a.address.is_ipv4());
        let message = address_message(address.unwrap(), SEQ, PID);
        let golden: [u8; 88] = [
            // nlmsghdr: 88 bytes, RTM_NEWADDR, NLM_F_MULTI.
            0x58, 0x00, 0x00, 0x00, 0x14, 0x00, 0x02, 0x00, 0x01, 0x00, 0x00, 0x00, 0x92, 0x10,
            0x00, 0x00, // ifaddrmsg: AF_INET, /24, IFA_F_PERMANENT, universe, net0.
            0x02, 0x18, 0x80, 0x00, 0x02, 0x00, 0x00, 0x00,
            // IFA_ADDRESS and IFA_LOCAL 10.0.2.15, IFA_BROADCAST 10.0.2.255.
            0x08, 0x00, 0x01, 0x00, 0x0a, 0x00, 0x02, 0x0f, 0x08, 0x00, 0x02, 0x00, 0x0a, 0x00,
            0x02, 0x0f, 0x08, 0x00, 0x04, 0x00, 0x0a, 0x00, 0x02, 0xff,
            // IFA_LABEL "net0", IFA_FLAGS, IFA_CACHEINFO: forever, forever.
            0x09, 0x00, 0x03, 0x00, 0x6e, 0x65, 0x74, 0x30, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00,
            0x08, 0x00, 0x80, 0x00, 0x00, 0x00, 0x14, 0x00, 0x06, 0x00, 0xff, 0xff, 0xff, 0xff,
            0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        assert_eq!(message, golden);
    }

    #[test]
    fn an_ipv6_address_is_laid_out_as_linux_lays_out_one() {
        let machine = Fixed::qemu();
        let found = machine.addresses.iter().find(|a| a.index == 2 && a.preferred == 86000);
        let message = address_message(found.unwrap(), SEQ, PID);
        let golden: [u8; 72] = [
            // nlmsghdr: 72 bytes, RTM_NEWADDR, NLM_F_MULTI.
            0x48, 0x00, 0x00, 0x00, 0x14, 0x00, 0x02, 0x00, 0x01, 0x00, 0x00, 0x00, 0x92, 0x10,
            0x00, 0x00, // ifaddrmsg: AF_INET6, /64, IFA_F_TEMPORARY, universe, net0.
            0x0a, 0x40, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, // IFA_ADDRESS 2001:db8::5.
            0x14, 0x00, 0x01, 0x00, 0x20, 0x01, 0x0d, 0xb8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x05,
            // IFA_FLAGS, IFA_CACHEINFO: preferred 86000, valid 172000.
            0x08, 0x00, 0x08, 0x00, 0x01, 0x00, 0x00, 0x00, 0x14, 0x00, 0x06, 0x00, 0xf0, 0x4f,
            0x01, 0x00, 0xe0, 0x9f, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        assert_eq!(message, golden);
    }

    #[test]
    fn a_connected_route_is_laid_out_as_linux_lays_out_one() {
        let machine = Fixed::qemu();
        let message = route_message(&machine.routes[0], SEQ, PID);
        let golden: [u8; 60] = [
            // nlmsghdr: 60 bytes, RTM_NEWROUTE, NLM_F_MULTI.
            0x3c, 0x00, 0x00, 0x00, 0x18, 0x00, 0x02, 0x00, 0x01, 0x00, 0x00, 0x00, 0x92, 0x10,
            0x00, 0x00,
            // rtmsg: AF_INET, /24, no source, no TOS, main table, kernel,
            // link scope, unicast, no flags.
            0x02, 0x18, 0x00, 0x00, 0xfe, 0x02, 0xfd, 0x01, 0x00, 0x00, 0x00, 0x00,
            // RTA_TABLE 254, RTA_DST 10.0.2.0, RTA_PREFSRC 10.0.2.15,
            // RTA_OIF 2.
            0x08, 0x00, 0x0f, 0x00, 0xfe, 0x00, 0x00, 0x00, 0x08, 0x00, 0x01, 0x00, 0x0a, 0x00,
            0x02, 0x00, 0x08, 0x00, 0x07, 0x00, 0x0a, 0x00, 0x02, 0x0f, 0x08, 0x00, 0x04, 0x00,
            0x02, 0x00, 0x00, 0x00,
        ];
        assert_eq!(message, golden);
    }

    #[test]
    fn a_default_route_has_a_gateway_and_no_destination() {
        let machine = Fixed::qemu();
        let message = route_message(&machine.routes[1], SEQ, PID);
        let golden: [u8; 52] = [
            0x34, 0x00, 0x00, 0x00, 0x18, 0x00, 0x02, 0x00, 0x01, 0x00, 0x00, 0x00, 0x92, 0x10,
            0x00, 0x00, // rtmsg: AF_INET, /0, main table, boot, universe, unicast.
            0x02, 0x00, 0x00, 0x00, 0xfe, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00,
            // RTA_TABLE 254, RTA_GATEWAY 10.0.2.2, RTA_OIF 2.
            0x08, 0x00, 0x0f, 0x00, 0xfe, 0x00, 0x00, 0x00, 0x08, 0x00, 0x05, 0x00, 0x0a, 0x00,
            0x02, 0x02, 0x08, 0x00, 0x04, 0x00, 0x02, 0x00, 0x00, 0x00,
        ];
        assert_eq!(message, golden);
    }

    #[test]
    fn an_ipv6_route_has_a_metric() {
        let machine = Fixed::qemu();
        let message = route_message(&machine.routes[4], SEQ, PID);
        // Header, rtmsg, RTA_TABLE, RTA_PRIORITY 1024, RTA_GATEWAY and
        // RTA_OIF; no destination for a default route.
        assert_eq!(message.len(), 16 + 12 + 8 + 8 + 20 + 8);
        assert_eq!(message[16], AF_INET6);
        assert_eq!(message[16 + 5], 9, "learned from a router advertisement");
        assert_eq!(message[36..40], [0x08, 0x00, 0x06, 0x00]);
        assert_eq!(message[40..44], 1024u32.to_ne_bytes());
    }

    /// The messages of a datagram or a list of them, by type.
    fn kinds(bytes: &[u8]) -> Vec<u16> {
        wire::messages(bytes).map(|message| message.kind).collect()
    }

    #[test]
    fn a_list_is_given_in_datagrams_and_ends_with_a_done_of_its_own() {
        let machine = Fixed::qemu();
        let mut dump = dump(Table::Links, AF_UNSPEC, &machine, 9, PID);
        let first = dump.chunk(3776).unwrap();
        assert_eq!(kinds(&first), [RTM_NEWLINK, RTM_NEWLINK]);
        assert!(!dump.is_finished());
        let done = dump.chunk(3776).unwrap();
        assert_eq!(done.len(), 20);
        assert_eq!(kinds(&done), [3]);
        assert!(dump.is_finished());
        assert_eq!(dump.chunk(3776), None);
    }

    #[test]
    fn a_datagram_holds_whole_messages_up_to_its_limit_and_at_least_one() {
        let machine = Fixed::qemu();
        let one = link_message(&machine.links[0], NLM_F_MULTI, 9, PID).len();
        let mut dump = dump(Table::Links, AF_UNSPEC, &machine, 9, PID);
        assert_eq!(kinds(&dump.chunk(2 * one - 1).unwrap()), [RTM_NEWLINK]);
        assert_eq!(kinds(&dump.chunk(1).unwrap()), [RTM_NEWLINK]);
        assert_eq!(kinds(&dump.chunk(1).unwrap()), [3]);
    }

    #[test]
    fn an_empty_list_is_only_a_done() {
        let machine = Fixed::default();
        let mut dump = dump(Table::Routes, AF_INET, &machine, 9, PID);
        assert_eq!(kinds(&dump.chunk(100).unwrap()), [3]);
        assert!(dump.is_finished());
    }

    #[test]
    fn addresses_of_all_families_are_listed_with_the_ipv4_ones_first() {
        let machine = Fixed::qemu();
        let mut dump = dump(Table::Addresses, AF_UNSPEC, &machine, 9, PID);
        let first = dump.chunk(10_000).unwrap();
        let families: Vec<u8> = wire::messages(&first).map(|m| m.payload[0]).collect();
        assert_eq!(families, [AF_INET, AF_INET, AF_INET6, AF_INET6, AF_INET6]);
    }

    #[test]
    fn a_family_is_asked_for_alone_or_none_of_it_is_there() {
        let machine = Fixed::qemu();
        let only = |table, family| {
            let mut dump = dump(table, family, &machine, 9, PID);
            let first = dump.chunk(10_000).unwrap();
            wire::messages(&first).filter(|message| message.kind != 3).count()
        };
        assert_eq!(only(Table::Addresses, AF_INET), 2);
        assert_eq!(only(Table::Addresses, AF_INET6), 3);
        assert_eq!(only(Table::Routes, AF_INET), 2);
        assert_eq!(only(Table::Routes, AF_INET6), 3);
        assert_eq!(only(Table::Routes, AF_UNSPEC), 5);
        assert_eq!(only(Table::Addresses, 17), 0);
        // Interfaces have no family.
        assert_eq!(only(Table::Links, 17), 2);
    }

    /// One request datagram.
    fn request(kind: u16, flags: u16, seq: u32, payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&((16 + payload.len()) as u32).to_ne_bytes());
        bytes.extend_from_slice(&kind.to_ne_bytes());
        bytes.extend_from_slice(&flags.to_ne_bytes());
        bytes.extend_from_slice(&seq.to_ne_bytes());
        bytes.extend_from_slice(&0u32.to_ne_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    fn only_reply(replies: Vec<Reply>) -> Reply {
        assert_eq!(replies.len(), 1);
        replies.into_iter().next().unwrap()
    }

    #[test]
    fn a_request_for_one_interface_is_answered_with_it_alone() {
        let machine = Fixed::qemu();
        let mut body = vec![0u8; 16];
        body[4..8].copy_from_slice(&2i32.to_ne_bytes());
        let bytes = request(18, 1, 5, &body);
        let Reply::Datagram(answer) = only_reply(replies(&bytes, &sender(), &machine)) else {
            panic!("a list was started");
        };
        let message = wire::messages(&answer).next().unwrap();
        assert_eq!((message.kind, message.flags, message.seq, message.pid), (16, 0, 5, PID));
        assert_eq!(&answer[16 + 16 + 4..16 + 16 + 4 + 5], b"net0\0");
    }

    #[test]
    fn an_interface_that_is_not_there_is_no_such_device() {
        let machine = Fixed::qemu();
        let mut body = vec![0u8; 16];
        body[4..8].copy_from_slice(&9i32.to_ne_bytes());
        let bytes = request(18, 1, 5, &body);
        let Reply::Datagram(answer) = only_reply(replies(&bytes, &sender(), &machine)) else {
            panic!("a list was started");
        };
        let code = i32::from_ne_bytes(answer[16..20].try_into().unwrap());
        assert_eq!(code, -libc::ENODEV);
    }

    #[test]
    fn a_request_to_change_the_network_is_an_error_that_repeats_it() {
        let machine = Fixed::qemu();
        let bytes = request(20, 1 | NLM_F_ACK, 5, &[2, 24, 0, 0, 2, 0, 0, 0]);
        let Reply::Datagram(answer) = only_reply(replies(&bytes, &sender(), &machine)) else {
            panic!("a list was started");
        };
        assert_eq!(i32::from_ne_bytes(answer[16..20].try_into().unwrap()), -libc::EPERM);
        assert_eq!(answer[20..], bytes[..]);
    }

    #[test]
    fn a_request_asking_for_an_acknowledgement_gets_one() {
        let machine = Fixed::qemu();
        // A control message that is skipped, but asked to be acknowledged.
        let bytes = request(1, 1 | NLM_F_ACK, 5, &[]);
        let Reply::Datagram(answer) = only_reply(replies(&bytes, &sender(), &machine)) else {
            panic!("a list was started");
        };
        assert_eq!(answer.len(), 36);
        assert_eq!(i32::from_ne_bytes(answer[16..20].try_into().unwrap()), 0);
    }

    #[test]
    fn a_dump_is_not_acknowledged_and_a_second_one_is_busy() {
        let machine = Fixed::qemu();
        let mut bytes = request(18, 0x301 | NLM_F_ACK, 5, &[0; 16]);
        bytes.extend_from_slice(&request(22, 0x301, 6, &[0]));
        let found = replies(&bytes, &sender(), &machine);
        assert_eq!(found.len(), 2);
        assert!(matches!(found[0], Reply::Dump { .. }));
        let Reply::Datagram(busy) = &found[1] else { panic!("a second list was started") };
        assert_eq!(i32::from_ne_bytes(busy[16..20].try_into().unwrap()), -libc::EBUSY);

        let running = Sender { dump_running: true, ..sender() };
        let found = replies(&request(18, 0x301, 5, &[0; 16]), &running, &machine);
        assert!(matches!(found[0], Reply::Datagram(_)));
    }

    #[test]
    fn a_list_carries_the_requests_sequence_number_and_the_sockets_number() {
        let machine = Fixed::qemu();
        let found = replies(&request(26, 0x301, 77, &[2]), &sender(), &machine);
        let Reply::Dump { mut dump, .. } = only_reply(found) else { panic!("no list") };
        let first = dump.chunk(10_000).unwrap();
        for message in wire::messages(&first) {
            assert_eq!((message.seq, message.pid), (77, PID));
        }
        let done = dump.chunk(10_000).unwrap();
        let message = wire::messages(&done).next().unwrap();
        assert_eq!((message.kind, message.flags, message.seq, message.pid), (3, 2, 77, PID));
    }
}
