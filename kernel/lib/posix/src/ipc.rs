// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! System V IPC's rules (ADR 0490): who may use a shared memory segment
//! or a set of semaphores, how a key finds one and how their numbers are
//! made, and what a semaphore operation does, as Linux has them.
//!
//! The kernel keeps each kind of object in a [`Table`] of the memory
//! filesystem mounted on `/devices/shm` (`radix::fs::shm`), each with a
//! [`Perm`], and carries out what this module decides: [`decide`] whether
//! a `get` makes an object or finds one, [`Perm::permits`] what a caller
//! may do, [`check`] and [`operate`] what `semop` does to a set's values
//! and to the caller's undo adjustments, and [`undo`] what those do when
//! the process ends.

use alloc::vec::Vec;
use core::alloc::Allocator;

/// The key no other `get` finds an object by: IPC_PRIVATE.
pub const PRIVATE: i32 = 0;

/// The most semaphores a set has, Linux's SEMMSL.
pub const SEMMSL: u32 = 32000;
/// The most operations one `semop` does, Linux's SEMOPM.
pub const SEMOPM: usize = 500;
/// The highest value a semaphore holds, Linux's SEMVMX.
pub const SEMVMX: i32 = 32767;
/// The largest undo adjustment, either way, Linux's SEMAEM.
pub const SEMAEM: i32 = SEMVMX;
/// The most sets one table holds, Linux's SEMMNI.
pub const SEMMNI: usize = 32000;
/// The most segments one table holds, Linux's SHMMNI.
pub const SHMMNI: usize = 4096;
/// How an object's number holds its place in its table: the number is its
/// sequence number times this, plus its place, as Linux's IPCMNI.
pub const IPCMNI: u32 = 32768;
/// The highest sequence number, after which they start from 0 again.
const SEQ_MAX: u32 = i32::MAX as u32 / IPCMNI;

/// A semaphore operation's flag: fail with EAGAIN rather than wait
/// (IPC_NOWAIT).
pub const NOWAIT: u16 = 0o4000;
/// A semaphore operation's flag: undo it when the process ends
/// (SEM_UNDO).
pub const UNDO: u16 = 0x1000;

/// What a call asks of an object, as the bits of a mode for one class.
pub mod want {
    /// Read its status or values, or attach a segment to read.
    pub const READ: u16 = 0o4;
    /// Change its values, or attach a segment to write.
    pub const WRITE: u16 = 0o2;
}

/// Who a call acts for: the name of the capability it comes through and
/// whether that has the bypass right, which stands for Linux's
/// CAP_IPC_OWNER and CAP_SYS_ADMIN.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caller<'a> {
    /// The user's number.
    pub user: u32,
    /// Its group's number.
    pub group: u32,
    /// The other groups it is in.
    pub groups: &'a [u32],
    /// Whether it passes every check.
    pub bypass: bool,
}

impl Caller<'_> {
    fn in_group(&self, group: u32) -> bool {
        self.group == group || self.groups.contains(&group)
    }
}

/// An object's key, owner and permission bits: Linux's `struct ipc_perm`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Perm {
    /// The key it was made with; [`PRIVATE`] once it is removed.
    pub key: i32,
    /// Its owner's user, which IPC_SET changes.
    pub owner: u32,
    /// Its owner's group, likewise.
    pub group: u32,
    /// Its maker's user.
    pub creator: u32,
    /// Its maker's group.
    pub creator_group: u32,
    /// Its nine permission bits.
    pub mode: u16,
}

impl Perm {
    /// An object `caller` makes with `key` and the permission bits of
    /// `mode`.
    pub fn new(key: i32, caller: &Caller, mode: u32) -> Perm {
        Perm {
            key,
            owner: caller.user,
            group: caller.group,
            creator: caller.user,
            creator_group: caller.group,
            mode: (mode & 0o777) as u16,
        }
    }

