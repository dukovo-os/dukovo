// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The socket calls of the programs that read the network through
//! netlink, run against a process in memory (`Fake`) and a fixed
//! machine: the calls glibc's getifaddrs makes, Go's
//! `syscall.NetlinkRIB`, and the cases around them that Linux answers
//! in its own way.

use std::collections::BTreeMap;
use std::fs::File;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::{Arc, Mutex};

use radix_abi::capability::ObjectKind;

use super::machine::{Fixed, Link, Stats};
use super::wire::{self, HEADER};
use super::{Netlinks, SocketFiles, id_of, node_of};
use crate::call::{Answer, Fake, Process, Who};
use crate::serve::nr;
use crate::sys::slot_of;

/// The process that makes the calls, and the user it acts for.
const PROCESS: u64 = 4242;
const USER: u32 = 1000;

/// Where the tests keep the program's data in its memory.
const BASE: u64 = 0x10_000;
const ADDRESS: u64 = BASE;
const LENGTH: u64 = BASE + 0x40;
const MSGHDR: u64 = BASE + 0x80;
const IOVECS: u64 = BASE + 0x100;
const VALUE: u64 = BASE + 0x200;
const REQUEST: u64 = BASE + 0x1000;
const REPLY: u64 = BASE + 0x2000;
const MEMORY: usize = 0x2_2000;

const AF_NETLINK: u64 = 16;
const SOCK_RAW: u64 = 3;
const SOCK_DGRAM: u64 = 2;
const SOCK_CLOEXEC: u64 = 0o2000000;
const SOCK_NONBLOCK: u64 = 0o4000;
const MSG_PEEK: u64 = 0x2;
const MSG_TRUNC: u64 = 0x20;
const MSG_DONTWAIT: u64 = 0x40;
const SOL_SOCKET: u64 = 1;
const SOL_NETLINK: u64 = 270;
const SO_RCVBUF: u64 = 8;
const SO_SNDBUF: u64 = 7;
const SO_PASSCRED: u64 = 16;
const SO_TYPE: u64 = 3;
const NETLINK_ADD_MEMBERSHIP: u64 = 1;
const NETLINK_EXT_ACK: u64 = 11;

const NLM_F_REQUEST: u16 = 1;
const NLM_F_ACK: u16 = 4;
const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const RTM_NEWLINK: u16 = 16;
const RTM_GETLINK: u16 = 18;
const RTM_NEWADDR: u16 = 20;
const RTM_GETADDR: u16 = 22;
const RTM_NEWROUTE: u16 = 24;
const RTM_GETROUTE: u16 = 26;

/// The descriptors' slots that linuxd opened the sockets' files in, and
/// the socket each holds.
type Opened = Arc<Mutex<BTreeMap<u32, u64>>>;

/// Files of `/dev/null`, which stand for the sockets' files: all a test
/// needs of them is a descriptor to name by its slot.
struct Files(Opened);

impl SocketFiles for Files {
    fn open(&self, id: u64) -> Result<OwnedFd, i32> {
        let file = File::open("/dev/null").map_err(|error| error.raw_os_error().unwrap_or(0))?;
        let fd = OwnedFd::from(file);
        self.0.lock().unwrap().insert(slot_of(fd.as_raw_fd()), id);
        Ok(fd)
    }
}

/// A process in memory that is given the sockets' files as descriptors.
struct Program {
    memory: Fake,
    opened: Opened,
    /// The error `give` answers with, for a program that has no room.
    full: Option<i32>,
}

impl Process for Program {
    fn who(&self) -> Who {
        self.memory.who()
    }

    fn read(&mut self, address: u64, buffer: &mut [u8]) -> Result<(), i32> {
        self.memory.read(address, buffer)
    }

    fn write(&mut self, address: u64, bytes: &[u8]) -> Result<(), i32> {
        self.memory.write(address, bytes)
    }

    fn read_string(&mut self, address: u64, max: usize) -> Result<Vec<u8>, i32> {
        self.memory.read_string(address, max)
    }

    fn give(&mut self, slot: u32, _cloexec: bool) -> Result<i32, i32> {
        if let Some(errno) = self.full {
            return Err(errno);
        }
        let id = *self.opened.lock().unwrap().get(&slot).ok_or(libc::EBADF)?;
        let fd = (0..).find(|fd| !self.memory.descriptors.contains_key(fd)).ok_or(libc::EMFILE)?;
        self.memory.descriptors.insert(fd, (ObjectKind::File, Some(node_of(id))));
        Ok(fd)
    }

    fn take(&mut self, fd: i32, slot: u32) -> Result<ObjectKind, i32> {
        self.memory.take(fd, slot)
    }

    fn node_of(&mut self, fd: i32) -> Result<u64, i32> {
        self.memory.node_of(fd)
    }
}

/// What a receive gave the program.
struct Received {
    /// The call's result.
    value: i64,
    /// The bytes in the buffer.
    bytes: Vec<u8>,
    /// `msg_flags`, for recvmsg.
    flags: i32,
    /// `msg_namelen` and the address written, for recvmsg.
    name: Option<(u32, Vec<u8>)>,
}

/// A program, the sockets it makes and the machine they read.
struct Session {
    netlinks: Netlinks,
    program: Program,
}

impl Session {
    fn new(machine: Fixed) -> Session {
        Session::as_user(machine, USER)
    }

    fn as_user(machine: Fixed, user: u32) -> Session {
        let opened = Opened::default();
        let netlinks = Netlinks::with_machine(Box::new(Files(opened.clone())), Box::new(machine));
        let who = Who { process: PROCESS, thread: PROCESS, user, group: user };
        let program = Program { memory: Fake::new(who, BASE, MEMORY), opened, full: None };
        Session { netlinks, program }
    }

    fn call(&mut self, number: u64, args: [u64; 6]) -> Answer {
        self.netlinks.call(&mut self.program, number, &args)
    }

    fn put(&mut self, address: u64, bytes: &[u8]) {
        self.program.write(address, bytes).unwrap();
    }

    fn get(&mut self, address: u64, length: usize) -> Vec<u8> {
        let mut bytes = vec![0; length];
        self.program.read(address, &mut bytes).unwrap();
        bytes
    }

    /// socket(AF_NETLINK, kind, protocol): the descriptor it makes.
    fn open(&mut self, kind: u64, protocol: u64) -> Result<i32, i32> {
        let args = [AF_NETLINK, kind, protocol, 0, 0, 0];
        self.netlinks.socket(&mut self.program, &args).map(|fd| fd as i32)
    }

    /// A socket like glibc's and Go's: SOCK_RAW, close on exec.
    fn route_socket(&mut self) -> i32 {
        self.open(SOCK_RAW | SOCK_CLOEXEC, 0).unwrap()
    }

    fn bind(&mut self, fd: i32, pid: u32, groups: u32) -> Answer {
        self.put(ADDRESS, &sockaddr(pid, groups));
        self.call(nr::BIND, [fd as u64, ADDRESS, 12, 0, 0, 0])
    }

