// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Sockets beyond what Rust's standard library and socket2 do:
//! descriptors sent along a local socket (SCM_RIGHTS), keep-alive checks,
//! a local connection that never waits, options of an int and a socket
//! bound to one interface, a datagram read with its control messages and
//! its sender, and the name a network address has.

use std::ffi::CStr;
use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::Path;

use libc::c_int;

use crate::checked;

/// The most descriptors one message carries, sent or received.
pub const FDS_MAX: usize = 16;

/// Room for a control message of [`FDS_MAX`] descriptors, aligned for its
/// header: 16 bytes of header and 64 of descriptors on Linux and Radix.
type Control = [u64; 16];

/// Sends `bytes` on `stream`, with `fds` (SCM_RIGHTS) in its first
/// message, so that a reader of the bytes takes the descriptors with them.
/// The bytes the first message does not take go after it.
///
/// # Errors
///
/// `ErrorKind::InvalidInput` for more than [`FDS_MAX`] descriptors, and
/// what sendmsg(2) and write(2) fail with: EPIPE when the other end is
/// closed.
pub fn send_with_fds(stream: &UnixStream, bytes: &[u8], fds: &[BorrowedFd<'_>]) -> io::Result<()> {
    if fds.len() > FDS_MAX {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "too many descriptors for one message",
        ));
    }
    if fds.is_empty() {
        return (&mut &*stream).write_all(bytes);
    }
    let mut iov = libc::iovec { iov_base: bytes.as_ptr().cast_mut().cast(), iov_len: bytes.len() };
    let mut control: Control = [0; 16];
    // At most FDS_MAX ints, so it fits.
    let room = (fds.len() * size_of::<c_int>()) as u32;
    // SAFETY: all zeros is a valid msghdr: numbers and null pointers.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &raw mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    // SAFETY: CMSG_SPACE only computes a size, which fits `control` for
    // FDS_MAX descriptors.
    message.msg_controllen = unsafe { libc::CMSG_SPACE(room) } as _;
    // SAFETY: the control space holds one header and the descriptors, as
    // CMSG_SPACE said, and the macros point into it; sendmsg reads the
    // message, its bytes and its descriptors, which are borrowed, so open.
    let sent = checked_size(unsafe {
        let header = libc::CMSG_FIRSTHDR(&raw const message);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(room) as _;
        let data = libc::CMSG_DATA(header);
        for (k, fd) in fds.iter().enumerate() {
            data.add(k * size_of::<c_int>()).cast::<c_int>().write_unaligned(fd.as_raw_fd());
        }
        libc::sendmsg(stream.as_raw_fd(), &raw const message, 0)
    })?;
    let rest = &bytes[sent..];
    if rest.is_empty() { Ok(()) } else { (&mut &*stream).write_all(rest) }
}

/// Reads from `socket` into `buffer`, as read(2) does, and keeps the
/// descriptors that came with the bytes (SCM_RIGHTS) in `fds`, each closed
/// on exec, until it holds `keep`: those after are closed. Returns how
/// many bytes came; 0 is the end.
///
/// # Errors
///
/// What recvmsg(2) fails with: EAGAIN on a socket that does not block and
/// has nothing, EINTR when a signal came first.
pub fn receive_with_fds(
    socket: impl AsFd,
    buffer: &mut [u8],
    fds: &mut Vec<OwnedFd>,
    keep: usize,
) -> io::Result<usize> {
    let mut iov = libc::iovec { iov_base: buffer.as_mut_ptr().cast(), iov_len: buffer.len() };
    let mut control: Control = [0; 16];
    // SAFETY: all zeros is a valid msghdr: numbers and null pointers.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &raw mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = size_of::<Control>() as _;
    // SAFETY: the message points at the buffer and the control space, each
    // valid for the length it gives.
    let received = checked_size(unsafe {
        libc::recvmsg(socket.as_fd().as_raw_fd(), &raw mut message, libc::MSG_CMSG_CLOEXEC)
    })?;
    // SAFETY: the control messages are those recvmsg wrote into `control`,
    // walked with the C library's macros, which stay inside the length it
    // set; each descriptor in an SCM_RIGHTS message is new to this
    // process, so it is owned here and nowhere else.
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(&raw const message);
        while !header.is_null() {
            if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(header);
                let length = (*header).cmsg_len as usize - libc::CMSG_LEN(0) as usize;
                for k in 0..length / size_of::<c_int>() {
                    let at = data.add(k * size_of::<c_int>());
                    let fd = OwnedFd::from_raw_fd(at.cast::<c_int>().read_unaligned());
                    if fds.len() < keep {
                        fds.push(fd);
                    }
                }
            }
            header = libc::CMSG_NXTHDR(&raw const message, header);
        }
    }
    Ok(received)
}

