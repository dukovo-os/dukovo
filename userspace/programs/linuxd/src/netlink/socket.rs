// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The state of netlink sockets: their numbers, options, queues and the
//! list each is giving out.
//!
//! A socket holds the datagrams answered to it, at most its receive
//! buffer's worth, as Linux's `sk_rmem_alloc` limits them. A list longer
//! than that is kept whole in [`Dump`] and given out a datagram at a
//! time as the program receives, which is how Linux's dump continuation
//! works: `receive` takes a datagram, and then adds the next of the list
//! if the queue is under half the buffer. Answers that find the queue
//! full are dropped and the next receive fails with ENOBUFS, unless the
//! socket asked for NETLINK_NO_ENOBUFS.
//!
//! The rules of `bind`, `connect` and the options follow
//! `net/netlink/af_netlink.c` and `net/core/sock.c`; where a rule is
//! Linux's alone, the function says which.

use std::collections::{BTreeMap, VecDeque};

use super::answer::{Dump, Reply, Sender};

/// `net.core.rmem_default` and `wmem_default`, and `rmem_max` and
/// `wmem_max`, on a stock Linux: what a socket starts with and what a
/// program without the privilege may ask for.
const BUFFER_DEFAULT: i32 = 212_992;
const BUFFER_MAX: i32 = 212_992;
/// SOCK_MIN_SNDBUF and SOCK_MIN_RCVBUF: no program makes a buffer
/// smaller.
const SNDBUF_MIN: i32 = 4_608;
const RCVBUF_MIN: i32 = 2_304;
/// NLMSG_GOODSIZE: the size of the datagram a list starts with, which
/// grows to what a program's receives offer, up to 32 KiB less the
/// bookkeeping of a datagram (SKB_WITH_OVERHEAD(32768)).
const DATAGRAM_MIN: usize = 3_776;
const DATAGRAM_MAX: usize = 32_448;

/// SOL_SOCKET and SOL_NETLINK.
const SOL_SOCKET: i32 = 1;
const SOL_NETLINK: i32 = 270;

/// SOL_SOCKET options.
const SO_REUSEADDR: i32 = 2;
const SO_TYPE: i32 = 3;
const SO_ERROR: i32 = 4;
const SO_BROADCAST: i32 = 6;
const SO_SNDBUF: i32 = 7;
const SO_RCVBUF: i32 = 8;
const SO_KEEPALIVE: i32 = 9;
const SO_PRIORITY: i32 = 12;
const SO_LINGER: i32 = 13;
const SO_REUSEPORT: i32 = 15;
const SO_PASSCRED: i32 = 16;
const SO_RCVTIMEO: i32 = 20;
const SO_SNDTIMEO: i32 = 21;
const SO_TIMESTAMP: i32 = 29;
const SO_SNDBUFFORCE: i32 = 32;
const SO_RCVBUFFORCE: i32 = 33;
const SO_PROTOCOL: i32 = 38;
const SO_DOMAIN: i32 = 39;

/// SOL_SOCKET options that every Linux socket takes and netlink does
/// nothing with: a program sets them out of habit.
const IGNORED: [i32; 9] = [
    SO_REUSEADDR,
    SO_BROADCAST,
    SO_KEEPALIVE,
    SO_PRIORITY,
    SO_LINGER,
    SO_REUSEPORT,
    SO_RCVTIMEO,
    SO_SNDTIMEO,
    SO_TIMESTAMP,
];

/// SOL_NETLINK options. Each of the flags is a bit of `Socket::options`,
/// by its number.
const NETLINK_ADD_MEMBERSHIP: i32 = 1;
const NETLINK_DROP_MEMBERSHIP: i32 = 2;
const NETLINK_PKTINFO: i32 = 3;
const NETLINK_BROADCAST_ERROR: i32 = 4;
const NETLINK_NO_ENOBUFS: i32 = 5;
const NETLINK_CAP_ACK: i32 = 10;
const NETLINK_EXT_ACK: i32 = 11;
const NETLINK_GET_STRICT_CHK: i32 = 12;

/// The options that are only a flag.
const NETLINK_FLAGS: [i32; 6] = [
    NETLINK_PKTINFO,
    NETLINK_BROADCAST_ERROR,
    NETLINK_NO_ENOBUFS,
    NETLINK_CAP_ACK,
    NETLINK_EXT_ACK,
    NETLINK_GET_STRICT_CHK,
];

