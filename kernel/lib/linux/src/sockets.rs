// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Linux's socket calls: the numbers, the socket addresses, `msghdr`,
//! `mmsghdr` and control messages, the `MSG_` flags, and the socket options
//! a program sets and reads, with their translation to Radix's.
//!
//! Dukovo's C library (`ports/mlibc/sysdeps/radix/generic/socket.cpp`)
//! is the reference for which socket a call makes, which flags it takes and
//! which options it knows; this module is that logic with Linux's raw
//! values and layouts, so that the kernel's table needs no more than the
//! native methods it calls. Every record is built in, or read from, a byte
//! slice, little-endian, with the layouts of the x86-64 uapi
//! (`linux/socket.h`, `linux/un.h`, `linux/in.h`, `linux/in6.h`): the host
//! tests check each field's offset.

use radix_abi::file::SocketOption;
use radix_abi::network::{Address, AddressFamily, SocketType};

use crate::errno::{
    EAFNOSUPPORT, EINVAL, EMSGSIZE, ENAMETOOLONG, ENOPROTOOPT, EOPNOTSUPP, EPERM, EPROTONOSUPPORT,
    ESOCKTNOSUPPORT,
};

/// Linux's `AF_UNSPEC`: no family, which `connect` takes to mean that a
/// datagram socket forgets its peer.
pub const AF_UNSPEC: u16 = 0;
/// Linux's `AF_UNIX`.
pub const AF_UNIX: u16 = 1;
/// Linux's `AF_INET`.
pub const AF_INET: u16 = 2;
/// Linux's `AF_INET6`.
pub const AF_INET6: u16 = 10;

/// `SOCK_STREAM`.
pub const SOCK_STREAM: u32 = 1;
/// `SOCK_DGRAM`.
pub const SOCK_DGRAM: u32 = 2;
/// `SOCK_RAW`.
pub const SOCK_RAW: u32 = 3;
/// `SOCK_SEQPACKET`.
pub const SOCK_SEQPACKET: u32 = 5;
/// The bits of a socket's type that are the type: the others are flags.
pub const SOCK_TYPE_MASK: u32 = 0xf;
/// `SOCK_NONBLOCK`: the socket's description is non-blocking.
pub const SOCK_NONBLOCK: u32 = 0o4000;
/// `SOCK_CLOEXEC`: the socket's descriptor closes on `exec`.
pub const SOCK_CLOEXEC: u32 = 0o2_000_000;

/// The protocol number of ICMP.
pub const IPPROTO_ICMP: u32 = 1;
/// The protocol number of TCP.
pub const IPPROTO_TCP: u32 = 6;
/// The protocol number of UDP.
pub const IPPROTO_UDP: u32 = 17;
/// The protocol number of ICMPv6.
pub const IPPROTO_ICMPV6: u32 = 58;

/// `MSG_OOB`: urgent data, which these sockets do not keep.
pub const MSG_OOB: u32 = 0x1;
/// `MSG_PEEK`: receive without taking the bytes.
pub const MSG_PEEK: u32 = 0x2;
/// `MSG_CTRUNC`: the control messages did not all fit.
pub const MSG_CTRUNC: u32 = 0x8;
/// `MSG_TRUNC`: a datagram was longer than the buffers.
pub const MSG_TRUNC: u32 = 0x20;
/// `MSG_DONTWAIT`: do not wait.
pub const MSG_DONTWAIT: u32 = 0x40;
/// `MSG_WAITALL`: wait for the buffers to fill.
pub const MSG_WAITALL: u32 = 0x100;
/// `MSG_ERRQUEUE`: receive an error from the socket's error queue.
pub const MSG_ERRQUEUE: u32 = 0x2000;
/// `MSG_NOSIGNAL`: no `SIGPIPE` for a send on a closed connection.
pub const MSG_NOSIGNAL: u32 = 0x4000;
/// `MSG_MORE`: more bytes follow, which a socket that does not coalesce
/// ignores.
pub const MSG_MORE: u32 = 0x8000;
/// `MSG_CMSG_CLOEXEC`: descriptors received close on `exec`.
pub const MSG_CMSG_CLOEXEC: u32 = 0x4000_0000;

/// `shutdown`'s `SHUT_RD`.
pub const SHUT_RD: i32 = 0;
/// `shutdown`'s `SHUT_WR`.
pub const SHUT_WR: i32 = 1;
/// `shutdown`'s `SHUT_RDWR`.
pub const SHUT_RDWR: i32 = 2;

/// What kind of socket a `socket` call asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An `AF_UNIX` socket, of datagrams if `datagram`, else a stream.
    Unix {
        /// Whether it is a datagram socket.
        datagram: bool,
    },
    /// A network socket of the kernel's TCP, UDP or ping.
    Inet {
        /// Whether its addresses are IPv6, else IPv4.
        ipv6: bool,
        /// Whether it is a stream, datagram or ping socket.
        kind: SocketType,
    },
}

/// A `socket` call's request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Make {
    /// The socket.
    pub kind: Kind,
    /// `SOCK_NONBLOCK` was asked for.
    pub non_blocking: bool,
    /// `SOCK_CLOEXEC` was asked for.
    pub close_on_exec: bool,
}

/// What `socket(domain, type, protocol)` asks for, as Dukovo's C library
/// answers it.
///
/// # Errors
///
/// EINVAL for flags or a type that does not exist, EAFNOSUPPORT for a
/// family that is not `AF_UNIX`, `AF_INET` or `AF_INET6`, EPROTONOSUPPORT
/// for a protocol the type does not have, EPERM for a raw socket (which
/// needs a right a program does not hold), and ESOCKTNOSUPPORT for a
/// sequenced packet socket.
pub fn socket(domain: i32, kind: i32, protocol: i32) -> Result<Make, i32> {
    let kind = kind as u32;
    if kind & !(SOCK_TYPE_MASK | SOCK_NONBLOCK | SOCK_CLOEXEC) != 0 {
        return Err(EINVAL);
    }
    let non_blocking = kind & SOCK_NONBLOCK != 0;
    let close_on_exec = kind & SOCK_CLOEXEC != 0;
    let protocol = protocol as u32;
    let made = |kind| Make { kind, non_blocking, close_on_exec };
    let domain = u16::try_from(domain).unwrap_or(u16::MAX);
    if domain == AF_INET || domain == AF_INET6 {
        let ipv6 = domain == AF_INET6;
        let ping = if ipv6 { IPPROTO_ICMPV6 } else { IPPROTO_ICMP };
        let kind = match kind & SOCK_TYPE_MASK {
            SOCK_STREAM if protocol == 0 || protocol == IPPROTO_TCP => SocketType::Stream,
            SOCK_DGRAM if protocol == ping => SocketType::Ping,
            SOCK_DGRAM if protocol == 0 || protocol == IPPROTO_UDP => SocketType::Datagram,
            SOCK_STREAM | SOCK_DGRAM => return Err(EPROTONOSUPPORT),
            SOCK_RAW => return Err(EPERM),
            SOCK_SEQPACKET => return Err(ESOCKTNOSUPPORT),
            _ => return Err(EINVAL),
        };
        return Ok(made(Kind::Inet { ipv6, kind }));
    }
    if domain != AF_UNIX {
        return Err(EAFNOSUPPORT);
    }
    let datagram = match kind & SOCK_TYPE_MASK {
        SOCK_STREAM => false,
        SOCK_DGRAM => true,
        SOCK_SEQPACKET => return Err(ESOCKTNOSUPPORT),
        _ => return Err(EINVAL),
    };
    if protocol != 0 && protocol != u32::from(AF_UNIX) {
        return Err(EPROTONOSUPPORT);
    }
    Ok(made(Kind::Unix { datagram }))
}