    /// getsockname(2) or getpeername(2) into a buffer of `room` bytes:
    /// the address as written, and the length it says.
    fn name(&mut self, number: u64, fd: i32, room: u32) -> Result<(Vec<u8>, u32), i32> {
        self.put(ADDRESS, &[0xaa; 16]);
        self.put(LENGTH, &room.to_ne_bytes());
        self.call(number, [fd as u64, ADDRESS, LENGTH, 0, 0, 0])?;
        let told = u32::from_ne_bytes(self.get(LENGTH, 4).try_into().unwrap());
        let written = (told.min(room) as usize).min(12);
        Ok((self.get(ADDRESS, written), told))
    }

    /// The pid and groups `getsockname` gives.
    fn own_address(&mut self, fd: i32) -> (u32, u32) {
        let (bytes, told) = self.name(nr::GETSOCKNAME, fd, 12).unwrap();
        assert_eq!(told, 12);
        assert_eq!(u16::from_ne_bytes(bytes[..2].try_into().unwrap()), 16);
        let word = |at: usize| u32::from_ne_bytes(bytes[at..at + 4].try_into().unwrap());
        (word(4), word(8))
    }

    /// sendto(2) of `request` to the kernel.
    fn send(&mut self, fd: i32, request: &[u8]) -> Answer {
        self.put(REQUEST, request);
        self.put(ADDRESS, &sockaddr(0, 0));
        let length = request.len() as u64;
        self.call(nr::SENDTO, [fd as u64, REQUEST, length, 0, ADDRESS, 12])
    }

    /// recvfrom(2) with no address, into a buffer of `size` bytes.
    fn receive(&mut self, fd: i32, size: usize, flags: u64) -> Result<Received, i32> {
        self.put(REPLY, &vec![0xee; size]);
        let value = self.call(nr::RECVFROM, [fd as u64, REPLY, size as u64, flags, 0, 0])?;
        let got = usize::try_from(value).unwrap().min(size);
        Ok(Received { value, bytes: self.get(REPLY, got), flags: 0, name: None })
    }

    /// recvmsg(2) into one buffer of `size` bytes, with room for an
    /// address of `name_room` bytes.
    fn receive_message(
        &mut self,
        fd: i32,
        size: usize,
        flags: u64,
        name_room: u32,
    ) -> Result<Received, i32> {
        self.put(REPLY, &vec![0xee; size]);
        self.put(IOVECS, &iovec(REPLY, size));
        self.put(ADDRESS, &[0xaa; 16]);
        let mut header = [0u8; 56];
        header[0..8].copy_from_slice(&ADDRESS.to_ne_bytes());
        header[8..12].copy_from_slice(&name_room.to_ne_bytes());
        header[16..24].copy_from_slice(&IOVECS.to_ne_bytes());
        header[24..32].copy_from_slice(&1u64.to_ne_bytes());
        header[48..52].copy_from_slice(&0x7777i32.to_ne_bytes());
        self.put(MSGHDR, &header);
        let value = self.call(nr::RECVMSG, [fd as u64, MSGHDR, flags, 0, 0, 0])?;
        let header = self.get(MSGHDR, 56);
        let got = usize::try_from(value).unwrap().min(size);
        let told = u32::from_ne_bytes(header[8..12].try_into().unwrap());
        let written = (told.min(name_room) as usize).min(12);
        Ok(Received {
            value,
            bytes: self.get(REPLY, got),
            flags: i32::from_ne_bytes(header[48..52].try_into().unwrap()),
            name: Some((told, self.get(ADDRESS, written))),
        })
    }

    /// Receives datagrams, each into a buffer of `size` bytes, until the
    /// list that was asked for is done: every message of it, in order.
    fn list(&mut self, fd: i32, size: usize) -> Vec<Vec<u8>> {
        let mut messages = Vec::new();
        loop {
            let datagram = self.receive(fd, size, 0).unwrap();
            assert!(datagram.value as usize <= size, "a datagram that fits");
            let mut done = false;
            for message in wire::messages(&datagram.bytes) {
                if message.kind == NLMSG_DONE {
                    done = true;
                } else {
                    messages.push(raw(&datagram.bytes, message.payload));
                }
            }
            if done {
                return messages;
            }
        }
    }
}

/// The message whose payload is `payload`, inside `datagram`, whole.
fn raw(datagram: &[u8], payload: &[u8]) -> Vec<u8> {
    let start = payload.as_ptr() as usize - datagram.as_ptr() as usize - HEADER;
    let len = u32::from_ne_bytes(datagram[start..start + 4].try_into().unwrap()) as usize;
    datagram[start..start + len].to_vec()
}

fn sockaddr(pid: u32, groups: u32) -> [u8; 12] {
    let mut bytes = [0u8; 12];
    bytes[..2].copy_from_slice(&16u16.to_ne_bytes());
    bytes[4..8].copy_from_slice(&pid.to_ne_bytes());
    bytes[8..12].copy_from_slice(&groups.to_ne_bytes());
    bytes
}

fn iovec(base: u64, length: usize) -> Vec<u8> {
    let mut bytes = base.to_ne_bytes().to_vec();
    bytes.extend_from_slice(&(length as u64).to_ne_bytes());
    bytes
}

/// A request datagram.
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

/// What glibc's getifaddrs sends: the header and a `struct rtgenmsg`
/// that a structure's padding makes four bytes, asking for a dump.
fn glibc_request(kind: u16, seq: u32) -> Vec<u8> {
    request(kind, 0x301, seq, &[0, 0, 0, 0])
}

/// What Go's `syscall.NetlinkRIB` sends: the header and the one byte of
/// `rtgenmsg`, and no padding.
fn go_request(kind: u16, seq: u32, family: u8) -> Vec<u8> {
    request(kind, 0x301, seq, &[family])
}

fn type_of(message: &[u8]) -> u16 {
    u16::from_ne_bytes(message[4..6].try_into().unwrap())
}

fn flags_of(message: &[u8]) -> u16 {
    u16::from_ne_bytes(message[6..8].try_into().unwrap())
}

fn seq_of(message: &[u8]) -> u32 {
    u32::from_ne_bytes(message[8..12].try_into().unwrap())
}

fn pid_of(message: &[u8]) -> u32 {
    u32::from_ne_bytes(message[12..16].try_into().unwrap())
}

/// The error code of an NLMSG_ERROR.
fn error_of(message: &[u8]) -> i32 {
    assert_eq!(type_of(message), NLMSG_ERROR);
    i32::from_ne_bytes(message[16..20].try_into().unwrap())
}

/// The name in an RTM_NEWLINK.
fn link_name(message: &[u8]) -> String {
    let attributes = wire::attributes(&message[HEADER + 16..]);
    let (_, name) = attributes.into_iter().find(|&(kind, _)| kind == 3).unwrap();
    String::from_utf8(name[..name.len() - 1].to_vec()).unwrap()
}

