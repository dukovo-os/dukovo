// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The socket calls on a netlink socket, as Linux's `net/socket.c` takes
//! their arguments from the program and `net/netlink/af_netlink.c`
//! answers them.
//!
//! Each call reads what it needs from the process's memory (the x86-64
//! layouts of `sockaddr_nl`, `msghdr` and `iovec`), asks the table
//! (`socket`) for the socket's part, and writes what it gives back. The
//! errors and their order follow Linux's, call by call; a comment names
//! the one that is easy to take for another.

use std::os::fd::AsRawFd;

use crate::call::{Answer, Process, read_value};
use crate::serve::nr;
use crate::sys::slot_of;

use super::socket::{Caller, NlAddress};
use super::{Netlinks, id_of};

/// AF_NETLINK, SOCK_DGRAM and SOCK_RAW, the types `socket` takes, and
/// the flags it takes with them.
const AF_NETLINK: u16 = 16;
const AF_UNSPEC: u16 = 0;
const SOCK_DGRAM: i32 = 2;
const SOCK_RAW: i32 = 3;
const SOCK_TYPE_MASK: i32 = 0xf;
const SOCK_NONBLOCK: i32 = 0o4000;
const SOCK_CLOEXEC: i32 = 0o2000000;
/// NETLINK_ROUTE, the one protocol.
const NETLINK_ROUTE: i32 = 0;

/// The flags of a receive that change what it does.
const MSG_PEEK: i32 = 0x2;
const MSG_TRUNC: i32 = 0x20;

/// The sizes of `struct sockaddr_nl`, `struct msghdr` and `struct
/// iovec` on x86-64, and the largest `sockaddr` a call takes
/// (`sizeof(struct sockaddr_storage)`).
const SOCKADDR_NL: usize = 12;
const MSGHDR: usize = 56;
const IOVEC: usize = 16;
const ADDRESS_MAX: i32 = 128;
/// The most `iovec`s of one call (UIO_MAXIOV).
const UIO_MAXIOV: usize = 1024;

/// Where `struct msghdr` keeps its fields.
const MSG_NAME: usize = 0;
const MSG_NAMELEN: usize = 8;
const MSG_IOV: usize = 16;
const MSG_IOVLEN: usize = 24;
const MSG_CONTROLLEN: usize = 40;
const MSG_FLAGS: usize = 48;