    /// Whether `caller` may do `want` ([`want`]'s bits) to it, as Linux's
    /// `ipcperms`: the owner's bits if it owns or made it, else the
    /// group's if it is in the owner's or the maker's group, else the
    /// others'.
    pub fn permits(&self, caller: &Caller, want: u16) -> bool {
        if caller.bypass {
            return true;
        }
        let shift = if caller.user == self.owner || caller.user == self.creator {
            6
        } else if caller.in_group(self.group) || caller.in_group(self.creator_group) {
            3
        } else {
            0
        };
        want & !(self.mode >> shift) & 0o7 == 0
    }

    /// Whether `caller` may change its owner and mode (IPC_SET) or remove
    /// it (IPC_RMID): its owner or maker, or one with the bypass right.
    pub fn controls(&self, caller: &Caller) -> bool {
        caller.bypass || caller.user == self.owner || caller.user == self.creator
    }

    /// IPC_SET: gives it to `owner` and `group`, with the permission bits
    /// of `mode`.
    pub fn set(&mut self, owner: u32, group: u32, mode: u32) {
        self.owner = owner;
        self.group = group;
        self.mode = (mode & 0o777) as u16;
    }
}

/// What a `get` with the permission bits of `mode` asks of an object it
/// finds, as Linux reads them: each class's bits, together. 0666 asks to
/// read and write; 0 asks for nothing.
pub fn asked(mode: u32) -> u16 {
    ((mode >> 6 | mode >> 3 | mode) & 0o7) as u16
}

/// Why a `get` fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GetError {
    /// No object has the key, and the call did not ask to make one
    /// (ENOENT).
    NotFound,
    /// One has, and the call asked to make it only if none did (EEXIST).
    Exists,
}

/// What a `get` of `key` does, `found` being the object that has it if
/// one does: make one ([`None`]), or use the one found.
pub fn decide(
    found: Option<u32>,
    key: i32,
    create: bool,
    exclusive: bool,
) -> Result<Option<u32>, GetError> {
    if key == PRIVATE {
        return Ok(None);
    }
    match found {
        Some(_) if create && exclusive => Err(GetError::Exists),
        Some(id) => Ok(Some(id)),
        None if create => Ok(None),
        None => Err(GetError::NotFound),
    }
}

/// Why an object cannot go in a table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableError {
    /// It holds as many as it may (ENOSPC).
    Full,
    /// Memory ran out.
    NoMemory,
}

struct Entry<T> {
    key: i32,
    seq: u32,
    value: T,
}

/// The objects of one kind: each in a place, the lowest free one, and
/// named by a number made of its place and the sequence number it was
/// made with, as Linux's: a number goes on naming nothing once its object
/// is removed, until the sequence comes round again.
pub struct Table<T, A: Allocator> {
    slots: Vec<Option<Entry<T>>, A>,
    count: usize,
    max: usize,
    seq: u32,
}

impl<T, A: Allocator> Table<T, A> {
    /// An empty table of at most `max` objects (and at most [`IPCMNI`]),
    /// whose memory comes from `alloc`.
    pub fn new(max: usize, alloc: A) -> Self {
        Table { slots: Vec::new_in(alloc), count: 0, max: max.min(IPCMNI as usize), seq: 0 }
    }