/// The QEMU machine with `count` more interfaces, named `veth0`, `veth1`...
fn crowded(count: usize) -> Fixed {
    let mut machine = Fixed::qemu();
    for number in 0..count {
        machine.links.push(Link {
            index: 3 + number as u32,
            name: format!("veth{number}"),
            kind: 1,
            flags: 0x1003,
            mtu: 1500,
            address: vec![2, 0, 0, 0, 0, number as u8],
            broadcast: vec![0xff; 6],
            operstate: 6,
            stats: Stats::default(),
        });
    }
    machine
}

#[test]
fn glibcs_getifaddrs_reads_the_interfaces_and_then_the_addresses() {
    let mut session = Session::new(crowded(30));
    // socket, bind, getsockname.
    let fd = session.route_socket();
    assert_eq!(fd, 0);
    assert_eq!(session.bind(fd, 0, 0), Ok(0));
    let (pid, groups) = session.own_address(fd);
    assert_eq!((pid, groups), (PROCESS as u32, 0));
    // The link request, and its answers in datagrams a 4 KiB buffer
    // holds, as glibc reads them.
    assert_eq!(session.send(fd, &glibc_request(RTM_GETLINK, 1000)), Ok(20));
    let links = session.list(fd, 4096);
    assert_eq!(links.len(), 32);
    let names: Vec<String> = links.iter().map(|message| link_name(message)).collect();
    assert_eq!(names[..3], ["lo", "net0", "veth0"]);
    assert_eq!(names[31], "veth29");
    for message in &links {
        assert_eq!(type_of(message), RTM_NEWLINK);
        assert_eq!((seq_of(message), pid_of(message)), (1000, pid));
        assert_eq!(flags_of(message), 2);
    }
    // Then the addresses, on the same socket.
    assert_eq!(session.send(fd, &glibc_request(RTM_GETADDR, 1001)), Ok(20));
    let addresses = session.list(fd, 4096);
    assert_eq!(addresses.len(), 5);
    for message in &addresses {
        assert_eq!(type_of(message), RTM_NEWADDR);
        assert_eq!((seq_of(message), pid_of(message)), (1001, pid));
    }
}

#[test]
fn glibcs_getifaddrs_reads_the_addresses_of_both_families() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    session.bind(fd, 0, 0).unwrap();
    session.send(fd, &glibc_request(RTM_GETADDR, 7)).unwrap();
    // glibc first asks how long the datagram is, with a zero-length
    // buffer, and then reads it.
    let peeked = session.receive_message(fd, 0, MSG_PEEK | MSG_TRUNC, 12).unwrap();
    assert!(peeked.value > 0);
    assert_eq!(peeked.flags, MSG_TRUNC as i32);
    let datagram = session.receive_message(fd, peeked.value as usize, 0, 12).unwrap();
    assert_eq!(datagram.value, peeked.value);
    assert_eq!(datagram.flags, 0);
    assert_eq!(datagram.name, Some((12, sockaddr(0, 0).to_vec())));
    let messages: Vec<_> = wire::messages(&datagram.bytes).collect();
    assert_eq!(messages.len(), 5);
    assert!(messages.iter().all(|message| message.kind == RTM_NEWADDR));
    // IPv4 first, then IPv6, as Linux lists its families.
    let families: Vec<u8> = messages.iter().map(|message| message.payload[0]).collect();
    assert_eq!(families, [2, 2, 10, 10, 10]);
    let done = session.receive_message(fd, 4096, 0, 12).unwrap();
    assert_eq!(wire::messages(&done.bytes).next().unwrap().kind, NLMSG_DONE);
    assert_eq!(session.receive(fd, 4096, 0).err(), Some(libc::EAGAIN));
}

#[test]
fn gos_netlinkrib_reads_the_routes_in_pages() {
    let mut machine = Fixed::qemu();
    // Enough routes that the list takes several 4 KiB reads.
    let template = machine.routes[0].clone();
    for _ in 0..200 {
        machine.routes.push(template.clone());
    }
    let mut session = Session::new(machine);
    // socket, bind of an empty address, sendto, getsockname.
    let fd = session.open(SOCK_RAW | SOCK_CLOEXEC, 0).unwrap();
    session.put(ADDRESS, &sockaddr(0, 0));
    assert_eq!(session.call(nr::BIND, [fd as u64, ADDRESS, 12, 0, 0, 0]), Ok(0));
    assert_eq!(session.send(fd, &go_request(RTM_GETROUTE, 1, 2)), Ok(17));
    let (pid, _) = session.own_address(fd);
    // recvfrom into 4096 bytes, with no flags and no address, until DONE.
    let mut datagrams = 0;
    let mut routes = 0;
    loop {
        let datagram = session.receive(fd, 4096, 0).unwrap();
        assert!(datagram.value >= 16, "Go wants a whole header");
        assert!(datagram.value <= 4096, "and a datagram that its buffer holds");
        datagrams += 1;
        let mut done = false;
        for message in wire::messages(&datagram.bytes) {
            // Go checks each message's sender and sequence number.
            assert_eq!((message.pid, message.seq), (pid, 1));
            match message.kind {
                RTM_NEWROUTE => routes += 1,
                NLMSG_DONE => done = true,
                other => panic!("type {other}"),
            }
        }
        if done {
            break;
        }
    }
    assert_eq!(routes, 202);
    // Each route is 60 bytes, so 202 of them need four datagrams, and
    // the NLMSG_DONE one more.
    assert!(datagrams > 3, "{datagrams} datagrams");
}

#[test]
fn a_list_is_read_whole_through_a_buffer_too_small_to_queue_it() {
    let mut session = Session::new(crowded(150));
    let fd = session.route_socket();
    session.bind(fd, 0, 0).unwrap();
    // The smallest buffer Linux gives holds under two datagrams.
    session.put(VALUE, &1i32.to_ne_bytes());
    let args = [fd as u64, SOL_SOCKET, SO_RCVBUF, VALUE, 4, 0];
    assert_eq!(session.call(nr::SETSOCKOPT, args), Ok(0));
    session.send(fd, &glibc_request(RTM_GETLINK, 5)).unwrap();
    let links = session.list(fd, 4096);
    assert_eq!(links.len(), 152);
    let numbers: Vec<u32> = links
        .iter()
        .map(|message| u32::from_ne_bytes(message[HEADER + 4..HEADER + 8].try_into().unwrap()))
        .collect();
    assert_eq!(numbers, (1..=152).collect::<Vec<u32>>());
}

#[test]
fn a_datagram_longer_than_the_buffer_is_cut_and_gone() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    session.send(fd, &glibc_request(RTM_GETLINK, 1)).unwrap();
    let cut = session.receive(fd, 16, 0).unwrap();
    assert_eq!(cut.value, 16);
    assert_eq!(cut.bytes.len(), 16);
    // What was cut is not kept: the next datagram is the DONE.
    let next = session.receive(fd, 4096, 0).unwrap();
    assert_eq!(next.value, 20);
    assert_eq!(wire::messages(&next.bytes).next().unwrap().kind, NLMSG_DONE);
}