/// The count of a call that returns one, or -1 and errno.
fn checked_size(result: isize) -> io::Result<usize> {
    usize::try_from(result).map_err(|_| io::Error::last_os_error())
}

/// Reads what waits on `socket` into `buffer` without waiting, as recv(2)
/// with MSG_DONTWAIT does, whether or not the socket blocks: for a check
/// that must not hang on a peer.
///
/// # Errors
///
/// `ErrorKind::WouldBlock` when nothing waits.
pub fn receive_now(socket: impl AsFd, buffer: &mut [u8]) -> io::Result<usize> {
    // SAFETY: recv writes at most the buffer's length into it.
    checked_size(unsafe {
        libc::recv(
            socket.as_fd().as_raw_fd(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            libc::MSG_DONTWAIT,
        )
    })
}

/// A control message that came with a datagram (cmsg(3)): its level and
/// type, `IPPROTO_IP` and `IP_RECVERR` for an ICMP error, and its data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlMessage {
    /// cmsg_level.
    pub level: c_int,
    /// cmsg_type.
    pub kind: c_int,
    /// Its data, after the header.
    pub data: Vec<u8>,
}

/// What [`receive_message`] read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    /// How many bytes came into the buffer.
    pub length: usize,
    /// Who sent it, for an IPv4 or IPv6 socket; for an error from the
    /// queue, where the packet that failed went.
    pub from: Option<SocketAddr>,
    /// The control messages that came with it, but descriptors
    /// (SCM_RIGHTS), which are closed: [`receive_with_fds`] keeps them.
    pub control: Vec<ControlMessage>,
}

/// How [`receive_message`] reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReceiveFlags {
    /// Without waiting (MSG_DONTWAIT).
    pub now: bool,
    /// The next error of the socket's error queue, not a datagram
    /// (MSG_ERRQUEUE), as traceroute(8) reads ICMP errors.
    pub errors: bool,
}

/// The most bytes of control messages one read takes: an error from the
/// queue is a header, a sock_extended_err and an address.
const CONTROL_MAX: usize = 512;

/// Reads a datagram, or an error, from `socket` into `buffer`, with who
/// sent it and its control messages, as recvmsg(2) does.
///
/// # Errors
///
/// `ErrorKind::WouldBlock` when `flags.now` and nothing waits, EAGAIN
/// too for an empty error queue.
pub fn receive_message(
    socket: impl AsFd,
    buffer: &mut [u8],
    flags: ReceiveFlags,
) -> io::Result<Message> {
    // SAFETY: all zeros is a valid sockaddr_storage.
    let mut name: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut control = [0u64; CONTROL_MAX / 8];
    let mut iov = libc::iovec { iov_base: buffer.as_mut_ptr().cast(), iov_len: buffer.len() };
    // SAFETY: all zeros is a valid msghdr: numbers and null pointers.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_name = (&raw mut name).cast();
    message.msg_namelen = size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    message.msg_iov = &raw mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = size_of_val(&control) as _;
    let mut how = libc::MSG_CMSG_CLOEXEC;
    if flags.now {
        how |= libc::MSG_DONTWAIT;
    }
    if flags.errors {
        how |= libc::MSG_ERRQUEUE;
    }
    // SAFETY: the message points at the buffer, the address and the
    // control space, each valid for the length it gives.
    let length =
        checked_size(unsafe { libc::recvmsg(socket.as_fd().as_raw_fd(), &raw mut message, how) })?;
    let mut messages = Vec::new();
    // SAFETY: the control messages are those recvmsg wrote into `control`,
    // walked with the C library's macros, which stay inside the length it
    // set; a descriptor in an SCM_RIGHTS message is new to this process,
    // so it is owned here, and closed.
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(&raw const message);
        while !header.is_null() {
            let data = libc::CMSG_DATA(header);
            let size =
                ((*header).cmsg_len as usize).saturating_sub(data as usize - header as usize);
            let bytes = std::slice::from_raw_parts(data, size);
            if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                for fd in bytes.as_chunks::<{ size_of::<c_int>() }>().0 {
                    drop(OwnedFd::from_raw_fd(c_int::from_ne_bytes(*fd)));
                }
            } else {
                messages.push(ControlMessage {
                    level: (*header).cmsg_level,
                    kind: (*header).cmsg_type,
                    data: bytes.to_vec(),
                });
            }
            header = libc::CMSG_NXTHDR(&raw const message, header);
        }
    }
    let from = socket_address(&name, message.msg_namelen as usize);
    Ok(Message { length, from, control: messages })
}

