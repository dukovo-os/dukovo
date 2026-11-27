// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The C library and the kernel's own calls, behind safe functions (ADR
//! 0730). Every program of Dukovo's on Rust's standard library forbids
//! `unsafe` code, and calls what the standard library does not offer
//! through this crate: signals, terminals, descriptors passed over a
//! socket, a fork, folders reached without following links, the
//! accounts' entries, local time, the C library's patterns, a driver's
//! card, mapped memory and accesses a test needs to happen, the kernel's
//! own interface, the descriptor numbers a shell keeps, and on Linux
//! what the lab's Linux programs need.
//! It is the one place in user space, beside the runtime, where such a
//! call is written, so it is where a review of how a program meets the
//! system starts.
//!
//! Each function takes descriptors as `AsFd` and gives new ones as
//! `OwnedFd`, never as numbers, so that a program cannot close or use a
//! descriptor it does not hold; [`numbered`] alone takes numbers, and
//! only ones its table holds. Each opens what it opens with
//! close-on-exec, as the standard library does: [`fd::handed_down`]
//! relies on it. A call that fails returns the C library's errno as an
//! `io::Error`.
//!
//! The crate builds for the build machine too, where the unit tests of
//! the programs run: there it calls Linux's C library, and the kernel's
//! own calls fail with no_such_syscall.

pub mod device;
pub mod dir;
pub mod fd;
pub mod fs;
pub mod kernel;
#[cfg(target_os = "linux")]
pub mod linux;
pub mod memory;
pub mod net;
pub mod numbered;
pub mod pattern;
pub mod poll;
pub mod process;
pub mod random;
pub mod signal;
pub mod system;
pub mod terminal;
pub mod text;
pub mod time;
pub mod users;
pub mod volatile;

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// A process's number, as the C library gives it.
pub type Pid = libc::pid_t;

/// The result of a C library call that returns -1 and sets errno when it
/// fails.
fn checked(result: libc::c_int) -> io::Result<libc::c_int> {
    if result == -1 { Err(io::Error::last_os_error()) } else { Ok(result) }
}

/// `path` as the C string the C library takes. A path with a NUL byte
/// names no file, and is refused as the standard library refuses it.
fn c_path(path: &Path) -> io::Result<CString> {
    c_string(path.as_os_str().as_bytes())
}

/// `bytes` as a C string; bytes with a NUL in them are refused.
fn c_string(bytes: &[u8]) -> io::Result<CString> {
    CString::new(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a name or path holds a NUL byte"))
}

/// The bytes of the C string at the start of `field`, a fixed array of the
/// C library's (`utsname`'s names), up to its NUL or its end.
fn field_bytes(field: &[libc::c_char]) -> Vec<u8> {
    // c_char is i8 on x86-64: each is a byte of the name, taken as it is.
    field.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect()
}
