// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! System V IPC (ADR 0490): the shared memory segments of `shmget` and
//! the semaphore sets of `semget`, which a memory filesystem keeps, and
//! radix-directory(2)'s `ipc_*` and `semaphore_*` methods on its root
//! reach. `radix_posix::ipc` decides what each call may do and what a
//! semaphore operation does; this module keeps the objects and waits.
//!
//! A segment's pages are a [`SharedPages`] charged to the filesystem's
//! room, as a memory file's are; `shmat` maps them as a shared mapping
//! whose region holds an [`Attachment`], and the segment counts the
//! regions that hold one (`shm_nattch`), as Linux counts its mappings.
//! A segment removed while attached stays, under no key, until its last
//! attachment goes.
//!
//! A semaphore operation that cannot go on waits for the set to change,
//! and tries again. One with SEM_UNDO keeps the opposite adjustment for
//! its process in the set, and the process's end makes them
//! ([`process_gone`]).
//!
//! Lock order: an address space's regions (whose dropped attachment can
//! remove its segment), then a table, then an object's state. A table's
//! lock is never held while regions are locked by its holder: `shmat`
//! takes its attachment with the table locked and maps it afterwards.

use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use radix_abi::Status;
use radix_abi::address_space::PAGE_SIZE;
use radix_abi::errors::{File as FileError, Kernel};
use radix_mm::{Flags, HEAP, Kmalloc};
use radix_posix::ipc::{
    self as rules, Caller, GetError, OpError, Operation, Outcome, Perm, Table, TableError,
};
use radix_sync::{Mutex, Once};

use crate::sched::{self, wait};
use crate::user::memory::{RoomRef, SharedPages, SharedRef};
use crate::user::posix::signal::{Restart, interrupted};

type Result<T> = core::result::Result<T, Status>;

fn kmalloc() -> Kmalloc {
    HEAP.with(Flags::MAY_SLEEP)
}

/// The most semaphores the sets of one filesystem hold together: each
/// takes 14 bytes of the kernel's memory, so about 14 MiB at most.
const SEMAPHORES_MAX: u64 = 1 << 20;

/// What kind of System V object a call names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A shared memory segment.
    Segment,
    /// A semaphore set.
    Semaphores,
}

/// What an object's status says: Linux's `shmid_ds` and `semid_ds`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IpcStatus {
    /// Its key, owner and permission bits.
    pub perm: Perm,
    /// Its number.
    pub id: u32,
    /// Whether it was removed while attached (SHM_DEST).
    pub removed: bool,
    /// A segment's size in bytes, or a set's count of semaphores.
    pub size: u64,
    /// How many regions attach a segment.
    pub attached: u64,
    /// The process that made a segment.
    pub creator_pid: u64,
    /// The last process to attach or detach a segment.
    pub last_pid: u64,
    /// When a segment was last attached, or a set's semaphores last
    /// operated on; 0 for never.
    pub used: u64,
    /// When a segment was last detached; 0 for never.
    pub detached: u64,
    /// When it was made or its status last changed.
    pub changed: u64,
}

/// One filesystem's System V objects.
pub struct Ipc {
    room: RoomRef,
    segments: Mutex<Table<SegmentRef, Kmalloc>>,
    sets: Mutex<Table<SetRef, Kmalloc>>,
    /// The semaphores its sets hold together.
    semaphores: AtomicU64,
    /// Itself, for the segments to find their table again.
    me: Once<Weak<Ipc, Kmalloc>>,
}

/// One filesystem's System V objects, shared.
pub type IpcRef = Arc<Ipc, Kmalloc>;

/// A shared memory segment.
pub struct Segment {
    pages: SharedRef,
    size: u64,
    /// How many regions hold an [`Attachment`] of it.
    attached: AtomicU64,
    state: Mutex<SegmentState>,
    ipc: Option<Weak<Ipc, Kmalloc>>,
}

/// A segment, shared.
pub type SegmentRef = Arc<Segment, Kmalloc>;

struct SegmentState {
    perm: Perm,
    id: u32,
    creator_pid: u64,
    last_pid: u64,
    attached_at: u64,
    detached_at: u64,
    changed: u64,
    /// Removed (IPC_RMID) while attached: it goes with its last
    /// attachment.
    removed: bool,
}

/// A segment's attachment, which the regions that map it hold: cloned
/// with a region, it counts once more, and dropped once less, and the
/// last to go of a removed segment takes it out of its table.
pub struct Attachment(SegmentRef);

