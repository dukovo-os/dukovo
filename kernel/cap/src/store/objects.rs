// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The object table: a record for each kernel object, with the counts that
//! say when it goes and what the kernel keeps of it.
//!
//! The records live in blocks that are made when first needed and never
//! move, so finding one by its index takes no lock, and each record has a
//! spin lock of its own, taken last of all the store's locks and only
//! around a few loads and stores. Opening and closing files in different
//! processes so touch different records and share no lock (step 3 of ADR
//! 0369).
//!
//! What the kernel keeps of each object (`K`: in the kernel, the object
//! itself and what it was charged) lives in its record too: it is put
//! there before the object's first capability is placed, so nobody finds
//! the capability without it, and taken out when the object goes, so the
//! kernel can close it.
//!
//! Free records are found by index. Each space keeps a few spare indexes
//! under its own lock ([`Spare`]), so operations make and release objects
//! without the table's own lock (O), which guards the indexes no space
//! keeps and the making of blocks; a space takes it only to fill or empty
//! its spares, a few at a time. An operation that holds O throughout
//! (the store's `with_all`) uses the spares and O's list both
//! ([`Indexes::Both`]).

use alloc::vec::Vec;
use core::alloc::Allocator;
use core::sync::atomic::{AtomicU32, Ordering};

use radix_sync::{Mutex, MutexGuard, Once, SpinLock, SpinLockGuard};

use super::{Extent, Released, bump};
use crate::cap::Capability;
use crate::error::CapError;
use crate::object::{ObjectId, ObjectKind};

/// Records in each block of the object table.
pub const OBJECT_BLOCK: usize = 512;

/// Blocks in the object table, so the most objects there can be at once
/// is `OBJECT_BLOCK * OBJECT_BLOCKS`, about a million.
pub const OBJECT_BLOCKS: usize = 2048;

/// Indexes a space keeps spare, and how many it takes from or gives back
/// to the table's at once.
const SPARE: usize = 8;
const BATCH: usize = SPARE / 2;

/// What the table knows about one object.
#[derive(Clone, Copy, Debug)]
pub(super) struct Entry {
    pub(super) kind: ObjectKind,
    /// Capabilities that name it.
    pub(super) caps: u32,
    /// Objects retyped from it that still exist.
    pub(super) children: u32,
    pub(super) extent: Option<Extent>,
    /// For memory: how many pages from its start are given to objects.
    pub(super) watermark: u64,
    /// The memory object it was retyped from.
    pub(super) origin: Option<ObjectId>,
}

impl Entry {
    /// A new object's entry, with its one capability: the original.
    pub(super) fn new(kind: ObjectKind, extent: Option<Extent>, origin: Option<ObjectId>) -> Entry {
        Entry { kind, caps: 1, children: 0, extent, watermark: 0, origin }
    }
}

/// One record: the object's generation, and the rest behind a lock of
/// its own.
pub(super) struct Record<K> {
    /// Moves on when the object goes, so its old name stays wrong. It
    /// changes only with the record locked, and is read with it locked,
    /// but for one reader: whoever takes a free record's index reads it
    /// without the lock ([`Objects::insert`]). Nothing changes it while
    /// the record is free, and the index came to that reader through a
    /// lock (its space's or the table's) taken after the last change.
    generation: AtomicU32,
    inner: SpinLock<Inner<K>>,
}

/// What a record's lock guards.
pub(super) struct Inner<K> {
    /// The object, while it lives.
    pub(super) entry: Option<Entry>,
    /// What the kernel keeps of it, if anything.
    pub(super) kept: Option<K>,
}

impl<K> Record<K> {
    /// Locks the record.
    #[track_caller]
    #[inline]
    pub(super) fn lock(&self) -> SpinLockGuard<'_, Inner<K>> {
        self.inner.lock()
    }

    /// Locks the record if no other CPU holds it.
    #[track_caller]
    pub(super) fn try_lock(&self) -> Option<SpinLockGuard<'_, Inner<K>>> {
        self.inner.try_lock()
    }

    /// The record's generation: read with the record locked, or by the
    /// holder of its free index.
    #[inline]
    pub(super) fn generation(&self) -> u32 {
        self.generation.load(Ordering::Relaxed)
    }

    /// Moves the generation on, with the record locked, as its object goes.
    #[inline]
    fn next_generation(&self) {
        self.generation.store(bump(self.generation()), Ordering::Relaxed);
    }

    /// Locks the record if it is of `generation`. Always inlined: it is
    /// on the path of every close, and as a call of its own it cost a
    /// fifth again what the lock does (saving registers, and handing the
    /// guard back through memory).
    #[track_caller]
    #[inline(always)]
    fn lock_if(&self, generation: u32) -> Option<SpinLockGuard<'_, Inner<K>>> {
        let inner = self.inner.lock();
        (self.generation() == generation).then_some(inner)
    }
}