#[test]
fn msg_trunc_gives_the_whole_length_and_recvmsg_says_it_was_cut() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    session.send(fd, &glibc_request(RTM_GETLINK, 1)).unwrap();
    let whole = session.receive(fd, 16, MSG_PEEK | MSG_TRUNC).unwrap();
    assert!(whole.value > 700, "{}", whole.value);
    assert_eq!(whole.bytes.len(), 16);
    let cut = session.receive_message(fd, 64, 0, 12).unwrap();
    assert_eq!(cut.value, 64);
    assert_eq!(cut.flags, MSG_TRUNC as i32);
    let fitting = session.receive_message(fd, 4096, 0, 12).unwrap();
    assert_eq!(fitting.value, 20);
    assert_eq!(fitting.flags, 0);
}

#[test]
fn a_peek_leaves_the_datagram_where_it_is() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    session.send(fd, &glibc_request(RTM_GETLINK, 1)).unwrap();
    let first = session.receive(fd, 4096, MSG_PEEK).unwrap();
    let second = session.receive(fd, 4096, MSG_PEEK).unwrap();
    let taken = session.receive(fd, 4096, 0).unwrap();
    assert_eq!(first.bytes, second.bytes);
    assert_eq!(first.bytes, taken.bytes);
    let done = session.receive(fd, 4096, MSG_DONTWAIT).unwrap();
    assert_eq!(done.value, 20);
    assert_eq!(session.receive(fd, 4096, MSG_DONTWAIT).err(), Some(libc::EAGAIN));
}

#[test]
fn nothing_queued_is_eagain_even_for_a_socket_that_may_wait() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.open(SOCK_DGRAM, 0).unwrap();
    assert_eq!(session.receive(fd, 4096, 0).err(), Some(libc::EAGAIN));
    assert_eq!(session.receive_message(fd, 4096, 0, 12).err(), Some(libc::EAGAIN));
}

#[test]
fn a_request_to_change_the_network_is_answered_with_an_error() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    session.bind(fd, 0, 0).unwrap();
    let bytes = request(RTM_NEWADDR, NLM_F_REQUEST | NLM_F_ACK, 9, &[2, 24, 0, 0, 2, 0, 0, 0]);
    session.send(fd, &bytes).unwrap();
    let answer = session.receive(fd, 4096, 0).unwrap().bytes;
    assert_eq!(error_of(&answer), -libc::EPERM);
    assert_eq!((seq_of(&answer), pid_of(&answer)), (9, PROCESS as u32));
    // The request is repeated after the error.
    assert_eq!(answer[20..], bytes[..]);
    assert_eq!(session.receive(fd, 4096, 0).err(), Some(libc::EAGAIN));

    let mut root = Session::as_user(Fixed::qemu(), 0);
    let fd = root.route_socket();
    root.send(fd, &bytes).unwrap();
    assert_eq!(error_of(&root.receive(fd, 4096, 0).unwrap().bytes), -libc::EOPNOTSUPP);
}

#[test]
fn a_request_that_is_not_served_gets_its_error_even_in_a_datagram_of_several() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    // A neighbour dump, then an interface dump, in one datagram.
    let mut bytes = request(30, 0x301, 1, &[0; 12]);
    bytes.extend_from_slice(&glibc_request(RTM_GETLINK, 2));
    session.send(fd, &bytes).unwrap();
    let first = session.receive(fd, 4096, 0).unwrap().bytes;
    assert_eq!((error_of(&first), seq_of(&first)), (-libc::EOPNOTSUPP, 1));
    let second = session.receive(fd, 4096, 0).unwrap().bytes;
    assert_eq!((type_of(&second), seq_of(&second)), (RTM_NEWLINK, 2));
}

#[test]
fn one_interface_is_asked_for_by_name_and_by_number() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    let mut body = vec![0u8; 16];
    body[4..8].copy_from_slice(&2i32.to_ne_bytes());
    session.send(fd, &request(RTM_GETLINK, NLM_F_REQUEST | NLM_F_ACK, 3, &body)).unwrap();
    let link = session.receive(fd, 4096, 0).unwrap().bytes;
    assert_eq!(
        (type_of(&link), flags_of(&link), link_name(&link)),
        (RTM_NEWLINK, 0, "net0".into())
    );
    // And the acknowledgement it asked for, after the answer.
    let ack = session.receive(fd, 4096, 0).unwrap().bytes;
    assert_eq!((error_of(&ack), seq_of(&ack)), (0, 3));

    let mut by_name = vec![0u8; 16];
    by_name.extend_from_slice(&[7, 0, 3, 0, b'l', b'o', 0, 0]);
    session.send(fd, &request(RTM_GETLINK, NLM_F_REQUEST, 4, &by_name)).unwrap();
    let link = session.receive(fd, 4096, 0).unwrap().bytes;
    assert_eq!(link_name(&link), "lo");

    body[4..8].copy_from_slice(&99i32.to_ne_bytes());
    session.send(fd, &request(RTM_GETLINK, NLM_F_REQUEST, 5, &body)).unwrap();
    let missing = session.receive(fd, 4096, 0).unwrap().bytes;
    assert_eq!(error_of(&missing), -libc::ENODEV);
}

#[test]
fn a_second_list_while_one_is_running_is_busy_and_then_it_is_not() {
    let mut session = Session::new(crowded(5));
    let fd = session.route_socket();
    session.send(fd, &glibc_request(RTM_GETLINK, 1)).unwrap();
    session.send(fd, &glibc_request(RTM_GETADDR, 2)).unwrap();
    // As on Linux, a list ends only when its last datagram is read, so
    // the second request finds the first still running.
    let mut seen = Vec::new();
    while let Ok(datagram) = session.receive(fd, 4096, 0) {
        let kinds: Vec<(u16, u32)> =
            wire::messages(&datagram.bytes).map(|message| (message.kind, message.seq)).collect();
        seen.push(kinds);
    }
    assert_eq!(seen.len(), 3);
    assert_eq!(seen[0], [(RTM_NEWLINK, 1); 7]);
    assert_eq!(seen[1], [(NLMSG_ERROR, 2)]);
    assert_eq!(seen[2], [(NLMSG_DONE, 1)]);
    // With the first list read to its end, a list is served again.
    session.send(fd, &glibc_request(RTM_GETADDR, 3)).unwrap();
    assert_eq!(session.list(fd, 4096).len(), 5);
}

#[test]
fn sockets_that_are_not_netlink_ones_are_not_sockets() {
    let mut session = Session::new(Fixed::qemu());
    // A file of some other filesystem, a descriptor that is not a file,
    // and one that is not open.
    session.program.memory.descriptors.insert(5, (ObjectKind::File, Some(77)));
    session.program.memory.descriptors.insert(6, (ObjectKind::Endpoint, None));
    for fd in [5u64, 6] {
        let args = [fd, ADDRESS, 12, 0, 0, 0];
        assert_eq!(session.call(nr::BIND, args), Err(libc::ENOTSOCK), "fd {fd}");
        assert_eq!(session.call(nr::RECVFROM, args), Err(libc::ENOTSOCK));
        assert_eq!(session.call(nr::SHUTDOWN, args), Err(libc::ENOTSOCK));
    }
    assert_eq!(session.call(nr::BIND, [9, ADDRESS, 12, 0, 0, 0]), Err(libc::EBADF));
    assert_eq!(session.call(nr::BIND, [u64::MAX, ADDRESS, 12, 0, 0, 0]), Err(libc::EBADF));
    // A file in the sockets' range of node numbers that no socket has.
    session.program.memory.descriptors.insert(7, (ObjectKind::File, Some(node_of(99))));
    assert_eq!(session.call(nr::BIND, [7, ADDRESS, 12, 0, 0, 0]), Err(libc::ENOTSOCK));
}

