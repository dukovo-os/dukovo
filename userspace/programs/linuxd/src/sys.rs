// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The kernel's own interface (radix-ipc(7)), through `radix_abi`'s
//! wrappers, generated from `abi/*.ridl`, which call it through
//! `sysapi`'s [`SYS`].

use radix_abi::errors::{File, Kernel};
use radix_abi::{Status, startup};
pub use sysapi::kernel::SYS;

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