/// What `socketpair(domain, type, protocol)` asks for: two connected
/// `AF_UNIX` sockets. The flags are as [`socket`]'s.
///
/// # Errors
///
/// As [`socket`]'s for `AF_UNIX`, EOPNOTSUPP for a network family, which
/// has no pairs, as on Linux, and EAFNOSUPPORT for any other.
pub fn socketpair(domain: i32, kind: i32, protocol: i32) -> Result<Make, i32> {
    match u16::try_from(domain) {
        Ok(AF_INET | AF_INET6) => Err(EOPNOTSUPP),
        Ok(AF_UNIX) => socket(domain, kind, protocol),
        _ => Err(EAFNOSUPPORT),
    }
}

/// The native socket flags a send's `MSG_` flags ask for. As on Linux, a
/// flag a send has no use for (`MSG_MORE`, `MSG_EOR`, `MSG_CONFIRM`,
/// `MSG_DONTROUTE`) changes nothing.
///
/// # Errors
///
/// EOPNOTSUPP for `MSG_OOB`, as these sockets have no urgent data.
pub fn send_flags(flags: u32) -> Result<u32, i32> {
    if flags & MSG_OOB != 0 {
        return Err(EOPNOTSUPP);
    }
    Ok(native_flags(flags))
}

/// The native socket flags for the `MSG_DONTWAIT` and `MSG_NOSIGNAL` in
/// `flags`, which are the native flags' bits 0 and 1.
fn native_flags(flags: u32) -> u32 {
    u32::from(flags & MSG_DONTWAIT != 0) | (u32::from(flags & MSG_NOSIGNAL != 0) << 1)
}

/// The native socket flags for a receive of a stream's bytes: `peek` and
/// `dont_wait` as asked for. `close_on_exec` is for descriptors that come
/// with the bytes.
pub fn receive_flags(dont_wait: bool, peek: bool, close_on_exec: bool) -> u32 {
    u32::from(dont_wait) | (u32::from(close_on_exec) << 2) | (u32::from(peek) << 3)
}

/// The native flag that asks for a datagram's whole size.
pub const WHOLE_SIZE: u32 = 0x10;

/// Whether a receive can do what its `MSG_` flags ask. As on Linux, a flag
/// a receive has no use for (`MSG_NOSIGNAL`, for one) changes nothing.
///
/// # Errors
///
/// EOPNOTSUPP for `MSG_OOB` and `MSG_ERRQUEUE`, as these sockets keep no
/// urgent data and no queue of errors, and for `MSG_TRUNC` on a stream
/// socket (not a `datagram` one), where it would throw the bytes away.
pub fn check_receive_flags(flags: u32, datagram: bool) -> Result<(), i32> {
    let refused = MSG_OOB | MSG_ERRQUEUE | if datagram { 0 } else { MSG_TRUNC };
    if flags & refused != 0 { Err(EOPNOTSUPP) } else { Ok(()) }
}

/// Writes `bytes` at `at` in `out`. Every offset is a constant of a
/// record's layout, which the record's size covers, so the range exists.
pub(crate) fn put<const N: usize>(out: &mut [u8], at: usize, bytes: [u8; N]) {
    if let Some(field) = out.get_mut(at..at + N) {
        field.copy_from_slice(&bytes);
    }
}

/// The `N` bytes at `at` in `bytes`, zero where `bytes` is short.
pub(crate) fn get<const N: usize>(bytes: &[u8], at: usize) -> [u8; N] {
    let mut field = [0u8; N];
    if let Some(source) = bytes.get(at..at + N) {
        field.copy_from_slice(source);
    }
    field
}

/// The size of `struct sockaddr_un`.
pub const SOCKADDR_UN_SIZE: usize = 110;
/// The offset of `sun_path` in `struct sockaddr_un`.
const SUN_PATH: usize = 2;
/// The size of `sun_path`.
const SUN_PATH_SIZE: usize = 108;
/// The size of `struct sockaddr_in`.
pub const SOCKADDR_IN_SIZE: usize = 16;
/// The size of `struct sockaddr_in6`.
pub const SOCKADDR_IN6_SIZE: usize = 28;
/// The longest name a Unix socket keeps, as the kernel's `SOCKET_NAME_MAX`.
const NAME_MAX: usize = radix_abi::file::SOCKET_NAME_MAX as usize;

/// The family of the socket address in `address`, which is its first two
/// bytes: `None` for an address too short to have one.
pub fn family(address: &[u8]) -> Option<u16> {
    (address.len() >= 2).then(|| u16::from_le_bytes(get(address, 0)))
}

/// The path of an `AF_UNIX` socket address, as the C library takes it: the
/// bytes of `sun_path` before its NUL, or all of them when there is none
/// (the address's length ends them).
///
/// # Errors
///
/// EINVAL for an address too short to hold a family, of another family, or
/// with no path (a socket that is bound to nothing), or whose path starts
/// with a NUL (Linux's abstract names, which Radix does not have), and
/// ENAMETOOLONG for a path longer than the kernel keeps.
pub fn unix_path(address: &[u8]) -> Result<&[u8], i32> {
    if family(address) != Some(AF_UNIX) {
        return Err(EINVAL);
    }
    let path = address.get(SUN_PATH..).unwrap_or(&[]);
    let path = &path[..path.len().min(SUN_PATH_SIZE)];
    let end = path.iter().position(|&b| b == 0).unwrap_or(path.len());
    if end == 0 {
        return Err(EINVAL);
    }
    if end > NAME_MAX {
        return Err(ENAMETOOLONG);
    }
    Ok(&path[..end])
}

/// Writes the `AF_UNIX` address of a socket named `name` (without a NUL;
/// empty for one with no name), and returns its length: 2 for a socket with
/// no name, else the family, the name and its NUL. A name longer than the
/// kernel keeps is cut.
pub fn put_unix(name: &[u8], out: &mut [u8; SOCKADDR_UN_SIZE]) -> usize {
    out.fill(0);
    put(out, 0, AF_UNIX.to_le_bytes());
    if name.is_empty() {
        return SUN_PATH;
    }
    let kept = name.len().min(NAME_MAX);
    out[SUN_PATH..SUN_PATH + kept].copy_from_slice(&name[..kept]);
    SUN_PATH + kept + 1
}

/// The native address of the `AF_INET` or `AF_INET6` socket address in
/// `address`, as the native `Address`'s bytes. An `AF_INET6` address may
/// stop short of `sin6_scope_id`, as RFC 2133's did and as Linux takes it.
///
/// # Errors
///
/// EINVAL for an address too short for its family, EAFNOSUPPORT for any
/// other family.
pub fn inet_address(address: &[u8]) -> Result<[u8; Address::SIZE], i32> {
    let Some(family) = family(address) else {
        return Err(EINVAL);
    };
    let mut native = [0u8; Address::SIZE];
    let port = u16::from_be_bytes(get(address, 2));
    put(&mut native, Address::OFFSET_PORT, port.to_le_bytes());
    match family {
        AF_INET => {
            if address.len() < SOCKADDR_IN_SIZE {
                return Err(EINVAL);
            }
            put(&mut native, Address::OFFSET_FAMILY, AddressFamily::Ipv4.raw().to_le_bytes());
            put(&mut native, Address::OFFSET_IP, get::<4>(address, 4));
        }
        AF_INET6 => {
            if address.len() < SOCKADDR_IN6_SIZE - 4 {
                return Err(EINVAL);
            }
            put(&mut native, Address::OFFSET_FAMILY, AddressFamily::Ipv6.raw().to_le_bytes());
            put(&mut native, Address::OFFSET_IP, get::<16>(address, 8));
            put(&mut native, Address::OFFSET_SCOPE, get::<4>(address, 24));
        }
        _ => return Err(EAFNOSUPPORT),
    }
    Ok(native)
}

