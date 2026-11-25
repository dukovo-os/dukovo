// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The kernel's own interface (radix-ipc(7)), for what the C library does
//! not wrap: `radix_abi`'s wrappers, generated from `abi/*.ridl`, call the
//! kernel through [`SYS`]; [`control`] sends a device a request with a
//! buffer; and [`new_descriptor`] owns a file a call puts in a
//! descriptor's slot. Built for another system (the unit tests run on the
//! build machine), every call fails with no_such_syscall.

use std::io;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};

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
    let slot = slot_of(fd);
    let address = if buffer.is_empty() { 0 } else { buffer.as_mut_ptr() as u64 };
    file::control(&SYS, slot, request, argument, address, buffer.len() as u64)
        .map(|done| done.result)
}

/// The slot of descriptor `fd`'s capability (radix-posix(7)), for a call
/// of `radix_abi`'s on the file it holds.
#[must_use]
pub fn slot_of(fd: impl AsFd) -> u32 {
    slot_of_number(fd.as_fd().as_raw_fd())
}

/// The null device, which [`new_descriptor`] opens to learn the lowest
/// free descriptor.
#[cfg(target_os = "radix")]
const NULL: &str = "/devices/null";
#[cfg(not(target_os = "radix"))]
const NULL: &str = "/dev/null";

/// Runs `call`, a kernel call that puts a new file in the slot it is given
/// (`open_tunnel`, `trace`), with the slot of the lowest free descriptor,
/// and owns the descriptor it made, closed on exec. While another thread
/// takes that number first, the call fails with `occupied`, and it runs
/// again with the next.
///
/// # Errors
///
/// `no_room` when the process can open no more descriptors; what `call`
/// fails with but `occupied`.
pub fn new_descriptor(mut call: impl FnMut(u32) -> Result<(), Status>) -> Result<OwnedFd, Status> {
    loop {
        // The C library gives the lowest free number, which is free again
        // once it is closed.
        let probe = std::fs::File::open(NULL).map_err(|_| Kernel::NoRoom.status(0))?;
        let number = probe.as_raw_fd();
        drop(probe);
        match call(slot_of_number(number)) {
            Ok(()) => {
                // SAFETY: the kernel put a new file in the slot of
                // descriptor `number`, which was free, as the call fails
                // with `occupied` otherwise; so nothing else in the process
                // holds the number.
                let fd = unsafe { OwnedFd::from_raw_fd(number) };
                // SAFETY: fcntl sets the flags of a descriptor held here.
                unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
                return Ok(fd);
            }
            Err(status) if Kernel::from_status(status) == Some(Kernel::Occupied) => {}
            Err(status) => return Err(status),
        }
    }
}

/// The slot of descriptor number `number`: the capability of a
/// descriptor is in the slot of its number past the base, and a
/// descriptor is never negative.
fn slot_of_number(number: libc::c_int) -> u32 {
    startup::SLOT_FD_BASE + number.unsigned_abs()
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

#[cfg(test)]
mod tests {
    use radix_abi::errors::Kernel;

    use super::{new_descriptor, slot_of};

    #[test]
    fn a_number_taken_first_is_passed_over_and_a_failure_is_given() {
        let mut slots = Vec::new();
        let error = new_descriptor(|slot| {
            slots.push(slot);
            if slots.len() < 3 {
                Err(Kernel::Occupied.status(0))
            } else {
                Err(Kernel::NoSuchSyscall.status(0))
            }
        })
        .unwrap_err();
        assert_eq!(slots.len(), 3);
        assert_eq!(Kernel::from_status(error), Some(Kernel::NoSuchSyscall));
    }

    #[test]
    fn a_descriptor_s_slot_is_past_the_base() {
        let file = std::fs::File::open("/dev/null").unwrap();
        let number = std::os::fd::AsRawFd::as_raw_fd(&file).unsigned_abs();
        assert_eq!(slot_of(&file), radix_abi::startup::SLOT_FD_BASE + number);
    }
}