#[test]
fn a_socket_is_made_for_the_types_and_the_protocol_netlink_has() {
    let mut session = Session::new(Fixed::qemu());
    assert_eq!(session.open(SOCK_RAW, 0), Ok(0));
    assert_eq!(session.open(SOCK_DGRAM, 0), Ok(1));
    assert_eq!(session.open(SOCK_RAW | SOCK_CLOEXEC | SOCK_NONBLOCK, 0), Ok(2));
    // SOCK_STREAM is no type of netlink's, and NETLINK_KOBJECT_UEVENT is
    // a protocol Dukovo has no events for.
    assert_eq!(session.open(1, 0), Err(libc::ESOCKTNOSUPPORT));
    assert_eq!(session.open(5, 0), Err(libc::ESOCKTNOSUPPORT));
    assert_eq!(session.open(SOCK_RAW, 15), Err(libc::EPROTONOSUPPORT));
    assert_eq!(session.open(SOCK_RAW, 0x1_0000_0001), Err(libc::EPROTONOSUPPORT));
    assert_eq!(session.open(SOCK_RAW, u64::from(u32::MAX)), Err(libc::EPROTONOSUPPORT));
    // A flag that is none of Linux's is invalid, before the type is.
    assert_eq!(session.open(SOCK_RAW | 0x10, 0), Err(libc::EINVAL));
    assert_eq!(session.open(1 | 0x1000_0000, 0), Err(libc::EINVAL));
    // The sockets that were refused have left no socket behind.
    assert_eq!(session.netlinks.ids().len(), 3);
}

#[test]
fn the_descriptor_is_the_sockets_file_and_the_file_is_the_socket() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    let node = session.program.memory.descriptors[&fd].1.unwrap();
    assert_eq!(id_of(node), Some(1));
    assert_eq!(node_of(1), node);
    assert!(session.netlinks.exists(1));
    assert_eq!(session.netlinks.ids(), [1]);
    session.netlinks.release(1);
    assert!(!session.netlinks.exists(1));
    assert!(session.netlinks.ids().is_empty());
    assert_eq!(id_of(5), None);
}

#[test]
fn a_socket_whose_file_cannot_be_opened_is_not_made() {
    let netlinks = Netlinks::with_machine(Box::new(super::NoFiles), Box::new(Fixed::qemu()));
    let who = Who { process: PROCESS, thread: PROCESS, user: USER, group: USER };
    let mut program =
        Program { memory: Fake::new(who, BASE, MEMORY), opened: Opened::default(), full: None };
    let args = [AF_NETLINK, SOCK_RAW, 0, 0, 0, 0];
    assert_eq!(netlinks.socket(&mut program, &args), Err(libc::ENFILE));
    assert!(netlinks.ids().is_empty());
}

#[test]
fn a_socket_the_program_cannot_be_given_is_not_made() {
    let mut session = Session::new(Fixed::qemu());
    session.program.full = Some(libc::EMFILE);
    assert_eq!(session.open(SOCK_RAW, 0), Err(libc::EMFILE));
    assert!(session.netlinks.ids().is_empty());
    // The next socket is made once there is room, and is not the one
    // that was refused.
    session.program.full = None;
    assert_eq!(session.open(SOCK_RAW, 0), Ok(0));
    assert_eq!(session.netlinks.ids().len(), 1);
}

#[test]
fn bind_reports_what_getsockname_gives() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    assert_eq!(session.own_address(fd), (0, 0));
    session.bind(fd, 0, 0b11).unwrap();
    assert_eq!(session.own_address(fd), (PROCESS as u32, 0b11));
    // A bound socket has the number it has.
    assert_eq!(session.bind(fd, 17, 0b11), Err(libc::EINVAL));
    assert_eq!(session.bind(fd, PROCESS as u32, 0b100), Ok(0));
    assert_eq!(session.own_address(fd), (PROCESS as u32, 0b100));
    let second = session.route_socket();
    assert_eq!(session.bind(second, PROCESS as u32, 0), Err(libc::EADDRINUSE));
    assert_eq!(session.bind(second, 0, 0), Ok(0));
    assert_eq!(session.own_address(second).0, 0xffff_f000);
}

#[test]
fn bind_takes_only_a_netlink_address_that_is_whole() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    session.put(ADDRESS, &sockaddr(0, 0));
    assert_eq!(session.call(nr::BIND, [fd as u64, ADDRESS, 11, 0, 0, 0]), Err(libc::EINVAL));
    assert_eq!(session.call(nr::BIND, [fd as u64, ADDRESS, 0, 0, 0, 0]), Err(libc::EINVAL));
    assert_eq!(session.call(nr::BIND, [fd as u64, ADDRESS, u64::MAX, 0, 0, 0]), Err(libc::EINVAL));
    assert_eq!(session.call(nr::BIND, [fd as u64, ADDRESS, 129, 0, 0, 0]), Err(libc::EINVAL));
    // AF_UNIX.
    session.put(ADDRESS, &[1, 0]);
    assert_eq!(session.call(nr::BIND, [fd as u64, ADDRESS, 12, 0, 0, 0]), Err(libc::EINVAL));
    assert_eq!(session.call(nr::BIND, [fd as u64, 0x5000, 12, 0, 0, 0]), Err(libc::EFAULT));
    // A longer address is read for its first twelve bytes.
    session.put(ADDRESS, &sockaddr(0, 0));
    assert_eq!(session.call(nr::BIND, [fd as u64, ADDRESS, 128, 0, 0, 0]), Ok(0));
}

#[test]
fn getsockname_cuts_the_address_to_the_room_and_says_how_long_it_is() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    session.bind(fd, 0, 0).unwrap();
    let (bytes, told) = session.name(nr::GETSOCKNAME, fd, 4).unwrap();
    assert_eq!((bytes.len(), told), (4, 12));
    let (bytes, told) = session.name(nr::GETSOCKNAME, fd, 0).unwrap();
    assert_eq!((bytes.len(), told), (0, 12));
    let (bytes, told) = session.name(nr::GETSOCKNAME, fd, 16).unwrap();
    assert_eq!((bytes.len(), told), (12, 12));
    // What is past the address is left as it was.
    assert_eq!(session.get(ADDRESS + 12, 4), [0xaa; 4]);
    assert_eq!(session.name(nr::GETSOCKNAME, fd, u32::MAX).err(), Some(libc::EINVAL));
    assert_eq!(
        session.call(nr::GETSOCKNAME, [fd as u64, ADDRESS, 0x5000, 0, 0, 0]),
        Err(libc::EFAULT)
    );
}