/// Writes the socket address for the native address `native`, as an
/// `AF_INET` or `AF_INET6` one by its family, and returns its length.
pub fn put_inet(native: &[u8; Address::SIZE], out: &mut [u8; SOCKADDR_IN6_SIZE]) -> usize {
    out.fill(0);
    let ipv6 = u32::from_le_bytes(get(native, Address::OFFSET_FAMILY)) == AddressFamily::Ipv6.raw();
    let port = u16::from_le_bytes(get(native, Address::OFFSET_PORT));
    put(out, 2, port.to_be_bytes());
    if ipv6 {
        put(out, 0, AF_INET6.to_le_bytes());
        put(out, 8, get::<16>(native, Address::OFFSET_IP));
        put(out, 24, get::<4>(native, Address::OFFSET_SCOPE));
        SOCKADDR_IN6_SIZE
    } else {
        put(out, 0, AF_INET.to_le_bytes());
        put(out, 4, get::<4>(native, Address::OFFSET_IP));
        SOCKADDR_IN_SIZE
    }
}

/// The size of `struct iovec`.
pub const IOVEC_SIZE: usize = 16;
/// The most buffers a `sendmsg` or `recvmsg` takes, Linux's `UIO_MAXIOV`.
pub const IOV_MAX: u64 = 1024;
/// The size of `struct msghdr`.
pub const MSGHDR_SIZE: usize = 56;
/// The size of `struct mmsghdr`: a `msghdr` and the byte count.
pub const MMSGHDR_SIZE: usize = 64;
/// The offset of `msg_len` in a `struct mmsghdr`.
pub const MMSG_LEN: usize = 56;
/// The offset of `msg_namelen` in a `msghdr`, a `u32`.
pub const MSG_NAMELEN: usize = 8;
/// The offset of `msg_controllen` in a `msghdr`, a `u64`.
pub const MSG_CONTROLLEN: usize = 40;
/// The offset of `msg_flags` in a `msghdr`, a `u32`.
pub const MSG_FLAGS: usize = 48;

/// A `struct msghdr`, as a program fills it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Msghdr {
    /// `msg_name`: where a datagram goes, or comes from.
    pub name: u64,
    /// `msg_namelen`.
    pub name_len: u32,
    /// `msg_iov`: the buffers.
    pub iov: u64,
    /// `msg_iovlen`.
    pub iov_len: u64,
    /// `msg_control`: the control messages.
    pub control: u64,
    /// `msg_controllen`.
    pub control_len: u64,
}

impl Msghdr {
    /// Reads a `msghdr`.
    pub fn read(bytes: &[u8; MSGHDR_SIZE]) -> Msghdr {
        Msghdr {
            name: u64::from_le_bytes(get(bytes, 0)),
            name_len: u32::from_le_bytes(get(bytes, MSG_NAMELEN)),
            iov: u64::from_le_bytes(get(bytes, 16)),
            iov_len: u64::from_le_bytes(get(bytes, 24)),
            control: u64::from_le_bytes(get(bytes, 32)),
            control_len: u64::from_le_bytes(get(bytes, MSG_CONTROLLEN)),
        }
    }
}

/// A buffer of a `struct iovec`: where it is, and how long.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Iovec {
    /// `iov_base`.
    pub base: u64,
    /// `iov_len`.
    pub len: u64,
}

impl Iovec {
    /// Reads a `struct iovec`.
    pub fn read(bytes: &[u8; IOVEC_SIZE]) -> Iovec {
        Iovec { base: u64::from_le_bytes(get(bytes, 0)), len: u64::from_le_bytes(get(bytes, 8)) }
    }

    /// Writes it as a `struct iovec`.
    pub fn put(&self) -> [u8; IOVEC_SIZE] {
        let mut bytes = [0u8; IOVEC_SIZE];
        bytes[..8].copy_from_slice(&self.base.to_le_bytes());
        bytes[8..].copy_from_slice(&self.len.to_le_bytes());
        bytes
    }
}

/// The most bytes one send or receive moves, as Linux's `MAX_RW_COUNT`.
pub const MAX_RW_COUNT: u64 = 0x7fff_f000;

/// The sum of the lengths of `iovs` (as `iovec` records, a whole number of
/// them), cut at [`MAX_RW_COUNT`] as Linux cuts it.
///
/// # Errors
///
/// EINVAL for a buffer whose length a signed count cannot hold.
pub fn iov_total(iovs: &[u8]) -> Result<u64, i32> {
    let mut total = 0u64;
    for iov in iovs.as_chunks::<IOVEC_SIZE>().0 {
        let len = u64::from_le_bytes(get(iov, 8));
        if i64::try_from(len).is_err() {
            return Err(EINVAL);
        }
        total = total.saturating_add(len).min(MAX_RW_COUNT);
    }
    Ok(total)
}

/// The number of buffers of a `msghdr`, if a call takes it.
///
/// # Errors
///
/// EMSGSIZE for more than [`IOV_MAX`].
pub fn iov_count(msg_iovlen: u64) -> Result<usize, i32> {
    if msg_iovlen > IOV_MAX { Err(EMSGSIZE) } else { Ok(msg_iovlen as usize) }
}

/// `SOL_SOCKET`, the level of the socket's own options and of the control
/// messages `SCM_RIGHTS` and `SCM_CREDENTIALS`.
pub const SOL_SOCKET: i32 = 1;
/// A control message that carries descriptors.
pub const SCM_RIGHTS: i32 = 1;
/// A control message that carries the sender's process, user and group.
pub const SCM_CREDENTIALS: i32 = 2;
/// The size of a `struct cmsghdr`.
pub const CMSG_HEADER: usize = 16;
/// The size of a `struct ucred`.
pub const UCRED_SIZE: usize = 12;

/// `CMSG_ALIGN`: `len` rounded up to the 8 bytes a control message is
/// aligned to.
pub const fn cmsg_align(len: usize) -> usize {
    (len + 0x7) & !0x7
}

/// `CMSG_LEN`: the length of a control message with `data` bytes of data.
pub const fn cmsg_len(data: usize) -> usize {
    CMSG_HEADER + data
}

/// `CMSG_SPACE`: the room a control message with `data` bytes of data takes
/// in a control buffer.
pub const fn cmsg_space(data: usize) -> usize {
    CMSG_HEADER + cmsg_align(data)
}

/// A control message: its level, its type and its data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cmsg<'a> {
    /// `cmsg_level`.
    pub level: i32,
    /// `cmsg_type`.
    pub kind: i32,
    /// The data after the header, without the padding.
    pub data: &'a [u8],
}

/// The control messages of a control buffer, as `CMSG_FIRSTHDR` and
/// `CMSG_NXTHDR` find them. An item is an error (EINVAL) for a message
/// whose length is shorter than its header or reaches past the buffer; the
/// iteration ends there.
pub struct Cmsgs<'a> {
    control: &'a [u8],
    at: usize,
}

/// The control messages in `control`.
pub fn cmsgs(control: &[u8]) -> Cmsgs<'_> {
    Cmsgs { control, at: 0 }
}

impl<'a> Iterator for Cmsgs<'a> {
    type Item = Result<Cmsg<'a>, i32>;

    fn next(&mut self) -> Option<Self::Item> {
        let rest = self.control.get(self.at..)?;
        if rest.len() < CMSG_HEADER {
            return None;
        }
        let len = u64::from_le_bytes(get(rest, 0));
        let Some(data) = usize::try_from(len)
            .ok()
            .filter(|&len| len >= CMSG_HEADER)
            .and_then(|len| rest.get(CMSG_HEADER..len))
        else {
            self.at = self.control.len();
            return Some(Err(EINVAL));
        };
        let message = Cmsg {
            level: i32::from_le_bytes(get(rest, 8)),
            kind: i32::from_le_bytes(get(rest, 12)),
            data,
        };
        self.at = self.at.saturating_add(cmsg_align(CMSG_HEADER + data.len()));
        Some(Ok(message))
    }
}

/// Writes a `struct ucred` for `process`, `user` and `group`.
pub fn put_ucred(process: u32, user: u32, group: u32) -> [u8; UCRED_SIZE] {
    let mut out = [0u8; UCRED_SIZE];
    put(&mut out, 0, process.to_le_bytes());
    put(&mut out, 4, user.to_le_bytes());
    put(&mut out, 8, group.to_le_bytes());
    out
}