impl Attachment {
    fn new(segment: SegmentRef) -> Attachment {
        segment.attached.fetch_add(1, Ordering::AcqRel);
        Attachment(segment)
    }

    /// The segment it attaches.
    pub fn segment(&self) -> &SegmentRef {
        &self.0
    }

    /// Whether it attaches `segment`.
    pub fn is(&self, segment: &SegmentRef) -> bool {
        Arc::ptr_eq(&self.0, segment)
    }
}

impl Clone for Attachment {
    fn clone(&self) -> Attachment {
        Attachment::new(self.0.clone())
    }
}

impl Drop for Attachment {
    fn drop(&mut self) {
        if self.0.attached.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.unattached();
        }
    }
}

impl Segment {
    /// How many pages it has.
    pub fn pages(&self) -> u64 {
        self.size.div_ceil(PAGE_SIZE)
    }

    /// Its pages, which an attachment maps.
    pub fn shared(&self) -> &SharedRef {
        &self.pages
    }

    /// Its last attachment went: a removed segment leaves its table. The
    /// count is looked at again with the table locked, as `attach` counts
    /// one more with it locked.
    fn unattached(&self) {
        let Some(ipc) = self.ipc.as_ref().and_then(Weak::upgrade) else { return };
        let mut table = ipc.segments.lock();
        let state = self.state.lock();
        if !state.removed || self.attached.load(Ordering::Acquire) != 0 {
            return;
        }
        let id = state.id;
        drop(state);
        if table.get(id).is_some_and(|found| core::ptr::eq(&raw const **found, self)) {
            // What the table held goes with its lock let go.
            let gone = table.remove(id);
            drop(table);
            drop(gone);
        }
    }

    /// Notes that process `pid` detached it now (`shmdt`).
    pub fn detached(&self, pid: u64) {
        let mut state = self.state.lock();
        state.last_pid = pid;
        state.detached_at = radix_vfs::now();
    }
}

/// A semaphore set.
pub struct SemSet {
    count: usize,
    state: Mutex<SetState>,
    /// Moves with every change of its values, or its removal: what a
    /// waiting operation waits on.
    changes: AtomicU64,
}

type SetRef = Arc<SemSet, Kmalloc>;

struct SetState {
    perm: Perm,
    id: u32,
    removed: bool,
    values: Vec<u16, Kmalloc>,
    /// The process that last changed each.
    pids: Vec<u64, Kmalloc>,
    /// How many wait for each to grow, and for each to be zero.
    waiting: Vec<u32, Kmalloc>,
    zero_waiting: Vec<u32, Kmalloc>,
    operated: u64,
    changed: u64,
    /// Each process's undo adjustments, by its number.
    undos: Vec<(u64, Vec<i32, Kmalloc>), Kmalloc>,
}

impl SemSet {
    /// The key its waiting operations wait on.
    fn key(&self) -> usize {
        core::ptr::from_ref(&self.changes) as usize
    }

    /// Its values changed, or it went: the operations waiting try again.
    fn changed(&self) {
        self.changes.fetch_add(1, Ordering::Release);
        wait::wake_all(self.key());
    }
}

impl SetState {
    fn undo_of(&mut self, process: u64) -> Option<&mut Vec<i32, Kmalloc>> {
        self.undos.iter_mut().find(|(p, _)| *p == process).map(|(_, adjust)| adjust)
    }
}

/// A vector of `len` copies of `value`, or `None` for want of memory.
fn filled<T: Copy>(len: usize, value: T) -> Option<Vec<T, Kmalloc>> {
    let mut v = Vec::new_in(kmalloc());
    v.try_reserve_exact(len).ok()?;
    for _ in 0..len {
        // Room was reserved above.
        let _ = v.push_within_capacity(value);
    }
    Some(v)
}

fn no_memory() -> Status {
    Kernel::NoMemory.status(0)
}

fn invalid() -> Status {
    Kernel::InvalidArgument.status(0)
}

fn denied() -> Status {
    FileError::Denied.status(0)
}

fn get_error(err: GetError) -> Status {
    match err {
        GetError::NotFound => FileError::NotFound.status(0),
        GetError::Exists => FileError::Exists.status(0),
    }
}

fn table_error(err: TableError) -> Status {
    match err {
        TableError::Full => FileError::NoSpace.status(0),
        TableError::NoMemory => no_memory(),
    }
}

