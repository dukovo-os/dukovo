// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The kernel's own interface (radix-ipc(7)), for what the C library does
//! not wrap: `radix_abi`'s wrappers, generated from `abi/*.ridl`, call the
//! kernel through [`SYS`], and [`control`] sends a device a request with a
//! buffer. Built for another system (the unit tests run on the build
//! machine), every call fails with no_such_syscall.

use std::io;
use std::os::fd::{AsFd, AsRawFd};

use radix_abi::errors::{File, Kernel};
use radix_abi::{Status, Syscalls, file, startup};

/// The kernel, through the `syscall` instruction.
pub struct Sys;

impl Syscalls for Sys {
    #[cfg(target_os = "radix")]
    #[inline(always)]
    fn syscall(&self, number: u64, args: [u64; 10]) -> (u64, [u64; 10]) {
        let mut r = args;
        let status: u64;
        // SAFETY: the kernel reads only these registers, writes only these
        // and rcx and r11 (declared clobbered), and touches no memory of
        // the program's but the buffers the arguments name, which the
        // wrappers pass from references they hold; it does not use the
        // program's stack.
        unsafe {
            std::arch::asm!(
                "syscall",
                inlateout("rax") number => status,
                inlateout("rdi") r[0],
                inlateout("rsi") r[1],
                inlateout("rdx") r[2],
                inlateout("r10") r[3],
                inlateout("r8") r[4],
                inlateout("r9") r[5],
                inlateout("r12") r[6],
                inlateout("r13") r[7],
                inlateout("r14") r[8],
                inlateout("r15") r[9],
                out("rcx") _,
                out("r11") _,
                options(nostack),
            );
        }
        (status, r)
    }

    #[cfg(not(target_os = "radix"))]
    fn syscall(&self, _number: u64, args: [u64; 10]) -> (u64, [u64; 10]) {
        (Kernel::NoSuchSyscall.status(0).0, args)
    }
}

/// The kernel, for `radix_abi`'s wrappers.
pub static SYS: Sys = Sys;

/// Sends the device open as `fd` its request `request` with `argument`,
/// and `buffer` for it to read or fill (radix-file(2)'s `control`); what
/// each request reads or writes is the device's page's (radix-input(2),
/// radix-framebuffer(2)). Returns the request's result.
///
/// # Errors
///
/// The kernel's status: `file.not_supported` for a request the device does
/// not take; [`io_error`] gives it as an errno.
pub fn control(
    fd: impl AsFd,
    request: u32,
    argument: u64,
    buffer: &mut [u8],
) -> Result<u64, Status> {
    // A descriptor's capability is in the slot of its number past the base;
    // a descriptor is never negative.
    let slot = startup::SLOT_FD_BASE + fd.as_fd().as_raw_fd().unsigned_abs();
    let address = if buffer.is_empty() { 0 } else { buffer.as_mut_ptr() as u64 };
    file::control(&SYS, slot, request, argument, address, buffer.len() as u64)
        .map(|done| done.result)
}

/// A status from the kernel as an I/O error, with the errno the C library
/// maps it to; EIO for one it has none for.
#[must_use]
pub fn io_error(status: Status) -> io::Error {
    let errno = if let Some(error) = Kernel::from_status(status) {
        error.errno()
    } else if let Some(error) = File::from_status(status) {
        error.errno()
    } else {
        libc::EIO
    };
    io::Error::from_raw_os_error(errno)
}