/// The IPv4 or IPv6 address of the first `length` bytes of `name`.
fn socket_address(name: &libc::sockaddr_storage, length: usize) -> Option<SocketAddr> {
    match c_int::from(name.ss_family) {
        libc::AF_INET if length >= size_of::<libc::sockaddr_in>() => {
            // SAFETY: a sockaddr_storage of family AF_INET, of at least a
            // sockaddr_in's length, holds one; the storage is aligned for
            // any address.
            let ip = unsafe { &*std::ptr::from_ref(name).cast::<libc::sockaddr_in>() };
            let address = Ipv4Addr::from(u32::from_be(ip.sin_addr.s_addr));
            Some(SocketAddr::V4(SocketAddrV4::new(address, u16::from_be(ip.sin_port))))
        }
        libc::AF_INET6 if length >= size_of::<libc::sockaddr_in6>() => {
            // SAFETY: as above, for AF_INET6 and a sockaddr_in6.
            let ip = unsafe { &*std::ptr::from_ref(name).cast::<libc::sockaddr_in6>() };
            let address = Ipv6Addr::from(ip.sin6_addr.s6_addr);
            Some(SocketAddr::V6(SocketAddrV6::new(
                address,
                u16::from_be(ip.sin6_port),
                ip.sin6_flowinfo,
                ip.sin6_scope_id,
            )))
        }
        _ => None,
    }
}

/// Sets the option `name` of `level` of `socket` to an int, `value`, as
/// setsockopt(2) does: `IPPROTO_IP` and `IP_TTL`, `SOL_SOCKET` and
/// `SO_BROADCAST`.
///
/// # Errors
///
/// ENOPROTOOPT for an option the socket has not; EINVAL for one that
/// takes no int.
pub fn set_int_option(
    socket: impl AsFd,
    level: c_int,
    name: c_int,
    value: c_int,
) -> io::Result<()> {
    // SAFETY: the option's value is the int it points at, of the length
    // given, which setsockopt only reads.
    checked(unsafe {
        libc::setsockopt(
            socket.as_fd().as_raw_fd(),
            level,
            name,
            (&raw const value).cast(),
            size_of::<c_int>() as libc::socklen_t,
        )
    })?;
    Ok(())
}

/// Binds `socket` to the interface named `interface` (SO_BINDTODEVICE),
/// so that it sends and receives on that one only, as a DHCP client does.
///
/// # Errors
///
/// ENODEV when there is no such interface, EPERM without the right.
pub fn bind_to_device(socket: impl AsFd, interface: &str) -> io::Result<()> {
    // SAFETY: the name is its bytes, of the length given, which setsockopt
    // only reads.
    checked(unsafe {
        libc::setsockopt(
            socket.as_fd().as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            interface.as_ptr().cast(),
            interface.len() as libc::socklen_t,
        )
    })?;
    Ok(())
}