impl Ipc {
    /// No objects yet; segments' pages are charged to `room`.
    pub fn new(room: RoomRef) -> Result<IpcRef> {
        let ipc = Ipc {
            room,
            segments: Mutex::new(Table::new(rules::SHMMNI, kmalloc())),
            sets: Mutex::new(Table::new(rules::SEMMNI, kmalloc())),
            semaphores: AtomicU64::new(0),
            me: Once::new(),
        };
        let ipc = Arc::try_new_in(ipc, kmalloc()).map_err(|_| no_memory())?;
        let _ = ipc.me.set(Arc::downgrade(&ipc));
        Ok(ipc)
    }

    /// `shmget`: the number of the segment `key` finds, or of one made
    /// for it of `size` bytes, by `caller` in process `pid`.
    #[allow(clippy::too_many_arguments)]
    pub fn get_segment(
        &self,
        caller: &Caller,
        pid: u64,
        key: i32,
        size: u64,
        create: bool,
        exclusive: bool,
        mode: u32,
    ) -> Result<u32> {
        let mut table = self.segments.lock();
        if let Some(id) =
            rules::decide(table.find(key), key, create, exclusive).map_err(get_error)?
        {
            let segment = table.get(id).ok_or(invalid())?;
            if !segment.state.lock().perm.permits(caller, rules::asked(mode)) {
                return Err(denied());
            }
            if size > segment.size {
                return Err(invalid());
            }
            return Ok(id);
        }
        let pages = size.div_ceil(PAGE_SIZE);
        if size == 0 || pages > self.room.max() {
            return Err(invalid());
        }
        let shared = SharedPages::in_room(self.room.clone(), pages)?;
        let now = radix_vfs::now();
        let state = SegmentState {
            perm: Perm::new(key, caller, mode),
            id: 0,
            creator_pid: pid,
            last_pid: 0,
            attached_at: 0,
            detached_at: 0,
            changed: now,
            removed: false,
        };
        let ipc = self.me.get().cloned();
        let segment = Segment {
            pages: shared,
            size,
            attached: AtomicU64::new(0),
            state: Mutex::new(state),
            ipc,
        };
        let segment = Arc::try_new_in(segment, kmalloc()).map_err(|_| no_memory())?;
        let id = table.insert(key, segment.clone()).map_err(|(err, _)| table_error(err))?;
        segment.state.lock().id = id;
        Ok(id)
    }

    /// `semget`: the number of the set `key` finds, or of one made for it
    /// of `count` semaphores, all zero.
    #[allow(clippy::too_many_arguments)]
    pub fn get_set(
        &self,
        caller: &Caller,
        key: i32,
        count: u64,
        create: bool,
        exclusive: bool,
        mode: u32,
    ) -> Result<u32> {
        if count > u64::from(rules::SEMMSL) {
            return Err(invalid());
        }
        let mut table = self.sets.lock();
        if let Some(id) =
            rules::decide(table.find(key), key, create, exclusive).map_err(get_error)?
        {
            let set = table.get(id).ok_or(invalid())?;
            if !set.state.lock().perm.permits(caller, rules::asked(mode)) {
                return Err(denied());
            }
            if count as usize > set.count {
                return Err(invalid());
            }
            return Ok(id);
        }
        if count == 0 {
            return Err(invalid());
        }
        let total = self.semaphores.load(Ordering::Relaxed);
        if total + count > SEMAPHORES_MAX {
            return Err(FileError::NoSpace.status(0));
        }
        let len = count as usize;
        let (Some(values), Some(pids), Some(waiting), Some(zero_waiting)) =
            (filled(len, 0u16), filled(len, 0u64), filled(len, 0u32), filled(len, 0u32))
        else {
            return Err(no_memory());
        };
        let state = SetState {
            perm: Perm::new(key, caller, mode),
            id: 0,
            removed: false,
            values,
            pids,
            waiting,
            zero_waiting,
            operated: 0,
            changed: radix_vfs::now(),
            undos: Vec::new_in(kmalloc()),
        };
        let set = SemSet { count: len, state: Mutex::new(state), changes: AtomicU64::new(0) };
        let set = Arc::try_new_in(set, kmalloc()).map_err(|_| no_memory())?;
        let id = table.insert(key, set.clone()).map_err(|(err, _)| table_error(err))?;
        set.state.lock().id = id;
        self.semaphores.fetch_add(count, Ordering::Relaxed);
        Ok(id)
    }

    fn segment(&self, id: u32) -> Result<SegmentRef> {
        self.segments.lock().get(id).cloned().ok_or(invalid())
    }