#[test]
fn the_peer_is_the_kernel_whether_or_not_the_socket_is_connected() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    let (bytes, told) = session.name(nr::GETPEERNAME, fd, 12).unwrap();
    assert_eq!((bytes, told), (sockaddr(0, 0).to_vec(), 12));
    session.put(ADDRESS, &sockaddr(0, 0));
    assert_eq!(session.call(nr::CONNECT, [fd as u64, ADDRESS, 12, 0, 0, 0]), Ok(0));
    assert_eq!(session.own_address(fd).0, PROCESS as u32);
    let (bytes, _) = session.name(nr::GETPEERNAME, fd, 12).unwrap();
    assert_eq!(bytes, sockaddr(0, 0));
}

#[test]
fn connect_has_the_kernel_for_its_only_peer() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    session.put(ADDRESS, &sockaddr(1, 0));
    assert_eq!(session.call(nr::CONNECT, [fd as u64, ADDRESS, 12, 0, 0, 0]), Err(libc::EPERM));
    let mut root = Session::as_user(Fixed::qemu(), 0);
    let fd = root.route_socket();
    root.put(ADDRESS, &sockaddr(1, 0));
    assert_eq!(root.call(nr::CONNECT, [fd as u64, ADDRESS, 12, 0, 0, 0]), Err(libc::ECONNREFUSED));
    // AF_UNSPEC ends a connection that was never there.
    session.put(ADDRESS, &[0, 0]);
    assert_eq!(session.call(nr::CONNECT, [fd as u64, ADDRESS, 2, 0, 0, 0]), Ok(0));
    assert_eq!(session.call(nr::CONNECT, [fd as u64, ADDRESS, 1, 0, 0, 0]), Err(libc::EINVAL));
    session.put(ADDRESS, &[1, 0]);
    assert_eq!(session.call(nr::CONNECT, [fd as u64, ADDRESS, 12, 0, 0, 0]), Err(libc::EINVAL));
}

#[test]
fn a_datagram_to_anyone_but_the_kernel_is_refused() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    let bytes = glibc_request(RTM_GETLINK, 1);
    session.put(REQUEST, &bytes);
    session.put(ADDRESS, &sockaddr(5, 0));
    let to_someone = [fd as u64, REQUEST, 20, 0, ADDRESS, 12];
    assert_eq!(session.call(nr::SENDTO, to_someone), Err(libc::EPERM));
    session.put(ADDRESS, &sockaddr(0, 1));
    assert_eq!(session.call(nr::SENDTO, to_someone), Err(libc::EPERM));
    // A name that is not a netlink one, or too short to be one.
    session.put(ADDRESS, &[1, 0]);
    assert_eq!(
        session.call(nr::SENDTO, [fd as u64, REQUEST, 20, 0, ADDRESS, 12]),
        Err(libc::EINVAL)
    );
    session.put(ADDRESS, &sockaddr(0, 0));
    assert_eq!(
        session.call(nr::SENDTO, [fd as u64, REQUEST, 20, 0, ADDRESS, 8]),
        Err(libc::EINVAL)
    );
    // A name of no length is no name.
    assert_eq!(session.call(nr::SENDTO, [fd as u64, REQUEST, 20, 0, ADDRESS, 0]), Ok(20));

    let mut root = Session::as_user(Fixed::qemu(), 0);
    let fd = root.route_socket();
    root.put(REQUEST, &bytes);
    root.put(ADDRESS, &sockaddr(5, 0));
    assert_eq!(
        root.call(nr::SENDTO, [fd as u64, REQUEST, 20, 0, ADDRESS, 12]),
        Err(libc::ECONNREFUSED)
    );
    // To a group nobody has joined: sent, and nothing comes of it.
    root.put(ADDRESS, &sockaddr(0, 1));
    assert_eq!(root.call(nr::SENDTO, [fd as u64, REQUEST, 20, 0, ADDRESS, 12]), Ok(20));
    assert_eq!(root.receive(fd, 4096, 0).err(), Some(libc::EAGAIN));
}

#[test]
fn a_send_binds_the_socket_and_checks_its_length_and_memory() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    assert_eq!(session.own_address(fd).0, 0);
    // Past the buffer less 32 bytes: too long, before the memory is read.
    let huge = 212_992 - 31;
    let args = [fd as u64, 0x5000, huge, 0, 0, 0];
    assert_eq!(session.call(nr::SENDTO, args), Err(libc::EMSGSIZE));
    assert_eq!(session.own_address(fd).0, PROCESS as u32);
    assert_eq!(
        session.call(nr::SENDTO, [fd as u64, 0x5000, u64::MAX, 0, 0, 0]),
        Err(libc::EMSGSIZE)
    );
    // Memory that is not there.
    assert_eq!(session.call(nr::SENDTO, [fd as u64, 0x5000, 20, 0, 0, 0]), Err(libc::EFAULT));
    // A datagram of nothing is sent and says nothing.
    assert_eq!(session.call(nr::SENDTO, [fd as u64, 0x5000, 0, 0, 0, 0]), Ok(0));
    assert_eq!(session.receive(fd, 4096, 0).err(), Some(libc::EAGAIN));
}

#[test]
fn sendmsg_gathers_its_datagram_from_the_iovecs() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    let bytes = glibc_request(RTM_GETLINK, 11);
    session.put(REQUEST, &bytes[..7]);
    session.put(REQUEST + 0x100, &[]);
    session.put(REQUEST + 0x200, &bytes[7..]);
    let mut iovecs = iovec(REQUEST, 7);
    iovecs.extend_from_slice(&iovec(REQUEST + 0x100, 0));
    iovecs.extend_from_slice(&iovec(REQUEST + 0x200, bytes.len() - 7));
    session.put(IOVECS, &iovecs);
    session.put(ADDRESS, &sockaddr(0, 0));
    let mut header = [0u8; 56];
    header[0..8].copy_from_slice(&ADDRESS.to_ne_bytes());
    header[8..12].copy_from_slice(&12u32.to_ne_bytes());
    header[16..24].copy_from_slice(&IOVECS.to_ne_bytes());
    header[24..32].copy_from_slice(&3u64.to_ne_bytes());
    session.put(MSGHDR, &header);
    assert_eq!(session.call(nr::SENDMSG, [fd as u64, MSGHDR, 0, 0, 0, 0]), Ok(20));
    let links = session.list(fd, 4096);
    assert_eq!(links.len(), 2);
    assert_eq!(seq_of(&links[0]), 11);

    // Too many iovecs, and an iovec whose length is negative.
    header[24..32].copy_from_slice(&1025u64.to_ne_bytes());
    session.put(MSGHDR, &header);
    assert_eq!(session.call(nr::SENDMSG, [fd as u64, MSGHDR, 0, 0, 0, 0]), Err(libc::EINVAL));
    session.put(IOVECS, &iovec(REQUEST, usize::MAX));
    header[24..32].copy_from_slice(&1u64.to_ne_bytes());
    session.put(MSGHDR, &header);
    assert_eq!(session.call(nr::SENDMSG, [fd as u64, MSGHDR, 0, 0, 0, 0]), Err(libc::EINVAL));
    // A header that is not in memory.
    assert_eq!(session.call(nr::SENDMSG, [fd as u64, 0x5000, 0, 0, 0, 0]), Err(libc::EFAULT));
}

