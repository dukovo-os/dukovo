// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the kernel's own calls (radix-ipc(7)) say when they fail:
//! radix-directory(2)'s `mount_fuse`, which sysapi's kernel calls.

use radix_abi::Status;
use radix_abi::errors::{File, Kernel};

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