/// The multicast groups of NETLINK_ROUTE: RTNLGRP_MAX.
const GROUPS: i32 = 36;

/// AF_NETLINK, and a socket's type, which `SO_TYPE` reports.
const AF_NETLINK: i32 = 16;

/// A `struct sockaddr_nl`'s port number and groups: `pid` is 0 for the
/// kernel, and for a socket to be given a number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct NlAddress {
    pub pid: u32,
    pub groups: u32,
}

/// Who makes a call: the process, whose number a socket is bound to when
/// it asks for none, and whether it may do what only root may.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Caller {
    /// The process's number; 0 where it is not known.
    pub process: u64,
    pub root: bool,
}

/// One netlink socket.
struct Socket {
    /// SOCK_RAW or SOCK_DGRAM, which `SO_TYPE` reports.
    kind: i32,
    /// The socket's own number, once it is bound.
    portid: u32,
    bound: bool,
    /// The multicast groups it joined: bit n is group n + 1.
    groups: u64,
    sndbuf: i32,
    rcvbuf: i32,
    pass_cred: bool,
    /// The NETLINK_ options that are flags, a bit by each option's
    /// number.
    options: u32,
    queue: VecDeque<Vec<u8>>,
    /// The bytes in `queue`.
    queued: usize,
    dump: Option<Dump>,
    /// The longest buffer a receive offered, which sizes the datagrams of
    /// the next list.
    max_receive: usize,
    /// Whether an answer was dropped for want of room.
    overrun: bool,
}

impl Socket {
    fn new(kind: i32) -> Socket {
        Socket {
            kind,
            portid: 0,
            bound: false,
            groups: 0,
            sndbuf: BUFFER_DEFAULT,
            rcvbuf: BUFFER_DEFAULT,
            pass_cred: false,
            options: 0,
            queue: VecDeque::new(),
            queued: 0,
            dump: None,
            max_receive: 0,
            overrun: false,
        }
    }

    fn has(&self, option: i32) -> bool {
        self.options & (1 << option) != 0
    }

    /// The bytes the receive buffer holds at most; its smallest value is
    /// `RCVBUF_MIN`, so it is never negative.
    fn room(&self) -> usize {
        self.rcvbuf as usize
    }

    /// Queues an answer, or drops it if the queue is already over the
    /// buffer, as Linux's `netlink_unicast` does.
    fn enqueue(&mut self, datagram: Vec<u8>) {
        if self.queued > self.room() {
            self.overrun = !self.has(NETLINK_NO_ENOBUFS);
            return;
        }
        self.push(datagram);
    }

    fn push(&mut self, datagram: Vec<u8>) {
        self.queued += datagram.len();
        self.queue.push_back(datagram);
    }

    /// Adds the next datagram of the list being given out, if the queue
    /// is under half the buffer.
    fn pump(&mut self) {
        if self.queued > self.room() / 2 {
            return;
        }
        let limit = self.max_receive.clamp(DATAGRAM_MIN, DATAGRAM_MAX);
        let Some(dump) = self.dump.as_mut() else {
            return;
        };
        let next = dump.chunk(limit);
        if dump.is_finished() {
            self.dump = None;
        }
        if let Some(datagram) = next {
            self.push(datagram);
        }
    }

    fn apply(&mut self, reply: Reply) {
        match reply {
            Reply::Datagram(datagram) => self.enqueue(datagram),
            Reply::Dump { dump, busy } => {
                if self.dump.is_some() {
                    self.enqueue(busy);
                } else {
                    self.dump = Some(dump);
                    self.pump();
                }
            }
        }
    }

    /// A SOL_SOCKET option's value, as `sock_getsockopt` reports it.
    fn socket_value(&mut self, name: i32) -> Result<i32, i32> {
        Ok(match name {
            SO_TYPE => self.kind,
            SO_PROTOCOL => 0,
            SO_DOMAIN => AF_NETLINK,
            SO_SNDBUF => self.sndbuf,
            SO_RCVBUF => self.rcvbuf,
            SO_PASSCRED => i32::from(self.pass_cred),
            SO_ERROR => {
                let failed = std::mem::take(&mut self.overrun);
                if failed { libc::ENOBUFS } else { 0 }
            }
            name if IGNORED.contains(&name) => 0,
            _ => return Err(libc::ENOPROTOOPT),
        })
    }