/// What the kernel says its address is when it sends: `sockaddr_nl` of
/// AF_NETLINK, no number and no groups.
const KERNEL: [u8; SOCKADDR_NL] = [AF_NETLINK as u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

/// An `int` argument: the low half of its register, as Linux's calls
/// take it.
fn int(argument: u64) -> i32 {
    argument as i32
}

/// socket(AF_NETLINK, type, protocol): the checks of Linux's
/// `__sys_socket_create` and `netlink_create`, in their order.
pub(super) fn socket(netlinks: &Netlinks, process: &mut dyn Process, args: &[u64; 6]) -> Answer {
    let requested = int(args[1]);
    if requested & !(SOCK_TYPE_MASK | SOCK_CLOEXEC | SOCK_NONBLOCK) != 0 {
        return Err(libc::EINVAL);
    }
    let kind = requested & SOCK_TYPE_MASK;
    if kind != SOCK_RAW && kind != SOCK_DGRAM {
        return Err(libc::ESOCKTNOSUPPORT);
    }
    if int(args[2]) != NETLINK_ROUTE {
        return Err(libc::EPROTONOSUPPORT);
    }
    // The file is opened with the table free: opening it reaches linuxd's
    // filesystem, which asks the table whether the socket exists.
    let id = netlinks.table().create(kind);
    let file = netlinks.files.open(id).inspect_err(|_| netlinks.table().remove(id))?;
    // SOCK_NONBLOCK needs no flag: linuxd never waits in any call.
    let given = process.give(slot_of(file.as_raw_fd()), requested & SOCK_CLOEXEC != 0);
    // The program's descriptor holds the file; linuxd's is not needed.
    drop(file);
    given.map(i64::from).inspect_err(|_| netlinks.table().remove(id))
}

/// A socket call on descriptor `args[0]`.
pub(super) fn call(
    netlinks: &Netlinks,
    process: &mut dyn Process,
    number: u64,
    args: &[u64; 6],
) -> Answer {
    let node = process.node_of(int(args[0]))?;
    let id = id_of(node).filter(|&id| netlinks.exists(id)).ok_or(libc::ENOTSOCK)?;
    let who = process.who();
    let caller = Caller { process: who.process, root: who.user == 0 };
    match number {
        nr::BIND => bind(netlinks, process, id, &caller, args),
        nr::CONNECT => connect(netlinks, process, id, &caller, args),
        nr::GETSOCKNAME | nr::GETPEERNAME => name(netlinks, process, id, number, args),
        nr::SENDTO => sendto(netlinks, process, id, &caller, args),
        nr::SENDMSG => sendmsg(netlinks, process, id, &caller, args),
        nr::RECVFROM => recvfrom(netlinks, process, id, args),
        nr::RECVMSG => recvmsg(netlinks, process, id, args),
        nr::SETSOCKOPT => setsockopt(netlinks, process, id, &caller, args),
        nr::GETSOCKOPT => getsockopt(netlinks, process, id, args),
        // Netlink has no `shutdown` (`sock_no_shutdown`).
        nr::SHUTDOWN => Err(libc::EOPNOTSUPP),
        _ => Err(libc::ENOSYS),
    }
}

/// The `sockaddr_nl` of `length` bytes at `pointer`, with the checks of
/// `move_addr_to_kernel` and `netlink_bind`.
fn read_address(process: &mut dyn Process, pointer: u64, length: i32) -> Result<NlAddress, i32> {
    if !(0..=ADDRESS_MAX).contains(&length) {
        return Err(libc::EINVAL);
    }
    let length = length as usize;
    let mut bytes = [0u8; SOCKADDR_NL];
    let present = length.min(SOCKADDR_NL);
    if present > 0 {
        process.read(pointer, &mut bytes[..present])?;
    }
    let family = u16::from_ne_bytes([bytes[0], bytes[1]]);
    if length < SOCKADDR_NL || family != AF_NETLINK {
        return Err(libc::EINVAL);
    }
    let word =
        |at: usize| u32::from_ne_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    Ok(NlAddress { pid: word(4), groups: word(8) })
}

/// The address a send names, if it names one: a length of 0 is none.
fn optional_address(
    process: &mut dyn Process,
    pointer: u64,
    length: i32,
) -> Result<Option<NlAddress>, i32> {
    if length == 0 {
        return Ok(None);
    }
    read_address(process, pointer, length).map(Some)
}

/// Writes `kernel`, the address of the socket's other end, to the program
/// as `move_addr_to_user` does: as much as fits in the length at
/// `length_at`, which is then set to the address's whole length.
fn write_address(
    process: &mut dyn Process,
    address: u64,
    length_at: u64,
    kernel: &[u8; SOCKADDR_NL],
) -> Result<(), i32> {
    let room = i32::from_ne_bytes(read_value::<_, 4>(process, length_at)?);
    if room != 0 {
        let room = usize::try_from(room).map_err(|_| libc::EINVAL)?;
        process.write(address, &kernel[..room.min(SOCKADDR_NL)])?;
    }
    process.write(length_at, &(SOCKADDR_NL as i32).to_ne_bytes())
}

fn bind(
    netlinks: &Netlinks,
    process: &mut dyn Process,
    id: u64,
    caller: &Caller,
    args: &[u64; 6],
) -> Answer {
    let address = read_address(process, args[1], int(args[2]))?;
    netlinks.table().bind(id, caller, address)?;
    Ok(0)
}

/// connect(2), whose first two bytes name the family: AF_UNSPEC ends a
/// connection, which a socket that only talks to the kernel does not
/// have.
fn connect(
    netlinks: &Netlinks,
    process: &mut dyn Process,
    id: u64,
    caller: &Caller,
    args: &[u64; 6],
) -> Answer {
    let length = int(args[2]);
    // A length that cannot hold the family is invalid.
    if !(2..=ADDRESS_MAX).contains(&length) {
        return Err(libc::EINVAL);
    }
    if u16::from_ne_bytes(read_value::<_, 2>(process, args[1])?) == AF_UNSPEC {
        return Ok(0);
    }
    let to = read_address(process, args[1], length)?;
    netlinks.table().connect(id, caller, to)?;
    Ok(0)
}

/// getsockname(2) and getpeername(2): `netlink_getname`. A socket has
/// the kernel for its peer, connected or not.
fn name(
    netlinks: &Netlinks,
    process: &mut dyn Process,
    id: u64,
    number: u64,
    args: &[u64; 6],
) -> Answer {
    let mut address = KERNEL;
    if number == nr::GETSOCKNAME {
        let own = netlinks.table().name(id)?;
        address[4..8].copy_from_slice(&own.pid.to_ne_bytes());
        address[8..12].copy_from_slice(&own.groups.to_ne_bytes());
    }
    write_address(process, args[1], args[2], &address)?;
    Ok(0)
}

/// Reads `length` bytes at `address`.
fn read_bytes(process: &mut dyn Process, address: u64, length: usize) -> Result<Vec<u8>, i32> {
    let mut bytes = vec![0u8; length];
    if length > 0 {
        process.read(address, &mut bytes)?;
    }
    Ok(bytes)
}

fn sendto(
    netlinks: &Netlinks,
    process: &mut dyn Process,
    id: u64,
    caller: &Caller,
    args: &[u64; 6],
) -> Answer {
    let destination =
        if args[4] != 0 { optional_address(process, args[4], int(args[5]))? } else { None };
    let length = usize::try_from(args[2]).unwrap_or(usize::MAX);
    let sent =
        netlinks.send(id, caller, destination, length, || read_bytes(process, args[1], length))?;
    Ok(sent as i64)
}

/// The fields of a `struct msghdr` that a send or a receive reads.
struct MsgHdr {
    name: u64,
    name_length: i32,
    iov: u64,
    iov_count: u64,
}

fn read_msghdr(process: &mut dyn Process, pointer: u64) -> Result<MsgHdr, i32> {
    let bytes = read_value::<_, MSGHDR>(process, pointer)?;
    let word = |at: usize| {
        let mut eight = [0u8; 8];
        eight.copy_from_slice(&bytes[at..at + 8]);
        u64::from_ne_bytes(eight)
    };
    let mut four = [0u8; 4];
    four.copy_from_slice(&bytes[MSG_NAMELEN..MSG_NAMELEN + 4]);
    Ok(MsgHdr {
        name: word(MSG_NAME),
        name_length: i32::from_ne_bytes(four),
        iov: word(MSG_IOV),
        iov_count: word(MSG_IOVLEN),
    })
}

/// The `iovec`s of a call as (address, length), and the length of them
/// all, which is the most that a receive offers or a send gives; a
/// negative length is EINVAL as in Linux's `iovec_from_user`.
fn read_iovecs(
    process: &mut dyn Process,
    pointer: u64,
    count: u64,
) -> Result<(Vec<(u64, usize)>, usize), i32> {
    let count = usize::try_from(count).ok().filter(|&n| n <= UIO_MAXIOV).ok_or(libc::EINVAL)?;
    let bytes = read_bytes(process, pointer, count * IOVEC)?;
    let mut entries = Vec::with_capacity(count);
    let mut total = 0usize;
    for entry in bytes.as_chunks::<IOVEC>().0 {
        let mut word = [0u8; 8];
        word.copy_from_slice(&entry[..8]);
        let base = u64::from_ne_bytes(word);
        word.copy_from_slice(&entry[8..]);
        let length = usize::try_from(u64::from_ne_bytes(word))
            .ok()
            .filter(|&length| isize::try_from(length).is_ok())
            .ok_or(libc::EINVAL)?;
        total = total.saturating_add(length);
        entries.push((base, length));
    }
    Ok((entries, total))
}

fn sendmsg(
    netlinks: &Netlinks,
    process: &mut dyn Process,
    id: u64,
    caller: &Caller,
    args: &[u64; 6],
) -> Answer {
    let header = read_msghdr(process, args[1])?;
    let (iovecs, length) = read_iovecs(process, header.iov, header.iov_count)?;
    // A name length past the largest address is cut to it, as
    // `copy_msghdr_from_user` cuts it; a negative one is invalid.
    let name_length = header.name_length.min(ADDRESS_MAX);
    let destination = if header.name != 0 {
        if name_length < 0 {
            return Err(libc::EINVAL);
        }
        optional_address(process, header.name, name_length)?
    } else {
        None
    };
    let sent = netlinks.send(id, caller, destination, length, || {
        let mut bytes = Vec::with_capacity(length);
        for &(base, each) in &iovecs {
            bytes.extend_from_slice(&read_bytes(process, base, each)?);
        }
        Ok(bytes)
    })?;
    Ok(sent as i64)
}

/// What a receive gives the program: the bytes that fit, whether the
/// datagram was longer, and what the call returns. With MSG_TRUNC that is
/// the datagram's whole length, as `netlink_recvmsg` returns it.
fn received(datagram: &[u8], offered: usize, flags: i32) -> (&[u8], bool, usize) {
    let cut = datagram.len() > offered;
    let fitted = &datagram[..datagram.len().min(offered)];
    let value = if flags & MSG_TRUNC != 0 { datagram.len() } else { fitted.len() };
    (fitted, cut, value)
}

fn recvfrom(netlinks: &Netlinks, process: &mut dyn Process, id: u64, args: &[u64; 6]) -> Answer {
    let flags = int(args[3]);
    let offered = usize::try_from(args[2]).unwrap_or(usize::MAX);
    let datagram = netlinks.table().receive(id, offered, flags & MSG_PEEK != 0)?;
    let (fitted, _, value) = received(&datagram, offered, flags);
    if !fitted.is_empty() {
        process.write(args[1], fitted)?;
    }
    if args[4] != 0 {
        write_address(process, args[4], args[5], &KERNEL)?;
    }
    Ok(value as i64)
}

fn recvmsg(netlinks: &Netlinks, process: &mut dyn Process, id: u64, args: &[u64; 6]) -> Answer {
    let flags = int(args[2]);
    let header = read_msghdr(process, args[1])?;
    let (iovecs, offered) = read_iovecs(process, header.iov, header.iov_count)?;
    let datagram = netlinks.table().receive(id, offered, flags & MSG_PEEK != 0)?;
    let (fitted, cut, value) = received(&datagram, offered, flags);
    let mut rest = fitted;
    for &(base, length) in &iovecs {
        let (now, later) = rest.split_at(rest.len().min(length));
        if !now.is_empty() {
            process.write(base, now)?;
        }
        rest = later;
    }
    if header.name != 0 {
        write_address(process, header.name, args[1] + MSG_NAMELEN as u64, &KERNEL)?;
    }
    let out_flags = if cut { MSG_TRUNC } else { 0 };
    process.write(args[1] + MSG_FLAGS as u64, &out_flags.to_ne_bytes())?;
    // No control message is ever given.
    process.write(args[1] + MSG_CONTROLLEN as u64, &0u64.to_ne_bytes())?;
    Ok(value as i64)
}

fn setsockopt(
    netlinks: &Netlinks,
    process: &mut dyn Process,
    id: u64,
    caller: &Caller,
    args: &[u64; 6],
) -> Answer {
    let (level, name, length) = (int(args[1]), int(args[2]), int(args[4]));
    if length < 0 {
        return Err(libc::EINVAL);
    }
    // A value of an int's size or more is read before the option is
    // looked at, as `sock_setsockopt` reads it.
    let value = if length >= 4 {
        Some(i32::from_ne_bytes(read_value::<_, 4>(process, args[3])?))
    } else {
        None
    };
    netlinks.table().set_option(id, level, name, value, caller)?;
    Ok(0)
}

fn getsockopt(netlinks: &Netlinks, process: &mut dyn Process, id: u64, args: &[u64; 6]) -> Answer {
    let (level, name) = (int(args[1]), int(args[2]));
    let room = i32::from_ne_bytes(read_value::<_, 4>(process, args[4])?);
    let room = usize::try_from(room).map_err(|_| libc::EINVAL)?;
    let value = netlinks.table().get_option(id, level, name, room)?;
    if !value.is_empty() {
        process.write(args[3], &value)?;
    }
    // The value is at most an int, whose size always fits.
    process.write(args[4], &(value.len() as i32).to_ne_bytes())?;
    Ok(0)
}