    fn set(&self, id: u32) -> Result<SetRef> {
        self.sets.lock().get(id).cloned().ok_or(invalid())
    }

    /// IPC_STAT, which needs read permission.
    pub fn status(&self, caller: &Caller, kind: Kind, id: u32) -> Result<IpcStatus> {
        match kind {
            Kind::Segment => {
                let segment = self.segment(id)?;
                let state = segment.state.lock();
                if !state.perm.permits(caller, rules::want::READ) {
                    return Err(denied());
                }
                Ok(IpcStatus {
                    perm: state.perm,
                    id,
                    removed: state.removed,
                    size: segment.size,
                    attached: segment.attached.load(Ordering::Acquire),
                    creator_pid: state.creator_pid,
                    last_pid: state.last_pid,
                    used: state.attached_at,
                    detached: state.detached_at,
                    changed: state.changed,
                })
            }
            Kind::Semaphores => {
                let set = self.set(id)?;
                let state = set.state.lock();
                if !state.perm.permits(caller, rules::want::READ) {
                    return Err(denied());
                }
                Ok(IpcStatus {
                    perm: state.perm,
                    id,
                    removed: false,
                    size: set.count as u64,
                    attached: 0,
                    creator_pid: 0,
                    last_pid: 0,
                    used: state.operated,
                    detached: 0,
                    changed: state.changed,
                })
            }
        }
    }

    /// IPC_SET: gives the object to `owner` and `group` with the
    /// permission bits of `mode`; only its owner or maker may.
    pub fn set_owner(
        &self,
        caller: &Caller,
        kind: Kind,
        id: u32,
        owner: u32,
        group: u32,
        mode: u32,
    ) -> Result<()> {
        let now = radix_vfs::now();
        match kind {
            Kind::Segment => {
                let segment = self.segment(id)?;
                let mut state = segment.state.lock();
                if !state.perm.controls(caller) {
                    return Err(Kernel::NotPermitted.status(0));
                }
                state.perm.set(owner, group, mode);
                state.changed = now;
            }
            Kind::Semaphores => {
                let set = self.set(id)?;
                let mut state = set.state.lock();
                if !state.perm.controls(caller) {
                    return Err(Kernel::NotPermitted.status(0));
                }
                state.perm.set(owner, group, mode);
                state.changed = now;
            }
        }
        Ok(())
    }

    /// IPC_RMID: a set goes at once, and its waiting operations fail with
    /// removed (EIDRM); a segment goes with its last attachment, and a
    /// `get` no longer finds it meanwhile.
    pub fn remove(&self, caller: &Caller, kind: Kind, id: u32) -> Result<()> {
        match kind {
            Kind::Segment => {
                let mut table = self.segments.lock();
                let segment = table.get(id).cloned().ok_or(invalid())?;
                let mut state = segment.state.lock();
                if !state.perm.controls(caller) {
                    return Err(Kernel::NotPermitted.status(0));
                }
                state.removed = true;
                state.perm.key = rules::PRIVATE;
                state.changed = radix_vfs::now();
                drop(state);
                // Counted with the table locked, as `attach` counts.
                let gone = if segment.attached.load(Ordering::Acquire) == 0 {
                    table.remove(id)
                } else {
                    table.forget_key(id);
                    None
                };
                drop(table);
                drop(gone);
            }
            Kind::Semaphores => {
                let mut table = self.sets.lock();
                let set = table.get(id).cloned().ok_or(invalid())?;
                let mut state = set.state.lock();
                if !state.perm.controls(caller) {
                    return Err(Kernel::NotPermitted.status(0));
                }
                state.removed = true;
                // Its undo adjustments go with it.
                state.undos.clear();
                drop(state);
                table.remove(id);
                drop(table);
                self.semaphores.fetch_sub(set.count as u64, Ordering::Relaxed);
                set.changed();
            }
        }
        Ok(())
    }

    /// `shmat`'s first half: an attachment of segment `id` by process
    /// `pid`, to read, and to write too if `write`, for the caller to map.
    pub fn attach(&self, caller: &Caller, pid: u64, id: u32, write: bool) -> Result<Attachment> {
        let table = self.segments.lock();
        let segment = table.get(id).cloned().ok_or(invalid())?;
        let mut state = segment.state.lock();
        let want = rules::want::READ | if write { rules::want::WRITE } else { 0 };
        if !state.perm.permits(caller, want) {
            return Err(denied());
        }
        state.last_pid = pid;
        state.attached_at = radix_vfs::now();
        drop(state);
        // Counted with the table locked, so a removal sees it.
        let attachment = Attachment::new(segment);
        drop(table);
        Ok(attachment)
    }

