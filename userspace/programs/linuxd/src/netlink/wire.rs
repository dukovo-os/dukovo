// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Netlink's message framing (netlink(7)) and the requests a program
//! sends on a NETLINK_ROUTE socket (rtnetlink(7)).
//!
//! [`messages`] takes a datagram apart as Linux's `netlink_rcv_skb` does,
//! and [`classify`] says what rtnetlink makes of each message: list a
//! table, describe one interface, refuse with an errno, or take it
//! without an answer. [`Builder`] writes the messages `answer` sends
//! back. Everything here reads bytes a program chose, so each length is
//! checked before it is used and no input makes it fail or panic; the
//! fuzz target `linuxd-netlink` feeds it arbitrary bytes.

/// The size of `struct nlmsghdr`.
pub const HEADER: usize = 16;

/// NLM_F_REQUEST: the message asks for something.
pub(super) const NLM_F_REQUEST: u16 = 0x1;
/// NLM_F_MULTI: the message is one of a list that NLMSG_DONE ends.
pub(super) const NLM_F_MULTI: u16 = 0x2;
/// NLM_F_ACK: the sender wants an NLMSG_ERROR with 0 when it succeeds.
pub(super) const NLM_F_ACK: u16 = 0x4;
/// NLM_F_ROOT | NLM_F_MATCH: either asks for a dump.
const NLM_F_DUMP: u16 = 0x300;
/// NLM_F_CAPPED: the NLMSG_ERROR holds the request's header, not its
/// body.
const NLM_F_CAPPED: u16 = 0x100;

/// NLMSG_ERROR.
const NLMSG_ERROR: u16 = 2;
/// NLMSG_DONE.
const NLMSG_DONE: u16 = 3;
/// NLMSG_MIN_TYPE: types below it are control messages the kernel skips.
const NLMSG_MIN_TYPE: u16 = 16;

/// RTM_NEWLINK, which a list of links is made of.
pub(super) const RTM_NEWLINK: u16 = 16;
/// RTM_GETLINK.
const RTM_GETLINK: u16 = 18;
/// RTM_NEWADDR.
pub(super) const RTM_NEWADDR: u16 = 20;
/// RTM_GETADDR.
const RTM_GETADDR: u16 = 22;
/// RTM_NEWROUTE.
pub(super) const RTM_NEWROUTE: u16 = 24;
/// RTM_GETROUTE.
const RTM_GETROUTE: u16 = 26;
/// RTM_MAX: no rtnetlink message has a type above it.
const RTM_MAX: u16 = 123;
/// The kind of a request that only reads, `type & 3` of its RTM_ number.
const RTNL_KIND_GET: u16 = 2;

/// IFLA_IFNAME, the attribute that names an interface in a request.
const IFLA_IFNAME: u16 = 3;
/// The size of `struct ifinfomsg`, which an RTM_GETLINK starts with.
const IFINFOMSG: usize = 16;
/// The longest interface name, without its NUL (IFNAMSIZ - 1).
const NAME_MAX: usize = 15;
/// NLA_TYPE_MASK: the bits of an attribute's type that are not flags.
const NLA_TYPE_MASK: u16 = 0x3fff;

/// NLMSG_ALIGN and RTA_ALIGN: messages and attributes are padded to four
/// bytes.
pub(super) const fn align(len: usize) -> usize {
    len.next_multiple_of(4)
}

/// One message of a datagram a program sent: its header's fields and the
/// bytes after the header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Message<'a> {
    /// `nlmsg_type`.
    pub kind: u16,
    /// `nlmsg_flags`.
    pub flags: u16,
    /// `nlmsg_seq`, which the answers carry.
    pub seq: u32,
    /// `nlmsg_pid`, which Linux ignores in a request.
    pub pid: u32,
    /// As many bytes after the header as `nlmsg_len` says.
    pub payload: &'a [u8],
}

/// The messages of a datagram, from [`messages`].
pub struct Messages<'a> {
    rest: &'a [u8],
}

/// The messages of `datagram`, as `netlink_rcv_skb` takes them apart:
/// each starts at a multiple of four bytes, and the first whose length
/// is under the header's or over what is left ends the list, silently.
pub fn messages(datagram: &[u8]) -> Messages<'_> {
    Messages { rest: datagram }
}

impl<'a> Iterator for Messages<'a> {
    type Item = Message<'a>;