/// The indexes no space keeps, and how many records were ever made: what
/// the table's own lock (O) guards.
pub(super) struct Free<A: Allocator> {
    /// Free indexes, last freed on top. Its room is reserved for every
    /// record made, so giving an index back never needs memory.
    pub(super) indexes: Vec<u32, A>,
    /// Records made so far, so the next new index.
    pub(super) made: u32,
}

/// A few free indexes a space keeps, under its own lock.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Spare {
    indexes: [u32; SPARE],
    len: u8,
}

impl Spare {
    /// The indexes it keeps.
    pub(super) fn indexes(&self) -> &[u32] {
        &self.indexes[..self.len as usize]
    }

    /// Takes the index put in last; there must be one.
    #[inline]
    fn pop(&mut self) -> u32 {
        self.len -= 1;
        self.indexes[self.len as usize]
    }

    /// Keeps `index`; there must be room.
    #[inline]
    fn push(&mut self, index: u32) {
        self.indexes[self.len as usize] = index;
        self.len += 1;
    }
}

/// Where an operation takes free indexes from and gives them back to.
pub(super) enum Indexes<'a, A: Allocator> {
    /// A space's spares, with its lock held: the table's lock is taken
    /// only to fill or empty them.
    Spare(&'a mut Spare),
    /// A space's spares, then the table's own when they run out or are
    /// full, with both locks held: for an operation that holds the
    /// table's lock already, which filling or emptying the spares would
    /// take again.
    Both(&'a mut Spare, &'a mut Free<A>),
}

/// One block of records, made once and never moved.
type Block<A, K> = Vec<Record<K>, A>;

/// The table: its blocks, and its free indexes behind its lock.
pub(super) struct Objects<A: Allocator, K> {
    alloc: A,
    blocks: Vec<Once<Block<A, K>>, A>,
    pub(super) free: Mutex<Free<A>>,
}

impl<A: Allocator + Clone, K> Objects<A, K> {
    /// An empty table. Fails if there is no memory for its list of blocks.
    pub(super) fn new(alloc: A) -> Result<Self, CapError> {
        let mut blocks = Vec::new_in(alloc.clone());
        blocks.try_reserve_exact(OBJECT_BLOCKS).map_err(|_| CapError::NoMemory)?;
        super::fill(&mut blocks, OBJECT_BLOCKS, Once::new);
        let free = Free { indexes: Vec::new_in(alloc.clone()), made: 0 };
        Ok(Objects { alloc, blocks, free: Mutex::new(free) })
    }

    /// The record at `index`, if its block was made.
    #[inline]
    pub(super) fn record(&self, index: u32) -> Option<&Record<K>> {
        let block = self.blocks.get(index as usize / OBJECT_BLOCK)?.get()?;
        block.get(index as usize % OBJECT_BLOCK)
    }

    /// How many blocks have been made: each stays once made, for a test
    /// that counts what the heap holds.
    pub(super) fn blocks_made(&self) -> usize {
        self.blocks.iter().filter(|block| block.get().is_some()).count()
    }

