// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What sshd asks of the system beyond Rust's standard library: random
//! bytes, the machine's name, pipes, descriptors sent to and from authd,
//! and descriptors that do not block.

use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;

/// `N` random bytes from the kernel, or an error: never weaker ones, as
/// they make the host's key.
pub fn random<const N: usize>() -> io::Result<[u8; N]> {
    let mut out = [0u8; N];
    let mut got = 0;
    while got < N {
        // SAFETY: the rest of `out` is valid for its length.
        let n = unsafe { libc::getrandom(out[got..].as_mut_ptr().cast(), N - got, 0) };
        if n < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        got += n as usize;
    }
    Ok(out)
}

/// The machine's name.
pub fn hostname() -> String {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is valid for its length.
    if unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } != 0 {
        return "localhost".into();
    }
    let end = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}

/// A pipe, both ends closed on exec: its read end, then its write end.
pub fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: `fds` has room for the two descriptors pipe2 writes.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: pipe2 made both, and nothing else owns them.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// Makes `fd`'s reads and writes return at once rather than wait.
pub fn set_nonblocking(fd: &impl AsRawFd) -> io::Result<()> {
    let fd = fd.as_raw_fd();
    // SAFETY: fcntl reads and sets the flags of a descriptor we hold.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// The most descriptors sent with one request, or kept from one answer.
const DESCRIPTORS_MAX: usize = 4;

/// Sends `bytes` on `stream` with the descriptors `fds` (SCM_RIGHTS) in
/// the first message, as authd reads a request that comes with them.
pub fn send_with_fds(stream: &UnixStream, bytes: &[u8], fds: &[RawFd]) -> io::Result<()> {
    assert!(fds.len() <= DESCRIPTORS_MAX, "at most four descriptors go with a request");
    if fds.is_empty() {
        return (&mut &*stream).write_all(bytes);
    }
    let mut iov = libc::iovec { iov_base: bytes.as_ptr().cast_mut().cast(), iov_len: bytes.len() };
    let mut control = [0u64; 8];
    let room = size_of_val(fds) as u32;
    // SAFETY: all zeros is a valid msghdr: numbers and null pointers.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &raw mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    // SAFETY: CMSG_SPACE only computes a size, which fits `control` for
    // DESCRIPTORS_MAX descriptors.
    message.msg_controllen = unsafe { libc::CMSG_SPACE(room) } as _;
    // SAFETY: the control space holds one header and the descriptors, as
    // CMSG_SPACE said; the macros point into it.
    let sent = unsafe {
        let header = libc::CMSG_FIRSTHDR(&raw const message);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(room) as _;
        let data = libc::CMSG_DATA(header);
        for (k, fd) in fds.iter().enumerate() {
            data.add(k * size_of::<libc::c_int>()).cast::<libc::c_int>().write_unaligned(*fd);
        }
        libc::sendmsg(stream.as_raw_fd(), &raw const message, 0)
    };
    if sent < 0 {
        return Err(io::Error::last_os_error());
    }
    // What the first message did not take goes after it, as a write.
    let rest = &bytes[sent as usize..];
    if rest.is_empty() { Ok(()) } else { (&mut &*stream).write_all(rest) }
}

/// Reads from the socket `fd` into `buffer`, as read(2) does, and keeps
/// the descriptors sent with the bytes (SCM_RIGHTS) in `fds`, closed on
/// exec, up to [`DESCRIPTORS_MAX`] in all; any more are closed.
pub fn receive(fd: RawFd, buffer: &mut [u8], fds: &mut Vec<OwnedFd>) -> io::Result<usize> {
    let mut iov = libc::iovec { iov_base: buffer.as_mut_ptr().cast(), iov_len: buffer.len() };
    // Room for more descriptors than are kept, aligned for the headers.
    let mut control = [0u64; 8];
    // SAFETY: all zeros is a valid msghdr: numbers and null pointers.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &raw mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = size_of_val(&control) as _;
    // SAFETY: the message points at our buffer and control space.
    let n = unsafe { libc::recvmsg(fd, &raw mut message, libc::MSG_CMSG_CLOEXEC) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the control messages are those recvmsg wrote into
    // `control`, walked with the C library's macros; each descriptor in
    // one is ours to own.
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(&raw const message);
        while !header.is_null() {
            if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(header);
                let bytes = (*header).cmsg_len as usize - libc::CMSG_LEN(0) as usize;
                for k in 0..bytes / size_of::<libc::c_int>() {
                    let at = data.add(k * size_of::<libc::c_int>());
                    let received = OwnedFd::from_raw_fd(at.cast::<libc::c_int>().read_unaligned());
                    if fds.len() < DESCRIPTORS_MAX {
                        fds.push(received);
                    }
                }
            }
            header = libc::CMSG_NXTHDR(&raw const message, header);
        }
    }
    Ok(n as usize)
}