    fn next(&mut self) -> Option<Message<'a>> {
        let rest = self.rest;
        if rest.len() < HEADER {
            return None;
        }
        let len = usize::try_from(u32_at(rest, 0)?).ok()?;
        if len < HEADER || len > rest.len() {
            self.rest = &[];
            return None;
        }
        self.rest = &rest[align(len).min(rest.len())..];
        Some(Message {
            kind: u16_at(rest, 4)?,
            flags: u16_at(rest, 6)?,
            seq: u32_at(rest, 8)?,
            pid: u32_at(rest, 12)?,
            payload: &rest[HEADER..len],
        })
    }
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_ne_bytes(bytes.get(at..at.checked_add(2)?)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(bytes.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

/// What a dump lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Table {
    /// The interfaces (RTM_GETLINK).
    Links,
    /// The addresses (RTM_GETADDR).
    Addresses,
    /// The routes (RTM_GETROUTE).
    Routes,
}

/// How a request names the one interface it asks about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lookup<'a> {
    /// By `ifi_index`.
    Index(u32),
    /// By IFLA_IFNAME, up to its first NUL.
    Name(&'a [u8]),
}

/// What rtnetlink does with one request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action<'a> {
    /// Nothing, unless the sender asked for an acknowledgement.
    Ignore,
    /// Fails with this errno.
    Refuse(i32),
    /// Lists every entry of `table` in address family `family`.
    Dump {
        /// What is listed.
        table: Table,
        /// `rtgen_family`: AF_UNSPEC, AF_INET, AF_INET6 or another.
        family: u8,
    },
    /// Describes one interface.
    Link(Lookup<'a>),
}

/// The messages of `datagram` with what rtnetlink does with each; `root`
/// says whether the sender may change the machine's network, which
/// decides the errno a request that would change it gets.
pub fn requests(
    datagram: &[u8],
    root: bool,
) -> impl Iterator<Item = (Message<'_>, Action<'_>)> + '_ {
    messages(datagram).map(move |message| (message, classify(&message, root)))
}

/// What rtnetlink does with `message`, in the order of its
/// `rtnetlink_rcv_msg`.
pub fn classify<'a>(message: &Message<'a>, root: bool) -> Action<'a> {
    // Messages that are not requests, and control messages, are skipped.
    if message.flags & NLM_F_REQUEST == 0 || message.kind < NLMSG_MIN_TYPE {
        return Action::Ignore;
    }
    if message.kind > RTM_MAX {
        return Action::Refuse(libc::EOPNOTSUPP);
    }
    // Every rtnetlink message has at least the one byte of rtgenmsg.
    let Some(&family) = message.payload.first() else {
        return Action::Ignore;
    };
    // Only requests that read are served; the rest need the right to
    // change the network, which a person without it is told first.
    if message.kind & 3 != RTNL_KIND_GET {
        return Action::Refuse(if root { libc::EOPNOTSUPP } else { libc::EPERM });
    }
    let dump = message.flags & NLM_F_DUMP != 0;
    match (message.kind, dump) {
        (RTM_GETLINK, true) => Action::Dump { table: Table::Links, family },
        (RTM_GETADDR, true) => Action::Dump { table: Table::Addresses, family },
        (RTM_GETROUTE, true) => Action::Dump { table: Table::Routes, family },
        (RTM_GETLINK, false) => link_request(message.payload),
        _ => Action::Refuse(libc::EOPNOTSUPP),
    }
}

/// The interface a single RTM_GETLINK names: by number if it gives one,
/// else by IFLA_IFNAME; EINVAL if it names none or is malformed.
fn link_request(payload: &[u8]) -> Action<'_> {
    let Some(index) = u32_at(payload, 4).filter(|_| payload.len() >= IFINFOMSG) else {
        return Action::Refuse(libc::EINVAL);
    };
    let mut name = None;
    for (kind, value) in attributes(&payload[IFINFOMSG..]) {
        if kind != IFLA_IFNAME {
            continue;
        }
        // NLA_STRING with room for IFNAMSIZ - 1: one trailing NUL is
        // dropped before the length is checked.
        let text = value.strip_suffix(&[0]).unwrap_or(value);
        if value.is_empty() || text.len() > NAME_MAX {
            return Action::Refuse(libc::EINVAL);
        }
        name = text.split(|&byte| byte == 0).next();
    }
    // `ifi_index` is a signed number: a negative one names nothing.
    match (i32::try_from(index), name) {
        (Ok(index @ 1..), _) => Action::Link(Lookup::Index(index.unsigned_abs())),
        (_, Some(name)) => Action::Link(Lookup::Name(name)),
        _ => Action::Refuse(libc::EINVAL),
    }
}

