// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What nfsd asks of the system beyond Rust's standard library: the
//! machine's name, random bytes, pipes, descriptors sent to authd, and
//! waits.

use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// The most descriptors sent with one request.
const DESCRIPTORS_MAX: usize = 4;

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

/// `N` random bytes from the kernel, or an error: never weaker ones, as
/// they make challenges and keys.
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

/// Sends `bytes` on `stream` with the descriptors `fds` (SCM_RIGHTS) in
/// the first message, as authd reads a request that comes with them.
pub fn send_with_fds(stream: &UnixStream, bytes: &[u8], fds: &[RawFd]) -> io::Result<()> {
    assert!(fds.len() <= DESCRIPTORS_MAX, "at most four descriptors go with a request");
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

/// Has the kernel check that the PC at the other end of `socket` is
/// still there when the connection is quiet, so that a PC that went away
/// without a word does not keep its session forever.
pub fn keep_alive(socket: &impl AsRawFd) {
    let on: libc::c_int = 1;
    // SAFETY: the option's value is the int it points at.
    unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_KEEPALIVE,
            (&raw const on).cast(),
            size_of::<libc::c_int>() as libc::socklen_t,
        );
    }
}

/// Waits until `fd` has something to read, or its other end closed, for
/// at most `time`: whether it has. Radix's local sockets take no receive
/// timeout (SO_RCVTIMEO), so a wait on authd's is a poll.
pub fn readable(fd: &impl AsRawFd, time: Duration) -> io::Result<bool> {
    let mut wanted = libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    // In whole milliseconds, rounded up, so that it never waits less.
    let ms = libc::c_int::try_from(time.as_nanos().div_ceil(1_000_000)).unwrap_or(libc::c_int::MAX);
    loop {
        // SAFETY: one pollfd, valid for the call.
        let ready = unsafe { libc::poll(&raw mut wanted, 1, ms) };
        if ready >= 0 {
            return Ok(ready > 0);
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}