    /// Takes the table's lock.
    pub(super) fn lock(&self) -> MutexGuard<'_, Free<A>> {
        self.free.lock()
    }

    // ---- Free indexes ----

    /// A free index, from `from`.
    #[inline]
    pub(super) fn take(&self, from: &mut Indexes<'_, A>) -> Result<u32, CapError> {
        match from {
            Indexes::Spare(spare) => {
                if spare.len == 0 {
                    self.fill(spare)?;
                }
                Ok(spare.pop())
            }
            Indexes::Both(spare, free) => {
                if spare.len == 0 {
                    return self.take_from_table(free);
                }
                Ok(spare.pop())
            }
        }
    }

    /// Gives the free index `index` back to `to`.
    #[inline]
    pub(super) fn give(&self, index: u32, to: &mut Indexes<'_, A>) {
        match to {
            Indexes::Spare(spare) => {
                if spare.len as usize == SPARE {
                    self.empty(spare, BATCH);
                }
                spare.push(index);
            }
            Indexes::Both(spare, free) => {
                if spare.len as usize == SPARE {
                    return give_to_table(free, index);
                }
                spare.push(index);
            }
        }
    }

    /// Gives all of `spare`'s indexes back to the table: for a space that
    /// goes. The table's lock must not be held.
    pub(super) fn give_back(&self, spare: &mut Spare) {
        if spare.len > 0 {
            self.empty(spare, spare.len as usize);
        }
    }

    /// Takes up to [`BATCH`] indexes from the table into the empty `spare`,
    /// at least one.
    #[inline(never)]
    fn fill(&self, spare: &mut Spare) -> Result<(), CapError> {
        let mut free = self.free.lock();
        while (spare.len as usize) < BATCH {
            match self.take_from_table(&mut free) {
                Ok(index) => spare.push(index),
                // The first is enough.
                Err(_) if spare.len > 0 => break,
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }

    /// Gives `count` of `spare`'s indexes back to the table.
    #[inline(never)]
    fn empty(&self, spare: &mut Spare, count: usize) {
        let mut free = self.free.lock();
        for _ in 0..count {
            give_to_table(&mut free, spare.pop());
        }
    }

    /// A free index from the table's, or a new one, making its block if it
    /// is the first.
    fn take_from_table(&self, free: &mut Free<A>) -> Result<u32, CapError> {
        if let Some(index) = free.indexes.pop() {
            return Ok(index);
        }
        let index = free.made;
        if (index as usize).is_multiple_of(OBJECT_BLOCK) {
            self.make_block(free, index)?;
        }
        free.made += 1;
        Ok(index)
    }

    /// Makes the block that starts at `index`, with room in the free list
    /// for every record up to its end. Called with the table's lock held,
    /// which every new index needs. If it fails, nothing changes, and the
    /// next new index tries again.
    fn make_block(&self, free: &mut Free<A>, index: u32) -> Result<(), CapError> {
        let Some(cell) = self.blocks.get(index as usize / OBJECT_BLOCK) else {
            return Err(CapError::NoMemory);
        };
        let room = index as usize + OBJECT_BLOCK;
        free.indexes
            .try_reserve_exact(room.saturating_sub(free.indexes.len()))
            .map_err(|_| CapError::NoMemory)?;
        let mut block = Vec::new_in(self.alloc.clone());
        block.try_reserve_exact(OBJECT_BLOCK).map_err(|_| CapError::NoMemory)?;
        for _ in 0..OBJECT_BLOCK {
            // Every record's lock is made on this line: one class for the
            // lock-order checker.
            let inner = SpinLock::new(Inner { entry: None, kept: None });
            let _ = block.push_within_capacity(Record { generation: AtomicU32::new(1), inner });
        }
        // Only the table's lock holder makes blocks, so nobody else can
        // have set it meanwhile.
        let _ = cell.set(block);
        Ok(())
    }

    // ---- Objects ----

    /// Makes an object with `entry`, in an index from `from`, and runs
    /// `keep` with its name to fill in what the kernel keeps of it, before
    /// anyone can name it. If `keep` fails, the index goes back and nothing
    /// is made.
    pub(super) fn insert<T>(
        &self,
        from: &mut Indexes<'_, A>,
        entry: Entry,
        keep: impl FnOnce(ObjectId, &mut Option<K>) -> Result<T, CapError>,
    ) -> Result<(ObjectId, T), CapError> {
        let index = self.take(from)?;
        let Some(record) = self.record(index) else {
            // Cannot happen: an index is only handed out once its block is made.
            return Err(CapError::NoMemory);
        };
        // Nobody else has the index, and nobody has a name with the
        // record's generation: it moved when the last object went.
        let id = ObjectId { index, generation: record.generation() };
        let mut kept = None;
        match keep(id, &mut kept) {
            Ok(made) => {
                let mut inner = record.lock();
                inner.entry = Some(entry);
                inner.kept = kept;
                Ok((id, made))
            }
            Err(err) => {
                self.give(index, from);
                Err(err)
            }
        }
    }

    /// Takes back an object [`Objects::insert`] just made, whose original
    /// capability was never placed: nothing was told of it, so nothing is
    /// told of its going, and what the kernel kept of it is dropped.
    pub(super) fn unmake(&self, id: ObjectId, to: &mut Indexes<'_, A>) {
        let Some(record) = self.record(id.index) else { return };
        let kept = {
            let Some(mut inner) = record.lock_if(id.generation) else { return };
            if inner.entry.take().is_none() {
                return;
            }
            record.next_generation();
            inner.kept.take()
        };
        drop(kept);
        self.give(id.index, to);
    }

    /// Counts one more capability to the object `cap` names, if any.
    pub(super) fn retain(&self, cap: &Capability) {
        if let Some(id) = cap.id() {
            self.update(id, |entry| entry.caps += 1);
        }
    }

    /// Drops a capability's hold on the object `id`, and the object if
    /// that was the last hold, and so on up the memory it was retyped from.
    /// `released` hears of each object that goes, and is given what the
    /// kernel kept of it, with no record's lock held; then its index goes
    /// back to `to`.
    pub(super) fn release(
        &self,
        id: ObjectId,
        to: &mut Indexes<'_, A>,
        released: &mut dyn FnMut(Released, Option<K>),
    ) {
        let mut id = id;
        // A capability to the first object goes; each one above it loses
        // an object retyped from it.
        let mut capability = true;
        loop {
            let Some(record) = self.record(id.index) else { return };
            let (gone, kept) = {
                let Some(mut inner) = record.lock_if(id.generation) else { return };
                let Some(entry) = inner.entry.as_mut() else { return };
                if capability {
                    entry.caps -= 1;
                } else {
                    entry.children -= 1;
                    if entry.children == 0 {
                        entry.watermark = 0;
                    }
                }
                if entry.caps > 0 || entry.children > 0 {
                    return;
                }
                // Only these three are needed once it has gone.
                let gone = (entry.kind, entry.extent, entry.origin);
                inner.entry = None;
                record.next_generation();
                (gone, inner.kept.take())
            };
            let (kind, extent, origin) = gone;
            let memory = if origin.is_none() { extent } else { None };
            released(Released { kind, id, memory }, kept);
            self.give(id.index, to);
            let Some(origin) = origin else { return };
            id = origin;
            capability = false;
        }
    }

    /// The entry of the object `id`, if it lives.
    pub(super) fn entry(&self, id: ObjectId) -> Option<Entry> {
        self.record(id.index)?.lock_if(id.generation)?.entry
    }

    /// Runs `f` on the entry of the object `id`, if it lives, under its
    /// record's lock.
    pub(super) fn update<T>(&self, id: ObjectId, f: impl FnOnce(&mut Entry) -> T) -> Option<T> {
        self.record(id.index)?.lock_if(id.generation)?.entry.as_mut().map(f)
    }

    /// Runs `f` on what the kernel keeps of the object `id`, if it lives
    /// and the kernel keeps something, under its record's lock.
    pub(super) fn with_kept<T>(&self, id: ObjectId, f: impl FnOnce(&mut K) -> T) -> Option<T> {
        let mut inner = self.record(id.index)?.lock_if(id.generation)?;
        let Inner { entry: Some(_), kept } = &mut *inner else { return None };
        kept.as_mut().map(f)
    }

    /// Runs `f` on each living object's name, entry and what the kernel
    /// keeps of it, one record's lock at a time, up to the index `made`.
    pub(super) fn each(&self, made: u32, mut f: impl FnMut(ObjectId, &Entry, Option<&mut K>)) {
        for index in 0..made {
            let Some(record) = self.record(index) else { continue };
            let mut inner = record.lock();
            let id = ObjectId { index, generation: record.generation() };
            let Inner { entry, kept } = &mut *inner;
            if let Some(entry) = entry {
                f(id, entry, kept.as_mut());
            }
        }
    }

    /// Records made so far.
    pub(super) fn made(&self) -> u32 {
        self.free.lock().made
    }

    /// Whether `cap` names an object retyped from the memory `parent`
    /// names, directly or through smaller memory retyped from it, with no
    /// more of its rights. Deleting the capability to that smaller memory
    /// moves the objects retyped from it up to `parent`.
    pub(super) fn retyped_from(&self, cap: &Capability, parent: &Capability) -> bool {
        let (Some(id), Some(memory)) = (cap.id(), parent.id()) else { return false };
        if parent.kind() != ObjectKind::Memory
            || !cap.kind().from_memory()
            || !parent.rights.contains(cap.rights)
            || cap.badge != 0
        {
            return false;
        }
        // Every object on the way stays while something retyped from it
        // does; the bound only guards a corrupt store against a cycle.
        let mut origin = self.entry(id).and_then(|entry| entry.origin);
        for _ in 0..OBJECT_BLOCK * OBJECT_BLOCKS {
            match origin {
                Some(above) if above == memory => return true,
                Some(above) => origin = self.entry(above).and_then(|entry| entry.origin),
                None => return false,
            }
        }
        false
    }
}

/// Puts `index` back on the table's free list, in room reserved when its
/// block was made.
fn give_to_table<A: Allocator>(free: &mut Free<A>, index: u32) {
    let pushed = free.indexes.push_within_capacity(index);
    debug_assert!(pushed.is_ok(), "the free list has room for every record");
}
