// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The kernel's own interface (radix-ipc(7)), through `radix_abi`'s
//! wrappers, generated from `abi/*.ridl`.
//!
//! Built for another system (the unit tests run on the build machine),
//! every call fails with no_such_syscall.

use radix_abi::errors::{File, Kernel};
use radix_abi::{Status, Syscalls, startup};

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

/// The slot of file descriptor `fd` (radix-posix(7)).
pub fn slot_of(fd: i32) -> u32 {
    startup::SLOT_FD_BASE + fd as u32
}

/// The errno a status maps to, as the C library maps it.
pub fn errno_of(status: Status) -> i32 {
    if let Some(error) = Kernel::from_status(status) {
        error.errno()
    } else if let Some(error) = File::from_status(status) {
        error.errno()
    } else {
        libc::EIO
    }
}
