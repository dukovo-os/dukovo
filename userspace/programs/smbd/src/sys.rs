// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What smbd asks of the system beyond Rust's standard library: the
//! machine's name, random bytes, pipes and descriptors sent to authd, and
//! the calls on files that Windows' requests need.

use std::ffi::CString;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use crate::info::Volume;

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

/// The most descriptors sent with one request.
const DESCRIPTORS_MAX: usize = 4;

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

/// `path` as a C string.
fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))
}

/// Whether this process may `mode` (`R_OK`, `W_OK`, `X_OK`) `path`, as
/// the kernel decides for its user and groups.
pub fn may(path: &Path, mode: libc::c_int) -> bool {
    let Ok(path) = c_path(path) else { return false };
    // SAFETY: the path is a C string.
    unsafe { libc::faccessat(libc::AT_FDCWD, path.as_ptr(), mode, libc::AT_EACCESS) == 0 }
}

/// What the filesystem holding `path` is.
pub fn volume(path: &Path) -> io::Result<Volume> {
    let c = c_path(path)?;
    // SAFETY: all zeros is a valid statvfs: numbers.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: the path is a C string and `st` a statvfs for the call.
    if unsafe { libc::statvfs(c.as_ptr(), &raw mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let unit = if st.f_frsize > 0 { st.f_frsize } else { st.f_bsize };
    let meta = std::fs::metadata(path)?;
    let device = std::os::unix::fs::MetadataExt::dev(&meta);
    Ok(Volume {
        blocks: st.f_blocks as u64,
        available: st.f_bavail as u64,
        free: st.f_bfree as u64,
        block_size: u32::try_from(unit).unwrap_or(4096),
        serial: (device ^ (device >> 32)) as u32,
        name_max: u32::try_from(st.f_namemax).unwrap_or(255).max(1),
    })
}

/// Sets `path`'s access and modification times, each as seconds and
/// nanoseconds; `None` leaves one as it is.
pub fn set_times(
    path: &Path,
    access: Option<(i64, i64)>,
    write: Option<(i64, i64)>,
) -> io::Result<()> {
    let c = c_path(path)?;
    let spec = |time: Option<(i64, i64)>| match time {
        Some((seconds, nanoseconds)) => {
            libc::timespec { tv_sec: seconds as _, tv_nsec: nanoseconds as _ }
        }
        None => libc::timespec { tv_sec: 0, tv_nsec: libc::UTIME_OMIT as _ },
    };
    let times = [spec(access), spec(write)];
    // SAFETY: the path is a C string, and `times` the two timespecs.
    if unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), times.as_ptr(), 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
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

/// This process's user.
pub fn uid() -> u32 {
    // SAFETY: geteuid only answers.
    unsafe { libc::geteuid() }
}

/// A user's or group's entry, looked up by number or by name with the
/// reentrant calls: its name and number.
fn lookup<T>(
    call: impl FnOnce(*mut T, *mut libc::c_char, usize, *mut *mut T) -> libc::c_int,
    read: impl FnOnce(&T) -> (*const libc::c_char, u32),
) -> Option<(String, u32)> {
    // SAFETY: all zeros is a valid passwd or group: numbers, and pointers
    // that stay null until the call fills them.
    let mut entry: T = unsafe { std::mem::zeroed() };
    let mut buffer = vec![0u8; 16 * 1024];
    let mut found: *mut T = std::ptr::null_mut();
    if call(&raw mut entry, buffer.as_mut_ptr().cast(), buffer.len(), &raw mut found) != 0
        || found.is_null()
    {
        return None;
    }
    let (name, id) = read(&entry);
    if name.is_null() {
        return None;
    }
    // SAFETY: the call set the name to a C string in the buffer, which
    // lives until here.
    let name = unsafe { std::ffi::CStr::from_ptr(name) }.to_str().ok()?.to_string();
    Some((name, id))
}

/// User `uid`'s name, if it has one.
pub fn user_name(uid: u32) -> Option<String> {
    lookup(
        // SAFETY: every pointer is valid for the call; the buffer for its
        // length.
        |pw, buffer, length, found| unsafe { libc::getpwuid_r(uid, pw, buffer, length, found) },
        |pw: &libc::passwd| (pw.pw_name.cast_const(), pw.pw_uid),
    )
    .map(|(name, _)| name)
}

/// Group `gid`'s name, if it has one.
pub fn group_name(gid: u32) -> Option<String> {
    lookup(
        // SAFETY: as for getpwuid_r.
        |gr, buffer, length, found| unsafe { libc::getgrgid_r(gid, gr, buffer, length, found) },
        |gr: &libc::group| (gr.gr_name.cast_const(), gr.gr_gid),
    )
    .map(|(name, _)| name)
}

/// The number of the user called `name`, if there is one.
pub fn user_id(name: &str) -> Option<u32> {
    let c = CString::new(name).ok()?;
    lookup(
        // SAFETY: as for getpwuid_r, and the name is a C string.
        |pw, buffer, length, found| unsafe {
            libc::getpwnam_r(c.as_ptr(), pw, buffer, length, found)
        },
        |pw: &libc::passwd| (pw.pw_name.cast_const(), pw.pw_uid),
    )
    .map(|(_, uid)| uid)
}

/// The number of the group called `name`, if there is one.
pub fn group_id(name: &str) -> Option<u32> {
    let c = CString::new(name).ok()?;
    lookup(
        // SAFETY: as for getpwnam_r.
        |gr, buffer, length, found| unsafe {
            libc::getgrnam_r(c.as_ptr(), gr, buffer, length, found)
        },
        |gr: &libc::group| (gr.gr_name.cast_const(), gr.gr_gid),
    )
    .map(|(_, gid)| gid)
}

/// Whether descriptor `fd` is open.
pub fn is_open(fd: RawFd) -> bool {
    // SAFETY: F_GETFD only looks at the descriptor.
    unsafe { libc::fcntl(fd, libc::F_GETFD) >= 0 }
}
