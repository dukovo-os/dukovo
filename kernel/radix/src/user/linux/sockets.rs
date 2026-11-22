// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Linux's socket calls (ADR 0610): sockets, their names and connections,
//! sends and receives, and their options, each ending in a native method of
//! the socket's own capability, as Dukovo's C library makes the same calls
//! (`ports/mlibc/sysdeps/radix/generic/socket.cpp`).
//!
//! An `AF_UNIX` socket is a pair or a socket the process's methods make,
//! named through a directory's `bind_socket` and `connect_socket` by a path
//! resolved as every path is (`paths`); an `AF_INET` or `AF_INET6` socket
//! is the kernel's TCP, UDP or ping, made, bound, connected and sent from
//! through the network capabilities in the process's own slots, each tried
//! in turn until one allows the call, and EACCES when none does. A Linux
//! socket's descriptor is its slot, so the descriptors a program passes
//! over a socket are the slots the native `send_files` and `receive_files`
//! move.
//!
//! The calls that take or give a Radix record (a network address, a
//! socket's name, the slots of descriptors) run the native method's work on
//! records in the kernel's memory (`crate::user::files`), so the call maps
//! nothing in the program's address space. The program's own buffers are
//! given to the socket as they are: the `iovec`s of a message are one
//! `Buffers`, which a send reads from and a receive writes to in place.

use alloc::vec::Vec;

use radix_abi::capability::{self, ObjectKind, Rights};
use radix_abi::directory;
use radix_abi::errors::{File, Kernel};
use radix_abi::file::{self, SocketFlags, SocketOption};
use radix_abi::network::{self, Address, SocketType};
use radix_abi::process::{self, PipeFlags};
use radix_abi::startup::{NETWORK_SLOTS, SLOT_FD_BASE, SLOT_NETWORK, SLOT_PROCESS};
use radix_abi::{Status, UserBuffer};
use radix_linux::errno::{
    self, EACCES, EADDRINUSE, EAFNOSUPPORT, EALREADY, EBADF, EINVAL, EMSGSIZE, ENOBUFS, ENOMEM,
    ENOPROTOOPT, ENOTSOCK, EOPNOTSUPP,
};
use radix_linux::records::{self, TIMESPEC_SIZE};
use radix_linux::sockets::{
    self as lx, CMSG_HEADER, IOVEC_SIZE, Iovec, MMSG_LEN, MMSGHDR_SIZE, MSG_CONTROLLEN, MSG_FLAGS,
    MSG_NAMELEN, MSGHDR_SIZE, Msghdr, SOCKADDR_UN_SIZE, UCRED_SIZE,
};
use radix_mm::Kmalloc;
use radix_vfs::Out;

use super::files::directory as call_directory;
use super::files::{at_path, delete, descriptor, given, into_free, on_object};
use super::{
    Args, Error, Result, copy_in, copy_out, fail, is, kmalloc, paths, process as linux_process,
    results,
};
use crate::fs::inet::given_of;
use crate::sched::{self, ThreadRef};
use crate::user::UserRef;
use crate::user::files::{self as native, Buffers, SocketName, UnixReceived};

/// The most control bytes a send takes, as Linux's `optmem_max`.
const CONTROL_MAX: u64 = 20480;
/// The longest socket address a call takes: `sizeof(sockaddr_storage)`.
const SOCKADDR_MAX: i32 = 128;
/// The most bytes of an option's value a `setsockopt` reads.
const OPTION_MAX: usize = 64;
/// `recvmmsg`'s `MSG_WAITFORONE`: no waiting after the first message.
const MSG_WAITFORONE: u32 = 0x1_0000;
/// The most messages `sendmmsg` and `recvmmsg` take, `UIO_MAXIOV`.
const VLEN_MAX: u32 = 1024;

/// `len` zero bytes in the kernel's memory, or ENOMEM.
pub(super) fn zeroed(len: usize) -> Result<Vec<u8, Kmalloc>> {
    radix_mm::bytes::zeroed(len, kmalloc()).map_err(|_| Error::Errno(ENOMEM))
}

/// The `len` bytes at `address` in the program's memory, in the kernel's.
pub(super) fn read_bytes(address: u64, len: usize) -> Result<Vec<u8, Kmalloc>> {
    let mut bytes = zeroed(len)?;
    copy_in(address, &mut bytes)?;
    Ok(bytes)
}

/// A socket method's failure as the C library gives it: what is no socket
/// (a directory, a file) is ENOTSOCK, and a socket not open for the call's
/// direction is EBADF, as for `read` and `write`.
fn socket_error(err: Error) -> Error {
    match err {
        Error::Native(status)
            if is(status, Kernel::NoSuchMethod) || is(status, Kernel::WrongKind) =>
        {
            Error::Errno(ENOTSOCK)
        }
        Error::Native(status) if is(status, Kernel::MissingRights) => Error::Errno(EBADF),
        err => err,
    }
}

/// Whether `status` is a refusal by the network capability the call was
/// made through, or its absence from the slot: the next capability may
/// allow what this one does not.
fn not_allowed(status: Status) -> bool {
    match Kernel::from_status(status) {
        Some(Kernel::EmptySlot | Kernel::NoSuchSlot | Kernel::WrongKind | Kernel::OutOfRange) => {
            true
        }
        // Without the send, local or receive right (ADR 0337); a socket's
        // own descriptor without the right a call needs is a bad one.
        Some(Kernel::MissingRights) => {
            let wanted = Rights::SEND.union(Rights::LOCAL).union(Rights::RECEIVE);
            u64::from(status.detail()) & wanted.bits() != 0
        }
        _ => false,
    }
}

/// [`socket_error`], and EACCES for a call a network capability refused.
fn inet_error(err: Error) -> Error {
    match err {
        Error::Native(status) if not_allowed(status) => Error::Errno(EACCES),
        err => socket_error(err),
    }
}

/// Makes `call` with each network capability's slot in turn until one
/// allows it: its answer, or the error of the first failure that is not a
/// refusal, or EACCES if every one refused. A call fails with
/// `not_supported` for an address of the other family, which is
/// EAFNOSUPPORT, as on Linux.
fn through_network<T>(mut call: impl FnMut(u32) -> Result<T>) -> Result<T> {
    for i in 0..NETWORK_SLOTS {
        match call(SLOT_NETWORK + i) {
            Err(Error::Native(status)) if is(status, Kernel::NotSupported) => {
                return fail(EAFNOSUPPORT);
            }
            Err(Error::Native(status)) if not_allowed(status) => {}
            Err(err) => return Err(socket_error(err)),
            done => return done,
        }
    }
    fail(EACCES)
}

/// What kind of socket a descriptor is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flavor {
    /// A network socket, a stream, a datagram or a ping socket.
    Inet(SocketType),
    /// A Unix socket.
    Unix { datagram: bool },
}

/// Reads, or with `set` writes, socket option `option` of `slot`: its
/// value.
fn option(
    me: &ThreadRef,
    user: &UserRef,
    slot: u32,
    option: SocketOption,
    set: Option<u64>,
) -> Result<u64> {
    let args = file::SocketOptionArgs { option, set: set.is_some(), value: set.unwrap_or(0) };
    let words = results(me, user, slot, file::SOCKET_OPTION, args.encode())?;
    Ok(file::SocketOptionResults::decode(&words)?.value)
}

/// The kind of socket in `slot`: a network socket answers the `kind`
/// option, a Unix socket says it is not one and answers `unix_kind`.
fn flavor(me: &ThreadRef, user: &UserRef, slot: u32) -> Result<Flavor> {
    match option(me, user, slot, SocketOption::Kind, None) {
        Ok(kind) => {
            Ok(Flavor::Inet(SocketType::from_raw(kind as u32).unwrap_or(SocketType::Stream)))
        }
        Err(Error::Native(status)) if is(status, Kernel::NotSupported) => {
            let kind =
                option(me, user, slot, SocketOption::UnixKind, None).map_err(socket_error)?;
            Ok(Flavor::Unix { datagram: kind == u64::from(SocketType::Datagram.raw()) })
        }
        Err(err) => Err(socket_error(err)),
    }
}