/// The attributes in `bytes` as (type without flags, value): the list
/// ends at the first one that does not fit.
pub(super) fn attributes(bytes: &[u8]) -> impl Iterator<Item = (u16, &[u8])> {
    let mut rest = bytes;
    std::iter::from_fn(move || {
        let len = usize::from(u16_at(rest, 0)?);
        let kind = u16_at(rest, 2)? & NLA_TYPE_MASK;
        if len < 4 || len > rest.len() {
            return None;
        }
        let value = &rest[4..len];
        rest = &rest[align(len).min(rest.len())..];
        Some((kind, value))
    })
}

/// A message being written: header first, then the body, then
/// attributes. [`Builder::finish`] sets the length.
pub(super) struct Builder {
    bytes: Vec<u8>,
}

impl Builder {
    /// A message of `kind` with no body yet.
    pub fn new(kind: u16, flags: u16, seq: u32, pid: u32) -> Builder {
        let mut bytes = Vec::with_capacity(HEADER + 64);
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&kind.to_ne_bytes());
        bytes.extend_from_slice(&flags.to_ne_bytes());
        bytes.extend_from_slice(&seq.to_ne_bytes());
        bytes.extend_from_slice(&pid.to_ne_bytes());
        Builder { bytes }
    }

    /// Appends `bytes` to the body.
    pub fn put(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    /// Appends an attribute of `kind` whose value is `value`, padded.
    pub fn attribute(&mut self, kind: u16, value: &[u8]) {
        // Every value written here is a few hundred bytes at most.
        let len = (4 + value.len()) as u16;
        self.bytes.extend_from_slice(&len.to_ne_bytes());
        self.bytes.extend_from_slice(&kind.to_ne_bytes());
        self.bytes.extend_from_slice(value);
        self.bytes.resize(align(self.bytes.len()), 0);
    }

    /// An attribute whose value is the number `value`.
    pub fn number(&mut self, kind: u16, value: u32) {
        self.attribute(kind, &value.to_ne_bytes());
    }

    /// The message, with its length set and padded to four bytes. The
    /// length leaves out padding after the body, as Linux's `nlmsg_put`
    /// does.
    pub fn finish(mut self) -> Vec<u8> {
        // A message is a few kilobytes at most.
        let len = self.bytes.len() as u32;
        self.bytes[..4].copy_from_slice(&len.to_ne_bytes());
        self.bytes.resize(align(self.bytes.len()), 0);
        self.bytes
    }
}

/// The NLMSG_DONE that ends a list.
pub(super) fn done(seq: u32, pid: u32) -> Vec<u8> {
    let mut message = Builder::new(NLMSG_DONE, NLM_F_MULTI, seq, pid);
    message.put(&0i32.to_ne_bytes());
    message.finish()
}