/// Checks the `SCM_CREDENTIALS` message of a send on a Unix socket, as the
/// C library does: the message must be a `struct ucred`'s size (EINVAL),
/// and name the caller's own process, user and group (EPERM), as Linux
/// answers a process without the privilege to send another's. The kernel
/// records the sender itself, so the message is not sent.
pub fn check_credentials(data: &[u8], own: [u32; 3]) -> Result<(), i32> {
    if data.len() != UCRED_SIZE {
        return Err(EINVAL);
    }
    let sent = [get(data, 0), get(data, 4), get(data, 8)].map(u32::from_le_bytes);
    if sent == own { Ok(()) } else { Err(EPERM) }
}

/// Goes through the control messages of a send on a Unix socket, calling
/// `each` with every descriptor an `SCM_RIGHTS` message carries, in order.
/// An `SCM_CREDENTIALS` message is checked ([`check_credentials`]) and not
/// sent, and a message of another level than `SOL_SOCKET` is passed over,
/// as Linux passes it over. `own` is the caller's process, user and group.
///
/// # Errors
///
/// EINVAL for a `SOL_SOCKET` message of any other kind, as Linux gives it,
/// or a message whose length is wrong, and EPERM for credentials that are
/// not the caller's. An error from `each` ends the walk and is returned.
pub fn send_control(
    control: &[u8],
    own: [u32; 3],
    mut each: impl FnMut(i32) -> Result<(), i32>,
) -> Result<(), i32> {
    for message in cmsgs(control) {
        let message = message?;
        match (message.level, message.kind) {
            (SOL_SOCKET, SCM_CREDENTIALS) => check_credentials(message.data, own)?,
            (SOL_SOCKET, SCM_RIGHTS) => {
                for fd in message.data.as_chunks::<4>().0 {
                    each(i32::from_le_bytes(*fd))?;
                }
            }
            (SOL_SOCKET, _) => return Err(EINVAL),
            _ => {}
        }
    }
    Ok(())
}

/// Writes the control messages of a receive on a Unix socket into `out`,
/// which holds at least `cmsg_space(UCRED_SIZE) + cmsg_space(4 * fds.len())`
/// bytes: first, if the socket asks for them, one `SCM_CREDENTIALS` message
/// (`credentials`), then one `SCM_RIGHTS` message holding `fds`, the
/// descriptors that came. `room` is the program's control buffer's length.
///
/// Returns how many bytes of `out` go to the program, which is never more
/// than `room`, and whether the messages were cut (`MSG_CTRUNC`): the
/// credentials are left out whole when the buffer has no room for them and
/// the descriptors with them.
pub fn put_unix_control(
    out: &mut [u8],
    room: usize,
    credentials: Option<[u8; UCRED_SIZE]>,
    fds: &[u32],
) -> (usize, bool) {
    out.fill(0);
    let rights = 4 * fds.len();
    let mut used = 0;
    let mut truncated = false;
    if let Some(credentials) = credentials {
        let needs = if fds.is_empty() {
            cmsg_len(UCRED_SIZE)
        } else {
            cmsg_space(UCRED_SIZE) + cmsg_len(rights)
        };
        if room < needs {
            truncated = true;
        } else {
            put_cmsg(out, used, SOL_SOCKET, SCM_CREDENTIALS, &credentials);
            used += cmsg_space(UCRED_SIZE);
        }
    }
    if !fds.is_empty() {
        put_header(out, used, SOL_SOCKET, SCM_RIGHTS, rights);
        for (i, fd) in fds.iter().enumerate() {
            put(out, used + CMSG_HEADER + 4 * i, fd.to_le_bytes());
        }
        used += cmsg_space(rights);
    }
    (used.min(room), truncated)
}

/// Writes a control message at `at` in `out`: its header and its data,
/// with the padding that follows left as it is. Nothing is written if `out`
/// has no room for all of it.
pub fn put_cmsg(out: &mut [u8], at: usize, level: i32, kind: i32, data: &[u8]) {
    if let Some(message) = out.get_mut(at + CMSG_HEADER..at + CMSG_HEADER + data.len()) {
        message.copy_from_slice(data);
        put_header(out, at, level, kind, data.len());
    }
}

/// Writes the header of a control message at `at` in `out`, for `data`
/// bytes of data.
fn put_header(out: &mut [u8], at: usize, level: i32, kind: i32, data: usize) {
    put(out, at, (cmsg_len(data) as u64).to_le_bytes());
    put(out, at + 8, level.to_le_bytes());
    put(out, at + 12, kind.to_le_bytes());
}

/// How a socket option's value is read and written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    /// An `int`, taken as it is (negative as 0).
    Int,
    /// An `int` whose non-zero values are 1.
    Flag,
    /// A `struct timeval`, which the kernel keeps in nanoseconds.
    Time,
    /// A time to live or hop limit: 1 to 255, and -1 for the default of 64.
    HopLimit,
    /// A multicast hop limit: 0 to 255, and -1 for the default of 1.
    MulticastHops,
    /// Whether multicast datagrams loop back: IPv4's takes any `int` or a
    /// byte, IPv6's only 0 and 1.
    MulticastLoop,
    /// An `int` that is only read: `SO_ERROR`.
    ReadOnly,
    /// The type of service or traffic class: IPv6's -1 to 255, IPv4's any
    /// `int`, whose low byte is kept.
    TypeOfService,
}

/// A network socket option: the native option it is, and how its value
/// reads and writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opt {
    /// The native option.
    pub native: SocketOption,
    /// The value's shape.
    pub shape: Shape,
}

/// The level of TCP's options, `IPPROTO_TCP`.
pub const SOL_TCP: i32 = 6;
/// The level of IPv4's options, `IPPROTO_IP`.
pub const SOL_IP: i32 = 0;
/// The level of IPv6's options, `IPPROTO_IPV6`.
pub const SOL_IPV6: i32 = 41;

/// `SO_TYPE`.
pub const SO_TYPE: i32 = 3;
/// `SO_LINGER`.
pub const SO_LINGER: i32 = 13;
/// `SO_PASSCRED`.
pub const SO_PASSCRED: i32 = 16;
/// `SO_PEERCRED`.
pub const SO_PEERCRED: i32 = 17;
/// `SO_ACCEPTCONN`.
pub const SO_ACCEPTCONN: i32 = 30;
/// `SO_PROTOCOL`.
pub const SO_PROTOCOL: i32 = 38;
/// `SO_DOMAIN`.
pub const SO_DOMAIN: i32 = 39;
/// `SO_ERROR`.
pub const SO_ERROR: i32 = 4;
/// `SO_SNDBUF`.
pub const SO_SNDBUF: i32 = 7;
/// `SO_RCVBUF`.
pub const SO_RCVBUF: i32 = 8;
/// `SO_REUSEADDR`.
pub const SO_REUSEADDR: i32 = 2;
/// `SO_KEEPALIVE`.
pub const SO_KEEPALIVE: i32 = 9;
/// `IP_TOS`.
pub const IP_TOS: i32 = 1;
/// `IPV6_TCLASS`.
pub const IPV6_TCLASS: i32 = 67;