/// The name `address` has (getnameinfo(3)), as the resolver finds it, or
/// its number written out where it has none.
#[must_use]
pub fn name_of(address: IpAddr) -> String {
    // SAFETY: all zeros is a valid sockaddr_storage.
    let mut name: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let length = match address {
        IpAddr::V4(ip) => {
            // SAFETY: a sockaddr_storage has room and alignment for any
            // address.
            let v4 = unsafe { &mut *std::ptr::from_mut(&mut name).cast::<libc::sockaddr_in>() };
            v4.sin_family = libc::AF_INET as libc::sa_family_t;
            v4.sin_addr.s_addr = u32::from(ip).to_be();
            size_of::<libc::sockaddr_in>()
        }
        IpAddr::V6(ip) => {
            // SAFETY: as above.
            let v6 = unsafe { &mut *std::ptr::from_mut(&mut name).cast::<libc::sockaddr_in6>() };
            v6.sin6_family = libc::AF_INET6 as libc::sa_family_t;
            v6.sin6_addr.s6_addr = ip.octets();
            size_of::<libc::sockaddr_in6>()
        }
    };
    let mut host = [0 as libc::c_char; 1025];
    // SAFETY: the address is a sockaddr of `length` bytes; getnameinfo
    // writes a C string of at most the host buffer's length, and no
    // service is asked for.
    let result = unsafe {
        libc::getnameinfo(
            (&raw const name).cast(),
            length as libc::socklen_t,
            host.as_mut_ptr(),
            host.len() as libc::socklen_t,
            std::ptr::null_mut(),
            0,
            0,
        )
    };
    if result != 0 {
        return address.to_string();
    }
    // SAFETY: getnameinfo wrote a C string into `host`, which ends in it.
    unsafe { CStr::from_ptr(host.as_ptr()) }.to_string_lossy().into_owned()
}

/// Has the kernel check, when the connection on `socket` is quiet, that
/// the other end is still there (SO_KEEPALIVE), so that a peer that went
/// away without a word ends the connection rather than hold it forever.
///
/// # Errors
///
/// ENOTSOCK when `socket` is no socket.
pub fn set_keepalive(socket: impl AsFd, on: bool) -> io::Result<()> {
    let on = c_int::from(on);
    // SAFETY: the option's value is the int it points at, of the length
    // given.
    checked(unsafe {
        libc::setsockopt(
            socket.as_fd().as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_KEEPALIVE,
            (&raw const on).cast(),
            size_of::<c_int>() as libc::socklen_t,
        )
    })?;
    Ok(())
}