/// Whether Unix socket `slot` asks for the credentials of what it receives
/// (`SO_PASSCRED`); what is no Unix socket does not.
fn passes_credentials(me: &ThreadRef, user: &UserRef, slot: u32) -> bool {
    option(me, user, slot, SocketOption::PassCredentials, None).is_ok_and(|value| value != 0)
}

/// The `PipeFlags` for `SOCK_NONBLOCK` and `SOCK_CLOEXEC`.
fn description_flags(non_blocking: bool, close_on_exec: bool) -> PipeFlags {
    let mut flags = PipeFlags::NONE;
    if non_blocking {
        flags = flags.union(PipeFlags::NON_BLOCKING);
    }
    if close_on_exec {
        flags = flags.union(PipeFlags::CLOSE_ON_EXEC);
    }
    flags
}

/// `socket(domain, type, protocol)`: the process's `socket` or
/// `datagram_socket` for an `AF_UNIX` socket, a network capability's
/// `socket` for a network one, into the lowest free descriptor.
pub fn socket(
    me: &ThreadRef,
    user: &UserRef,
    domain: u64,
    kind: u64,
    protocol: u64,
) -> Result<u64> {
    let make = lx::socket(domain as i32, kind as i32, protocol as i32).map_err(Error::Errno)?;
    let flags = description_flags(make.non_blocking, make.close_on_exec);
    into_free(user, 0, |slot| match make.kind {
        lx::Kind::Unix { datagram } => {
            let (label, words) = if datagram {
                (process::DATAGRAM_SOCKET, process::DatagramSocketArgs { to: slot, flags }.encode())
            } else {
                (process::SOCKET, process::SocketArgs { to: slot, flags }.encode())
            };
            results(me, user, SLOT_PROCESS, label, words).map_err(socket_error)?;
            Ok(slot)
        }
        lx::Kind::Inet { ipv6, kind } => {
            let family =
                if ipv6 { network::AddressFamily::Ipv6 } else { network::AddressFamily::Ipv4 };
            through_network(|network| {
                let args = network::SocketArgs { to: slot, family, kind, flags };
                results(me, user, network, network::SOCKET, args.encode())
            })?;
            Ok(slot)
        }
    })
    .map(descriptor)
}

/// `socketpair(domain, type, protocol, sv)`: the process's `socket_pair`
/// or `datagram_pair`, into the two lowest free descriptors, which it
/// writes to `sv`.
pub fn socketpair(
    me: &ThreadRef,
    user: &UserRef,
    domain: u64,
    kind: u64,
    protocol: u64,
    sv: u64,
) -> Result<u64> {
    let make = lx::socketpair(domain as i32, kind as i32, protocol as i32).map_err(Error::Errno)?;
    let lx::Kind::Unix { datagram } = make.kind else { return fail(EOPNOTSUPP) };
    let flags = description_flags(make.non_blocking, make.close_on_exec);
    let (first, second) = into_free(user, 0, |first| {
        into_free(user, first - SLOT_FD_BASE + 1, |second| {
            let (label, words) = if datagram {
                (
                    process::DATAGRAM_PAIR,
                    process::DatagramPairArgs { first, second, flags }.encode(),
                )
            } else {
                (process::SOCKET_PAIR, process::SocketPairArgs { first, second, flags }.encode())
            };
            results(me, user, SLOT_PROCESS, label, words).map_err(socket_error)?;
            Ok((first, second))
        })
    })?;
    let mut out = [0u8; 8];
    out[..4].copy_from_slice(&(descriptor(first) as i32).to_le_bytes());
    out[4..].copy_from_slice(&(descriptor(second) as i32).to_le_bytes());
    if let Err(err) = copy_out(sv, &out) {
        delete(me, user, first);
        delete(me, user, second);
        return Err(err);
    }
    Ok(0)
}

/// The socket address of `len` bytes at `address`, in the kernel's memory.
fn read_sockaddr(address: u64, len: u64) -> Result<Vec<u8, Kmalloc>> {
    let len = len as i32;
    if !(0..=SOCKADDR_MAX).contains(&len) {
        return fail(EINVAL);
    }
    read_bytes(address, len as usize)
}

/// Writes a socket address `name` for a program that gave `address` and
/// `length`, as Linux does: at most the length it gave (a `socklen_t` it
/// reads), and the address's whole length in its place.
fn put_sockaddr(address: u64, length: u64, name: &[u8]) -> Result<()> {
    let mut room = [0u8; 4];
    copy_in(length, &mut room)?;
    let Ok(room) = usize::try_from(i32::from_le_bytes(room)) else { return fail(EINVAL) };
    let kept = room.min(name.len());
    if kept > 0 {
        copy_out(address, &name[..kept])?;
    }
    copy_out(length, &(name.len() as u32).to_le_bytes())
}

/// A socket's address as a program reads it.
struct Sockaddr {
    bytes: [u8; SOCKADDR_UN_SIZE],
    len: usize,
}

impl Sockaddr {
    /// The address's bytes.
    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len.min(SOCKADDR_UN_SIZE)]
    }

    /// The address of a network socket, from the native address's bytes.
    fn inet(native: &[u8; Address::SIZE]) -> Sockaddr {
        let mut name = [0u8; lx::SOCKADDR_IN6_SIZE];
        let len = lx::put_inet(native, &mut name);
        let mut bytes = [0u8; SOCKADDR_UN_SIZE];
        bytes[..name.len()].copy_from_slice(&name);
        Sockaddr { bytes, len }
    }

    /// The address of a Unix socket named `name`.
    fn unix(name: &[u8]) -> Sockaddr {
        let mut bytes = [0u8; SOCKADDR_UN_SIZE];
        let len = lx::put_unix(name, &mut bytes);
        Sockaddr { bytes, len }
    }
}

/// The address of socket `slot`, or with `peer` of the socket at the other
/// end of its connection: the native `socket_name`.
fn socket_address(user: &UserRef, slot: u32, peer: bool) -> Result<Sockaddr> {
    let name =
        on_object(user, slot, |found| native::socket_name_of(found, peer)).map_err(socket_error)?;
    Ok(match name {
        SocketName::Inet(address) => Sockaddr::inet(&address),
        SocketName::Unix(name) => Sockaddr::unix(name.bytes()),
    })
}

/// `getsockname` and `getpeername` (`peer`): the native `socket_name`.
fn name_of(user: &UserRef, fd: u64, address: u64, length: u64, peer: bool) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    let name = socket_address(user, slot, peer)?;
    put_sockaddr(address, length, name.as_bytes()).map(|()| 0)
}

/// `getsockname(fd, addr, addrlen)`.
pub fn getsockname(user: &UserRef, fd: u64, address: u64, length: u64) -> Result<u64> {
    name_of(user, fd, address, length, false)
}

/// `getpeername(fd, addr, addrlen)`.
pub fn getpeername(user: &UserRef, fd: u64, address: u64, length: u64) -> Result<u64> {
    name_of(user, fd, address, length, true)
}

/// The path of a Unix socket name, for the directory methods: they resolve
/// it as any path of the program is, from its working directory.
fn on_name<T>(
    user: &UserRef,
    name: &[u8],
    mut call: impl FnMut(&paths::Place, &[u8]) -> Result<T>,
) -> Result<T> {
    let path = lx::unix_path(name).map_err(Error::Errno)?;
    at_path(user, paths::AT_FDCWD, path, false, |place| call(place, path))
}