    /// A SOL_NETLINK option's value, as `netlink_getsockopt` reports it.
    fn netlink_value(&self, name: i32) -> Result<i32, i32> {
        if NETLINK_FLAGS.contains(&name) {
            Ok(i32::from(self.has(name)))
        } else {
            Err(libc::ENOPROTOOPT)
        }
    }

    /// Sets a SOL_SOCKET option, as `sock_setsockopt` does; the buffer
    /// sizes only a privileged caller may force.
    fn set_socket_option(&mut self, name: i32, value: i32, root: bool) -> Result<(), i32> {
        match name {
            SO_SNDBUF => self.sndbuf = buffer(value, SNDBUF_MIN),
            SO_RCVBUF => self.rcvbuf = buffer(value, RCVBUF_MIN),
            SO_SNDBUFFORCE | SO_RCVBUFFORCE => {
                if !root {
                    return Err(libc::EPERM);
                }
                let forced = value.min(i32::MAX / 2).saturating_mul(2);
                if name == SO_SNDBUFFORCE {
                    self.sndbuf = forced.max(SNDBUF_MIN);
                } else {
                    self.rcvbuf = forced.max(RCVBUF_MIN);
                }
            }
            SO_PASSCRED => self.pass_cred = value != 0,
            name if IGNORED.contains(&name) => {}
            _ => return Err(libc::ENOPROTOOPT),
        }
        Ok(())
    }

    /// Sets a SOL_NETLINK option, as `netlink_setsockopt` does. No event
    /// is ever sent, so a membership only changes what `getsockname`
    /// reports.
    fn set_netlink_option(&mut self, name: i32, value: i32) -> Result<(), i32> {
        match name {
            NETLINK_ADD_MEMBERSHIP | NETLINK_DROP_MEMBERSHIP => {
                if !(1..=GROUPS).contains(&value) {
                    return Err(libc::EINVAL);
                }
                let group = 1u64 << (value - 1);
                if name == NETLINK_ADD_MEMBERSHIP {
                    self.groups |= group;
                } else {
                    self.groups &= !group;
                }
            }
            name if NETLINK_FLAGS.contains(&name) => {
                if value != 0 {
                    self.options |= 1 << name;
                } else {
                    self.options &= !(1 << name);
                }
            }
            _ => return Err(libc::ENOPROTOOPT),
        }
        Ok(())
    }
}

/// The size `SO_SNDBUF` or `SO_RCVBUF` of `requested` makes a buffer:
/// twice what is asked for, as Linux counts its own bookkeeping in, but
/// not above twice the limit a program may ask for, and not below
/// `minimum`. Linux reads the request as unsigned, so a negative one is
/// above the limit.
fn buffer(requested: i32, minimum: i32) -> i32 {
    let asked = if requested < 0 { BUFFER_MAX } else { requested.min(BUFFER_MAX) };
    (asked * 2).max(minimum)
}

/// What `send` needs to know before it reads the data.
pub(super) struct Prepared {
    /// Who the answers go to.
    pub sender: Sender,
    /// The largest datagram the socket may send, with Linux's 32 bytes
    /// of overhead left out.
    pub sndbuf: usize,
    /// Whether the kernel takes the datagram: one sent to a multicast
    /// group is taken by nobody.
    pub delivers: bool,
}

/// Every netlink socket, by number.
#[derive(Default)]
pub(super) struct Table {
    sockets: BTreeMap<u64, Socket>,
    last: u64,
}

impl Table {
    /// Makes a socket of `kind` (SOCK_RAW or SOCK_DGRAM) and gives its
    /// number, which is never used again.
    pub fn create(&mut self, kind: i32) -> u64 {
        self.last += 1;
        self.sockets.insert(self.last, Socket::new(kind));
        self.last
    }

    pub fn remove(&mut self, id: u64) {
        self.sockets.remove(&id);
    }

    pub fn contains(&self, id: u64) -> bool {
        self.sockets.contains_key(&id)
    }

    pub fn ids(&self) -> Vec<u64> {
        self.sockets.keys().copied().collect()
    }

