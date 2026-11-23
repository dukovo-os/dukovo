// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Process files (ADR 0613, radix-process(2)'s `process_file`): a file that
//! names a process and is readable once it has exited, as Linux's pidfd.
//!
//! The file holds the process weakly, so a process nobody else holds ends
//! as it would without one. It is readable once the process has exited,
//! and also hung up once its parent has waited for it (or it never had
//! one), when its number names nothing any more. Its pollers count
//! themselves in the process's `polled`, which `ProcessObject::finish`
//! and `ProcessObject::reaped` change. Reads and writes fail, as they do on
//! Linux. Process numbers are not used again, so the file never names a
//! second process.
//!
//! When it misbehaves, the boot test `posix` runs `kernelfiles.c`.

use alloc::sync::{Arc, Weak};
use core::any::Any;

use radix_abi::Status;
use radix_abi::errors::Kernel;
use radix_abi::process::PipeFlags;
use radix_mm::Kmalloc;
use radix_vfs::{Access, Node, NodeRef, Stat, mem, next_id};

use super::own_file;
use crate::user::UserRef;
use crate::user::posix::find_process;
use crate::user::posix::poll::{self, Polled};
use crate::user::process::ProcessObject;

/// A process file's node.
pub struct ProcessFile {
    id: u64,
    process: Weak<ProcessObject, Kmalloc>,
    /// The process's number, kept for when it is gone.
    number: u64,
}

impl Node for ProcessFile {
    fn stat(&self) -> Stat {
        own_file::stat(self.id)
    }

    fn access(&self) -> Access {
        own_file::access()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The process file `node` is, if it is one.
pub fn process_file_of(node: &NodeRef) -> Option<&ProcessFile> {
    radix_vfs::downcast::<ProcessFile>(&**node)
}

/// Makes a process file for process `pid` for `user`'s process, in slot
/// `to`. Any process may be named, as `kill` finds it but without asking
/// whether the caller may signal it. The slot is closed by `exec` whatever
/// `flags` say, as a pidfd is.
///
/// # Errors
///
/// no_such_process if no process is numbered `pid` (one that ended and was
/// waited for is none), no_memory, and the slot's status if the file cannot
/// go there.
pub fn make(user: &UserRef, pid: u64, to: u32, flags: PipeFlags) -> Result<(), Status> {
    let process = find_process(&user.process, pid)?;
    let file = ProcessFile { id: next_id(), process: Arc::downgrade(&process), number: pid };
    let node: NodeRef = mem::arc(file).map_err(|_| Kernel::NoMemory.status(0))?;
    own_file::install(user, node, to, flags.union(PipeFlags::CLOSE_ON_EXEC))
}

impl ProcessFile {
    /// The process's number, and whether it has exited.
    pub fn of(&self) -> (u64, bool) {
        let exited = self.process.upgrade().is_none_or(|process| process.has_exited());
        (self.number, exited)
    }

    /// Calls `each` with what polls of it count themselves in: the
    /// process's `polled`, while the process is there to be looked at.
    pub fn polled(&self, each: &mut dyn FnMut(&Polled)) {
        if let Some(process) = self.process.upgrade() {
            each(&process.polled);
        }
    }

    /// What `poll` finds: readable once the process has exited, hung up as
    /// well once it has been waited for.
    pub fn events(&self) -> u16 {
        match self.process.upgrade() {
            None => poll::READABLE | poll::HANG_UP,
            Some(process) if process.is_reaped() => poll::READABLE | poll::HANG_UP,
            Some(process) if process.has_exited() => poll::READABLE,
            Some(_) => 0,
        }
    }
}