/// Binds Unix socket `slot` to the name in the socket address `address`:
/// the native `bind_socket` of the directory it is resolved beneath.
fn bind_unix(user: &UserRef, slot: u32, address: &[u8]) -> Result<()> {
    on_name(user, address, |place, name| {
        let args = directory::BindSocketArgs {
            path: given(0, place.path.len()),
            socket: slot,
            name: given(1, name.len()),
        };
        match call_directory(user, place, directory::BIND_SOCKET, args.encode(), &[name]) {
            // A file of that name is there already.
            Err(Error::Native(status)) if File::from_status(status) == Some(File::Exists) => {
                fail(EADDRINUSE)
            }
            Err(Error::Native(status)) if is(status, Kernel::InvalidArgument) => fail(EINVAL),
            Err(err) => Err(socket_error(err)),
            Ok(_) => Ok(()),
        }
    })
}

/// Whether network capability `network` has the receive right on every
/// port the kernel may pick, so that a socket bound through it to port 0
/// may listen on whichever port it gets.
fn receives_on_picked_ports(me: &ThreadRef, user: &UserRef, network: u32) -> bool {
    let Ok(words) = results(me, user, network, network::PORTS, [0; 8]) else { return false };
    let Ok(ports) = network::PortsResults::decode(&words) else { return false };
    u64::from(ports.first) <= network::EPHEMERAL_FIRST
        && u64::from(ports.first) + u64::from(ports.count) >= 65536
}

/// Binds network socket `slot` to the native `address` through the network
/// capability in slot `network`: the native `bind_address`.
fn bind_through(
    user: &UserRef,
    slot: u32,
    network: u32,
    address: &[u8; Address::SIZE],
) -> Result<()> {
    on_object(user, slot, |found| {
        native::bind_address_of(user, found, network, || given_of(address))
    })
}

/// Binds network socket `slot` to the native `address` through a network
/// capability with the receive right, as a socket that is to listen must be
/// bound: one that receives on every port the kernel may pick comes first,
/// then any other with the right. EACCES if there is none.
fn bind_receiving(
    me: &ThreadRef,
    user: &UserRef,
    slot: u32,
    address: &[u8; Address::SIZE],
) -> Result<()> {
    let mut chosen = None;
    for i in 0..NETWORK_SLOTS {
        let network = SLOT_NETWORK + i;
        let Ok(words) = results(me, user, network, capability::INFO, [0; 8]) else { continue };
        let Ok(info) = capability::InfoResults::decode(&words) else { continue };
        if info.kind != ObjectKind::Network || !info.rights.contains(Rights::RECEIVE) {
            continue;
        }
        if receives_on_picked_ports(me, user, network) {
            chosen = Some(network);
            break;
        }
        chosen.get_or_insert(network);
    }
    let Some(network) = chosen else { return fail(EACCES) };
    bind_through(user, slot, network, address).map_err(inet_error)
}

/// `bind(fd, addr, addrlen)`: a Unix socket's `bind_socket`, a network
/// socket's `bind_address`.
pub fn bind(me: &ThreadRef, user: &UserRef, fd: u64, address: u64, length: u64) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    let name = read_sockaddr(address, length)?;
    match flavor(me, user, slot)? {
        Flavor::Unix { .. } => bind_unix(user, slot, &name)?,
        Flavor::Inet(kind) => {
            let bytes = lx::inet_address(&name).map_err(Error::Errno)?;
            // A stream socket bound to a port the kernel picks is likely to
            // listen next, so it is bound through a capability that may
            // receive, if there is one.
            let port =
                u16::from_le_bytes([bytes[Address::OFFSET_PORT], bytes[Address::OFFSET_PORT + 1]]);
            if kind == SocketType::Stream && port == 0 {
                match bind_receiving(me, user, slot, &bytes) {
                    Err(Error::Errno(EACCES)) => {}
                    done => return done.map(|()| 0),
                }
            }
            through_network(|network| bind_through(user, slot, network, &bytes))?;
        }
    }
    Ok(0)
}

/// `listen(fd, backlog)`: the native `listen`. A network socket that is not
/// bound is bound to a port the kernel picks first, as on Linux.
pub fn listen(me: &ThreadRef, user: &UserRef, fd: u64, backlog: u64) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    let most = u32::try_from(backlog as i32)
        .ok()
        .filter(|&backlog| u64::from(backlog) <= file::BACKLOG_MAX)
        .unwrap_or(file::BACKLOG_MAX as u32);
    let args = file::ListenArgs { backlog: most };
    let Err(err) = results(me, user, slot, file::LISTEN, args.encode()) else { return Ok(0) };
    if !matches!(flavor(me, user, slot), Ok(Flavor::Inet(_))) {
        return Err(socket_error(err));
    }
    match err {
        Error::Native(status) if not_allowed(status) => listen_unbound(me, user, slot, args, err),
        err => Err(inet_error(err)),
    }
}

/// Binds network socket `slot` to any address and a port the kernel picks,
/// through a capability with the receive right, and listens, if it has no
/// port yet. If it has one, or its name cannot be read, it fails as
/// `refused` did: the capability it was bound through does not allow it.
fn listen_unbound(
    me: &ThreadRef,
    user: &UserRef,
    slot: u32,
    listen: file::ListenArgs,
    refused: Error,
) -> Result<u64> {
    let named = on_object(user, slot, |found| native::socket_name_of(found, false));
    let Ok(SocketName::Inet(name)) = named else { return Err(inet_error(refused)) };
    if name[Address::OFFSET_PORT..Address::OFFSET_PORT + 2] != [0, 0] {
        return Err(inet_error(refused));
    }
    // The same family, any address, any port.
    let mut any = [0u8; Address::SIZE];
    any[..4].copy_from_slice(&name[..4]);
    bind_receiving(me, user, slot, &any)?;
    results(me, user, slot, file::LISTEN, listen.encode()).map_err(inet_error)?;
    Ok(0)
}

/// `connect(fd, addr, addrlen)`: a Unix socket's `connect_socket`, or
/// `disconnect` for `AF_UNSPEC`; a network socket's `connect_address`.
pub fn connect(me: &ThreadRef, user: &UserRef, fd: u64, address: u64, length: u64) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    let name = read_sockaddr(address, length)?;
    match flavor(me, user, slot)? {
        Flavor::Inet(_) => {
            let bytes = lx::inet_address(&name).map_err(Error::Errno)?;
            through_network(|network| {
                on_object(user, slot, |found| {
                    native::connect_address_of(user, found, network, || given_of(&bytes))
                })
            })?;
        }
        Flavor::Unix { .. } => connect_unix(me, user, slot, &name)?,
    }
    Ok(0)
}

/// Connects Unix socket `slot` to the name in `address`, or lets a datagram
/// socket forget its peer for `AF_UNSPEC`, as the C library does.
fn connect_unix(me: &ThreadRef, user: &UserRef, slot: u32, address: &[u8]) -> Result<()> {
    if lx::family(address) == Some(lx::AF_UNSPEC) {
        return match results(me, user, slot, file::DISCONNECT, [0; 8]) {
            Err(Error::Native(status)) if is(status, Kernel::NotSupported) => fail(EINVAL),
            Err(err) => Err(socket_error(err)),
            Ok(_) => Ok(()),
        };
    }
    on_name(user, address, |place, _| {
        let args = directory::ConnectSocketArgs { path: given(0, place.path.len()), socket: slot };
        match call_directory(user, place, directory::CONNECT_SOCKET, args.encode(), &[]) {
            // Another thread's connect is under way.
            Err(Error::Native(status)) if is(status, Kernel::Busy) => fail(EALREADY),
            Err(err) => Err(socket_error(err)),
            Ok(_) => Ok(()),
        }
    })
}