#[test]
fn recvmsg_scatters_the_datagram_and_reports_the_address_it_came_from() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    session.send(fd, &glibc_request(RTM_GETLINK, 1)).unwrap();
    let whole = session.receive(fd, 4096, MSG_PEEK).unwrap().bytes;
    // Two buffers of 100 bytes, which the datagram fills, and its rest
    // is cut.
    session.put(IOVECS, &[iovec(REPLY, 100), iovec(REPLY + 0x1000, 100)].concat());
    session.put(ADDRESS, &[0xaa; 16]);
    let mut header = [0u8; 56];
    header[0..8].copy_from_slice(&ADDRESS.to_ne_bytes());
    header[8..12].copy_from_slice(&16u32.to_ne_bytes());
    header[16..24].copy_from_slice(&IOVECS.to_ne_bytes());
    header[24..32].copy_from_slice(&2u64.to_ne_bytes());
    header[40..48].copy_from_slice(&0xdeadu64.to_ne_bytes());
    header[48..52].copy_from_slice(&0x7777i32.to_ne_bytes());
    session.put(MSGHDR, &header);
    assert_eq!(session.call(nr::RECVMSG, [fd as u64, MSGHDR, 0, 0, 0, 0]), Ok(200));
    assert_eq!(session.get(REPLY, 100), whole[..100]);
    assert_eq!(session.get(REPLY + 0x1000, 100), whole[100..200]);
    let after = session.get(MSGHDR, 56);
    assert_eq!(u32::from_ne_bytes(after[8..12].try_into().unwrap()), 12);
    assert_eq!(i32::from_ne_bytes(after[48..52].try_into().unwrap()), MSG_TRUNC as i32);
    assert_eq!(u64::from_ne_bytes(after[40..48].try_into().unwrap()), 0);
    assert_eq!(session.get(ADDRESS, 16)[..12], sockaddr(0, 0));
    assert_eq!(session.get(ADDRESS + 12, 4), [0xaa; 4]);
}

#[test]
fn recvmsg_with_no_address_buffer_writes_none() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    session.send(fd, &glibc_request(RTM_GETLINK, 1)).unwrap();
    session.put(IOVECS, &iovec(REPLY, 4096));
    session.put(ADDRESS, &[0xaa; 16]);
    let mut header = [0u8; 56];
    header[16..24].copy_from_slice(&IOVECS.to_ne_bytes());
    header[24..32].copy_from_slice(&1u64.to_ne_bytes());
    header[8..12].copy_from_slice(&99u32.to_ne_bytes());
    session.put(MSGHDR, &header);
    assert!(session.call(nr::RECVMSG, [fd as u64, MSGHDR, 0, 0, 0, 0]).unwrap() > 0);
    assert_eq!(session.get(ADDRESS, 16), [0xaa; 16]);
    assert_eq!(u32::from_ne_bytes(session.get(MSGHDR + 8, 4).try_into().unwrap()), 99);
}

#[test]
fn a_buffer_that_is_not_there_loses_the_datagram_as_it_does_on_linux() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    session.send(fd, &glibc_request(RTM_GETLINK, 1)).unwrap();
    assert_eq!(session.call(nr::RECVFROM, [fd as u64, 0x5000, 4096, 0, 0, 0]), Err(libc::EFAULT));
    let next = session.receive(fd, 4096, 0).unwrap();
    assert_eq!(next.value, 20);
}

#[test]
fn recvfrom_writes_the_address_it_is_given_room_for() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    session.send(fd, &glibc_request(RTM_GETLINK, 1)).unwrap();
    session.put(ADDRESS, &[0xaa; 16]);
    session.put(LENGTH, &8u32.to_ne_bytes());
    let args = [fd as u64, REPLY, 4096, 0, ADDRESS, LENGTH];
    assert!(session.call(nr::RECVFROM, args).unwrap() > 0);
    assert_eq!(session.get(ADDRESS, 8), sockaddr(0, 0)[..8]);
    assert_eq!(session.get(ADDRESS + 8, 8), [0xaa; 8]);
    assert_eq!(u32::from_ne_bytes(session.get(LENGTH, 4).try_into().unwrap()), 12);
}

#[test]
fn read_and_write_on_the_file_are_receive_and_send_without_addresses() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    let id = id_of(session.program.memory.descriptors[&fd].1.unwrap()).unwrap();
    assert_eq!(session.netlinks.read(id, 4096), Err(libc::EAGAIN));
    assert_eq!(session.netlinks.write(id, &glibc_request(RTM_GETLINK, 8)), Ok(20));
    let first = session.netlinks.read(id, 4096).unwrap();
    assert_eq!(type_of(&first), RTM_NEWLINK);
    assert_eq!(seq_of(&first), 8);
    // The socket was bound by the write, to a number of its own.
    assert_eq!(pid_of(&first), 0xffff_f000);
    // A read of fewer bytes than the datagram has cuts it.
    assert_eq!(session.netlinks.read(id, 7).unwrap().len(), 7);
    assert_eq!(session.netlinks.write(id + 1, b"x"), Err(libc::EBADF));
    assert_eq!(session.netlinks.read(id + 1, 10), Err(libc::EBADF));
}

#[test]
fn the_buffer_options_are_doubled_and_report_what_was_set() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket() as u64;
    let option = |session: &mut Session, number, name, value: i32| {
        session.put(VALUE, &value.to_ne_bytes());
        session.call(number, [fd, SOL_SOCKET, name, VALUE, 4, 0])
    };
    let get = |session: &mut Session, name| {
        session.put(LENGTH, &4i32.to_ne_bytes());
        session.call(nr::GETSOCKOPT, [fd, SOL_SOCKET, name, VALUE, LENGTH, 0]).unwrap();
        i32::from_ne_bytes(session.get(VALUE, 4).try_into().unwrap())
    };
    assert_eq!(get(&mut session, SO_RCVBUF), 212_992);
    assert_eq!(option(&mut session, nr::SETSOCKOPT, SO_RCVBUF, 32_768), Ok(0));
    assert_eq!(get(&mut session, SO_RCVBUF), 65_536);
    assert_eq!(option(&mut session, nr::SETSOCKOPT, SO_SNDBUF, 1), Ok(0));
    assert_eq!(get(&mut session, SO_SNDBUF), 4_608);
    assert_eq!(option(&mut session, nr::SETSOCKOPT, SO_PASSCRED, 1), Ok(0));
    assert_eq!(get(&mut session, SO_PASSCRED), 1);
    assert_eq!(get(&mut session, SO_TYPE), 3);
    // The forced sizes are for whoever may change the network.
    assert_eq!(option(&mut session, nr::SETSOCKOPT, 33, 1 << 20), Err(libc::EPERM));
    // SO_TYPE cannot be set, and no such option is one.
    assert_eq!(option(&mut session, nr::SETSOCKOPT, SO_TYPE, 1), Err(libc::ENOPROTOOPT));
    assert_eq!(option(&mut session, nr::SETSOCKOPT, 1234, 1), Err(libc::ENOPROTOOPT));
    // The send buffer limits what can be sent.
    let args = [fd, REQUEST, 4_577, 0, 0, 0];
    assert_eq!(session.call(nr::SENDTO, args), Err(libc::EMSGSIZE));
}