    /// `semop` and `semtimedop`: does `ops` on set `id` for `caller` in
    /// process `pid`, all of them or none, waiting until they can go on,
    /// or until `deadline` (on the kernel's clock) if one is given.
    /// `note_undo` is called before a first adjustment of the process is
    /// kept, so that its end makes them.
    pub fn operate(
        &self,
        caller: &Caller,
        pid: u64,
        id: u32,
        ops: &[Operation],
        deadline: Option<u64>,
        note_undo: impl Fn(),
    ) -> Result<()> {
        let set = self.set(id)?;
        let checked = rules::check(ops, set.count).map_err(|err| match err {
            OpError::Empty => invalid(),
            OpError::TooMany => Kernel::ArgumentsTooLong.status(0),
            OpError::BadNumber => FileError::TooBig.status(0),
        })?;
        let want = if checked.alters { rules::want::WRITE } else { rules::want::READ };
        if checked.undoes {
            undo_record(&set, pid, &note_undo)?;
        }
        loop {
            let seen = set.changes.load(Ordering::Acquire);
            let waits = {
                let mut state = set.state.lock();
                if state.removed {
                    return Err(FileError::Removed.status(0));
                }
                if !state.perm.permits(caller, want) {
                    return Err(denied());
                }
                let SetState { values, undos, .. } = &mut *state;
                let adjust = match undos.iter_mut().find(|(p, _)| *p == pid) {
                    Some((_, adjust)) => &mut adjust[..],
                    None => &mut [],
                };
                match rules::operate(&mut values[..], adjust, ops) {
                    Outcome::Done => {
                        for op in ops {
                            state.pids[op.number as usize] = pid;
                        }
                        state.operated = radix_vfs::now();
                        drop(state);
                        set.changed();
                        return Ok(());
                    }
                    Outcome::WouldBlock => return Err(Kernel::WouldBlock.status(0)),
                    Outcome::OutOfRange => return Err(Kernel::OutOfRange.status(0)),
                    Outcome::Wait { number, zero } => {
                        let counts =
                            if zero { &mut state.zero_waiting } else { &mut state.waiting };
                        counts[number as usize] += 1;
                        (number as usize, zero)
                    }
                }
            };
            let changed = || set.changes.load(Ordering::Acquire) != seen || sched::interrupted();
            let woken = match deadline {
                Some(deadline) => {
                    wait::wait_until_or_timeout(set.key(), "semaphore", deadline, changed)
                }
                None => {
                    wait::wait_until(set.key(), "semaphore", changed);
                    true
                }
            };
            {
                let mut state = set.state.lock();
                let (number, zero) = waits;
                let counts = if zero { &mut state.zero_waiting } else { &mut state.waiting };
                counts[number] = counts[number].saturating_sub(1);
            }
            if sched::interrupted() {
                return Err(interrupted(Restart::NoHandler));
            }
            if !woken {
                // semtimedop's time ran out: EAGAIN, as on Linux.
                return Err(Kernel::WouldBlock.status(0));
            }
        }
    }

    /// `semctl`'s GETVAL, GETPID, GETNCNT and GETZCNT (read permission)
    /// and SETVAL (write permission, by process `pid`), on semaphore
    /// `number` of set `id`.
    pub fn control(
        &self,
        caller: &Caller,
        pid: u64,
        id: u32,
        number: u32,
        command: Command,
    ) -> Result<u64> {
        let set = self.set(id)?;
        let mut state = set.state.lock();
        let want = match command {
            Command::SetValue(_) => rules::want::WRITE,
            _ => rules::want::READ,
        };
        if !state.perm.permits(caller, want) {
            return Err(denied());
        }
        let n = number as usize;
        if n >= set.count {
            return Err(invalid());
        }
        Ok(match command {
            Command::Value => u64::from(state.values[n]),
            Command::Pid => state.pids[n],
            Command::Waiting => u64::from(state.waiting[n]),
            Command::ZeroWaiting => u64::from(state.zero_waiting[n]),
            Command::SetValue(value) => {
                if !rules::valid_value(i64::from(value)) {
                    return Err(Kernel::OutOfRange.status(0));
                }
                state.values[n] = value as u16;
                state.pids[n] = pid;
                for (_, adjust) in &mut state.undos {
                    adjust[n] = 0;
                }
                state.changed = radix_vfs::now();
                drop(state);
                set.changed();
                0
            }
        })
    }