/// `accept4(fd, addr, addrlen, flags)`, and `accept` with no flags: the
/// native `accept` into the lowest free descriptor, then the peer's name.
pub fn accept(
    me: &ThreadRef,
    user: &UserRef,
    fd: u64,
    address: u64,
    length: u64,
    flags: u64,
) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    let flags = flags as u32;
    if flags & !(lx::SOCK_NONBLOCK | lx::SOCK_CLOEXEC) != 0 {
        return fail(EINVAL);
    }
    let kept = description_flags(flags & lx::SOCK_NONBLOCK != 0, flags & lx::SOCK_CLOEXEC != 0);
    let new = into_free(user, 0, |to| {
        let args = file::AcceptArgs { to, flags: kept };
        results(me, user, slot, file::ACCEPT, args.encode()).map_err(socket_error)?;
        Ok(to)
    })?;
    if address != 0 {
        // A peer that cannot be named has an address of no length.
        let name = socket_address(user, new, true)
            .unwrap_or(Sockaddr { bytes: [0; SOCKADDR_UN_SIZE], len: 0 });
        if let Err(err) = put_sockaddr(address, length, name.as_bytes()) {
            delete(me, user, new);
            return Err(err);
        }
    }
    Ok(descriptor(new))
}

/// `shutdown(fd, how)`: the native `shutdown`.
pub fn shutdown(me: &ThreadRef, user: &UserRef, fd: u64, how: u64) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    let how = how as i32;
    if !matches!(how, lx::SHUT_RD | lx::SHUT_WR | lx::SHUT_RDWR) {
        return fail(EINVAL);
    }
    let args = file::ShutdownArgs { read: how != lx::SHUT_WR, write: how != lx::SHUT_RD };
    results(me, user, slot, file::SHUTDOWN, args.encode()).map_err(socket_error)?;
    Ok(0)
}

/// The native flags for a send, as the `SocketFlags` they are.
fn socket_flags(bits: u32) -> SocketFlags {
    SocketFlags(bits)
}

/// The buffers of a `msghdr`, as the list of the program's memory they
/// are, in the kernel's.
struct Iovs {
    parts: Vec<UserBuffer, Kmalloc>,
    /// Their total length, cut at what one call moves.
    total: u64,
}

impl Iovs {
    /// The buffers of `hdr`: at most `IOV_MAX`, EMSGSIZE if more, and EINVAL
    /// for one whose length a signed count cannot hold. Each is cut at what
    /// one call moves, as Linux cuts the sum, so that a length that only
    /// says how much room to use is not taken for memory out of reach.
    fn of(hdr: &Msghdr) -> Result<Iovs> {
        let count = lx::iov_count(hdr.iov_len).map_err(Error::Errno)?;
        let raw = read_bytes(hdr.iov, count * IOVEC_SIZE)?;
        let total = lx::iov_total(&raw).map_err(Error::Errno)?;
        let mut parts = Vec::new_in(kmalloc());
        parts.try_reserve_exact(count).map_err(|_| Error::Errno(ENOMEM))?;
        for iov in raw.as_chunks::<IOVEC_SIZE>().0 {
            let iov = Iovec::read(iov);
            let part = UserBuffer { address: iov.base, len: iov.len.min(lx::MAX_RW_COUNT) };
            // Room was reserved for every buffer.
            let _ = parts.push_within_capacity(part);
        }
        Ok(Iovs { parts, total })
    }
}

/// A native `send` of `data` on `slot`, or with the descriptors `slots`
/// `send_files`: the bytes it sent. The descriptors ride on the first byte.
fn send_bytes(
    user: &UserRef,
    slot: u32,
    data: &Buffers<'_>,
    flags: u32,
    slots: &[u32],
) -> Result<u64> {
    let flags = socket_flags(flags);
    let sent = if slots.is_empty() {
        on_object(user, slot, |found| native::send_of(user, found, data, flags))
    } else {
        on_object(user, slot, |found| {
            native::send_files_of(user, found, data, || native::files_of_slots(user, slots), flags)
        })
    };
    sent.map_err(socket_error)
}

/// Sends `data` as one datagram from Unix socket `slot` to the socket the
/// name in `address` gives, with the descriptors `slots`, as the
/// directory's native `send_datagram` resolves it, as `connect` does. A
/// stream socket fails as it does on Linux: EISCONN if it is connected,
/// else EOPNOTSUPP.
fn send_to_name(
    user: &UserRef,
    slot: u32,
    data: &Buffers<'_>,
    flags: u32,
    address: &[u8],
    slots: &[u32],
) -> Result<u64> {
    on_name(user, address, |place, _| {
        on_object(user, place.slot, |found| {
            let flags = socket_flags(flags);
            native::send_datagram_given(user, found, &place.path, slot, data, slots, flags)
        })
        .map_err(socket_error)
    })
}

/// Sends `data` on network socket `slot`, to the network address in
/// `address` if it has one: the native `send_to` through a network
/// capability, or `send`.
fn send_inet(
    user: &UserRef,
    slot: u32,
    data: &Buffers<'_>,
    flags: u32,
    address: Option<&[u8]>,
) -> Result<u64> {
    let Some(address) = address else { return send_bytes(user, slot, data, flags, &[]) };
    let bytes = lx::inet_address(address).map_err(Error::Errno)?;
    let flags = socket_flags(flags);
    through_network(|network| {
        on_object(user, slot, |found| {
            native::send_to_of(user, found, network, || given_of(&bytes), data, flags)
        })
    })
}

/// `sendto(fd, buf, len, flags, addr, addrlen)`, and `send` with no
/// address: the native `send`, or with an address `send_to` for a network
/// socket and the directory's `send_datagram` for a Unix one.
pub fn sendto(me: &ThreadRef, user: &UserRef, a: &Args) -> Result<u64> {
    let slot = paths::slot_of(a[0])?;
    let (flags, address, length) = (a[3], a[4], a[5]);
    let flags = lx::send_flags(flags as u32).map_err(Error::Errno)?;
    let parts = [UserBuffer { address: a[1], len: a[2].min(lx::MAX_RW_COUNT) }];
    let data = Buffers::new(&parts)?;
    if address == 0 || length as i32 == 0 {
        return send_bytes(user, slot, &data, flags, &[]);
    }
    let name = read_sockaddr(address, length)?;
    match flavor(me, user, slot)? {
        Flavor::Inet(_) => send_inet(user, slot, &data, flags, Some(&name)),
        Flavor::Unix { .. } => send_to_name(user, slot, &data, flags, &name, &[]),
    }
}

/// A `msghdr` as read from the program: its name, buffers and control
/// messages, in the kernel's memory.
struct Message {
    name: Vec<u8, Kmalloc>,
    iovs: Iovs,
    control: Vec<u8, Kmalloc>,
}

impl Message {
    /// Reads what `hdr` points to.
    fn read(hdr: &Msghdr) -> Result<Message> {
        let name = if hdr.name != 0 && hdr.name_len != 0 {
            read_sockaddr(hdr.name, u64::from(hdr.name_len))?
        } else {
            zeroed(0)?
        };
        let control = if hdr.control != 0 && hdr.control_len >= CMSG_HEADER as u64 {
            if hdr.control_len > CONTROL_MAX {
                return fail(ENOBUFS);
            }
            read_bytes(hdr.control, hdr.control_len as usize)?
        } else {
            zeroed(0)?
        };
        Ok(Message { name, iovs: Iovs::of(hdr)?, control })
    }
}