#[test]
fn an_option_value_must_be_whole_and_a_buffer_may_be_longer() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket() as u64;
    session.put(VALUE, &[1, 0, 0, 0, 9, 9, 9, 9]);
    assert_eq!(
        session.call(nr::SETSOCKOPT, [fd, SOL_SOCKET, SO_PASSCRED, VALUE, 3, 0]),
        Err(libc::EINVAL)
    );
    assert_eq!(
        session.call(nr::SETSOCKOPT, [fd, SOL_SOCKET, SO_PASSCRED, VALUE, u64::MAX, 0]),
        Err(libc::EINVAL)
    );
    assert_eq!(
        session.call(nr::SETSOCKOPT, [fd, SOL_SOCKET, SO_PASSCRED, 0x5000, 4, 0]),
        Err(libc::EFAULT)
    );
    session.put(LENGTH, &8i32.to_ne_bytes());
    assert_eq!(session.call(nr::GETSOCKOPT, [fd, SOL_SOCKET, SO_TYPE, VALUE, LENGTH, 0]), Ok(0));
    // Four bytes are given for room for eight, and it says four.
    assert_eq!(session.get(VALUE, 8), [3, 0, 0, 0, 9, 9, 9, 9]);
    assert_eq!(session.get(LENGTH, 4), 4i32.to_ne_bytes());
    session.put(LENGTH, &(-1i32).to_ne_bytes());
    assert_eq!(
        session.call(nr::GETSOCKOPT, [fd, SOL_SOCKET, SO_TYPE, VALUE, LENGTH, 0]),
        Err(libc::EINVAL)
    );
    session.put(LENGTH, &2i32.to_ne_bytes());
    assert_eq!(session.call(nr::GETSOCKOPT, [fd, SOL_SOCKET, SO_TYPE, VALUE, LENGTH, 0]), Ok(0));
    assert_eq!(session.get(LENGTH, 4), 2i32.to_ne_bytes());
}

#[test]
fn netlinks_own_options_are_remembered_and_a_membership_changes_the_groups() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    session.bind(fd, 0, 0).unwrap();
    let call = |session: &mut Session, number, name, value: i32| {
        session.put(VALUE, &value.to_ne_bytes());
        session.put(LENGTH, &4i32.to_ne_bytes());
        let args = [
            fd as u64,
            SOL_NETLINK,
            name,
            VALUE,
            if number == nr::SETSOCKOPT { 4 } else { LENGTH },
            0,
        ];
        session.call(number, args)
    };
    assert_eq!(call(&mut session, nr::GETSOCKOPT, NETLINK_EXT_ACK, 0), Ok(0));
    assert_eq!(session.get(VALUE, 4), 0i32.to_ne_bytes());
    assert_eq!(call(&mut session, nr::SETSOCKOPT, NETLINK_EXT_ACK, 1), Ok(0));
    assert_eq!(call(&mut session, nr::GETSOCKOPT, NETLINK_EXT_ACK, 0), Ok(0));
    assert_eq!(session.get(VALUE, 4), 1i32.to_ne_bytes());
    assert_eq!(call(&mut session, nr::SETSOCKOPT, NETLINK_ADD_MEMBERSHIP, 1), Ok(0));
    assert_eq!(call(&mut session, nr::SETSOCKOPT, NETLINK_ADD_MEMBERSHIP, 0), Err(libc::EINVAL));
    assert_eq!(session.own_address(fd), (PROCESS as u32, 1));
    // An option that is not netlink's, or is one that is not offered.
    assert_eq!(call(&mut session, nr::SETSOCKOPT, 99, 1), Err(libc::ENOPROTOOPT));
    assert_eq!(
        call(&mut session, nr::GETSOCKOPT, NETLINK_ADD_MEMBERSHIP, 0),
        Err(libc::ENOPROTOOPT)
    );
    // And a level that is neither.
    let args = [fd as u64, 6, 1, VALUE, 4, 0];
    assert_eq!(session.call(nr::SETSOCKOPT, args), Err(libc::ENOPROTOOPT));
}

#[test]
fn netlink_has_no_shutdown() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    for how in 0..3 {
        assert_eq!(session.call(nr::SHUTDOWN, [fd as u64, how, 0, 0, 0, 0]), Err(libc::EOPNOTSUPP));
    }
}

#[test]
fn calls_that_are_not_socket_calls_are_not_answered() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    assert_eq!(session.call(0, [fd as u64, 0, 0, 0, 0, 0]), Err(libc::ENOSYS));
}

#[test]
fn what_a_program_sends_never_stops_the_socket() {
    let mut session = Session::new(Fixed::qemu());
    let fd = session.route_socket();
    // Every length a header can claim, and the types around the ones
    // that are served, in one datagram that is never well formed.
    let mut garbage = Vec::new();
    for length in [0u32, 1, 15, 16, 17, 19, 20, 21, 0xffff_ffff] {
        for kind in [0u16, 1, 2, 3, 15, 16, 18, 20, 22, 26, 123, 124, 0xffff] {
            garbage.extend_from_slice(&length.to_ne_bytes());
            garbage.extend_from_slice(&kind.to_ne_bytes());
            garbage.extend_from_slice(&0x0305u16.to_ne_bytes());
            garbage.extend_from_slice(&[7; 12]);
        }
    }
    session.send(fd, &garbage).unwrap();
    // Whatever came of it, the socket still answers a good request.
    while session.receive(fd, 4096, 0).is_ok() {}
    session.send(fd, &glibc_request(RTM_GETLINK, 3)).unwrap();
    assert_eq!(session.list(fd, 4096).len(), 2);
}

#[test]
fn two_programs_of_a_process_each_get_all_of_a_list() {
    let mut session = Session::new(Fixed::qemu());
    let first = session.route_socket();
    let second = session.route_socket();
    session.send(first, &glibc_request(RTM_GETLINK, 1)).unwrap();
    session.send(second, &glibc_request(RTM_GETROUTE, 2)).unwrap();
    assert_eq!(session.list(first, 4096).len(), 2);
    assert_eq!(session.list(second, 4096).len(), 5);
    let pids = [session.own_address(first).0, session.own_address(second).0];
    assert_eq!(pids, [PROCESS as u32, 0xffff_f000]);
}