    /// How many objects it holds.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether it holds none.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    fn id(index: usize, seq: u32) -> u32 {
        seq * IPCMNI + index as u32
    }

    fn entry(&self, id: u32) -> Option<&Entry<T>> {
        let entry = self.slots.get((id % IPCMNI) as usize)?.as_ref()?;
        (entry.seq == id / IPCMNI).then_some(entry)
    }

    /// The object numbered `id`.
    pub fn get(&self, id: u32) -> Option<&T> {
        self.entry(id).map(|entry| &entry.value)
    }

    /// The object numbered `id`, to change.
    pub fn get_mut(&mut self, id: u32) -> Option<&mut T> {
        let entry = self.slots.get_mut((id % IPCMNI) as usize)?.as_mut()?;
        (entry.seq == id / IPCMNI).then_some(&mut entry.value)
    }

    /// The key of the object numbered `id`.
    pub fn key(&self, id: u32) -> Option<i32> {
        self.entry(id).map(|entry| entry.key)
    }

    /// The number of the object `key` finds: none for [`PRIVATE`].
    pub fn find(&self, key: i32) -> Option<u32> {
        if key == PRIVATE {
            return None;
        }
        self.slots.iter().enumerate().find_map(|(index, slot)| {
            slot.as_ref().filter(|entry| entry.key == key).map(|entry| Self::id(index, entry.seq))
        })
    }

    /// Puts `value` in with `key`, and returns its number; with an error,
    /// `value` back.
    pub fn insert(&mut self, key: i32, value: T) -> Result<u32, (TableError, T)> {
        if self.count >= self.max {
            return Err((TableError::Full, value));
        }
        let index = match self.slots.iter().position(Option::is_none) {
            Some(index) => index,
            None => {
                if self.slots.try_reserve(1).is_err() {
                    return Err((TableError::NoMemory, value));
                }
                // Room was made above.
                let _ = self.slots.push_within_capacity(None);
                self.slots.len() - 1
            }
        };
        let seq = self.seq;
        self.seq = if seq >= SEQ_MAX { 0 } else { seq + 1 };
        self.slots[index] = Some(Entry { key, seq, value });
        self.count += 1;
        Ok(Self::id(index, seq))
    }

    /// Takes out the object numbered `id`.
    pub fn remove(&mut self, id: u32) -> Option<T> {
        self.entry(id)?;
        let entry = self.slots[(id % IPCMNI) as usize].take()?;
        self.count -= 1;
        while self.slots.last().is_some_and(Option::is_none) {
            self.slots.pop();
        }
        Some(entry.value)
    }

    /// Leaves the object numbered `id` in, under no key: a segment
    /// removed while attached, which a `get` no longer finds.
    pub fn forget_key(&mut self, id: u32) {
        if let Some(entry) = self.slots.get_mut((id % IPCMNI) as usize).and_then(Option::as_mut)
            && entry.seq == id / IPCMNI
        {
            entry.key = PRIVATE;
        }
    }

    /// Every object, with its number and key, in order of place.
    pub fn iter(&self) -> impl Iterator<Item = (u32, i32, &T)> {
        self.slots.iter().enumerate().filter_map(|(index, slot)| {
            slot.as_ref().map(|entry| (Self::id(index, entry.seq), entry.key, &entry.value))
        })
    }
}

/// One semaphore operation, as Linux's `struct sembuf`: six bytes, the
/// semaphore's number, the operation and its flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Operation {
    /// The semaphore it works on.
    pub number: u16,
    /// What it adds (taking away if negative), or with 0 what it waits
    /// for: the value to be zero.
    pub op: i16,
    /// [`NOWAIT`] and [`UNDO`].
    pub flags: u16,
}

impl Operation {
    /// The size of one, in bytes.
    pub const SIZE: usize = 6;

    /// The operation in `bytes`, little-endian as on x86-64.
    pub fn from_bytes(bytes: [u8; Self::SIZE]) -> Operation {
        Operation {
            number: u16::from_le_bytes([bytes[0], bytes[1]]),
            op: i16::from_le_bytes([bytes[2], bytes[3]]),
            flags: u16::from_le_bytes([bytes[4], bytes[5]]),
        }
    }

    fn undo(self) -> bool {
        self.flags & UNDO != 0
    }
}

/// Why a list of operations is refused before any is tried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpError {
    /// None (EINVAL).
    Empty,
    /// More than [`SEMOPM`] (E2BIG).
    TooMany,
    /// One names a semaphore past the set's (EFBIG).
    BadNumber,
}