/// The slots of the descriptors in the `SCM_RIGHTS` messages of `control`,
/// after the credentials in it are checked: EBADF for a descriptor that is
/// none, EINVAL for more than one send carries.
fn rights_of(me: &ThreadRef, user: &UserRef, control: &[u8]) -> Result<Vec<u32, Kmalloc>> {
    let mut slots = Vec::new_in(kmalloc());
    if control.is_empty() {
        return Ok(slots);
    }
    let (own_user, own_group) = linux_process::identity(me, user)?;
    let own = [user.process.number() as u32, own_user, own_group];
    lx::send_control(control, own, |fd| {
        if slots.len() >= file::SEND_FILES_MAX as usize {
            return Err(EINVAL);
        }
        let slot = paths::slot_of(u64::from(fd as u32)).map_err(|_| EBADF)?;
        radix_mm::bytes::extend(&mut slots, &[slot]).map_err(|_| ENOMEM)
    })
    .map_err(Error::Errno)?;
    Ok(slots)
}

/// A `sendmsg` on Unix socket `slot`: a datagram, with or without a name,
/// or a stream's bytes, with the descriptors `slots` riding on the first.
/// The buffers go in one call, a datagram whole and a stream's as many
/// bytes as the socket takes.
fn send_unix_message(
    me: &ThreadRef,
    user: &UserRef,
    slot: u32,
    datagram: bool,
    msg: &Message,
    flags: u32,
) -> Result<u64> {
    let slots = rights_of(me, user, &msg.control)?;
    let named = !msg.name.is_empty();
    if datagram && msg.iovs.total > file::DATAGRAM_MAX {
        return fail(EMSGSIZE);
    }
    // A stream with no bytes sends none, as on Linux, whatever it carries;
    // a stream given a name fails as Linux's does, whatever it sends.
    if !datagram && !named && msg.iovs.total == 0 {
        return Ok(0);
    }
    let data = Buffers::new(&msg.iovs.parts)?;
    if named {
        return send_to_name(user, slot, &data, flags, &msg.name, &slots);
    }
    send_bytes(user, slot, &data, flags, &slots)
}

/// A `sendmsg` on a network socket: one send of the buffers, to the name if
/// there is one.
fn send_inet_message(user: &UserRef, slot: u32, msg: &Message, flags: u32) -> Result<u64> {
    // Descriptors go only over Unix sockets.
    if !msg.control.is_empty() {
        return fail(EOPNOTSUPP);
    }
    let data = Buffers::new(&msg.iovs.parts)?;
    let name = (!msg.name.is_empty()).then_some(&msg.name[..]);
    send_inet(user, slot, &data, flags, name)
}

/// One `sendmsg` of `msg`, on `slot`, for the program's `flags`.
fn send_message(
    me: &ThreadRef,
    user: &UserRef,
    slot: u32,
    msg: &Message,
    flags: u32,
) -> Result<u64> {
    let bits = lx::send_flags(flags).map_err(Error::Errno)?;
    match flavor(me, user, slot)? {
        Flavor::Inet(_) => send_inet_message(user, slot, msg, bits),
        Flavor::Unix { datagram } => send_unix_message(me, user, slot, datagram, msg, bits),
    }
}

/// Reads the `struct msghdr` at `address`.
fn read_msghdr(address: u64) -> Result<Msghdr> {
    let mut raw = [0u8; MSGHDR_SIZE];
    copy_in(address, &mut raw)?;
    Ok(Msghdr::read(&raw))
}

/// `sendmsg(fd, msg, flags)`: a send of the message's buffers, with its
/// name and its control messages, as the C library sends them.
pub fn sendmsg(me: &ThreadRef, user: &UserRef, fd: u64, msg: u64, flags: u64) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    let message = Message::read(&read_msghdr(msg)?)?;
    send_message(me, user, slot, &message, flags as u32)
}

/// `sendmmsg(fd, msgvec, vlen, flags)`: `sendmsg` of each message in
/// turn, and the bytes each sent in its `msg_len`. The count sent, or the
/// first message's error.
pub fn sendmmsg(
    me: &ThreadRef,
    user: &UserRef,
    fd: u64,
    msgvec: u64,
    vlen: u64,
    flags: u64,
) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    let flags = flags as u32;
    let mut sent = 0;
    for i in 0..(vlen as u32).min(VLEN_MAX) {
        let at = msgvec.saturating_add(u64::from(i) * MMSGHDR_SIZE as u64);
        let one = read_msghdr(at).and_then(|hdr| Message::read(&hdr)).and_then(|message| {
            let len = send_message(me, user, slot, &message, flags)?;
            copy_out(at + MMSG_LEN as u64, &(len as u32).to_le_bytes())
        });
        match one {
            Ok(()) => sent += 1,
            Err(err) if sent == 0 => return Err(err),
            Err(_) => break,
        }
    }
    Ok(sent)
}

/// A receive on a socket: the process that calls, the socket's slot and the
/// program's `MSG_` flags.
struct Receiving<'a> {
    user: &'a UserRef,
    slot: u32,
    flags: u32,
}

impl Receiving<'_> {
    /// The native flags of a receive: `MSG_DONTWAIT` and `MSG_PEEK` as given,
    /// `close_on_exec` for the descriptors that come with the bytes, and
    /// with `whole` a datagram's whole size in the place of what was
    /// copied, which says whether the buffers cut it short.
    fn native_flags(&self, close_on_exec: bool, whole: bool) -> SocketFlags {
        let dont_wait = self.flags & lx::MSG_DONTWAIT != 0;
        let bits = lx::receive_flags(dont_wait, self.flags & lx::MSG_PEEK != 0, close_on_exec);
        socket_flags(if whole { bits | lx::WHOLE_SIZE } else { bits })
    }

    /// A native `receive` into `out`: how many bytes.
    fn bytes(&self, out: &mut Buffers<'_>) -> Result<u64> {
        let flags = self.native_flags(false, false);
        on_object(self.user, self.slot, |found| native::receive_of(self.user, found, out, flags))
            .map_err(socket_error)
    }

    /// A native `receive_unix` into `out`, with room for `room`
    /// descriptors, which come in free slots of the process.
    fn unix_once(&self, out: &mut Buffers<'_>, room: usize) -> Result<UnixReceived> {
        let close_on_exec = self.flags & lx::MSG_CMSG_CLOEXEC != 0;
        let flags = self.native_flags(close_on_exec, true);
        on_object(self.user, self.slot, |found| {
            native::receive_unix_of(self.user, found, out, room, flags)
        })
        .map_err(socket_error)
    }

    /// Receives one datagram on a network socket into `out`, and who sent
    /// it: the native `receive_from`. `MSG_WAITALL` waits for no more than
    /// the one datagram, as on Linux. The datagram's whole size, and the
    /// sender as a native address.
    fn datagram(&self, out: &mut Buffers<'_>) -> Result<(u64, [u8; Address::SIZE])> {
        lx::check_receive_flags(self.flags & !lx::MSG_WAITALL, true).map_err(Error::Errno)?;
        let flags = self.native_flags(false, true);
        on_object(self.user, self.slot, |found| {
            native::receive_from_of(self.user, found, out, flags)
        })
        .map_err(inet_error)
    }

    /// A receive into `out` on a Unix socket, or a network stream socket,
    /// as the C library receives into buffers: what waits now, after
    /// waiting for some unless `MSG_DONTWAIT`; with `MSG_WAITALL`, until
    /// `out` is full, the other end stops sending, or an error comes after
    /// some bytes (which are then what it returns). `step` is one native
    /// receive into what is left of `out`: how many bytes it took, and
    /// whether descriptors came with them, which end the receive, as on
    /// Linux. A datagram socket takes one datagram, whatever `MSG_WAITALL`
    /// says, and `MSG_PEEK` is one receive, as a second would see the same
    /// bytes again. A cancellation point until bytes come.
    fn vector(
        &self,
        out: &mut Buffers<'_>,
        datagram: bool,
        mut step: impl FnMut(&mut Buffers<'_>) -> Result<(u64, bool)>,
    ) -> Result<u64> {
        lx::check_receive_flags(self.flags, datagram).map_err(Error::Errno)?;
        if datagram || self.flags & lx::MSG_PEEK != 0 {
            return step(out).map(|(taken, _)| taken);
        }
        let wait_all = self.flags & lx::MSG_WAITALL != 0 && self.flags & lx::MSG_DONTWAIT == 0;
        let mut total = 0;
        // A stream receive into no bytes asks the socket for none.
        while !out.is_empty() {
            let (taken, came) = match step(out) {
                Ok(done) => done,
                // An error after some bytes: what came is the answer.
                Err(_) if total > 0 => break,
                Err(err) => return Err(err),
            };
            total += taken;
            out.skip(taken as usize);
            if !wait_all || taken == 0 || came {
                break;
            }
        }
        Ok(total)
    }

    /// [`Receiving::vector`] of the bytes alone, as a stream socket, or a
    /// datagram socket asked for no sender, gives them.
    fn stream(&self, out: &mut Buffers<'_>) -> Result<u64> {
        self.vector(out, false, |out| Ok((self.bytes(out)?, false)))
    }

    /// [`Receiving::vector`] on a Unix socket, with room for `room`
    /// descriptors: the bytes, and what the last native receive gave besides
    /// them, none if no receive was made.
    fn unix(
        &self,
        out: &mut Buffers<'_>,
        room: usize,
        datagram: bool,
    ) -> Result<(u64, Option<UnixReceived>)> {
        let mut last = None;
        let length = self.vector(out, datagram, |out| {
            let got = self.unix_once(out, room)?;
            // A datagram's count is its whole size, which `out` may have cut.
            let taken = got.count.min(out.len() as u64);
            let came = !got.slots.is_empty() || got.lost != 0;
            last = Some(got);
            Ok((taken, came))
        })?;
        Ok((length, last))
    }
}