    fn get(&mut self, id: u64) -> Result<&mut Socket, i32> {
        self.sockets.get_mut(&id).ok_or(libc::ENOTSOCK)
    }

    /// Whether a bound socket has `portid`.
    fn used(&self, portid: u32) -> bool {
        self.sockets.values().any(|socket| socket.bound && socket.portid == portid)
    }

    /// The number a socket that asks for none is given: its process's,
    /// or if another socket has that, the first free one counting down
    /// from -4096 as a 32-bit number, as `netlink_autobind` does.
    fn free_portid(&self, process: u64) -> u32 {
        let own = u32::try_from(process).ok().filter(|&number| number != 0);
        if let Some(number) = own.filter(|&number| !self.used(number)) {
            return number;
        }
        let mut number = (-4096i32).cast_unsigned();
        while self.used(number) {
            number = number.wrapping_sub(1);
        }
        number
    }

    /// Binds socket `id` to a number of its own if it has none.
    fn autobind(&mut self, id: u64, caller: &Caller) -> Result<(), i32> {
        if self.get(id)?.bound {
            return Ok(());
        }
        let portid = self.free_portid(caller.process);
        let socket = self.get(id)?;
        socket.portid = portid;
        socket.bound = true;
        Ok(())
    }

    /// bind(2): to `address.pid`, or to a number of the socket's own if
    /// that is 0. A socket already bound keeps its number: binding it to
    /// another is EINVAL, and to the same only changes its groups.
    pub fn bind(&mut self, id: u64, caller: &Caller, address: NlAddress) -> Result<(), i32> {
        let socket = self.get(id)?;
        let (bound, portid) = (socket.bound, socket.portid);
        if bound && address.pid != portid {
            return Err(libc::EINVAL);
        }
        if !bound {
            if address.pid != 0 && self.used(address.pid) {
                return Err(libc::EADDRINUSE);
            }
            let portid =
                if address.pid != 0 { address.pid } else { self.free_portid(caller.process) };
            let socket = self.get(id)?;
            socket.portid = portid;
            socket.bound = true;
        }
        // The word of groups 1 to 32 is the one `sockaddr_nl` can set.
        let socket = self.get(id)?;
        socket.groups = (socket.groups & !0xffff_ffff) | u64::from(address.groups);
        Ok(())
    }

    /// What `netlink_connect` and `netlink_sendmsg` make of a
    /// destination: only the kernel is there to talk to. Whoever may not
    /// change the network is refused anything else with EPERM, as Linux
    /// refuses it without CAP_NET_ADMIN; for whoever may, a number that
    /// no socket has is refused as Linux refuses it at the send. Gives
    /// whether the kernel is the destination, and not a group.
    fn check_destination(caller: &Caller, to: NlAddress) -> Result<bool, i32> {
        if (to.pid != 0 || to.groups != 0) && !caller.root {
            return Err(libc::EPERM);
        }
        if to.pid != 0 {
            return Err(libc::ECONNREFUSED);
        }
        Ok(to.groups == 0)
    }

    /// connect(2): to the kernel, which is the only peer; a socket not
    /// bound yet is bound.
    pub fn connect(&mut self, id: u64, caller: &Caller, to: NlAddress) -> Result<(), i32> {
        self.get(id)?;
        Table::check_destination(caller, to)?;
        self.autobind(id, caller)
    }

    /// getsockname(2): the socket's number and groups, both 0 until it
    /// is bound.
    pub fn name(&mut self, id: u64) -> Result<NlAddress, i32> {
        let socket = self.get(id)?;
        // The low word of the groups is what `sockaddr_nl` has room for.
        let groups = u32::try_from(socket.groups & 0xffff_ffff).unwrap_or(0);
        Ok(NlAddress { pid: socket.portid, groups })
    }

    /// The first part of a send, up to where the data is read: where it
    /// goes, binding the socket if it is not, and who its answers go to.
    pub fn prepare_send(
        &mut self,
        id: u64,
        caller: &Caller,
        destination: Option<NlAddress>,
    ) -> Result<Prepared, i32> {
        self.get(id)?;
        let delivers = match destination {
            Some(to) => Table::check_destination(caller, to)?,
            None => true,
        };
        self.autobind(id, caller)?;
        let socket = self.get(id)?;
        Ok(Prepared {
            sender: Sender {
                pid: socket.portid,
                root: caller.root,
                cap_ack: socket.has(NETLINK_CAP_ACK),
                dump_running: socket.dump.is_some(),
            },
            // A buffer is never smaller than SNDBUF_MIN.
            sndbuf: socket.sndbuf.saturating_sub(32) as usize,
            delivers,
        })
    }

