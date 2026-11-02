// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The kernel's own interface (radix-ipc(7)), for what the C library does
//! not wrap: radix-directory(2)'s `mount_fuse`. Built for another system
//! (the unit tests run on the build machine), every call fails with
//! no_such_syscall.

use radix_abi::errors::{File, Kernel};
use radix_abi::{Status, Syscalls};

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

/// What a status from the kernel says, for a person.
pub fn said(status: Status) -> String {
    if let Some(error) = Kernel::from_status(status) {
        return match error {
            // Mounting needs the control right, which administrators'
            // sessions hold, and is refused through a view.
            Kernel::MissingRights | Kernel::NotSupported => {
                "mounting needs an administrator's session (elevate)".to_string()
            }
            error => error.name().replace('_', " "),
        };
    }
    match File::from_status(status) {
        Some(error) => error.name().replace('_', " "),
        None => format!("error {:#x}", status.0),
    }
}