/// What a receive of a message gives back to the program.
#[derive(Default)]
struct Received {
    /// What the call returns: the bytes, or a datagram's whole size with
    /// `MSG_TRUNC`.
    length: u64,
    /// `msg_namelen`.
    name_len: u32,
    /// `msg_controllen`.
    control_len: u64,
    /// `msg_flags`.
    flags: u32,
}

/// How many descriptors a receive into `hdr` has room for on Unix socket
/// `slot`: as many as the control buffer holds after the credentials, which
/// come first if the socket asks for them.
fn descriptor_room(me: &ThreadRef, user: &UserRef, slot: u32, hdr: &Msghdr) -> usize {
    let mut control = if hdr.control != 0 { hdr.control_len } else { 0 };
    let credentials_room = lx::cmsg_space(UCRED_SIZE) as u64;
    if control > 0 && passes_credentials(me, user, slot) {
        control -= control.min(credentials_room);
    }
    if control >= lx::cmsg_len(4) as u64 {
        (((control - CMSG_HEADER as u64) / 4) as usize).min(file::SEND_FILES_MAX as usize)
    } else {
        0
    }
}

/// One `recvmsg` into `hdr` on `slot`: a datagram or a stream's bytes, and
/// the sender's name and the control messages that come with them, as the
/// C library gives them: first, if the socket asks for them
/// (`SO_PASSCRED`), the sender's credentials; then the descriptors that
/// came, in the room the credentials left, with `MSG_CTRUNC` for those
/// without room.
fn receive_message(
    me: &ThreadRef,
    user: &UserRef,
    slot: u32,
    hdr: &Msghdr,
    flags: u32,
) -> Result<Received> {
    let iovs = Iovs::of(hdr)?;
    if flags & lx::MSG_ERRQUEUE != 0 {
        return fail(EOPNOTSUPP);
    }
    let kind = flavor(me, user, slot)?;
    let mut out = Buffers::new(&iovs.parts)?;
    let receiving = Receiving { user, slot, flags };
    match kind {
        Flavor::Inet(kind) => inet_message(&receiving, kind, hdr, iovs.total, &mut out),
        Flavor::Unix { datagram } => {
            unix_message(me, &receiving, hdr, iovs.total, &mut out, datagram)
        }
    }
}

/// A `recvmsg` into `out`, `total` bytes of buffers, on a Unix socket.
fn unix_message(
    me: &ThreadRef,
    receiving: &Receiving<'_>,
    hdr: &Msghdr,
    total: u64,
    out: &mut Buffers<'_>,
    datagram: bool,
) -> Result<Received> {
    let user = receiving.user;
    let room = descriptor_room(me, user, receiving.slot, hdr);
    let (length, last) = receiving.unix(out, room, datagram)?;
    let Some(got) = last else { return Ok(Received::default()) };
    let whole = receiving.flags & lx::MSG_TRUNC != 0 && datagram;
    let mut received = Received {
        length: if whole { got.count } else { length },
        flags: if got.count > total { lx::MSG_TRUNC } else { 0 },
        ..Received::default()
    };
    let named = hdr.name != 0 && hdr.name_len != 0 && !got.from.bytes().is_empty();
    let sender = named.then(|| Sockaddr::unix(got.from.bytes()));
    let put = sender
        .map_or(Ok(()), |sender| put_name(hdr, &sender, &mut received))
        .and_then(|()| put_control(hdr, &got, &mut received));
    if let Err(err) = put {
        // The program never learns of the descriptors that came.
        for &slot in &got.slots {
            delete(me, user, slot);
        }
        return Err(err);
    }
    Ok(received)
}

/// Writes the sender's `name` at the program's `msg_name`, cut to its
/// `msg_namelen`, and `received`'s `name_len` as the whole length.
fn put_name(hdr: &Msghdr, name: &Sockaddr, received: &mut Received) -> Result<()> {
    let kept = (hdr.name_len as usize).min(name.len);
    copy_out(hdr.name, &name.as_bytes()[..kept])?;
    received.name_len = name.len as u32;
    Ok(())
}

/// Builds the control messages of `got` in the program's control buffer,
/// and sets `received`'s `control_len` and flags.
fn put_control(hdr: &Msghdr, got: &UnixReceived, received: &mut Received) -> Result<()> {
    if got.lost != 0 {
        received.flags |= lx::MSG_CTRUNC;
    }
    let room = if hdr.control != 0 { hdr.control_len as usize } else { 0 };
    let credentials = got
        .credentials
        .then(|| lx::put_ucred(got.sender.process as u32, got.sender.user, got.sender.group));
    if credentials.is_none() && got.slots.is_empty() {
        return Ok(());
    }
    let fds: Vec<u32, Kmalloc> = {
        let mut fds = Vec::new_in(kmalloc());
        fds.try_reserve_exact(got.slots.len()).map_err(|_| Error::Errno(ENOMEM))?;
        for &slot in &got.slots {
            // Room was reserved for every descriptor.
            let _ = fds.push_within_capacity(descriptor(slot) as u32);
        }
        fds
    };
    let size = lx::cmsg_space(UCRED_SIZE) + lx::cmsg_space(4 * fds.len());
    let mut bytes = zeroed(size)?;
    let (used, truncated) = lx::put_unix_control(&mut bytes, room, credentials, &fds);
    if truncated {
        received.flags |= lx::MSG_CTRUNC;
    }
    if used > 0 {
        copy_out(hdr.control, &bytes[..used])?;
    }
    received.control_len = used as u64;
    Ok(())
}