/// A connection to the local socket at `path` that does not block, made
/// at once or not at all, for a loop that must never wait, not even for a
/// server that does not accept.
///
/// # Errors
///
/// ENAMETOOLONG for a path longer than a socket's address holds, ENOENT
/// or ECONNREFUSED when nothing listens there, and EAGAIN when the server
/// has more waiting than it takes.
pub fn connect_now(path: &Path) -> io::Result<UnixStream> {
    // SAFETY: all zeros is a valid sockaddr_un.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    let path = path.as_os_str().as_bytes();
    if path.len() >= address.sun_path.len() || path.contains(&0) {
        return Err(io::Error::from_raw_os_error(libc::ENAMETOOLONG));
    }
    for (to, &from) in address.sun_path.iter_mut().zip(path) {
        *to = from as libc::c_char;
    }
    // SAFETY: socket(2) takes no pointers.
    let fd = checked(unsafe {
        libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC, 0)
    })?;
    // SAFETY: `fd` is a new socket, which nothing else holds.
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    let length = size_of::<libc::sockaddr_un>() as libc::socklen_t;
    // SAFETY: `address` is a sockaddr_un of `length` bytes.
    checked(unsafe { libc::connect(stream.as_raw_fd(), (&raw const address).cast(), length) })?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::os::fd::AsFd;
    use std::os::unix::net::{UnixListener, UnixStream};

    use super::{
        FDS_MAX, ReceiveFlags, connect_now, name_of, receive_message, receive_now,
        receive_with_fds, send_with_fds, set_int_option, set_keepalive,
    };
    use crate::fd;

    #[test]
    fn descriptors_go_with_the_bytes_and_extras_are_closed() {
        let (here, there) = UnixStream::pair().unwrap();
        let (from, to) = fd::pipe().unwrap();
        let (other, _other_to) = fd::pipe().unwrap();
        send_with_fds(&here, b"hello", &[to.as_fd(), other.as_fd()]).unwrap();
        drop(to);
        let mut buffer = [0; 16];
        let mut fds = Vec::new();
        assert_eq!(receive_with_fds(&there, &mut buffer, &mut fds, 1).unwrap(), 5);
        assert_eq!(&buffer[..5], b"hello");
        assert_eq!(fds.len(), 1, "the second descriptor was closed");
        fd::write(&fds[0], b"!").unwrap();
        let mut byte = [0; 1];
        assert_eq!(fd::read(&from, &mut byte).unwrap(), 1);
        assert_eq!(&byte, b"!");
    }

    #[test]
    fn bytes_without_descriptors_are_plain_bytes() {
        let (mut here, there) = UnixStream::pair().unwrap();
        send_with_fds(&here, b"plain", &[]).unwrap();
        here.flush().unwrap();
        let mut buffer = [0; 8];
        let mut fds = Vec::new();
        assert_eq!(receive_with_fds(&there, &mut buffer, &mut fds, 4).unwrap(), 5);
        assert!(fds.is_empty());
    }

    #[test]
    fn too_many_descriptors_are_refused() {
        let (here, _there) = UnixStream::pair().unwrap();
        let (from, _to) = fd::pipe().unwrap();
        let many = vec![from.as_fd(); FDS_MAX + 1];
        let error = send_with_fds(&here, b"x", &many).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn a_local_connection_is_made_at_once_or_fails() {
        let folder = std::env::temp_dir().join(format!("sysapi-net-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("socket");
        assert!(connect_now(&path).is_err(), "nothing listens yet");
        let listener = UnixListener::bind(&path).unwrap();
        let stream = connect_now(&path).unwrap();
        listener.accept().unwrap();
        drop(stream);
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn a_datagram_comes_with_its_sender() {
        let here = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let there = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut buffer = [0; 16];
        let empty = receive_message(&here, &mut buffer, ReceiveFlags { now: true, errors: false });
        assert_eq!(empty.unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
        there.send_to(b"ping", here.local_addr().unwrap()).unwrap();
        let message = receive_message(&here, &mut buffer, ReceiveFlags::default()).unwrap();
        assert_eq!(&buffer[..message.length], b"ping");
        assert_eq!(message.from, Some(there.local_addr().unwrap()));
        assert!(message.control.is_empty());
    }

    #[test]
    fn a_control_message_is_given_as_its_level_kind_and_data() {
        let here = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        set_int_option(&here, libc::IPPROTO_IP, libc::IP_RECVTTL, 1).unwrap();
        let there = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        there.set_ttl(42).unwrap();
        there.send_to(b"ttl", here.local_addr().unwrap()).unwrap();
        let mut buffer = [0; 16];
        let message = receive_message(&here, &mut buffer, ReceiveFlags::default()).unwrap();
        let ttl = message
            .control
            .iter()
            .find(|c| c.level == libc::IPPROTO_IP && c.kind == libc::IP_TTL)
            .expect("the TTL came with it");
        assert_eq!(ttl.data, 42i32.to_ne_bytes());
    }

    #[test]
    fn a_read_now_does_not_wait() {
        let (here, there) = UnixStream::pair().unwrap();
        let mut buffer = [0; 4];
        let error = receive_now(&here, &mut buffer).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        (&there).write_all(b"now").unwrap();
        assert_eq!(receive_now(&here, &mut buffer).unwrap(), 3);
    }

    #[test]
    fn the_loopback_address_has_a_name() {
        // The hosts file names it, so no resolver is asked.
        let name = name_of("127.0.0.1".parse().unwrap());
        assert!(!name.is_empty() && !name.contains('\0'), "{name:?}");
    }

    #[test]
    fn keepalive_is_set_on_a_socket_only() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        set_keepalive(&stream, true).unwrap();
        let (from, _to) = fd::pipe().unwrap();
        assert_eq!(set_keepalive(&from, true).unwrap_err().raw_os_error(), Some(libc::ENOTSOCK));
    }
}
