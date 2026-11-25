// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Sockets beyond what Rust's standard library does: descriptors sent
//! along a local socket (SCM_RIGHTS), keep-alive checks, and a local
//! connection that never waits.

use std::io::{self, Write};
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

    use super::{FDS_MAX, connect_now, receive_with_fds, send_with_fds, set_keepalive};
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
    fn keepalive_is_set_on_a_socket_only() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let stream = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        set_keepalive(&stream, true).unwrap();
        let (from, _to) = fd::pipe().unwrap();
        assert_eq!(set_keepalive(&from, true).unwrap_err().raw_os_error(), Some(libc::ENOTSOCK));
    }
}