/// A `recvmsg` into `out`, `total` bytes of buffers, on a network socket of
/// kind `kind`: one datagram, or a stream's bytes, and who sent a datagram.
/// Nothing comes with a stream's bytes, and the control messages a network
/// datagram carries are not given (docs/deferred.md).
fn inet_message(
    receiving: &Receiving<'_>,
    kind: SocketType,
    hdr: &Msghdr,
    total: u64,
    out: &mut Buffers<'_>,
) -> Result<Received> {
    if kind == SocketType::Stream {
        return Ok(Received { length: receiving.stream(out)?, ..Received::default() });
    }
    let (whole, from) = receiving.datagram(out)?;
    let mut received = Received {
        length: if receiving.flags & lx::MSG_TRUNC != 0 { whole } else { whole.min(total) },
        flags: if whole > total { lx::MSG_TRUNC } else { 0 },
        ..Received::default()
    };
    if hdr.name != 0 && hdr.name_len != 0 {
        put_name(hdr, &Sockaddr::inet(&from), &mut received)?;
    }
    Ok(received)
}

/// Writes what a receive gave back to the `msghdr` at `address`: its
/// `msg_namelen`, `msg_controllen` and `msg_flags`.
fn put_received(address: u64, received: &Received) -> Result<()> {
    copy_out(address + MSG_NAMELEN as u64, &received.name_len.to_le_bytes())?;
    copy_out(address + MSG_CONTROLLEN as u64, &received.control_len.to_le_bytes())?;
    copy_out(address + MSG_FLAGS as u64, &received.flags.to_le_bytes())
}

/// `recvmsg(fd, msg, flags)`.
pub fn recvmsg(me: &ThreadRef, user: &UserRef, fd: u64, msg: u64, flags: u64) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    let received = receive_message(me, user, slot, &read_msghdr(msg)?, flags as u32)?;
    put_received(msg, &received)?;
    Ok(received.length)
}

/// `recvmmsg(fd, msgvec, vlen, flags, timeout)`: `recvmsg` into each
/// message in turn, and the bytes each got in its `msg_len`. With
/// `MSG_WAITFORONE` nothing waits after the first; a `timeout` is checked
/// between messages, as on Linux. The count received, or the first
/// message's error.
pub fn recvmmsg(
    me: &ThreadRef,
    user: &UserRef,
    fd: u64,
    msgvec: u64,
    vlen: u64,
    flags: u64,
    timeout: u64,
) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    let mut flags = flags as u32;
    let wait_for_one = flags & MSG_WAITFORONE != 0;
    flags &= !MSG_WAITFORONE;
    let deadline = if timeout == 0 {
        None
    } else {
        let mut raw = [0u8; TIMESPEC_SIZE];
        copy_in(timeout, &mut raw)?;
        let time = records::timespec(&raw).map_err(Error::Errno)?;
        Some(sched::now_ns().saturating_add(time.as_ns()))
    };
    let mut received = 0;
    for i in 0..(vlen as u32).min(VLEN_MAX) {
        let at = msgvec.saturating_add(u64::from(i) * MMSGHDR_SIZE as u64);
        let one = read_msghdr(at).and_then(|hdr| {
            let got = receive_message(me, user, slot, &hdr, flags)?;
            put_received(at, &got)?;
            copy_out(at + MMSG_LEN as u64, &(got.length as u32).to_le_bytes())
        });
        match one {
            Ok(()) => received += 1,
            Err(err) if received == 0 => return Err(err),
            Err(_) => break,
        }
        if wait_for_one {
            flags |= lx::MSG_DONTWAIT;
        }
        if deadline.is_some_and(|deadline| sched::now_ns() >= deadline) {
            break;
        }
    }
    Ok(received)
}

/// `recvfrom(fd, buf, len, flags, addr, addrlen)`, and `recv` with no
/// address: the native `receive`, or `receive_from` for a network datagram
/// socket and `receive_unix` for a Unix socket when the sender is asked
/// for.
pub fn recvfrom(me: &ThreadRef, user: &UserRef, a: &Args) -> Result<u64> {
    let slot = paths::slot_of(a[0])?;
    let (len, flags, address, length) = (a[2], a[3] as u32, a[4], a[5]);
    let parts = [UserBuffer { address: a[1], len: len.min(lx::MAX_RW_COUNT) }];
    let mut out = Buffers::new(&parts)?;
    let receiving = Receiving { user, slot, flags };
    if flags & lx::MSG_ERRQUEUE != 0 {
        return fail(EOPNOTSUPP);
    }
    // A datagram socket's `MSG_TRUNC` and `MSG_WAITALL` mean something else
    // than a stream's, and a receive of no bytes takes a datagram all the
    // same: each needs the kind of socket it is.
    let by_kind = flags & (lx::MSG_TRUNC | lx::MSG_WAITALL) != 0 || len == 0;
    let wants_name = address != 0 && length != 0;
    if !wants_name && !by_kind {
        return receiving.stream(&mut out);
    }
    let (count, sender) = match flavor(me, user, slot)? {
        Flavor::Inet(SocketType::Stream) => return receiving.stream(&mut out),
        Flavor::Inet(_) => {
            let (whole, from) = receiving.datagram(&mut out)?;
            let count = if flags & lx::MSG_TRUNC != 0 { whole } else { whole.min(parts[0].len) };
            (count, Some(Sockaddr::inet(&from)))
        }
        // A Unix socket says who sent what it received, which for a
        // datagram socket is not the socket it is connected to.
        Flavor::Unix { datagram } => {
            let (got, last) = receiving.unix(&mut out, 0, datagram)?;
            let whole = last.as_ref().filter(|_| flags & lx::MSG_TRUNC != 0 && datagram);
            let sender =
                last.as_ref().map(|last| last.from.bytes()).filter(|name| !name.is_empty());
            (whole.map_or(got, |last| last.count), sender.map(Sockaddr::unix))
        }
    };
    if wants_name {
        put_sockaddr(address, length, sender.as_ref().map_or(&[], Sockaddr::as_bytes))?;
    }
    Ok(count)
}

/// The bytes a program reads for a socket option, and how many.
type OptionValue = ([u8; lx::TIMEVAL_SIZE], usize);

/// `SO_PEERCRED`'s `struct ucred` for the state of Unix socket `slot`: the
/// process at the other end, and the user and group it acted for when the
/// connection was made. A socket with none to give has process number 0
/// and the unset ids of Linux's, never user 0, which would read as root.
fn peer_credentials(me: &ThreadRef, user: &UserRef, slot: u32) -> Result<[u8; UCRED_SIZE]> {
    let words = results(me, user, slot, file::SOCKET_STATE, [0; 8]).map_err(socket_error)?;
    let state = file::SocketStateResults::decode(&words)?;
    let known = state.peer != 0;
    Ok(lx::put_ucred(
        if known { state.peer as u32 } else { 0 },
        if known { state.user } else { u32::MAX },
        if known { state.group } else { u32::MAX },
    ))
}

/// The value of a socket option that is an `int`, in `room` bytes.
fn int_value(value: i32, room: usize) -> Result<OptionValue> {
    if room < 4 {
        return fail(EINVAL);
    }
    let mut out = [0u8; lx::TIMEVAL_SIZE];
    out[..4].copy_from_slice(&value.to_le_bytes());
    Ok((out, 4))
}