/// The network socket option that `level` and `name` name, if the kernel
/// has it: the options of the C library's table, with Linux's numbers.
/// `SO_LINGER`, `SO_TYPE` and the others whose value is not the native
/// option's alone are not here.
pub fn inet_option(level: i32, name: i32) -> Option<Opt> {
    use Shape::{Flag, HopLimit, Int, MulticastHops, MulticastLoop, ReadOnly, Time, TypeOfService};
    let (native, shape) = match (level, name) {
        (SOL_SOCKET, SO_REUSEADDR) => (SocketOption::ReuseAddress, Int),
        (SOL_SOCKET, 15) => (SocketOption::ReusePort, Flag),
        (SOL_SOCKET, SO_KEEPALIVE) => (SocketOption::KeepAlive, Int),
        (SOL_SOCKET, 6) => (SocketOption::Broadcast, Int),
        (SOL_SOCKET, SO_RCVBUF) => (SocketOption::ReceiveBuffer, Int),
        (SOL_SOCKET, SO_SNDBUF) => (SocketOption::SendBuffer, Int),
        (SOL_SOCKET, SO_ERROR) => (SocketOption::Error, ReadOnly),
        (SOL_SOCKET, 10) => (SocketOption::OutOfBandInline, Int),
        (SOL_SOCKET, 20 | 66) => (SocketOption::ReceiveTimeout, Time),
        (SOL_SOCKET, 21 | 67) => (SocketOption::SendTimeout, Time),
        (SOL_TCP, 1) => (SocketOption::NoDelay, Int),
        (SOL_TCP, 4) => (SocketOption::KeepIdle, Int),
        (SOL_TCP, 5) => (SocketOption::KeepInterval, Int),
        (SOL_TCP, 6) => (SocketOption::KeepCount, Int),
        (SOL_TCP, 12) => (SocketOption::QuickAck, Int),
        (SOL_IP, IP_TOS) | (SOL_IPV6, IPV6_TCLASS) => (SocketOption::TypeOfService, TypeOfService),
        (SOL_IP, 2) => (SocketOption::TimeToLive, HopLimit),
        (SOL_IP, 33) => (SocketOption::MulticastHops, MulticastHops),
        (SOL_IP, 34) => (SocketOption::MulticastLoop, MulticastLoop),
        (SOL_IP, 8) => (SocketOption::PacketInfo, Flag),
        (SOL_IP, 11) => (SocketOption::ReceiveErrors, Flag),
        (SOL_IPV6, 26) => (SocketOption::V6Only, Int),
        (SOL_IPV6, 16) => (SocketOption::TimeToLive, HopLimit),
        (SOL_IPV6, 18) => (SocketOption::MulticastHops, MulticastHops),
        (SOL_IPV6, 19) => (SocketOption::MulticastLoop, MulticastLoop),
        (SOL_IPV6, 49) => (SocketOption::PacketInfo, Flag),
        (SOL_IPV6, 25) => (SocketOption::ReceiveErrors, Flag),
        _ => return None,
    };
    Some(Opt { native, shape })
}

/// The size of `struct timeval`, which a time option's value is.
pub const TIMEVAL_SIZE: usize = 16;
/// The size of `struct linger`.
pub const LINGER_SIZE: usize = 8;
/// Nanoseconds in a second.
const NANOS: u64 = 1_000_000_000;

/// The native value for the option `opt`, from the `value` a program set
/// at `level`, which has the length it was given.
///
/// # Errors
///
/// EINVAL for a value too short for its shape or out of its range, ENOPROTOOPT
/// for a read-only option, and EDOM for a time that is not valid.
pub fn set_value(level: i32, opt: Opt, value: &[u8]) -> Result<u64, i32> {
    if opt.shape == Shape::Time {
        if value.len() < TIMEVAL_SIZE {
            return Err(EINVAL);
        }
        let (sec, usec) = (i64::from_le_bytes(get(value, 0)), i64::from_le_bytes(get(value, 8)));
        return match (u64::try_from(sec), u64::try_from(usec)) {
            (Ok(sec), Ok(usec)) if usec < 1_000_000 => {
                Ok(sec.saturating_mul(NANOS).saturating_add(usec * 1000))
            }
            _ => Err(crate::errno::EDOM),
        };
    }
    // IPv4's multicast options take a byte as well as an `int`.
    let byte = level == SOL_IP
        && matches!(opt.native, SocketOption::MulticastHops | SocketOption::MulticastLoop);
    let int = if value.len() >= 4 {
        i32::from_le_bytes(get(value, 0))
    } else if byte && !value.is_empty() {
        i32::from(u8::from_le_bytes(get(value, 0)))
    } else {
        return Err(EINVAL);
    };
    let int = match opt.shape {
        Shape::HopLimit => match int {
            -1 => 64,
            1..=255 => int,
            _ => return Err(EINVAL),
        },
        Shape::MulticastHops => match int {
            -1 => 1,
            0..=255 => int,
            _ => return Err(EINVAL),
        },
        Shape::MulticastLoop if level == SOL_IPV6 && !matches!(int, 0 | 1) => {
            return Err(EINVAL);
        }
        Shape::MulticastLoop | Shape::Flag => i32::from(int != 0),
        Shape::TypeOfService if level == SOL_IPV6 => match int {
            -1 => 0,
            0..=255 => int,
            _ => return Err(EINVAL),
        },
        Shape::TypeOfService => int & 0xff,
        Shape::ReadOnly => return Err(ENOPROTOOPT),
        Shape::Int | Shape::Time => int,
    };
    Ok(u64::try_from(int).unwrap_or(0))
}

/// The bytes a program reads for the option `opt` at `level`, from the
/// native `value`, in room for `room` bytes, and their count.
///
/// # Errors
///
/// EINVAL for room too small for an `int`, except for the one-byte
/// multicast options and the time, which are cut as Linux cuts them.
pub fn get_value(
    level: i32,
    opt: Opt,
    value: u64,
    room: usize,
) -> Result<([u8; TIMEVAL_SIZE], usize), i32> {
    let mut out = [0u8; TIMEVAL_SIZE];
    if opt.shape == Shape::Time {
        put(&mut out, 0, ((value / NANOS) as i64).to_le_bytes());
        put(&mut out, 8, ((value % NANOS / 1000) as i64).to_le_bytes());
        return Ok((out, room.min(TIMEVAL_SIZE)));
    }
    let byte = level == SOL_IP
        && matches!(opt.native, SocketOption::MulticastHops | SocketOption::MulticastLoop);
    if byte && room > 0 && room < 4 {
        out[0] = value as u8;
        return Ok((out, 1));
    }
    if room < 4 {
        return Err(EINVAL);
    }
    put(&mut out, 0, (value as i32).to_le_bytes());
    Ok((out, 4))
}

/// The native value of `SO_LINGER`'s `struct linger`: 0 for off, else 1
/// plus the seconds, a negative number being as long as can be.
///
/// # Errors
///
/// EINVAL for a value shorter than the record.
pub fn set_linger(value: &[u8]) -> Result<u64, i32> {
    if value.len() < LINGER_SIZE {
        return Err(EINVAL);
    }
    let (on, seconds) = (i32::from_le_bytes(get(value, 0)), i32::from_le_bytes(get(value, 4)));
    let seconds = u64::try_from(seconds).unwrap_or(i32::MAX as u64);
    Ok(if on != 0 { 1 + seconds } else { 0 })
}