/// Checks `ops` for a set of `count` semaphores: whether they may run,
/// and whether any changes a value (needing write permission rather than
/// read) and any asks to be undone.
pub fn check(ops: &[Operation], count: usize) -> Result<Checked, OpError> {
    if ops.is_empty() {
        return Err(OpError::Empty);
    }
    if ops.len() > SEMOPM {
        return Err(OpError::TooMany);
    }
    if ops.iter().any(|op| op.number as usize >= count) {
        return Err(OpError::BadNumber);
    }
    Ok(Checked { alters: ops.iter().any(|op| op.op != 0), undoes: ops.iter().any(|op| op.undo()) })
}

/// What [`check`] found of a list of operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Checked {
    /// Whether one changes a value.
    pub alters: bool,
    /// Whether one is to be undone when the process ends.
    pub undoes: bool,
}

/// What [`operate`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Every operation is done.
    Done,
    /// None is: semaphore `number` must grow first, or with `zero` be
    /// zero, and the call waits until a change lets it try again.
    Wait {
        /// The semaphore the first operation that cannot go on works on.
        number: u16,
        /// Whether it waits for the value to be zero.
        zero: bool,
    },
    /// None is: the first that cannot go on has [`NOWAIT`] (EAGAIN).
    WouldBlock,
    /// None is: one would take a value past [`SEMVMX`], or an undo
    /// adjustment past [`SEMAEM`] (ERANGE).
    OutOfRange,
}

/// Does `ops`, which [`check`] passed, to `values`, all of them or none,
/// in order: an operation that adds goes on at once, one that takes away
/// only if the value stays at least zero, one of zero only if the value
/// is zero. Those with [`UNDO`] change the process's adjustments,
/// `adjust`, the other way, so that [`undo`] takes them back. On anything
/// but [`Outcome::Done`] both are as they were.
pub fn operate(values: &mut [u16], adjust: &mut [i32], ops: &[Operation]) -> Outcome {
    for (done, op) in ops.iter().enumerate() {
        let n = op.number as usize;
        let Some(&value) = values.get(n) else {
            roll_back(values, adjust, &ops[..done]);
            return Outcome::OutOfRange;
        };
        let value = i32::from(value);
        let result = value + i32::from(op.op);
        let blocked = if op.op == 0 { value != 0 } else { result < 0 };
        let undo_past = op.undo()
            && adjust
                .get(n)
                .is_none_or(|&a| !(-SEMAEM - 1..=SEMAEM).contains(&(a - i32::from(op.op))));
        let outcome = if blocked {
            Some(if op.flags & NOWAIT != 0 {
                Outcome::WouldBlock
            } else {
                Outcome::Wait { number: op.number, zero: op.op == 0 }
            })
        } else if result > SEMVMX || undo_past {
            Some(Outcome::OutOfRange)
        } else {
            None
        };
        if let Some(outcome) = outcome {
            roll_back(values, adjust, &ops[..done]);
            return outcome;
        }
        values[n] = result as u16;
        if op.undo() {
            adjust[n] -= i32::from(op.op);
        }
    }
    Outcome::Done
}

/// Takes back `done`, the operations [`operate`] did, last first.
fn roll_back(values: &mut [u16], adjust: &mut [i32], done: &[Operation]) {
    for op in done.iter().rev() {
        let n = op.number as usize;
        values[n] = (i32::from(values[n]) - i32::from(op.op)) as u16;
        if op.undo() {
            adjust[n] += i32::from(op.op);
        }
    }
}

/// What an ending process's adjustments `adjust` do to `values`: each is
/// added, and the value kept from 0 to [`SEMVMX`], as Linux's `exit_sem`.
pub fn undo(values: &mut [u16], adjust: &[i32]) {
    for (value, &a) in values.iter_mut().zip(adjust) {
        *value = (i32::from(*value) + a).clamp(0, SEMVMX) as u16;
    }
}

/// Whether `value` may be a semaphore's (SETVAL, SETALL): 0 to
/// [`SEMVMX`].
pub fn valid_value(value: i64) -> bool {
    (0..=i64::from(SEMVMX)).contains(&value)
}
