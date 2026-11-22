// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Linux's error numbers, and the error number of a native error status.
//!
//! A Linux system call returns `-errno` in `rax` for a failure, with the
//! numbers of Linux's `asm-generic/errno-base.h` (1 to 34) and
//! `asm-generic/errno.h` (35 to 133), which are the numbers of x86-64. The
//! constants here are the ones a call of the Linux table can return, and
//! the ones a native status maps to.
//!
//! [`from_status`] is the Rust form of `radix::to_errno` in the C library's
//! Radix layer (`ports/mlibc/sysdeps/radix/generic/radix.cpp`): a native
//! method that fails gives the Linux table the same errno there that a
//! program using the C library gets. Both read the `=> ERRNO` each error
//! carries in `abi/errors.ridl`, so they cannot differ in what an error
//! maps to; a call that needs another number for one error (a missing file
//! in a path walk, say) remaps it after this.

use radix_abi::Status;
use radix_abi::errors::{File, Kernel};

/// Operation not permitted.
pub const EPERM: i32 = 1;
/// No such file or directory.
pub const ENOENT: i32 = 2;
/// No such process.
pub const ESRCH: i32 = 3;
/// Interrupted system call.
pub const EINTR: i32 = 4;
/// Input/output error.
pub const EIO: i32 = 5;
/// No such device or address.
pub const ENXIO: i32 = 6;
/// Argument list too long.
pub const E2BIG: i32 = 7;
/// Exec format error.
pub const ENOEXEC: i32 = 8;
/// Bad file descriptor.
pub const EBADF: i32 = 9;
/// No child processes.
pub const ECHILD: i32 = 10;
/// Try again: the call would block. Also named EWOULDBLOCK.
pub const EAGAIN: i32 = 11;
/// Out of memory.
pub const ENOMEM: i32 = 12;
/// Permission denied.
pub const EACCES: i32 = 13;
/// Bad address.
pub const EFAULT: i32 = 14;
/// Block device required.
pub const ENOTBLK: i32 = 15;
/// Device or resource busy.
pub const EBUSY: i32 = 16;
/// File exists.
pub const EEXIST: i32 = 17;
/// Cross-device link.
pub const EXDEV: i32 = 18;
/// No such device.
pub const ENODEV: i32 = 19;
/// Not a directory.
pub const ENOTDIR: i32 = 20;
/// Is a directory.
pub const EISDIR: i32 = 21;
/// Invalid argument.
pub const EINVAL: i32 = 22;
/// File table overflow.
pub const ENFILE: i32 = 23;
/// Too many open files.
pub const EMFILE: i32 = 24;
/// Not a typewriter: the file is not a terminal, or does not take the
/// `ioctl`.
pub const ENOTTY: i32 = 25;
/// Text file busy.
pub const ETXTBSY: i32 = 26;
/// File too large.
pub const EFBIG: i32 = 27;
/// No space left on device.
pub const ENOSPC: i32 = 28;
/// Illegal seek.
pub const ESPIPE: i32 = 29;
/// Read-only file system.
pub const EROFS: i32 = 30;
/// Too many links.
pub const EMLINK: i32 = 31;
/// Broken pipe.
pub const EPIPE: i32 = 32;
/// Numerical argument out of domain.
pub const EDOM: i32 = 33;
/// Numerical result out of range.
pub const ERANGE: i32 = 34;
/// Resource deadlock would occur. Also named EDEADLOCK on x86-64.
pub const EDEADLK: i32 = 35;
/// File name too long.
pub const ENAMETOOLONG: i32 = 36;
/// No record locks available.
pub const ENOLCK: i32 = 37;
/// Invalid system call number, or a call this kernel does not have.
pub const ENOSYS: i32 = 38;
/// Directory not empty.
pub const ENOTEMPTY: i32 = 39;
/// Too many levels of symbolic links.
pub const ELOOP: i32 = 40;
/// No message of the desired type.
pub const ENOMSG: i32 = 42;
/// Identifier removed.
pub const EIDRM: i32 = 43;
/// No data available.
pub const ENODATA: i32 = 61;
/// Timer expired.
pub const ETIME: i32 = 62;
/// Machine is not on the network.
pub const ENONET: i32 = 64;
/// Protocol error.
pub const EPROTO: i32 = 71;
/// Value too large for defined data type.
pub const EOVERFLOW: i32 = 75;
/// File descriptor in bad state.
pub const EBADFD: i32 = 77;
/// Invalid or incomplete multibyte or wide character.
pub const EILSEQ: i32 = 84;
/// Socket operation on non-socket.
pub const ENOTSOCK: i32 = 88;
/// Destination address required.
pub const EDESTADDRREQ: i32 = 89;
/// Message too long.
pub const EMSGSIZE: i32 = 90;
/// Protocol wrong type for socket.
pub const EPROTOTYPE: i32 = 91;
/// Protocol not available.
pub const ENOPROTOOPT: i32 = 92;
/// Protocol not supported.
pub const EPROTONOSUPPORT: i32 = 93;
/// Socket type not supported.
pub const ESOCKTNOSUPPORT: i32 = 94;
/// Operation not supported. Also named ENOTSUP.
pub const EOPNOTSUPP: i32 = 95;
/// Address family not supported by protocol.
pub const EAFNOSUPPORT: i32 = 97;
/// Address already in use.
pub const EADDRINUSE: i32 = 98;
/// Cannot assign requested address.
pub const EADDRNOTAVAIL: i32 = 99;
/// Network is down.
pub const ENETDOWN: i32 = 100;
/// Network is unreachable.
pub const ENETUNREACH: i32 = 101;
/// Software caused connection abort.
pub const ECONNABORTED: i32 = 103;
/// Connection reset by peer.
pub const ECONNRESET: i32 = 104;
/// No buffer space available.
pub const ENOBUFS: i32 = 105;
/// Transport endpoint is already connected.
pub const EISCONN: i32 = 106;
/// Transport endpoint is not connected.
pub const ENOTCONN: i32 = 107;
/// Cannot send after transport endpoint shutdown.
pub const ESHUTDOWN: i32 = 108;
/// Too many references: cannot splice.
pub const ETOOMANYREFS: i32 = 109;
/// Connection timed out.
pub const ETIMEDOUT: i32 = 110;
/// Connection refused.
pub const ECONNREFUSED: i32 = 111;
/// Host is down.
pub const EHOSTDOWN: i32 = 112;
/// No route to host.
pub const EHOSTUNREACH: i32 = 113;
/// Operation already in progress.
pub const EALREADY: i32 = 114;
/// Operation now in progress.
pub const EINPROGRESS: i32 = 115;
/// Stale file handle.
pub const ESTALE: i32 = 116;
/// Structure needs cleaning: the filesystem is damaged.
pub const EUCLEAN: i32 = 117;
/// Disk quota exceeded.
pub const EDQUOT: i32 = 122;
/// Operation canceled.
pub const ECANCELED: i32 = 125;
/// Owner died, for a robust mutex.
pub const EOWNERDEAD: i32 = 130;
/// State not recoverable, for a robust mutex.
pub const ENOTRECOVERABLE: i32 = 131;