/// The `struct linger` of the native value of `SO_LINGER`.
pub fn put_linger(value: u64) -> [u8; LINGER_SIZE] {
    let mut out = [0u8; LINGER_SIZE];
    put(&mut out, 0, i32::from(value != 0).to_le_bytes());
    put(&mut out, 4, (value.saturating_sub(1) as i32).to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sockaddr_in(port: u16, ip: [u8; 4]) -> [u8; SOCKADDR_IN_SIZE] {
        let mut a = [0u8; SOCKADDR_IN_SIZE];
        put(&mut a, 0, AF_INET.to_le_bytes());
        put(&mut a, 2, port.to_be_bytes());
        put(&mut a, 4, ip);
        a
    }

    #[test]
    fn sockets_are_made_as_the_c_library_makes_them() {
        let m = socket(2, 1 | 0o4000 | 0o2_000_000, 0).unwrap();
        assert_eq!(m.kind, Kind::Inet { ipv6: false, kind: SocketType::Stream });
        assert!(m.non_blocking && m.close_on_exec);
        assert_eq!(
            socket(10, 2, 0).unwrap().kind,
            Kind::Inet { ipv6: true, kind: SocketType::Datagram }
        );
        assert_eq!(
            socket(2, 2, 1).unwrap().kind,
            Kind::Inet { ipv6: false, kind: SocketType::Ping }
        );
        assert_eq!(
            socket(10, 2, 58).unwrap().kind,
            Kind::Inet { ipv6: true, kind: SocketType::Ping }
        );
        assert_eq!(socket(2, 2, 58), Err(EPROTONOSUPPORT));
        assert_eq!(socket(2, 1, 17), Err(EPROTONOSUPPORT));
        assert_eq!(socket(1, 1, 0).unwrap().kind, Kind::Unix { datagram: false });
        assert_eq!(socket(1, 2, 1).unwrap().kind, Kind::Unix { datagram: true });
        assert_eq!(socket(1, 2, 6), Err(EPROTONOSUPPORT));
        assert_eq!(socket(1, 5, 0), Err(ESOCKTNOSUPPORT));
        assert_eq!(socket(2, 3, 0), Err(EPERM));
        assert_eq!(socket(1, 3, 0), Err(EINVAL));
        assert_eq!(socket(2, 9, 0), Err(EINVAL));
        assert_eq!(socket(2, 0x10, 0), Err(EINVAL));
        assert_eq!(socket(16, 1, 0), Err(EAFNOSUPPORT));
        assert_eq!(socket(-1, 1, 0), Err(EAFNOSUPPORT));
    }

    #[test]
    fn socket_pairs_are_unix_only() {
        assert_eq!(socketpair(1, 1, 0).unwrap().kind, Kind::Unix { datagram: false });
        assert_eq!(socketpair(2, 1, 0), Err(EOPNOTSUPP));
        assert_eq!(socketpair(10, 1, 0), Err(EOPNOTSUPP));
        assert_eq!(socketpair(16, 1, 0), Err(EAFNOSUPPORT));
    }

    #[test]
    fn send_flags_map_and_refuse() {
        assert_eq!(send_flags(0), Ok(0));
        assert_eq!(send_flags(MSG_DONTWAIT), Ok(1));
        assert_eq!(send_flags(MSG_NOSIGNAL), Ok(2));
        assert_eq!(send_flags(MSG_DONTWAIT | MSG_NOSIGNAL | MSG_MORE), Ok(3));
        // MSG_DONTROUTE, MSG_EOR and MSG_CONFIRM change nothing.
        for flag in [0x4, 0x80, 0x800] {
            assert_eq!(send_flags(flag), Ok(0));
        }
        assert_eq!(send_flags(MSG_OOB | MSG_NOSIGNAL), Err(EOPNOTSUPP));
    }

    #[test]
    fn receive_flags_are_checked_by_kind_of_socket() {
        assert_eq!(check_receive_flags(MSG_PEEK | MSG_WAITALL, false), Ok(()));
        assert_eq!(check_receive_flags(MSG_TRUNC, false), Err(EOPNOTSUPP));
        assert_eq!(check_receive_flags(MSG_TRUNC, true), Ok(()));
        assert_eq!(check_receive_flags(MSG_ERRQUEUE, true), Err(EOPNOTSUPP));
        assert_eq!(check_receive_flags(MSG_OOB, false), Err(EOPNOTSUPP));
        assert_eq!(check_receive_flags(MSG_NOSIGNAL | MSG_CMSG_CLOEXEC, false), Ok(()));
        assert_eq!(receive_flags(true, true, true), 0b1101);
        assert_eq!(receive_flags(false, false, false), 0);
    }

    #[test]
    fn unix_paths_are_taken_as_the_c_library_takes_them() {
        let mut a = [0u8; SOCKADDR_UN_SIZE];
        put(&mut a, 0, AF_UNIX.to_le_bytes());
        a[2..8].copy_from_slice(b"/a/b\0x");
        assert_eq!(unix_path(&a), Ok(&b"/a/b"[..]));
        // Without a NUL the length ends the path.
        assert_eq!(unix_path(&a[..6]), Ok(&b"/a/b"[..]));
        // No path, or an abstract one.
        assert_eq!(unix_path(&a[..2]), Err(EINVAL));
        a[2] = 0;
        assert_eq!(unix_path(&a), Err(EINVAL));
        // Another family, or too short for one.
        a[0] = 2;
        assert_eq!(unix_path(&a), Err(EINVAL));
        assert_eq!(unix_path(&a[..1]), Err(EINVAL));
        // 108 bytes with no NUL are too long for the kernel's 107.
        let mut long = [b'a'; SOCKADDR_UN_SIZE];
        put(&mut long, 0, AF_UNIX.to_le_bytes());
        assert_eq!(unix_path(&long), Err(ENAMETOOLONG));
        assert_eq!(unix_path(&long[..2 + 107]).map(<[u8]>::len), Ok(107));
    }

    #[test]
    fn unix_addresses_are_written_with_their_nul() {
        let mut out = [0xaa; SOCKADDR_UN_SIZE];
        assert_eq!(put_unix(b"/run/x", &mut out), 2 + 6 + 1);
        assert_eq!(&out[..2], &AF_UNIX.to_le_bytes());
        assert_eq!(&out[2..9], b"/run/x\0");
        assert!(out[9..].iter().all(|&b| b == 0));
        assert_eq!(put_unix(b"", &mut out), 2);
        assert!(out[2..].iter().all(|&b| b == 0));
        let long = [b'n'; 200];
        assert_eq!(put_unix(&long, &mut out), 2 + 107 + 1);
    }

    #[test]
    fn inet_addresses_convert_both_ways() {
        let native = inet_address(&sockaddr_in(8080, [10, 0, 2, 2])).unwrap();
        assert_eq!(&native[Address::OFFSET_FAMILY..][..4], &4u32.to_le_bytes());
        assert_eq!(&native[Address::OFFSET_PORT..][..2], &8080u16.to_le_bytes());
        assert_eq!(&native[Address::OFFSET_IP..][..4], &[10, 0, 2, 2]);
        assert!(native[Address::OFFSET_IP + 4..].iter().all(|&b| b == 0));
        let mut out = [0xaa; SOCKADDR_IN6_SIZE];
        assert_eq!(put_inet(&native, &mut out), SOCKADDR_IN_SIZE);
        assert_eq!(&out[..16], &sockaddr_in(8080, [10, 0, 2, 2]));
        assert!(out[16..].iter().all(|&b| b == 0));

        let mut v6 = [0u8; SOCKADDR_IN6_SIZE];
        put(&mut v6, 0, AF_INET6.to_le_bytes());
        put(&mut v6, 2, 443u16.to_be_bytes());
        v6[8..24].copy_from_slice(&[0xfe, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        put(&mut v6, 24, 3u32.to_le_bytes());
        let native = inet_address(&v6).unwrap();
        assert_eq!(&native[Address::OFFSET_FAMILY..][..4], &6u32.to_le_bytes());
        assert_eq!(&native[Address::OFFSET_SCOPE..][..4], &3u32.to_le_bytes());
        let mut back = [0u8; SOCKADDR_IN6_SIZE];
        assert_eq!(put_inet(&native, &mut back), SOCKADDR_IN6_SIZE);
        assert_eq!(back, v6);
        // Without the scope id.
        assert!(inet_address(&v6[..24]).is_ok());
        assert_eq!(inet_address(&v6[..23]), Err(EINVAL));
    }

    #[test]
    fn bad_inet_addresses_fail() {
        assert_eq!(inet_address(&[]), Err(EINVAL));
        assert_eq!(inet_address(&sockaddr_in(1, [0; 4])[..15]), Err(EINVAL));
        let mut a = [0u8; 28];
        put(&mut a, 0, AF_UNIX.to_le_bytes());
        assert_eq!(inet_address(&a), Err(EAFNOSUPPORT));
    }

    #[test]
    fn msghdr_has_linuxs_layout() {
        let mut bytes = [0u8; MSGHDR_SIZE];
        put(&mut bytes, 0, 0x1000u64.to_le_bytes());
        put(&mut bytes, 8, 16u32.to_le_bytes());
        put(&mut bytes, 16, 0x2000u64.to_le_bytes());
        put(&mut bytes, 24, 3u64.to_le_bytes());
        put(&mut bytes, 32, 0x3000u64.to_le_bytes());
        put(&mut bytes, 40, 48u64.to_le_bytes());
        let m = Msghdr::read(&bytes);
        assert_eq!(
            m,
            Msghdr {
                name: 0x1000,
                name_len: 16,
                iov: 0x2000,
                iov_len: 3,
                control: 0x3000,
                control_len: 48
            }
        );
        assert_eq!((MSG_NAMELEN, MSG_CONTROLLEN, MSG_FLAGS, MMSG_LEN), (8, 40, 48, 56));
        assert_eq!(MMSGHDR_SIZE, MSGHDR_SIZE + 8);
    }

    #[test]
    fn an_iovec_is_written_as_it_is_read() {
        let iov = Iovec { base: 0x7fff_0000_1000, len: 77 };
        let bytes = iov.put();
        assert_eq!(Iovec::read(&bytes), iov);
        assert_eq!(bytes[8..], 77u64.to_le_bytes());
    }

    #[test]
    fn iovecs_are_summed_and_cut() {
        let mut iovs = [0u8; 2 * IOVEC_SIZE];
        put(&mut iovs, 8, 100u64.to_le_bytes());
        put(&mut iovs, 24, 28u64.to_le_bytes());
        assert_eq!(iov_total(&iovs), Ok(128));
        assert_eq!(Iovec::read(&get(&iovs, 16)), Iovec { base: 0, len: 28 });
        put(&mut iovs, 8, 0x7fff_ffff_0000u64.to_le_bytes());
        assert_eq!(iov_total(&iovs), Ok(MAX_RW_COUNT));
        put(&mut iovs, 8, u64::MAX.to_le_bytes());
        assert_eq!(iov_total(&iovs), Err(EINVAL));
        assert_eq!(iov_count(1024), Ok(1024));
        assert_eq!(iov_count(1025), Err(EMSGSIZE));
    }

    #[test]
    fn control_sizes_are_linuxs() {
        assert_eq!(cmsg_len(4), 20);
        assert_eq!(cmsg_space(4), 24);
        assert_eq!(cmsg_space(UCRED_SIZE), 32);
        assert_eq!(cmsg_align(0), 0);
        assert_eq!(cmsg_align(17), 24);
    }

    #[test]
    fn control_messages_are_found_in_order() {
        let mut control = [0u8; 56];
        put_cmsg(&mut control, 0, SOL_SOCKET, SCM_RIGHTS, &[1, 0, 0, 0, 2, 0, 0, 0]);
        put_cmsg(&mut control, 24, SOL_SOCKET, SCM_CREDENTIALS, &[7; UCRED_SIZE]);
        let all: Vec<_> = cmsgs(&control).collect();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0], Ok(Cmsg { level: 1, kind: 1, data: &[1, 0, 0, 0, 2, 0, 0, 0] }));
        assert_eq!(all[1], Ok(Cmsg { level: 1, kind: 2, data: &[7; UCRED_SIZE] }));
        // A header alone is not a message past the first, and nothing is
        // found in a buffer too short for one.
        assert_eq!(cmsgs(&control[..15]).count(), 0);
        assert_eq!(cmsgs(&[]).count(), 0);
    }

    #[test]
    fn a_bad_control_message_ends_the_walk() {
        let mut control = [0u8; 32];
        put(&mut control, 0, 8u64.to_le_bytes());
        assert_eq!(cmsgs(&control).collect::<Vec<_>>(), [Err(EINVAL)]);
        put(&mut control, 0, 33u64.to_le_bytes());
        assert_eq!(cmsgs(&control).collect::<Vec<_>>(), [Err(EINVAL)]);
        put(&mut control, 0, u64::MAX.to_le_bytes());
        assert_eq!(cmsgs(&control).collect::<Vec<_>>(), [Err(EINVAL)]);
    }

    #[test]
    fn rights_are_collected_and_credentials_checked() {
        let own = [10, 20, 30];
        let mut control = [0u8; 64];
        put_cmsg(&mut control, 0, SOL_SOCKET, SCM_CREDENTIALS, &put_ucred(10, 20, 30));
        put_cmsg(&mut control, 32, SOL_SOCKET, SCM_RIGHTS, &[5, 0, 0, 0, 6, 0, 0, 0]);
        let mut fds = Vec::new();
        assert_eq!(
            send_control(&control[..56], own, |fd| {
                fds.push(fd);
                Ok(())
            }),
            Ok(())
        );
        assert_eq!(fds, [5, 6]);
        // Another process's credentials.
        put_cmsg(&mut control, 0, SOL_SOCKET, SCM_CREDENTIALS, &put_ucred(11, 20, 30));
        assert_eq!(send_control(&control[..56], own, |_| Ok(())), Err(EPERM));
        // Another message, and a credentials message of the wrong length.
        put_cmsg(&mut control, 0, SOL_SOCKET, 29, &[0; 4]);
        assert_eq!(send_control(&control[..20], own, |_| Ok(())), Err(EINVAL));
        put_cmsg(&mut control, 0, SOL_SOCKET, SCM_CREDENTIALS, &[0; 8]);
        assert_eq!(send_control(&control[..24], own, |_| Ok(())), Err(EINVAL));
        // A descriptor the caller refuses ends the walk.
        put_cmsg(&mut control, 0, SOL_SOCKET, SCM_RIGHTS, &[5, 0, 0, 0]);
        assert_eq!(send_control(&control[..20], own, |_| Err(9)), Err(9));
        // A message of another level is passed over.
        put_cmsg(&mut control, 0, SOL_IP, 8, &[1, 0, 0, 0]);
        put_cmsg(&mut control, 24, SOL_SOCKET, SCM_RIGHTS, &[7, 0, 0, 0]);
        let mut fds = Vec::new();
        let collect = |fd| {
            fds.push(fd);
            Ok(())
        };
        assert_eq!(send_control(&control[..44], own, collect), Ok(()));
        assert_eq!(fds, [7]);
    }

    #[test]
    fn received_control_has_credentials_then_descriptors() {
        let creds = put_ucred(4, 5, 6);
        let mut out = [0u8; 64];
        // Both fit.
        let (used, cut) = put_unix_control(&mut out, 64, Some(creds), &[7, 8]);
        assert_eq!((used, cut), (32 + 24, false));
        let all: Vec<_> = cmsgs(&out[..used]).map(Result::unwrap).collect();
        assert_eq!(all[0].kind, SCM_CREDENTIALS);
        assert_eq!(all[0].data, &creds);
        assert_eq!(all[1].kind, SCM_RIGHTS);
        assert_eq!(all[1].data, &[7, 0, 0, 0, 8, 0, 0, 0]);
        // Descriptors alone.
        let (used, cut) = put_unix_control(&mut out, 64, None, &[7]);
        assert_eq!((used, cut), (24, false));
        // No room for the credentials, and so none for the descriptors
        // with them.
        let (used, cut) = put_unix_control(&mut out, 40, Some(creds), &[7, 8]);
        assert!(cut);
        assert_eq!(used, 24);
        // Credentials alone, in exactly their length.
        let (used, cut) = put_unix_control(&mut out, 28, Some(creds), &[]);
        assert_eq!((used, cut), (28, false));
        // Nothing came.
        assert_eq!(put_unix_control(&mut out, 64, None, &[]), (0, false));
    }

    #[test]
    fn options_have_linuxs_numbers() {
        let o = |l, n| inet_option(l, n).map(|o| o.native);
        assert_eq!(o(1, 2), Some(SocketOption::ReuseAddress));
        assert_eq!(o(1, 15), Some(SocketOption::ReusePort));
        assert_eq!(o(1, 9), Some(SocketOption::KeepAlive));
        assert_eq!(o(1, 6), Some(SocketOption::Broadcast));
        assert_eq!(o(1, 8), Some(SocketOption::ReceiveBuffer));
        assert_eq!(o(1, 7), Some(SocketOption::SendBuffer));
        assert_eq!(o(1, 4), Some(SocketOption::Error));
        assert_eq!(o(1, 10), Some(SocketOption::OutOfBandInline));
        assert_eq!(o(1, 20), Some(SocketOption::ReceiveTimeout));
        assert_eq!(o(1, 66), Some(SocketOption::ReceiveTimeout));
        assert_eq!(o(1, 21), Some(SocketOption::SendTimeout));
        assert_eq!(o(6, 1), Some(SocketOption::NoDelay));
        assert_eq!(o(6, 4), Some(SocketOption::KeepIdle));
        assert_eq!(o(6, 5), Some(SocketOption::KeepInterval));
        assert_eq!(o(6, 6), Some(SocketOption::KeepCount));
        assert_eq!(o(6, 12), Some(SocketOption::QuickAck));
        assert_eq!(o(0, 2), Some(SocketOption::TimeToLive));
        assert_eq!(o(41, 16), Some(SocketOption::TimeToLive));
        assert_eq!(o(0, 1), Some(SocketOption::TypeOfService));
        assert_eq!(o(41, 67), Some(SocketOption::TypeOfService));
        assert_eq!(o(41, 26), Some(SocketOption::V6Only));
        assert_eq!(o(0, 8), Some(SocketOption::PacketInfo));
        assert_eq!(o(41, 49), Some(SocketOption::PacketInfo));
        assert_eq!(o(1, 1), None);
        assert_eq!(o(99, 1), None);
    }

    fn int(value: i32) -> [u8; 4] {
        value.to_le_bytes()
    }

    #[test]
    fn option_values_are_checked_as_the_c_library_checks_them() {
        let opt = |l, n| inet_option(l, n).unwrap();
        let ttl = opt(SOL_IP, 2);
        assert_eq!(set_value(SOL_IP, ttl, &int(-1)), Ok(64));
        assert_eq!(set_value(SOL_IP, ttl, &int(255)), Ok(255));
        assert_eq!(set_value(SOL_IP, ttl, &int(0)), Err(EINVAL));
        assert_eq!(set_value(SOL_IP, ttl, &int(256)), Err(EINVAL));
        assert_eq!(set_value(SOL_IP, ttl, &[1, 2]), Err(EINVAL));
        let hops = opt(SOL_IP, 33);
        assert_eq!(set_value(SOL_IP, hops, &int(-1)), Ok(1));
        assert_eq!(set_value(SOL_IP, hops, &int(0)), Ok(0));
        assert_eq!(set_value(SOL_IP, hops, &[7]), Ok(7));
        let looping = opt(SOL_IP, 34);
        assert_eq!(set_value(SOL_IP, looping, &int(5)), Ok(1));
        assert_eq!(set_value(SOL_IPV6, opt(SOL_IPV6, 19), &int(5)), Err(EINVAL));
        assert_eq!(set_value(SOL_IPV6, opt(SOL_IPV6, 19), &int(1)), Ok(1));
        let tos = opt(SOL_IP, IP_TOS);
        assert_eq!(set_value(SOL_IP, tos, &int(0x1ff)), Ok(0xff));
        assert_eq!(set_value(SOL_IPV6, opt(SOL_IPV6, IPV6_TCLASS), &int(-1)), Ok(0));
        assert_eq!(set_value(SOL_IPV6, opt(SOL_IPV6, IPV6_TCLASS), &int(256)), Err(EINVAL));
        assert_eq!(set_value(SOL_SOCKET, opt(SOL_SOCKET, 15), &int(9)), Ok(1));
        assert_eq!(set_value(SOL_SOCKET, opt(SOL_SOCKET, 2), &int(-4)), Ok(0));
        assert_eq!(set_value(SOL_SOCKET, opt(SOL_SOCKET, 4), &int(0)), Err(ENOPROTOOPT));
        assert_eq!(set_value(SOL_SOCKET, opt(SOL_SOCKET, 2), &[]), Err(EINVAL));
    }

    #[test]
    fn times_are_timevals_in_nanoseconds() {
        let opt = inet_option(SOL_SOCKET, 20).unwrap();
        let mut tv = [0u8; TIMEVAL_SIZE];
        put(&mut tv, 0, 2i64.to_le_bytes());
        put(&mut tv, 8, 500_000i64.to_le_bytes());
        assert_eq!(set_value(SOL_SOCKET, opt, &tv), Ok(2_500_000_000));
        put(&mut tv, 8, 1_000_000i64.to_le_bytes());
        assert_eq!(set_value(SOL_SOCKET, opt, &tv), Err(crate::errno::EDOM));
        put(&mut tv, 0, (-1i64).to_le_bytes());
        assert_eq!(set_value(SOL_SOCKET, opt, &tv), Err(crate::errno::EDOM));
        assert_eq!(set_value(SOL_SOCKET, opt, &tv[..8]), Err(EINVAL));
        let (out, len) = get_value(SOL_SOCKET, opt, 2_500_000_000, 16).unwrap();
        assert_eq!(
            (i64::from_le_bytes(get(&out, 0)), i64::from_le_bytes(get(&out, 8))),
            (2, 500_000)
        );
        assert_eq!(len, 16);
        assert_eq!(get_value(SOL_SOCKET, opt, 0, 8).unwrap().1, 8);
    }

    #[test]
    fn option_values_are_read_as_ints_or_bytes() {
        let opt = |l, n| inet_option(l, n).unwrap();
        let (out, len) = get_value(SOL_SOCKET, opt(SOL_SOCKET, 2), 1, 4).unwrap();
        assert_eq!((&out[..len], len), (&int(1)[..], 4));
        assert_eq!(get_value(SOL_SOCKET, opt(SOL_SOCKET, 2), 1, 3), Err(EINVAL));
        // More room than an int gives an int.
        assert_eq!(get_value(SOL_SOCKET, opt(SOL_SOCKET, 2), 1, 64).unwrap().1, 4);
        // The multicast options give a byte for a byte of room.
        let (out, len) = get_value(SOL_IP, opt(SOL_IP, 33), 3, 1).unwrap();
        assert_eq!((out[0], len), (3, 1));
        assert_eq!(get_value(SOL_IP, opt(SOL_IP, 33), 3, 0), Err(EINVAL));
    }

    #[test]
    fn linger_is_one_more_than_its_seconds() {
        let mut linger = [0u8; LINGER_SIZE];
        assert_eq!(set_linger(&linger), Ok(0));
        put(&mut linger, 0, 1i32.to_le_bytes());
        put(&mut linger, 4, 5i32.to_le_bytes());
        assert_eq!(set_linger(&linger), Ok(6));
        assert_eq!(put_linger(6), linger);
        put(&mut linger, 4, (-1i32).to_le_bytes());
        assert_eq!(set_linger(&linger), Ok(1 + i32::MAX as u64));
        assert_eq!(set_linger(&linger[..7]), Err(EINVAL));
        assert_eq!(put_linger(0), [0; LINGER_SIZE]);
    }

    #[test]
    fn family_reads_the_first_two_bytes() {
        assert_eq!(family(&[]), None);
        assert_eq!(family(&[1]), None);
        assert_eq!(family(&[10, 0, 9]), Some(AF_INET6));
    }

    proptest::proptest! {
        #[test]
        fn no_address_makes_a_reader_panic(bytes in proptest::collection::vec(0u8.., 0..140)) {
            let _ = unix_path(&bytes);
            let _ = inet_address(&bytes);
            let _ = cmsgs(&bytes).count();
            let _ = send_control(&bytes, [1, 2, 3], |_| Ok(()));
            let _ = iov_total(&bytes);
        }
    }
}