    /// GETALL: the values of set `id`, into `out`; returns how many it
    /// has, which may be more than `out` holds.
    pub fn values(&self, caller: &Caller, id: u32, out: &mut [u16]) -> Result<usize> {
        let set = self.set(id)?;
        let state = set.state.lock();
        if !state.perm.permits(caller, rules::want::READ) {
            return Err(denied());
        }
        let n = out.len().min(set.count);
        out[..n].copy_from_slice(&state.values[..n]);
        Ok(set.count)
    }

    /// The count of semaphores of set `id`, for a caller about to give
    /// SETALL that many values.
    pub fn count(&self, id: u32) -> Result<usize> {
        Ok(self.set(id)?.count)
    }

    /// SETALL: gives set `id` `values`, one for each semaphore, by
    /// process `pid`.
    pub fn set_values(&self, caller: &Caller, pid: u64, id: u32, values: &[u16]) -> Result<()> {
        let set = self.set(id)?;
        if values.len() != set.count {
            return Err(invalid());
        }
        if values.iter().any(|&v| !rules::valid_value(i64::from(v))) {
            return Err(Kernel::OutOfRange.status(0));
        }
        let mut state = set.state.lock();
        if !state.perm.permits(caller, rules::want::WRITE) {
            return Err(denied());
        }
        state.values.copy_from_slice(values);
        state.pids.iter_mut().for_each(|p| *p = pid);
        for (_, adjust) in &mut state.undos {
            adjust.iter_mut().for_each(|a| *a = 0);
        }
        state.changed = radix_vfs::now();
        drop(state);
        set.changed();
        Ok(())
    }
}

/// What `semctl` asks of one semaphore ([`Ipc::control`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// GETVAL.
    Value,
    /// GETPID.
    Pid,
    /// GETNCNT.
    Waiting,
    /// GETZCNT.
    ZeroWaiting,
    /// SETVAL.
    SetValue(u32),
}

/// Each process that keeps undo adjustments in a set, by its number, and
/// the set.
type Undos = Vec<(u64, Weak<SemSet, Kmalloc>), Kmalloc>;

/// The sets processes keep undo adjustments in, by process number: what
/// [`process_gone`] looks through. A set removed since is passed over.
static UNDOS: Mutex<Option<Undos>> = Mutex::new(None);

/// Makes sure process `pid` has undo adjustments in `set`, all zero at
/// first, and that its end makes them.
fn undo_record(set: &SetRef, pid: u64, note_undo: &impl Fn()) -> Result<()> {
    if set.state.lock().undo_of(pid).is_some() {
        return Ok(());
    }
    // Made with no lock held, then put in if no other thread of the
    // process made one meanwhile.
    let adjust = filled(set.count, 0i32).ok_or(no_memory())?;
    {
        let mut undos = UNDOS.lock();
        let undos = undos.get_or_insert_with(|| Vec::new_in(kmalloc()));
        undos.retain(|(_, set)| set.strong_count() > 0);
        undos.try_reserve(1).map_err(|_| no_memory())?;
        let _ = undos.push_within_capacity((pid, Arc::downgrade(set)));
    }
    note_undo();
    let mut state = set.state.lock();
    if state.undo_of(pid).is_none() {
        state.undos.try_reserve(1).map_err(|_| no_memory())?;
        let _ = state.undos.push_within_capacity((pid, adjust));
    }
    Ok(())
}

/// Process `process` ended: each set it kept undo adjustments in takes
/// them, as Linux's `exit_sem`, and its waiting operations try again.
pub fn process_gone(process: u64) {
    loop {
        let set = {
            let mut undos = UNDOS.lock();
            let Some(undos) = undos.as_mut() else { return };
            let Some(at) = undos.iter().position(|(p, _)| *p == process) else { return };
            undos.swap_remove(at).1
        };
        let Some(set) = set.upgrade() else { continue };
        let mut state = set.state.lock();
        let Some(at) = state.undos.iter().position(|(p, _)| *p == process) else { continue };
        let (_, adjust) = state.undos.swap_remove(at);
        if state.removed {
            continue;
        }
        rules::undo(&mut state.values, &adjust);
        for (pid, _) in state.pids.iter_mut().zip(adjust.iter()).filter(|(_, a)| **a != 0) {
            *pid = process;
        }
        state.operated = radix_vfs::now();
        drop(state);
        set.changed();
    }
}