/// The kernel's own code for a call that a signal interrupted and that is
/// to run again if the handler has `SA_RESTART`. It never reaches a
/// program: the return path turns it into EINTR or restarts the call.
pub const ERESTARTSYS: i32 = 512;
/// The kernel's own code for a call that a signal interrupted and that
/// runs again whatever the handler says. It never reaches a program.
pub const ERESTARTNOINTR: i32 = 513;
/// The kernel's own code for a call that runs again only if no handler ran
/// for the signal. It never reaches a program: with a handler it becomes
/// EINTR.
pub const ERESTARTNOHAND: i32 = 514;
/// The kernel's own code for a call that runs again through
/// `restart_syscall` with the time it had left. It never reaches a
/// program.
pub const ERESTART_RESTARTBLOCK: i32 = 516;

/// The status the C library's Radix layer uses for a call that a thread
/// cancellation cut short. The kernel never returns it; it is mapped here
/// because `radix::to_errno` maps it.
const CANCELED: u64 = u64::MAX;

/// The errno of a native status: 0 for success, ECANCELED for the C
/// library's cancellation mark, EIO for an error of a domain or with a
/// code this crate does not know, and for every other native error the
/// number its `abi/errors.ridl` entry names after `=>`.
///
/// The detail in the status's high bits does not change the answer.
pub fn from_status(status: Status) -> i32 {
    if status.is_ok() {
        return 0;
    }
    if status.0 == CANCELED {
        return ECANCELED;
    }
    if let Some(error) = Kernel::from_status(status) {
        return error.errno();
    }
    if let Some(error) = File::from_status(status) {
        return error.errno();
    }
    EIO
}
