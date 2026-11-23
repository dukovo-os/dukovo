// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the kernel's own files share (ADR 0613): the event counter
//! (`counter`), the timer file (`timer_file`), the signal file
//! (`signal_file`) and the process file (`process_file`). Each is a node of
//! character device kind on the pipes' filesystem (`pipe::mount`), made by
//! a method of the process, put in a slot, and recognised by
//! `FileObject::opened_as`, so that reads, writes and polls reach it
//! through `user::files` and `user::posix::poll` as they reach a tunnel's
//! end (`tunnel`). This module holds the node's status and access, which
//! the four have alike, and putting a new one in a slot.

use radix_abi::Status;
use radix_abi::errors::Kernel;
use radix_abi::process::PipeFlags;
use radix_vfs::mount::Location;
use radix_vfs::{Access, Kind, NodeRef, Stat};

use super::{Description, FileObject, inet, pipe};
use crate::user::UserRef;

/// What a node of one of these files says of itself, with the number `id`
/// that `next_id` gave it.
pub(super) fn stat(id: u64) -> Stat {
    Stat {
        kind: Kind::CharDevice,
        size: 0,
        allocated: 0,
        id,
        created: 0,
        modified: 0,
        accessed: 0,
        changed: 0,
        links: 1,
        device: 0,
    }
}

/// Who may use one of these files: its program, alone.
pub(super) fn access() -> Access {
    Access { owner: 0, group: 0, mode: 0o600 }
}

/// Puts a file for `node` in `user`'s slot `to`, non-blocking and closed
/// by `exec` as `flags` say, with the rights a pipe's end has for both
/// directions.
///
/// # Errors
///
/// no_memory if the file cannot be made, and the slot's status if it
/// cannot go there (`occupied`, `no_such_slot`).
pub(super) fn install(
    user: &UserRef,
    node: NodeRef,
    to: u32,
    flags: PipeFlags,
) -> Result<(), Status> {
    let mount = pipe::mount().map_err(super::status)?;
    let description = Description {
        non_blocking: flags.contains(PipeFlags::NON_BLOCKING),
        ..Description::default()
    };
    let file = FileObject::with(Location::new(mount, node), description, user.process.identity())
        .ok_or(Kernel::NoMemory.status(0))?;
    inet::install(user, to, file, flags)
}