/// `getsockopt` on Unix socket `slot` at level `SOL_SOCKET`.
fn unix_getsockopt(
    me: &ThreadRef,
    user: &UserRef,
    slot: u32,
    name: i32,
    room: usize,
) -> Result<OptionValue> {
    let state = || results(me, user, slot, file::SOCKET_STATE, [0; 8]).map_err(socket_error);
    let kind = || option(me, user, slot, SocketOption::UnixKind, None).map_err(socket_error);
    let value = match name {
        lx::SO_PEERCRED => {
            let mut out = [0u8; lx::TIMEVAL_SIZE];
            out[..UCRED_SIZE].copy_from_slice(&peer_credentials(me, user, slot)?);
            return Ok((out, room.min(UCRED_SIZE)));
        }
        lx::SO_TYPE => {
            if kind()? == u64::from(SocketType::Datagram.raw()) {
                lx::SOCK_DGRAM as i32
            } else {
                lx::SOCK_STREAM as i32
            }
        }
        lx::SO_PASSCRED => i32::from(
            option(me, user, slot, SocketOption::PassCredentials, None).map_err(socket_error)? != 0,
        ),
        lx::SO_DOMAIN => i32::from(lx::AF_UNIX),
        // Errors come back from each call, so none waits here.
        lx::SO_PROTOCOL | lx::SO_ERROR => 0,
        lx::SO_ACCEPTCONN => i32::from(file::SocketStateResults::decode(&state()?)?.listening),
        // Each way of a stream has a pipe's buffer, and a datagram socket
        // the bytes its queue holds (radix-posix(7)).
        lx::SO_RCVBUF | lx::SO_SNDBUF => {
            if kind()? == u64::from(SocketType::Datagram.raw()) {
                file::DATAGRAM_BYTES_MAX as i32
            } else {
                65536
            }
        }
        _ => return fail(ENOPROTOOPT),
    };
    int_value(value, room)
}

/// `getsockopt` on network socket `slot` of kind `kind`.
fn inet_getsockopt(
    me: &ThreadRef,
    user: &UserRef,
    slot: u32,
    kind: SocketType,
    level: i32,
    name: i32,
    room: usize,
) -> Result<OptionValue> {
    let read = |option_kind| option(me, user, slot, option_kind, None).map_err(inet_error);
    if level == lx::SOL_SOCKET && name == lx::SO_LINGER {
        let mut out = [0u8; lx::TIMEVAL_SIZE];
        out[..lx::LINGER_SIZE].copy_from_slice(&lx::put_linger(read(SocketOption::Linger)?));
        return Ok((out, room.min(lx::LINGER_SIZE)));
    }
    if level == lx::SOL_SOCKET
        && matches!(name, lx::SO_TYPE | lx::SO_DOMAIN | lx::SO_PROTOCOL | lx::SO_ACCEPTCONN)
    {
        let words = results(me, user, slot, file::SOCKET_STATE, [0; 8]).map_err(inet_error)?;
        let state = file::SocketStateResults::decode(&words)?;
        let ipv6 = read(SocketOption::Family)? == u64::from(network::AddressFamily::Ipv6.raw());
        let value = match name {
            lx::SO_TYPE if kind == SocketType::Stream => lx::SOCK_STREAM,
            lx::SO_TYPE => lx::SOCK_DGRAM,
            lx::SO_DOMAIN => u32::from(if ipv6 { lx::AF_INET6 } else { lx::AF_INET }),
            lx::SO_PROTOCOL => match (kind, ipv6) {
                (SocketType::Ping, true) => lx::IPPROTO_ICMPV6,
                (SocketType::Ping, false) => lx::IPPROTO_ICMP,
                (SocketType::Datagram, _) => lx::IPPROTO_UDP,
                (SocketType::Stream, _) => lx::IPPROTO_TCP,
            },
            _ => u32::from(state.listening),
        };
        return int_value(value as i32, room);
    }
    let Some(opt) = lx::inet_option(level, name) else { return fail(ENOPROTOOPT) };
    let mut value = read(opt.native)?;
    // Errors come back from the calls, and SO_ERROR as an errno.
    if opt.native == SocketOption::Error && value != 0 {
        value = errno::from_status(Status(value)) as u64;
    }
    lx::get_value(level, opt, value, room).map_err(Error::Errno)
}

/// `getsockopt(fd, level, optname, optval, optlen)`: the native
/// `socket_option`, `socket_state` and `socket_name`, for the options the
/// C library knows.
pub fn getsockopt(
    me: &ThreadRef,
    user: &UserRef,
    fd: u64,
    level: u64,
    name: u64,
    value: u64,
    length: u64,
) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    let (level, name) = (level as i32, name as i32);
    let mut room = [0u8; 4];
    copy_in(length, &mut room)?;
    let Ok(room) = usize::try_from(i32::from_le_bytes(room)) else { return fail(EINVAL) };
    let (bytes, len) = match flavor(me, user, slot)? {
        Flavor::Inet(kind) => inet_getsockopt(me, user, slot, kind, level, name, room)?,
        Flavor::Unix { .. } if level != lx::SOL_SOCKET => return fail(ENOPROTOOPT),
        Flavor::Unix { .. } => unix_getsockopt(me, user, slot, name, room)?,
    };
    if len > 0 {
        copy_out(value, &bytes[..len])?;
    }
    copy_out(length, &(len as u32).to_le_bytes())?;
    Ok(0)
}

/// `setsockopt` on network socket `slot`.
fn inet_setsockopt(
    me: &ThreadRef,
    user: &UserRef,
    slot: u32,
    level: i32,
    name: i32,
    value: &[u8],
) -> Result<()> {
    let native = if level == lx::SOL_SOCKET && name == lx::SO_LINGER {
        (SocketOption::Linger, lx::set_linger(value).map_err(Error::Errno)?)
    } else {
        let Some(opt) = lx::inet_option(level, name) else { return fail(ENOPROTOOPT) };
        (opt.native, lx::set_value(level, opt, value).map_err(Error::Errno)?)
    };
    option(me, user, slot, native.0, Some(native.1)).map_err(inet_error)?;
    Ok(())
}

/// `setsockopt` on Unix socket `slot`: the buffers' sizes, which have one
/// size, and a local socket's address reuse and keep-alive, which mean
/// nothing, are taken; `SO_PASSCRED` is the native `pass_credentials`.
fn unix_setsockopt(
    me: &ThreadRef,
    user: &UserRef,
    slot: u32,
    level: i32,
    name: i32,
    value: &[u8],
) -> Result<()> {
    if level != lx::SOL_SOCKET {
        return fail(ENOPROTOOPT);
    }
    let Some(&int) = value.as_chunks::<4>().0.first() else {
        return if matches!(
            name,
            lx::SO_RCVBUF | lx::SO_SNDBUF | lx::SO_REUSEADDR | lx::SO_KEEPALIVE | lx::SO_PASSCRED
        ) {
            fail(EINVAL)
        } else {
            fail(ENOPROTOOPT)
        };
    };
    match name {
        lx::SO_RCVBUF | lx::SO_SNDBUF | lx::SO_REUSEADDR | lx::SO_KEEPALIVE => Ok(()),
        lx::SO_PASSCRED => {
            let on = u64::from(i32::from_le_bytes(int) != 0);
            option(me, user, slot, SocketOption::PassCredentials, Some(on))
                .map_err(socket_error)?;
            Ok(())
        }
        _ => fail(ENOPROTOOPT),
    }
}

/// `setsockopt(fd, level, optname, optval, optlen)`.
pub fn setsockopt(
    me: &ThreadRef,
    user: &UserRef,
    fd: u64,
    level: u64,
    name: u64,
    value: u64,
    length: u64,
) -> Result<u64> {
    let slot = paths::slot_of(fd)?;
    let (level, name) = (level as i32, name as i32);
    let Ok(length) = usize::try_from(length as i32) else { return fail(EINVAL) };
    // An option's value is a few bytes; the rest of a longer one is never
    // read, as the options take no more.
    let mut bytes = [0u8; OPTION_MAX];
    let kept = length.min(OPTION_MAX);
    copy_in(value, &mut bytes[..kept])?;
    let value = &bytes[..kept];
    match flavor(me, user, slot)? {
        Flavor::Inet(_) => inet_setsockopt(me, user, slot, level, name, value)?,
        Flavor::Unix { .. } => unix_setsockopt(me, user, slot, level, name, value)?,
    }
    Ok(0)
}
