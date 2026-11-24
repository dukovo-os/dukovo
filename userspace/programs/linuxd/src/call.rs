// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! One forwarded call, and what linuxd may do to the process that made it
//! (radix-linux-call(2), ADR 0618).
//!
//! The kernel gives each forwarded call a capability of its own, which
//! reaches the calling process until the call has its answer: its memory,
//! to read and write, and its descriptors, to give and to take. The
//! handlers of the calls work through [`Process`], so that the host's
//! tests run them against [`Fake`], a process in memory.

use radix_abi::capability::{self, ObjectKind};
use radix_abi::directory::FileStatus;
use radix_abi::linux_call::{self, GiveFlags};
use radix_abi::{file, process, startup};

use crate::sys::{SYS, errno_of};

/// What a handler answers: Linux's result, or the errno it fails with.
pub type Answer = Result<i64, i32>;

/// Who made a call: the process's and thread's numbers, and the user and
/// group it acts for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Who {
    /// The process's number.
    pub process: u64,
    /// The thread's number.
    pub thread: u64,
    /// The user it acts for.
    pub user: u32,
    /// The group it acts for.
    pub group: u32,
}

/// The process that made a call, as far as the call reaches it.
pub trait Process {
    /// Who made the call.
    fn who(&self) -> Who;

    /// Fills `buffer` from the caller's memory at `address`; EFAULT where
    /// it cannot be read.
    fn read(&mut self, address: u64, buffer: &mut [u8]) -> Result<(), i32>;

    /// Writes `bytes` to the caller's memory at `address`; EFAULT where
    /// it cannot be written.
    fn write(&mut self, address: u64, bytes: &[u8]) -> Result<(), i32>;

    /// The NUL-terminated string at `address`, without its NUL, of at
    /// most `max` bytes before it: ENAMETOOLONG if longer, EFAULT where it
    /// cannot be read.
    fn read_string(&mut self, address: u64, max: usize) -> Result<Vec<u8>, i32>;

    /// Gives the caller the capability in linuxd's slot `slot` as its
    /// lowest free descriptor, closed on exec if `cloexec`; returns the
    /// descriptor, or EMFILE.
    fn give(&mut self, slot: u32, cloexec: bool) -> Result<i32, i32>;

    /// Copies the capability of the caller's descriptor `fd` into
    /// linuxd's slot `slot`, and says what kind of object it is: EBADF if
    /// the descriptor is not open.
    fn take(&mut self, fd: i32, slot: u32) -> Result<ObjectKind, i32>;

    /// The node number (`st_ino`) of the file the caller's descriptor `fd`
    /// holds: EBADF if it is not open, ENOTSOCK if it is not a file.
    fn node_of(&mut self, fd: i32) -> Result<u64, i32>;
}

/// Reads a value of Linux's, laid out as the caller's memory holds it,
/// from `address`.
pub fn read_value<P: Process + ?Sized, const N: usize>(
    process: &mut P,
    address: u64,
) -> Result<[u8; N], i32> {
    let mut bytes = [0u8; N];
    process.read(address, &mut bytes)?;
    Ok(bytes)
}

/// A call the kernel forwarded: its capability, in linuxd's `slot`, and
/// a slot of the worker's own that is empty between uses.
pub struct Call {
    slot: u32,
    scratch: u32,
    who: Who,
}

impl Call {
    /// The call the running thread received and has not answered yet, its
    /// capability taken into `slot`: refused unless the kernel forwarded
    /// it, as a message someone else sent on the endpoint reaches no
    /// process. `scratch` is an empty slot the call may use while it
    /// lasts. Returns the call and its number.
    pub fn received(slot: u32, scratch: u32) -> Result<(Call, u64), i32> {
        process::forwarded_call(&SYS, startup::SLOT_PROCESS, slot).map_err(errno_of)?;
        let call = match linux_call::info(&SYS, slot) {
            Ok(call) => call,
            Err(status) => {
                let _ = capability::delete(&SYS, slot);
                return Err(errno_of(status));
            }
        };
        let who =
            Who { process: call.process, thread: call.thread, user: call.user, group: call.group };
        Ok((Call { slot, scratch, who }, call.number))
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        let _ = capability::delete(&SYS, self.slot);
    }
}

/// The most bytes one method moves.
const STEP: usize = linux_call::COPY_MAX as usize;

impl Process for Call {
    fn who(&self) -> Who {
        self.who
    }

    fn read(&mut self, address: u64, buffer: &mut [u8]) -> Result<(), i32> {
        let mut at = address;
        for chunk in buffer.chunks_mut(STEP) {
            linux_call::read(&SYS, self.slot, at, chunk).map_err(errno_of)?;
            at = at.checked_add(chunk.len() as u64).ok_or(libc::EFAULT)?;
        }
        Ok(())
    }

    fn write(&mut self, address: u64, bytes: &[u8]) -> Result<(), i32> {
        let mut at = address;
        for chunk in bytes.chunks(STEP) {
            linux_call::write(&SYS, self.slot, at, chunk).map_err(errno_of)?;
            at = at.checked_add(chunk.len() as u64).ok_or(libc::EFAULT)?;
        }
        Ok(())
    }

    fn read_string(&mut self, address: u64, max: usize) -> Result<Vec<u8>, i32> {
        // Room for the NUL, which the kernel looks for within the buffer.
        let mut buffer = vec![0u8; max.min(STEP - 1) + 1];
        let len =
            linux_call::read_string(&SYS, self.slot, address, &mut buffer).map_err(errno_of)?.len;
        buffer.truncate(len as usize);
        Ok(buffer)
    }

