// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A packet capture's file (ADR 0346): what radix-network(2)'s
//! `open_capture` puts in a slot, whose reads take the frames the
//! capture kept (`crate::net::capture`), as a BSD BPF device's do.
//!
//! A read takes as many whole frames as fit in its buffer, each
//! radix-network(2)'s CaptureHeader, its bytes and zeros to a multiple of
//! 8, waiting for one unless the description is non-blocking, and cuts
//! the first to the buffer if it alone does not fit; a buffer too small
//! for a header is refused. A poll finds it readable while a frame
//! waits. Once the capture's interface is taken away, a read finds the
//! frames that wait, then fails with ENXIO, and a poll finds the file
//! readable and in error. Its `control` takes CAPTURE_FILTER, a classic
//! BPF program, and CAPTURE_COUNTS. It cannot be written.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::Any;

use radix_abi::Status;
use radix_abi::errors::{File as FileError, Kernel};
use radix_abi::network::{CAPTURE_COUNTS, CAPTURE_FILTER, CaptureCounts, CaptureHeader};
use radix_abi::process::PipeFlags;
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_net::bpf::{self, INSTRUCTION, Instruction, MAX_INSTRUCTIONS};
use radix_vfs::mount::Location;
use radix_vfs::{Access, In, Kind, Node, NodeRef, Out, Stat, mem, next_id};

use super::{Description, FileObject, pipe};
use crate::net::capture::{self, Capture};
use crate::sched::{self, wait};
use crate::user::posix::poll::{self, Polled};
use crate::user::posix::signal::{Restart, interrupted};

/// What polls of every capture's file look at: a frame for its program,
/// or an interface taken away, wakes them.
pub static POLLED: Polled = Polled::new();

/// A capture's file.
pub struct End {
    id: u64,
    capture: Arc<Capture, Kmalloc>,
}

impl Drop for End {
    fn drop(&mut self) {
        capture::close(&self.capture);
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
        Access { owner: 0, group: 0, mode: 0o400 }
    }

    // The kernel's own reads and writes through the node find nothing:
    // the descriptions' are `read` below.
    fn read(&self, _offset: u64, _out: &mut dyn Out) -> radix_vfs::Result<usize> {
        Ok(0)
    }