    /// Queues the answers of a send on socket `id`, and starts the list
    /// one asks for.
    pub fn deliver(&mut self, id: u64, replies: Vec<Reply>) -> Result<(), i32> {
        let socket = self.get(id)?;
        for reply in replies {
            socket.apply(reply);
        }
        Ok(())
    }

    /// The next datagram of socket `id`: the queue's first, which stays
    /// there if only `peek`ed. `offered` is the length of the buffer the
    /// program receives into. Nothing queued is EAGAIN, since linuxd
    /// never waits; a datagram dropped for want of room is ENOBUFS,
    /// once.
    pub fn receive(&mut self, id: u64, offered: usize, peek: bool) -> Result<Vec<u8>, i32> {
        let socket = self.get(id)?;
        if std::mem::take(&mut socket.overrun) {
            return Err(libc::ENOBUFS);
        }
        let datagram = if peek {
            socket.queue.front().cloned()
        } else {
            let taken = socket.queue.pop_front();
            socket.queued -= taken.as_ref().map_or(0, Vec::len);
            taken
        };
        let datagram = datagram.ok_or(libc::EAGAIN)?;
        socket.max_receive = socket.max_receive.max(offered).min(DATAGRAM_MAX);
        socket.pump();
        Ok(datagram)
    }

    /// setsockopt(2) with the int `value` the call gave, if its length
    /// was that of an int.
    pub fn set_option(
        &mut self,
        id: u64,
        level: i32,
        name: i32,
        value: Option<i32>,
        caller: &Caller,
    ) -> Result<(), i32> {
        let socket = self.get(id)?;
        match level {
            // An option shorter than an int is invalid here, and reads
            // as 0 for netlink's own.
            SOL_SOCKET => socket.set_socket_option(name, value.ok_or(libc::EINVAL)?, caller.root),
            SOL_NETLINK => socket.set_netlink_option(name, value.unwrap_or(0)),
            _ => Err(libc::ENOPROTOOPT),
        }
    }