    fn give(&mut self, slot: u32, cloexec: bool) -> Result<i32, i32> {
        let flags = if cloexec { GiveFlags::CLOSE_ON_EXEC } else { GiveFlags::NONE };
        match linux_call::give(&SYS, self.slot, slot, 0, flags) {
            Ok(given) => i32::try_from(given.descriptor).map_err(|_| libc::EMFILE),
            Err(status) => match errno_of(status) {
                libc::ENOSPC => Err(libc::EMFILE),
                errno => Err(errno),
            },
        }
    }

    fn take(&mut self, fd: i32, slot: u32) -> Result<ObjectKind, i32> {
        let fd = u32::try_from(fd).map_err(|_| libc::EBADF)?;
        linux_call::take(&SYS, self.slot, fd, slot).map(|taken| taken.kind).map_err(errno_of)
    }

    fn node_of(&mut self, fd: i32) -> Result<u64, i32> {
        let kind = self.take(fd, self.scratch)?;
        // The file's id, which FUSE's attributes give as the node number
        // (radix-file(2)'s `stat`, a radix-directory(2) FileStatus).
        let mut status = [0u8; FileStatus::SIZE];
        let stated = if kind == ObjectKind::File {
            file::stat(&SYS, self.scratch, &mut status).map_err(errno_of)
        } else {
            Err(libc::ENOTSOCK)
        };
        let _ = capability::delete(&SYS, self.scratch);
        stated?;
        let id = &status[FileStatus::OFFSET_ID..FileStatus::OFFSET_ID + 8];
        Ok(u64::from_le_bytes(id.try_into().expect("eight bytes")))
    }
}

/// A process in memory, for the host's tests: its memory a list of
/// regions, its descriptors a table of kinds, and what was given to it.
#[cfg(test)]
pub struct Fake {
    /// Who it is.
    pub who: Who,
    /// Its memory: each region's address and bytes.
    pub regions: Vec<(u64, Vec<u8>)>,
    /// Its descriptors, by number: the kind of what each holds, and its
    /// node number if it is a file.
    pub descriptors: std::collections::BTreeMap<i32, (ObjectKind, Option<u64>)>,
    /// What linuxd's slots hold, for `give`, as the descriptors do.
    pub slots: std::collections::BTreeMap<u32, (ObjectKind, Option<u64>)>,
}

#[cfg(test)]
impl Fake {
    /// A process of `who` with one region of `len` zeros at `address`.
    pub fn new(who: Who, address: u64, len: usize) -> Fake {
        Fake {
            who,
            regions: vec![(address, vec![0; len])],
            descriptors: std::collections::BTreeMap::new(),
            slots: std::collections::BTreeMap::new(),
        }
    }

    /// The region and the offset in it of `len` bytes at `address`.
    fn find(&mut self, address: u64, len: usize) -> Result<(&mut Vec<u8>, usize), i32> {
        let end = address.checked_add(len as u64).ok_or(libc::EFAULT)?;
        self.regions
            .iter_mut()
            .find(|(start, bytes)| *start <= address && end <= start + bytes.len() as u64)
            .map(|(start, bytes)| {
                let offset = (address - *start) as usize;
                (bytes, offset)
            })
            .ok_or(libc::EFAULT)
    }
}

#[cfg(test)]
impl Process for Fake {
    fn who(&self) -> Who {
        self.who
    }

    fn read(&mut self, address: u64, buffer: &mut [u8]) -> Result<(), i32> {
        let len = buffer.len();
        let (bytes, at) = self.find(address, len)?;
        buffer.copy_from_slice(&bytes[at..at + len]);
        Ok(())
    }

    fn write(&mut self, address: u64, data: &[u8]) -> Result<(), i32> {
        let (bytes, at) = self.find(address, data.len())?;
        bytes[at..at + data.len()].copy_from_slice(data);
        Ok(())
    }

    fn read_string(&mut self, address: u64, max: usize) -> Result<Vec<u8>, i32> {
        let mut out = Vec::new();
        let mut at = address;
        loop {
            let mut byte = [0u8];
            self.read(at, &mut byte)?;
            if byte[0] == 0 {
                return Ok(out);
            }
            if out.len() == max {
                return Err(libc::ENAMETOOLONG);
            }
            out.push(byte[0]);
            at += 1;
        }
    }

    fn give(&mut self, slot: u32, _cloexec: bool) -> Result<i32, i32> {
        let held = *self.slots.get(&slot).ok_or(libc::EBADF)?;
        let fd = (0..).find(|fd| !self.descriptors.contains_key(fd)).ok_or(libc::EMFILE)?;
        self.descriptors.insert(fd, held);
        Ok(fd)
    }

    fn take(&mut self, fd: i32, slot: u32) -> Result<ObjectKind, i32> {
        let held = *self.descriptors.get(&fd).ok_or(libc::EBADF)?;
        self.slots.insert(slot, held);
        Ok(held.0)
    }

    fn node_of(&mut self, fd: i32) -> Result<u64, i32> {
        match *self.descriptors.get(&fd).ok_or(libc::EBADF)? {
            (ObjectKind::File, Some(node)) => Ok(node),
            _ => Err(libc::ENOTSOCK),
        }
    }
}