    fn write(&self, _offset: u64, _data: &dyn In) -> radix_vfs::Result<usize> {
        Err(radix_vfs::Error::NotSupported)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The capture `node` is the file of, if it is one.
pub fn end_of(node: &NodeRef) -> Option<&End> {
    radix_vfs::downcast::<End>(&**node)
}

/// Opens a capture of the interface numbered `interface`, or of every
/// one for `None`, for `user`'s process, keeping `snap` bytes of each
/// frame and `buffer` bytes of frames, and puts its file in slot `to`
/// with `flags` (radix-network(2)'s `open_capture`).
///
/// # Errors
///
/// ENXIO's status if the interface is not there, EINVAL's for a snap
/// length or buffer out of range, EBUSY's while every capture there may
/// be is open, and the slot's if the file cannot go there.
pub fn open(
    user: &crate::user::UserRef,
    interface: Option<usize>,
    snap: u32,
    buffer: u32,
    to: u32,
    flags: PipeFlags,
) -> Result<(), Status> {
    let capture = capture::open(interface, snap, buffer).map_err(capture::OpenError::status)?;
    // From here, the end closes the capture when it goes, made or not.
    let end = End { id: next_id(), capture };
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

/// Why a copy to the reader stopped.
enum Stopped {
    /// A page of its buffer is not there yet.
    Absent,
    Failed(radix_vfs::Error),
}

/// The program reads the frames `file`'s capture kept into `out`, waiting
/// for one unless `non_blocking`: how many bytes it read.
pub fn read(file: &FileObject, out: &mut dyn Out, non_blocking: bool) -> Result<usize, Status> {
    let end = end_of(&file.at.node).ok_or(Kernel::InvalidArgument.status(0))?;
    let capture = &end.capture;
    if out.len() < CaptureHeader::SIZE {
        return Err(Kernel::InvalidArgument.status(0));
    }
    loop {
        // Copied under the capture's lock without waiting for a page,
        // which is made with no lock held.
        let taken = capture.take(out.len(), |at, bytes| match out.put_now(at, bytes) {
            Ok(true) => Ok(()),
            Ok(false) => Err(Stopped::Absent),
            Err(err) => Err(Stopped::Failed(err)),
        });
        match taken {
            Ok(Some(len)) => return Ok(len),
            Err(Stopped::Absent) => {
                out.fault_in(0, out.len()).map_err(super::status)?;
                continue;
            }
            Err(Stopped::Failed(err)) => return Err(super::status(err)),
            Ok(None) => {}
        }
        if capture.ready().1 {
            return Err(FileError::NoDevice.status(0));
        }
        if non_blocking {
            return Err(Kernel::WouldBlock.status(0));
        }
        wait::wait_until(capture.key(), "capture", || {
            let (waiting, gone) = capture.ready();
            waiting || gone || sched::interrupted()
        });
        let (waiting, gone) = capture.ready();
        if !waiting && !gone && sched::interrupted() {
            return Err(interrupted(Restart::Always));
        }
    }
}

/// What a poll of `file`'s capture finds: readable while a frame waits,
/// readable and in error once its interface is gone.
pub fn events(file: &FileObject) -> u16 {
    let Some(end) = end_of(&file.at.node) else { return poll::ERROR };
    let (waiting, gone) = end.capture.ready();
    let mut events = 0;
    if waiting || gone {
        events |= poll::READABLE;
    }
    if gone {
        events |= poll::ERROR;
    }
    events
}

/// `control`'s requests of `file`'s capture: `request` with the `size`
/// bytes at `buffer`, which `copy_in` reads and `copy_out` writes.
///
/// # Errors
///
/// EINVAL's status for a filter the kernel does not run or a size that
/// is not right, ENOTTY's for another request, and the copies'.
pub fn control(
    file: &FileObject,
    request: u32,
    size: u64,
    copy_in: impl Fn(&mut [u8]) -> Result<(), Status>,
    copy_out: impl Fn(&[u8]) -> Result<(), Status>,
) -> Result<(), Status> {
    let end = end_of(&file.at.node).ok_or(Kernel::NotTerminal.status(0))?;
    let invalid = || Kernel::InvalidArgument.status(0);
    match request {
        CAPTURE_FILTER if size == 0 => {
            end.capture.set_filter(None);
            Ok(())
        }
        CAPTURE_FILTER => {
            let size = usize::try_from(size).map_err(|_| invalid())?;
            let count = size / INSTRUCTION;
            if size % INSTRUCTION != 0 || count > MAX_INSTRUCTIONS {
                return Err(invalid());
            }
            let mut program: Vec<Instruction, Kmalloc> = Vec::new_in(HEAP.with(Flags::MAY_SLEEP));
            program.try_reserve_exact(count).map_err(|_| Kernel::NoMemory.status(0))?;
            let mut bytes = radix_mm::bytes::zeroed(size, HEAP.with(Flags::MAY_SLEEP))
                .map_err(|_| Kernel::NoMemory.status(0))?;
            copy_in(&mut bytes)?;
            for word in bytes.as_chunks::<INSTRUCTION>().0 {
                // Within the capacity reserved for `count`.
                let _ = program.push_within_capacity(Instruction::from_bytes(*word));
            }
            bpf::check(&program).map_err(|_| invalid())?;
            end.capture.set_filter(Some(program));
            Ok(())
        }
        CAPTURE_COUNTS => {
            if size < CaptureCounts::SIZE as u64 {
                return Err(invalid());
            }
            let (received, dropped) = end.capture.counts();
            let mut counts = [0u8; CaptureCounts::SIZE];
            counts[CaptureCounts::OFFSET_RECEIVED..CaptureCounts::OFFSET_RECEIVED + 8]
                .copy_from_slice(&received.to_le_bytes());
            counts[CaptureCounts::OFFSET_DROPPED..CaptureCounts::OFFSET_DROPPED + 8]
                .copy_from_slice(&dropped.to_le_bytes());
            copy_out(&counts)
        }
        _ => Err(Kernel::NotTerminal.status(0)),
    }
}
