// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A tunnel's file (ADR 0339): the program's end of a tunnel interface
//! (`crate::net::tunnel`), which radix-network(2)'s `open_tunnel` puts in
//! a slot, as Linux's `/dev/net/tun` gives a file once TUNSETIFF names
//! the interface.
//!
//! A read takes the next IP packet the tunnel's stack sent, waiting for
//! one unless the description is non-blocking, cut to the buffer as a
//! datagram is. A write gives its stack a packet, which must be an IPv4
//! or IPv6 one of at most [`TAGGED_FRAME`] bytes, and never waits: a
//! packet that finds the queue full is lost, as a card loses a frame. A
//! poll finds it readable while a packet waits, and always writable. Once
//! the tunnel is taken away (`remove_link`), a poll finds it readable
//! and in error, and reads and writes fail with ENXIO. When its program's
//! last description of it goes, the tunnel is let go, and its link is
//! down until a program opens it again.

use core::any::Any;

use radix_abi::Status;
use radix_abi::errors::{File as FileError, Kernel};
use radix_abi::process::PipeFlags;
use radix_net::TAGGED_FRAME;
use radix_vfs::mount::Location;
use radix_vfs::{Access, In, Kind, Node, NodeRef, Out, Stat, mem, next_id};

use super::{Description, FileObject, pipe};
use crate::net::tunnel::{self, PipeError};
use crate::sched::{self, wait};
use crate::user::posix::poll::{self, Polled};
use crate::user::posix::signal::{Restart, interrupted};

/// What polls of every tunnel's file look at: a packet for its program,
/// or a tunnel taken away, wakes them.
pub static POLLED: Polled = Polled::new();

/// The program's end of the tunnel numbered `number`.
pub struct End {
    id: u64,
    number: usize,
}

impl Drop for End {
    fn drop(&mut self) {
        tunnel::close(self.number);
    }
}

impl Node for End {
    fn stat(&self) -> Stat {
        Stat {
            kind: Kind::CharDevice,
            size: 0,
            allocated: 0,
            id: self.id,
            created: 0,
            modified: 0,
            accessed: 0,
            changed: 0,
            links: 1,
            device: 0,
        }
    }

    fn kind(&self) -> Kind {
        Kind::CharDevice
    }

    fn access(&self) -> Access {
        Access { owner: 0, group: 0, mode: 0o600 }
    }

    // The kernel's own reads and writes through the node find nothing:
    // the descriptions' are `read` and `write` below.
    fn read(&self, _offset: u64, _out: &mut dyn Out) -> radix_vfs::Result<usize> {
        Ok(0)
    }

    fn write(&self, _offset: u64, data: &dyn In) -> radix_vfs::Result<usize> {
        Ok(data.len())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The end of a tunnel `node` is, if it is one.
pub fn end_of(node: &NodeRef) -> Option<&End> {
    radix_vfs::downcast::<End>(&**node)
}

fn status(err: PipeError) -> Status {
    match err {
        PipeError::Gone => FileError::NoDevice.status(0),
        // As Linux's TUNSETIFF for a tunnel another file has.
        PipeError::Taken => Kernel::Busy.status(0),
    }
}

/// Opens the tunnel numbered `number` for `user`'s process, putting its
/// file in slot `to` with `flags` (radix-network(2)'s `open_tunnel`).
///
/// # Errors
///
/// ENXIO's status if it is not a tunnel, EBUSY's if a program has it,
/// and the slot's if the file cannot go there.
pub fn open(
    user: &crate::user::UserRef,
    number: usize,
    to: u32,
    flags: PipeFlags,
) -> Result<(), Status> {
    tunnel::open(number).map_err(status)?;
    // From here, the end lets the tunnel go when it goes, made or not.
    let end = End { id: next_id(), number };
    let node: NodeRef = mem::arc(end).map_err(|_| Kernel::NoMemory.status(0))?;
    let mount = pipe::mount().map_err(super::status)?;
    let description = Description {
        non_blocking: flags.contains(PipeFlags::NON_BLOCKING),
        ..Description::default()
    };
    let file = FileObject::with(Location::new(mount, node), description, user.process.identity())
        .ok_or(Kernel::NoMemory.status(0))?;
    super::inet::install(user, to, file, flags)
}

/// What a read found of the oldest packet.
enum Read {
    /// It was copied, this long.
    Copied(usize),
    /// A page of the caller's buffer is not there yet.
    Absent,
    Failed(radix_vfs::Error),
}

/// The program reads the next packet from `file`'s tunnel into `out`,
/// waiting for one unless `non_blocking`: its length.
pub fn read(file: &FileObject, out: &mut dyn Out, non_blocking: bool) -> Result<usize, Status> {
    let end = end_of(&file.at.node).ok_or(Kernel::InvalidArgument.status(0))?;
    let number = end.number;
    loop {
        // Copied under the tunnel's lock without waiting for a page, which
        // is made with no lock held.
        let read = tunnel::take(number, |packet| {
            let len = packet.len().min(out.len());
            match out.put_now(0, &packet[..len]) {
                Ok(true) => (Read::Copied(len), true),
                Ok(false) => (Read::Absent, false),
                Err(err) => (Read::Failed(err), false),
            }
        })
        .map_err(status)?;
        match read {
            Some(Read::Copied(len)) => return Ok(len),
            Some(Read::Absent) => {
                out.fault_in(0, out.len().min(TAGGED_FRAME)).map_err(super::status)?;
                continue;
            }
            Some(Read::Failed(err)) => return Err(super::status(err)),
            None => {}
        }
        if non_blocking {
            return Err(Kernel::WouldBlock.status(0));
        }
        wait::wait_until(tunnel::key(number), "tunnel", || {
            let (waiting, gone) = tunnel::ready(number);
            waiting || gone || sched::interrupted()
        });
        let (waiting, gone) = tunnel::ready(number);
        if !waiting && !gone && sched::interrupted() {
            return Err(interrupted(Restart::Always));
        }
    }
}

/// The program writes the packet `data` to `file`'s tunnel, for its
/// stack: its length.
pub fn write(file: &FileObject, data: &dyn In) -> Result<usize, Status> {
    let end = end_of(&file.at.node).ok_or(Kernel::InvalidArgument.status(0))?;
    let len = data.len();
    if !(1..=TAGGED_FRAME).contains(&len) {
        return Err(Kernel::InvalidArgument.status(0));
    }
    loop {
        let (mut absent, mut failed) = (false, None);
        let written = tunnel::give(end.number, len, |room| match data.get_now(0, room) {
            // An IPv4 or IPv6 packet, by the version in its first byte.
            Ok(true) => matches!(room[0] >> 4, 4 | 6),
            Ok(false) => {
                absent = true;
                false
            }
            Err(err) => {
                failed = Some(err);
                false
            }
        })
        .map_err(status)?;
        if written {
            return Ok(len);
        }
        if let Some(err) = failed {
            return Err(super::status(err));
        }
        if !absent {
            return Err(Kernel::InvalidArgument.status(0));
        }
        data.fault_in(0, len).map_err(super::status)?;
    }
}

/// What a poll of `file`'s tunnel finds: readable while a packet waits,
/// readable and in error once the tunnel is gone, and always writable.
pub fn events(file: &FileObject) -> u16 {
    let Some(end) = end_of(&file.at.node) else { return poll::ERROR };
    let (waiting, gone) = tunnel::ready(end.number);
    let mut events = poll::WRITABLE;
    if waiting || gone {
        events |= poll::READABLE;
    }
    if gone {
        events |= poll::ERROR;
    }
    events
}