/// The NLMSG_ERROR that answers `request` from the socket `pid`: `errno`
/// is what failed, or 0 for an acknowledgement. The message repeats the
/// request's header, and its body too unless the request succeeded or
/// the socket asked for NETLINK_CAP_ACK (`cap_ack`).
pub fn error(errno: i32, request: &Message<'_>, pid: u32, cap_ack: bool) -> Vec<u8> {
    let whole = errno != 0 && !cap_ack;
    let mut message =
        Builder::new(NLMSG_ERROR, if whole { 0 } else { NLM_F_CAPPED }, request.seq, pid);
    message.put(&(-errno).to_ne_bytes());
    // The request's own header, as it was sent.
    let len = u32::try_from(HEADER + request.payload.len()).unwrap_or(u32::MAX);
    message.put(&len.to_ne_bytes());
    message.put(&request.kind.to_ne_bytes());
    message.put(&request.flags.to_ne_bytes());
    message.put(&request.seq.to_ne_bytes());
    message.put(&request.pid.to_ne_bytes());
    if whole {
        message.put(request.payload);
    }
    message.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A request header and `payload`, with the length it needs.
    fn request(kind: u16, flags: u16, seq: u32, payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&((HEADER + payload.len()) as u32).to_ne_bytes());
        bytes.extend_from_slice(&kind.to_ne_bytes());
        bytes.extend_from_slice(&flags.to_ne_bytes());
        bytes.extend_from_slice(&seq.to_ne_bytes());
        bytes.extend_from_slice(&0u32.to_ne_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    fn only(datagram: &[u8], root: bool) -> Action<'_> {
        let all: Vec<_> = requests(datagram, root).collect();
        assert_eq!(all.len(), 1);
        all[0].1
    }

    #[test]
    fn a_message_of_seventeen_bytes_is_one_message() {
        // Go's syscall.NetlinkRIB sends the header and one byte, unpadded.
        let bytes = request(RTM_GETROUTE, NLM_F_REQUEST | NLM_F_DUMP, 1, &[2]);
        assert_eq!(bytes.len(), 17);
        let found: Vec<_> = messages(&bytes).collect();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].payload, [2]);
        assert_eq!(found[0].seq, 1);
        assert_eq!(only(&bytes, false), Action::Dump { table: Table::Routes, family: 2 });
    }

    #[test]
    fn messages_follow_each_other_at_multiples_of_four() {
        let mut bytes = request(RTM_GETLINK, NLM_F_REQUEST | NLM_F_DUMP, 7, &[0]);
        bytes.resize(20, 0);
        bytes.extend_from_slice(&request(
            RTM_GETADDR,
            NLM_F_REQUEST | NLM_F_DUMP,
            8,
            &[0, 0, 0, 0],
        ));
        let seqs: Vec<u32> = messages(&bytes).map(|message| message.seq).collect();
        assert_eq!(seqs, [7, 8]);
    }

    #[test]
    fn a_message_that_cannot_be_one_ends_the_list_without_an_error() {
        let mut bytes = request(RTM_GETLINK, NLM_F_REQUEST | NLM_F_DUMP, 1, &[0; 4]);
        let mut short = request(RTM_GETLINK, NLM_F_REQUEST, 2, &[0; 4]);
        short[..4].copy_from_slice(&15u32.to_ne_bytes());
        bytes.extend_from_slice(&short);
        bytes.extend_from_slice(&request(RTM_GETLINK, NLM_F_REQUEST, 3, &[0; 4]));
        assert_eq!(messages(&bytes).count(), 1);

        let mut long = request(RTM_GETLINK, NLM_F_REQUEST, 4, &[0; 4]);
        long[..4].copy_from_slice(&21u32.to_ne_bytes());
        assert_eq!(messages(&long).count(), 0);
        assert_eq!(messages(&[1, 2, 3]).count(), 0);
        assert_eq!(messages(&[]).count(), 0);
    }

    #[test]
    fn a_dump_is_asked_for_by_either_of_its_two_flags() {
        for flags in [0x100, 0x200, 0x300] {
            let bytes = request(RTM_GETLINK, NLM_F_REQUEST | flags, 1, &[0; 16]);
            assert_eq!(only(&bytes, false), Action::Dump { table: Table::Links, family: 0 });
        }
    }

    #[test]
    fn what_is_not_a_request_or_not_rtnetlink_is_skipped() {
        let reply = request(RTM_GETLINK, 0, 1, &[0; 16]);
        assert_eq!(only(&reply, false), Action::Ignore);
        let noop = request(1, NLM_F_REQUEST, 1, &[0; 16]);
        assert_eq!(only(&noop, false), Action::Ignore);
        let empty = request(RTM_GETLINK, NLM_F_REQUEST | NLM_F_DUMP, 1, &[]);
        assert_eq!(only(&empty, false), Action::Ignore);
        let beyond = request(RTM_MAX + 1, NLM_F_REQUEST, 1, &[0]);
        assert_eq!(only(&beyond, false), Action::Refuse(libc::EOPNOTSUPP));
    }

    #[test]
    fn a_request_to_change_the_network_is_refused_by_who_may_not() {
        // RTM_NEWADDR and RTM_DELROUTE.
        for kind in [RTM_NEWADDR, 25] {
            let bytes = request(kind, NLM_F_REQUEST | NLM_F_ACK, 1, &[0; 8]);
            assert_eq!(only(&bytes, false), Action::Refuse(libc::EPERM));
            assert_eq!(only(&bytes, true), Action::Refuse(libc::EOPNOTSUPP));
        }
    }

    #[test]
    fn a_request_to_read_that_is_not_served_is_not_supported() {
        // RTM_GETNEIGH, RTM_GETRULE and RTM_GETROUTE without a dump.
        for kind in [30, 34, RTM_GETROUTE] {
            let bytes = request(kind, NLM_F_REQUEST, 1, &[2, 0, 0, 0]);
            assert_eq!(only(&bytes, false), Action::Refuse(libc::EOPNOTSUPP));
        }
    }

    /// An RTM_GETLINK body: `ifinfomsg` for `index`, then `attributes`.
    fn link_body(index: i32, attributes: &[u8]) -> Vec<u8> {
        let mut body = vec![0u8; IFINFOMSG];
        body[4..8].copy_from_slice(&index.to_ne_bytes());
        body.extend_from_slice(attributes);
        body
    }

    /// An attribute of `kind` holding `value`, padded.
    fn attribute(kind: u16, value: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&((4 + value.len()) as u16).to_ne_bytes());
        bytes.extend_from_slice(&kind.to_ne_bytes());
        bytes.extend_from_slice(value);
        bytes.resize(align(bytes.len()), 0);
        bytes
    }

    #[test]
    fn one_interface_is_named_by_number_or_by_name() {
        let by_index = request(RTM_GETLINK, NLM_F_REQUEST, 1, &link_body(2, &[]));
        assert_eq!(only(&by_index, false), Action::Link(Lookup::Index(2)));

        let name = attribute(IFLA_IFNAME, b"net0\0");
        let by_name = request(RTM_GETLINK, NLM_F_REQUEST, 1, &link_body(0, &name));
        assert_eq!(only(&by_name, false), Action::Link(Lookup::Name(b"net0")));

        // The number wins over a name beside it.
        let both = request(RTM_GETLINK, NLM_F_REQUEST, 1, &link_body(3, &name));
        assert_eq!(only(&both, false), Action::Link(Lookup::Index(3)));
    }

    #[test]
    fn a_request_for_one_interface_that_names_none_is_invalid() {
        for index in [0, -1] {
            let bytes = request(RTM_GETLINK, NLM_F_REQUEST, 1, &link_body(index, &[]));
            assert_eq!(only(&bytes, false), Action::Refuse(libc::EINVAL));
        }
        let short = request(RTM_GETLINK, NLM_F_REQUEST, 1, &[0; 8]);
        assert_eq!(only(&short, false), Action::Refuse(libc::EINVAL));
    }

    #[test]
    fn a_name_that_cannot_be_one_is_invalid() {
        let long = attribute(IFLA_IFNAME, b"sixteen-chars-xx\0");
        let empty = attribute(IFLA_IFNAME, b"");
        for attribute in [long, empty] {
            let bytes = request(RTM_GETLINK, NLM_F_REQUEST, 1, &link_body(0, &attribute));
            assert_eq!(only(&bytes, false), Action::Refuse(libc::EINVAL));
        }
        let longest = attribute(IFLA_IFNAME, b"fifteen-chars-x\0");
        let bytes = request(RTM_GETLINK, NLM_F_REQUEST, 1, &link_body(0, &longest));
        assert_eq!(only(&bytes, false), Action::Link(Lookup::Name(b"fifteen-chars-x")));
    }

    #[test]
    fn attributes_end_at_the_first_that_does_not_fit() {
        let mut bytes = attribute(IFLA_IFNAME, b"a\0");
        bytes.extend_from_slice(&[0xff, 0xff, 3, 0, 1, 2]);
        assert_eq!(attributes(&bytes).count(), 1);
        assert_eq!(attributes(&[3, 0]).count(), 0);
        assert_eq!(attributes(&[2, 0, 1, 0]).count(), 0);
    }

    #[test]
    fn an_error_repeats_the_request_and_its_body_unless_capped() {
        let bytes = request(RTM_NEWADDR, NLM_F_REQUEST, 9, &[1, 2, 3, 4, 5]);
        let message = messages(&bytes).next().unwrap();

        let whole = error(libc::EOPNOTSUPP, &message, 77, false);
        assert_eq!(u32_at(&whole, 0), Some(16 + 4 + 16 + 5));
        assert_eq!(whole.len(), 44);
        assert_eq!(u16_at(&whole, 4), Some(NLMSG_ERROR));
        assert_eq!(u16_at(&whole, 6), Some(0));
        assert_eq!(u32_at(&whole, 8), Some(9));
        assert_eq!(u32_at(&whole, 12), Some(77));
        assert_eq!(i32::from_ne_bytes(whole[16..20].try_into().unwrap()), -libc::EOPNOTSUPP);
        assert_eq!(&whole[20..36], &bytes[..16]);
        assert_eq!(&whole[36..41], [1, 2, 3, 4, 5]);

        let capped = error(libc::EOPNOTSUPP, &message, 77, true);
        assert_eq!(u32_at(&capped, 0), Some(36));
        assert_eq!(u16_at(&capped, 6), Some(NLM_F_CAPPED));

        let ack = error(0, &message, 77, false);
        assert_eq!(u32_at(&ack, 0), Some(36));
        assert_eq!(u16_at(&ack, 6), Some(NLM_F_CAPPED));
        assert_eq!(i32::from_ne_bytes(ack[16..20].try_into().unwrap()), 0);
    }

    #[test]
    fn a_list_ends_with_a_done_of_twenty_bytes() {
        let bytes = done(5, 42);
        assert_eq!(bytes, [20, 0, 0, 0, 3, 0, 2, 0, 5, 0, 0, 0, 42, 0, 0, 0, 0, 0, 0, 0]);
    }
}