    /// getsockopt(2) with room for `room` bytes: the bytes to give. A
    /// SOL_SOCKET value is cut to the room there is; a netlink one needs
    /// room for all of an int.
    pub fn get_option(
        &mut self,
        id: u64,
        level: i32,
        name: i32,
        room: usize,
    ) -> Result<Vec<u8>, i32> {
        let socket = self.get(id)?;
        match level {
            SOL_SOCKET => {
                let value = socket.socket_value(name)?;
                Ok(value.to_ne_bytes()[..room.min(4)].to_vec())
            }
            SOL_NETLINK => {
                let value = socket.netlink_value(name)?;
                if room < 4 {
                    return Err(libc::EINVAL);
                }
                Ok(value.to_ne_bytes().to_vec())
            }
            _ => Err(libc::ENOPROTOOPT),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller(process: u64) -> Caller {
        Caller { process, root: false }
    }

    fn kernel(groups: u32) -> NlAddress {
        NlAddress { pid: 0, groups }
    }

    #[test]
    fn a_socket_binds_to_its_process_number() {
        let mut table = Table::default();
        let id = table.create(3);
        table.bind(id, &caller(4242), kernel(0)).unwrap();
        assert_eq!(table.name(id), Ok(kernel_named(4242, 0)));
    }

    fn kernel_named(pid: u32, groups: u32) -> NlAddress {
        NlAddress { pid, groups }
    }

    #[test]
    fn a_second_socket_of_a_process_takes_a_number_counting_down_from_minus_4096() {
        let mut table = Table::default();
        let first = table.create(3);
        let second = table.create(3);
        let third = table.create(3);
        for id in [first, second, third] {
            table.bind(id, &caller(100), kernel(0)).unwrap();
        }
        let pids: Vec<u32> = [first, second, third].map(|id| table.name(id).unwrap().pid).into();
        assert_eq!(pids, [100, 0xffff_f000, 0xffff_efff]);
    }

    #[test]
    fn a_number_is_free_again_when_its_socket_is_gone() {
        let mut table = Table::default();
        let first = table.create(3);
        table.bind(first, &caller(100), kernel(0)).unwrap();
        table.remove(first);
        let second = table.create(3);
        table.bind(second, &caller(100), kernel(0)).unwrap();
        assert_eq!(table.name(second).unwrap().pid, 100);
    }

    #[test]
    fn a_number_that_is_asked_for_and_taken_is_in_use() {
        let mut table = Table::default();
        let first = table.create(3);
        let second = table.create(3);
        table.bind(first, &caller(1), NlAddress { pid: 77, groups: 0 }).unwrap();
        let taken = table.bind(second, &caller(1), NlAddress { pid: 77, groups: 0 });
        assert_eq!(taken, Err(libc::EADDRINUSE));
        assert_eq!(table.name(second).unwrap().pid, 0);
    }

    #[test]
    fn a_bound_socket_keeps_its_number_and_takes_new_groups() {
        let mut table = Table::default();
        let id = table.create(3);
        table.bind(id, &caller(5), kernel(0)).unwrap();
        assert_eq!(table.bind(id, &caller(5), NlAddress { pid: 6, groups: 0 }), Err(libc::EINVAL));
        // Binding with number 0 to a socket that has one is also a
        // different number.
        assert_eq!(table.bind(id, &caller(5), kernel(0b101)), Err(libc::EINVAL));
        table.bind(id, &caller(5), NlAddress { pid: 5, groups: 0b101 }).unwrap();
        assert_eq!(table.name(id), Ok(kernel_named(5, 0b101)));
    }

    #[test]
    fn connecting_binds_and_only_the_kernel_is_there_to_connect_to() {
        let mut table = Table::default();
        let id = table.create(3);
        table.connect(id, &caller(9), kernel(0)).unwrap();
        assert_eq!(table.name(id).unwrap().pid, 9);
        let someone = NlAddress { pid: 5, groups: 0 };
        assert_eq!(table.connect(id, &caller(9), someone), Err(libc::EPERM));
        let root = Caller { process: 9, root: true };
        assert_eq!(table.connect(id, &root, someone), Err(libc::ECONNREFUSED));
        assert_eq!(table.connect(id, &root, kernel(1)), Ok(()));
    }

    #[test]
    fn buffers_are_doubled_and_limited_as_linux_does() {
        assert_eq!(buffer(65536, RCVBUF_MIN), 131_072);
        assert_eq!(buffer(1, RCVBUF_MIN), 2_304);
        assert_eq!(buffer(1, SNDBUF_MIN), 4_608);
        assert_eq!(buffer(1 << 24, RCVBUF_MIN), 425_984);
        assert_eq!(buffer(-1, RCVBUF_MIN), 425_984);
    }

    fn get(table: &mut Table, id: u64, level: i32, name: i32, room: usize) -> Result<Vec<u8>, i32> {
        table.get_option(id, level, name, room)
    }

    fn datagrams(bytes: &[u8], count: usize) -> Vec<Reply> {
        (0..count).map(|_| Reply::Datagram(bytes.to_vec())).collect()
    }

    fn number(bytes: Result<Vec<u8>, i32>) -> i32 {
        i32::from_ne_bytes(bytes.unwrap().try_into().unwrap())
    }

    #[test]
    fn socket_options_report_what_the_socket_is() {
        let mut table = Table::default();
        let id = table.create(2);
        assert_eq!(number(get(&mut table, id, SOL_SOCKET, SO_TYPE, 4)), 2);
        assert_eq!(number(get(&mut table, id, SOL_SOCKET, SO_PROTOCOL, 4)), 0);
        assert_eq!(number(get(&mut table, id, SOL_SOCKET, SO_DOMAIN, 4)), 16);
        assert_eq!(number(get(&mut table, id, SOL_SOCKET, SO_RCVBUF, 4)), 212_992);
        assert_eq!(number(get(&mut table, id, SOL_SOCKET, SO_SNDBUF, 4)), 212_992);
        assert_eq!(get(&mut table, id, SOL_SOCKET, 999, 4), Err(libc::ENOPROTOOPT));
    }

    #[test]
    fn a_socket_option_is_cut_to_the_room_it_is_given() {
        let mut table = Table::default();
        let id = table.create(3);
        assert_eq!(get(&mut table, id, SOL_SOCKET, SO_TYPE, 8), Ok(3i32.to_ne_bytes().to_vec()));
        assert_eq!(get(&mut table, id, SOL_SOCKET, SO_TYPE, 1), Ok(vec![3u8]));
        assert_eq!(get(&mut table, id, SOL_SOCKET, SO_TYPE, 0), Ok(Vec::new()));
    }

    #[test]
    fn buffer_sizes_are_set_and_forced_by_whoever_may() {
        let mut table = Table::default();
        let id = table.create(3);
        let user = caller(1);
        let root = Caller { process: 1, root: true };
        table.set_option(id, SOL_SOCKET, SO_RCVBUF, Some(1), &user).unwrap();
        assert_eq!(number(get(&mut table, id, SOL_SOCKET, SO_RCVBUF, 4)), 2_304);
        assert_eq!(
            table.set_option(id, SOL_SOCKET, SO_RCVBUFFORCE, Some(1 << 20), &user),
            Err(libc::EPERM)
        );
        table.set_option(id, SOL_SOCKET, SO_RCVBUFFORCE, Some(1 << 20), &root).unwrap();
        assert_eq!(number(get(&mut table, id, SOL_SOCKET, SO_RCVBUF, 4)), 2 << 20);
        table.set_option(id, SOL_SOCKET, SO_SNDBUFFORCE, Some(0), &root).unwrap();
        assert_eq!(number(get(&mut table, id, SOL_SOCKET, SO_SNDBUF, 4)), 4_608);
    }

    #[test]
    fn an_int_option_with_no_int_is_invalid_for_a_socket_and_off_for_netlink() {
        let mut table = Table::default();
        let id = table.create(3);
        let user = caller(1);
        assert_eq!(table.set_option(id, SOL_SOCKET, SO_PASSCRED, None, &user), Err(libc::EINVAL));
        assert_eq!(table.set_option(id, SOL_SOCKET, 999, None, &user), Err(libc::EINVAL));
        assert_eq!(table.set_option(id, SOL_NETLINK, NETLINK_EXT_ACK, None, &user), Ok(()));
        assert_eq!(number(get(&mut table, id, SOL_NETLINK, NETLINK_EXT_ACK, 4)), 0);
    }

    #[test]
    fn netlink_flags_are_remembered_and_the_rest_are_not_options() {
        let mut table = Table::default();
        let id = table.create(3);
        let user = caller(1);
        for name in NETLINK_FLAGS {
            assert_eq!(number(get(&mut table, id, SOL_NETLINK, name, 4)), 0);
            table.set_option(id, SOL_NETLINK, name, Some(1), &user).unwrap();
            assert_eq!(number(get(&mut table, id, SOL_NETLINK, name, 4)), 1);
            table.set_option(id, SOL_NETLINK, name, Some(0), &user).unwrap();
            assert_eq!(number(get(&mut table, id, SOL_NETLINK, name, 4)), 0);
        }
        assert_eq!(table.set_option(id, SOL_NETLINK, 8, Some(1), &user), Err(libc::ENOPROTOOPT));
        assert_eq!(get(&mut table, id, SOL_NETLINK, 8, 4), Err(libc::ENOPROTOOPT));
        assert_eq!(get(&mut table, id, SOL_NETLINK, NETLINK_PKTINFO, 3), Err(libc::EINVAL));
        assert_eq!(table.set_option(id, 0, 1, Some(1), &user), Err(libc::ENOPROTOOPT));
        assert_eq!(get(&mut table, id, 6, 1, 4), Err(libc::ENOPROTOOPT));
    }

    #[test]
    fn a_membership_is_a_group_from_one_to_the_last() {
        let mut table = Table::default();
        let id = table.create(3);
        let user = caller(1);
        table.bind(id, &user, kernel(0)).unwrap();
        for group in [0, GROUPS + 1, -1] {
            let added =
                table.set_option(id, SOL_NETLINK, NETLINK_ADD_MEMBERSHIP, Some(group), &user);
            assert_eq!(added, Err(libc::EINVAL), "group {group}");
        }
        table.set_option(id, SOL_NETLINK, NETLINK_ADD_MEMBERSHIP, Some(1), &user).unwrap();
        table.set_option(id, SOL_NETLINK, NETLINK_ADD_MEMBERSHIP, Some(3), &user).unwrap();
        // Group 36 is past the 32 `getsockname` can report.
        table.set_option(id, SOL_NETLINK, NETLINK_ADD_MEMBERSHIP, Some(GROUPS), &user).unwrap();
        assert_eq!(table.name(id).unwrap().groups, 0b101);
        table.set_option(id, SOL_NETLINK, NETLINK_DROP_MEMBERSHIP, Some(1), &user).unwrap();
        assert_eq!(table.name(id).unwrap().groups, 0b100);
    }

    #[test]
    fn options_every_socket_takes_are_taken_and_read_as_zero() {
        let mut table = Table::default();
        let id = table.create(3);
        for name in IGNORED {
            table.set_option(id, SOL_SOCKET, name, Some(1), &caller(1)).unwrap();
            assert_eq!(number(get(&mut table, id, SOL_SOCKET, name, 4)), 0);
        }
    }

    #[test]
    fn what_cannot_be_queued_is_dropped_and_the_next_receive_fails_once() {
        let mut table = Table::default();
        let id = table.create(3);
        let user = caller(1);
        table.set_option(id, SOL_SOCKET, SO_RCVBUF, Some(1), &user).unwrap();
        let datagram = vec![7u8; 1_500];
        // The queue takes datagrams while it is not over the buffer.
        table.deliver(id, datagrams(&datagram, 3)).unwrap();
        assert_eq!(table.get(id).unwrap().queue.len(), 2);
        assert_eq!(table.receive(id, 4096, false), Err(libc::ENOBUFS));
        assert_eq!(table.receive(id, 4096, false), Ok(datagram.clone()));
        assert_eq!(number(get(&mut table, id, SOL_SOCKET, SO_ERROR, 4)), 0);
    }

    #[test]
    fn a_socket_that_wants_no_enobufs_loses_the_answer_without_an_error() {
        let mut table = Table::default();
        let id = table.create(3);
        let user = caller(1);
        table.set_option(id, SOL_SOCKET, SO_RCVBUF, Some(1), &user).unwrap();
        table.set_option(id, SOL_NETLINK, NETLINK_NO_ENOBUFS, Some(1), &user).unwrap();
        table.deliver(id, datagrams(&[7u8; 1_500], 3)).unwrap();
        assert_eq!(table.receive(id, 4096, false).map(|datagram| datagram.len()), Ok(1_500));
    }

    #[test]
    fn the_error_of_a_drop_is_read_and_cleared_by_so_error() {
        let mut table = Table::default();
        let id = table.create(3);
        let user = caller(1);
        table.set_option(id, SOL_SOCKET, SO_RCVBUF, Some(1), &user).unwrap();
        table.deliver(id, datagrams(&[7u8; 1_500], 3)).unwrap();
        assert_eq!(number(get(&mut table, id, SOL_SOCKET, SO_ERROR, 4)), libc::ENOBUFS);
        assert_eq!(number(get(&mut table, id, SOL_SOCKET, SO_ERROR, 4)), 0);
    }

    #[test]
    fn an_empty_queue_is_eagain_and_a_peek_leaves_the_datagram() {
        let mut table = Table::default();
        let id = table.create(3);
        assert_eq!(table.receive(id, 100, false), Err(libc::EAGAIN));
        table.deliver(id, vec![Reply::Datagram(vec![1, 2, 3])]).unwrap();
        assert_eq!(table.receive(id, 100, true), Ok(vec![1, 2, 3]));
        assert_eq!(table.receive(id, 100, false), Ok(vec![1, 2, 3]));
        assert_eq!(table.receive(id, 100, false), Err(libc::EAGAIN));
        assert_eq!(table.get(id).unwrap().queued, 0);
    }

    #[test]
    fn a_socket_that_is_not_there_is_not_a_socket() {
        let mut table = Table::default();
        let id = table.create(3);
        assert!(table.contains(id));
        table.remove(id);
        assert!(!table.contains(id));
        assert_eq!(table.name(id), Err(libc::ENOTSOCK));
        assert_eq!(table.receive(id, 1, false), Err(libc::ENOTSOCK));
        assert!(table.ids().is_empty());
    }
}
