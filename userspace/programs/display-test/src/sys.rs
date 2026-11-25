// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the program asks of the system beside files: the display
//! layer's `control` requests (radix-file(2)), through `radix_abi`'s
//! wrappers, memory files and mappings of them, the clock, and waiting
//! for a file to have something to read.
//!
//! Built for another system (the unit tests run on the build machine),
//! every `control` fails with no_such_syscall.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use radix_abi::errors::{File as FileError, Kernel};
use radix_abi::{Status, Syscalls, file, startup};

/// The kernel, through the `syscall` instruction.
struct Sys;

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

static SYS: Sys = Sys;

/// A status as an I/O error, with the errno the C library maps it to.
fn io_error(status: Status) -> io::Error {
    let errno = if let Some(error) = Kernel::from_status(status) {
        error.errno()
    } else if let Some(error) = FileError::from_status(status) {
        error.errno()
    } else {
        libc::EIO
    };
    io::Error::from_raw_os_error(errno)
}

/// The slot of the program's own that descriptor `fd` is in, as the
/// kernel's requests name a file.
pub fn slot(fd: i32) -> u32 {
    startup::SLOT_FD_BASE + fd as u32
}

/// Sends the device open as descriptor `fd` its request `request` with
/// `argument`, and `buffer` for it to read or fill (radix-file(2)'s
/// `control`).
pub fn control(fd: i32, request: u32, argument: u64, buffer: &mut [u8]) -> io::Result<u64> {
    let address = if buffer.is_empty() { 0 } else { buffer.as_mut_ptr() as u64 };
    file::control(&SYS, slot(fd), request, argument, address, buffer.len() as u64)
        .map(|done| done.result)
        .map_err(io_error)
}

/// The time since the machine started, in nanoseconds: the clock the
/// display layer's events are stamped with.
pub fn now_ns() -> u64 {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: the call writes the one timespec it is given.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &raw mut time) };
    time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64
}

/// Waits until `file` has something to read, for `patience` at most.
/// Returns whether it has.
pub fn readable(file: &File, patience: Duration) -> bool {
    let mut waited = libc::pollfd { fd: file.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    let patience = i32::try_from(patience.as_millis()).unwrap_or(i32::MAX);
    // SAFETY: the call reads and writes the one pollfd it is given.
    let ready = unsafe { libc::poll(&raw mut waited, 1, patience) };
    ready > 0 && waited.revents & libc::POLLIN != 0
}

/// Counts the memory files the program has made, for their names.
static MADE: AtomicU32 = AtomicU32::new(0);

/// A new memory file with no name: one of _/devices/shm_ whose name is
/// taken away as soon as it is open, so that it is gone with its last
/// description.
pub fn memory_file() -> io::Result<File> {
    let number = MADE.fetch_add(1, Ordering::Relaxed);
    let path = format!("/devices/shm/display-test-{}-{number}", std::process::id());
    let file = File::options().read(true).write(true).create_new(true).open(&path)?;
    std::fs::remove_file(&path)?;
    Ok(file)
}

/// The pages of a memory file, mapped to read and write: the very pages
/// a picture made of the file shows.
#[derive(Debug)]
pub struct Mapping {
    start: *mut u8,
    bytes: usize,
}

impl Mapping {
    /// Maps the first `bytes` bytes of `file`.
    pub fn of(file: &File, bytes: usize) -> io::Result<Mapping> {
        let protection = libc::PROT_READ | libc::PROT_WRITE;
        // SAFETY: a new mapping, wherever the system puts it, takes
        // nothing of the program's; the file is open.
        let start = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                bytes,
                protection,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if start == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Mapping { start: start.cast(), bytes })
    }

    /// Its bytes. The host may read them at any moment, as it does any
    /// picture a program draws in while it shows.
    pub fn bytes(&mut self) -> &mut [u8] {
        // SAFETY: the mapping is `bytes` long, readable and writable,
        // and lives as long as `self`, which is borrowed for as long as
        // the slice; nothing else in the program has its address.
        unsafe { std::slice::from_raw_parts_mut(self.start, self.bytes) }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: the mapping is the one `of` made, and no slice of it
        // outlives `self`.
        unsafe { libc::munmap(self.start.cast(), self.bytes) };
    }
}
