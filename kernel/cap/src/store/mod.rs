// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The capability store: capability spaces, the object table and the
//! derivation tree.
//!
//! A capability space (CSpace) is an array of slots; a process will name a
//! capability by its slot number and nothing else. Every capability in
//! every space is a node of one derivation tree: a capability made from
//! another is that one's child, wherever the two are kept. Each slot has
//! its links (parent, first child, previous and next sibling), so the tree
//! costs nothing to walk and needs no memory beyond the slots.
//!
//! - Deleting a capability makes its children children of its parent, so
//!   whoever could revoke the deleted one can still revoke them.
//! - Revoking a capability deletes everything derived from it, in every
//!   space, and keeps the capability itself. It works in turns of a given
//!   size, letting go of every lock in between; while a revoke is under
//!   way, nothing can be derived from the capability or from anything
//!   below it, so the work only shrinks and always ends.
//! - Moving a capability keeps its place in the tree.
//!
//! Kernel objects (every kind but the hardware ranges: interrupt lines,
//! I/O ports and device registers) have a record in the object table
//! that counts the capabilities naming them and the objects retyped from
//! them, beside what the kernel keeps of them (`K`). When both counts
//! reach zero the object is gone, and the store tells the caller through
//! a [`Released`], with what the kernel kept, so the kernel can close the
//! object and give its memory back.
//!
//! # Locks (ADR 0369)
//!
//! Each space has its own lock (S), over what its slots hold; one lock
//! (T) guards each linked slot's place in the tree, which crosses spaces;
//! the object table has a lock of its own (O), over its free indexes; and
//! each object's record has its own spin lock, over its counts and what
//! the kernel keeps of it. They are taken in that order, spaces by
//! increasing index, and a thread holding a space's lock never waits for
//! the tree's. Each space keeps a few free object indexes, so the fast
//! path takes O only now and then, to fill or empty them.
//!
//! - The fast path ([`Capabilities::in_space`], [`SpaceView`]) takes one
//!   space's lock: looking capabilities up, slot flags, making an object,
//!   and deleting or moving a capability whose slot is not linked. It
//!   cannot reach the tree at all. Finding the slot linked, it answers
//!   [`Fast::Linked`], and the caller lets go and asks the slow path.
//! - The slow path ([`Capabilities::with_tree`], [`TreeView`]) takes the
//!   tree lock and the locks of the spaces it names: deriving, sealing,
//!   retyping, and deleting or moving a linked capability.
//! - A revoke ([`Capabilities::revoke`]) holds the tree lock through each
//!   turn and one space's lock at a time.
//!
//! `kernel/cap/tla/CapabilityLocks.tla` checks that these locks keep every
//! invariant of the tree. Operations that delete capabilities report the
//! objects that went to a `released` callback, which runs with the
//! operation's space and tree locks held, but no record's, and must not
//! call back into the store.

mod audit;
mod fast;
mod objects;
#[cfg(kani)]
mod proofs;
mod slots;
mod slow;

use alloc::vec::Vec;
use core::alloc::Allocator;
use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};

use radix_sync::{Mutex, MutexGuard, Once};

use crate::cap::{Capability, Derivation};
use crate::error::CapError;
use crate::object::{KINDS, ObjectId, ObjectKind};
use crate::rights::Rights;

pub use fast::{Fast, SpaceView};
pub use slow::TreeView;

pub use objects::{OBJECT_BLOCK, OBJECT_BLOCKS};
use objects::{Objects, Spare};
use slots::{Forest, Slot, SpaceState, Tree};

/// The longest chain of derivations: a capability is at most this many
/// steps from one the kernel made. It bounds every walk up the tree.
pub const MAX_DEPTH: u8 = 64;

/// The most slots a capability space can have.
pub const MAX_SLOTS: u32 = 1 << 16;

/// The most objects one retype can make.
pub const MAX_RETYPE: u32 = 1024;

/// The bytes each slot of a capability space takes, for charging spaces
/// to whoever asked for them.
pub const SLOT_BYTES: usize = core::mem::size_of::<Slot>() + core::mem::size_of::<Tree>();

/// The name of a capability space: its index in the store and that
/// index's generation, so a name stays wrong once the space is gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SpaceId {
    /// Index in the store.
    pub index: u32,
    /// Generation of the index when the space was made.
    pub generation: u32,
}

impl fmt::Display for SpaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "space {}.{}", self.index, self.generation)
    }
}

/// A slot in a capability space.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SlotRef {
    /// The space.
    pub space: SpaceId,
    /// The slot's number in it.
    pub slot: u32,
}

impl SlotRef {
    /// Slot `slot` of `space`.
    pub const fn new(space: SpaceId, slot: u32) -> SlotRef {
        SlotRef { space, slot }
    }
}

impl fmt::Display for SlotRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} slot {}", self.space, self.slot)
    }
}

/// Whether a revoke finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// Nothing derived from the capability is left.
    Done,
    /// The step's budget ran out; call again to go on.
    More,
}

/// A range of page frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extent {
    /// The first page frame number.
    pub base: u64,
    /// How many pages.
    pub pages: u64,
}

/// A kernel object whose last capability is gone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Released {
    /// Its kind.
    pub kind: ObjectKind,
    /// Its name, which no longer refers to anything.
    pub id: ObjectId,
    /// For memory the kernel gave the store ([`Capabilities::create_memory`]),
    /// the pages to take back. Objects retyped from memory return their
    /// pages to that memory instead.
    pub memory: Option<Extent>,
}

/// What the store knows about a kernel object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectInfo {
    /// Its kind.
    pub kind: ObjectKind,
    /// Capabilities that name it.
    pub caps: u32,
    /// Objects retyped from it that still exist.
    pub children: u32,
    /// The memory it covers, for memory, frames and DMA buffers.
    pub extent: Option<Extent>,
    /// For memory: how many pages from its start are given to objects.
    pub watermark: u64,
    /// The memory object it was retyped from.
    pub origin: Option<ObjectId>,
}

/// Counts of what the store holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Census {
    /// Capability spaces.
    pub spaces: u32,
    /// Slots in all spaces.
    pub slots: u64,
    /// Capabilities, by the kind of object they designate.
    pub caps: [u64; KINDS],
    /// Kernel objects, by kind.
    pub objects: [u64; KINDS],
    /// Pages of memory the kernel gave the store.
    pub memory_pages: u64,
    /// The deepest capability's depth.
    pub deepest: u8,
    /// Revokes under way.
    pub revoking: u32,
}

impl Census {
    /// All capabilities.
    pub fn total_caps(&self) -> u64 {
        self.caps.iter().sum()
    }

    /// All kernel objects.
    pub fn total_objects(&self) -> u64 {
        self.objects.iter().sum()
    }
}

/// A broken invariant that [`Capabilities::audit`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inconsistency {
    /// What is wrong.
    pub what: &'static str,
    /// Where, if it is about a slot.
    pub at: Option<SlotRef>,
}

impl fmt::Display for Inconsistency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.at {
            Some(at) => write!(f, "{} ({at})", self.what),
            None => f.write_str(self.what),
        }
    }
}

/// Spaces in each block of the space table.
const BLOCK: usize = 256;

/// Blocks in the space table, so the most spaces there can be is
/// `BLOCK * BLOCKS`.
const BLOCKS: usize = 1024;

/// One capability space's lock and what it guards, on cache lines of its
/// own: the spaces of a block sit side by side, and processes on other
/// CPUs, each taking its own space's lock, would otherwise pass the lines
/// they share between them on every operation.
#[repr(align(64))]
struct Space<A: Allocator> {
    state: Mutex<SpaceState<A>>,
}

/// Every capability space, the object table and the derivation tree, each
/// behind its own lock (see the module's documentation).
///
/// Operations that delete capabilities report the objects that went with
/// them to a `released` callback, with what the kernel kept of each (`K`).
/// It runs with the locks of the spaces the operation changed held, but no
/// object record's, and must not call back into the store.
pub struct Capabilities<A: Allocator + Clone, K = ()> {
    alloc: A,
    /// The tree lock (T), over every slot's place in the tree and the
    /// list of free space indexes.
    tree: Mutex<Forest<A>>,
    /// The spaces, by index, in blocks made when a space first needs them
    /// and never moved, so finding a space's lock takes no lock.
    blocks: Vec<Once<Vec<Space<A>, A>>, A>,
    /// The object table: its records, each behind its own lock, and its
    /// free indexes behind the table's (O).
    objects: Objects<A, K>,
    /// Moves on whenever a capability looked up to be cached leaves its
    /// slot, before the lock that guarded the slot goes.
    version: AtomicU64,
}

/// The next generation number, skipping 0.
fn bump(generation: u32) -> u32 {
    generation.wrapping_add(1).max(1)
}

/// Makes `vec` `len` long with values from `value`, in room reserved
/// already: the kernel builds `alloc` without the infallible methods that
/// would do this (ADR 0017).
fn fill<T, A: Allocator>(vec: &mut Vec<T, A>, len: usize, mut value: impl FnMut() -> T) {
    debug_assert!(vec.capacity() >= len, "room is reserved before a fill");
    while vec.len() < len {
        if vec.push_within_capacity(value()).is_err() {
            break;
        }
    }
}

impl<A: Allocator + Clone, K> Capabilities<A, K> {
    /// An empty store whose memory comes from `alloc`. Each object's
    /// record keeps a `K` beside it for the kernel: what the kernel keeps
    /// of the object, put there when the object is made and handed back
    /// when it goes. Fails if there is no memory for the store's tables of
    /// space and object blocks, a few tens of KiB.
    pub fn new(alloc: A) -> Result<Self, CapError> {
        let mut blocks = Vec::new_in(alloc.clone());
        blocks.try_reserve_exact(BLOCKS).map_err(|_| CapError::NoMemory)?;
        fill(&mut blocks, BLOCKS, Once::new);
        Ok(Capabilities {
            tree: Mutex::new(Forest { spaces: Vec::new_in(alloc.clone()), free: None }),
            blocks,
            objects: Objects::new(alloc.clone())?,
            version: AtomicU64::new(0),
            alloc,
        })
    }

    // ---- The locks ----

    /// The space with index `index`, if its block was made.
    fn space(&self, index: u32) -> Option<&Space<A>> {
        let block = self.blocks.get(index as usize / BLOCK)?.get()?;
        block.get(index as usize % BLOCK)
    }

    /// Makes the block that holds index `index`, if it is not there yet.
    /// Called with the tree lock held, which every new index needs.
    fn make_room_for(&self, index: u32) -> Result<(), CapError> {
        let Some(cell) = self.blocks.get(index as usize / BLOCK) else {
            return Err(CapError::NoMemory);
        };
        if cell.get().is_some() {
            return Ok(());
        }
        let mut block = Vec::new_in(self.alloc.clone());
        block.try_reserve_exact(BLOCK).map_err(|_| CapError::NoMemory)?;
        for _ in 0..BLOCK {
            // Every space's lock is made on this line: one class for the
            // lock-order checker.
            let state = SpaceState {
                generation: 1,
                live: false,
                slots: Vec::new_in(self.alloc.clone()),
                spare: Spare::default(),
            };
            let _ = block.push_within_capacity(Space { state: Mutex::new(state) });
        }
        // Only the tree lock's holder makes blocks, so nobody else can
        // have set it meanwhile.
        let _ = cell.set(block);
        Ok(())
    }

    /// How many blocks of the space table have been made: each holds
    /// [`BLOCK`] spaces' locks and stays once made, for a test that counts
    /// what the heap holds.
    pub fn space_blocks(&self) -> usize {
        self.blocks.iter().filter(|block| block.get().is_some()).count()
    }

    /// How many blocks of the object table have been made: each holds
    /// [`OBJECT_BLOCK`] records and stays once made, as the space table's.
    pub fn object_blocks(&self) -> usize {
        self.objects.blocks_made()
    }

    /// Takes the lock of the space with index `index`, as the `nth` space
    /// lock of one operation (0 for the first).
    #[inline]
    fn lock_index(&self, index: u32, nth: u8) -> Option<MutexGuard<'_, SpaceState<A>>> {
        let space = self.space(index)?;
        Some(if nth == 0 { space.state.lock() } else { space.state.lock_nested(nth) })
    }

    /// Takes the lock of `space`, which must be live.
    #[inline]
    fn lock_space(
        &self,
        space: SpaceId,
        nth: u8,
    ) -> Result<MutexGuard<'_, SpaceState<A>>, CapError> {
        let state = self.lock_index(space.index, nth).ok_or(CapError::NoSuchSpace)?;
        if !state.live || state.generation != space.generation {
            return Err(CapError::NoSuchSpace);
        }
        Ok(state)
    }

    /// Runs `f` with the lock of `space` held: the fast path, for
    /// everything that needs no link of the tree.
    pub fn in_space<R>(
        &self,
        space: SpaceId,
        f: impl FnOnce(&mut SpaceView<'_, A, K>) -> R,
    ) -> Result<R, CapError> {
        let mut state = self.lock_space(space, 0)?;
        let mut view = SpaceView { store: self, id: space, state: &mut state, stale: false };
        let result = f(&mut view);
        if view.stale {
            self.bump_version();
        }
        Ok(result)
    }

    /// The store's version, which moves whenever a capability looked up to
    /// be cached ([`SpaceView::get_to_cache`]) leaves its slot. Read before
    /// the lookup, it says the cached copy is still what the slot holds
    /// for as long as the version stays.
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    /// Moves the version on: for whatever the kernel caches beside the
    /// store, when it changes.
    pub fn bump_version(&self) {
        self.version.fetch_add(1, Ordering::Release);
    }

    /// Runs `f` on what the kernel keeps of the object `id`, if the object
    /// lives and the kernel keeps something of it, with the object's record
    /// locked: last of the store's locks, so `f` must not sleep or call
    /// back into the store.
    pub fn with_kept<T>(&self, id: ObjectId, f: impl FnOnce(&mut K) -> T) -> Option<T> {
        self.objects.with_kept(id, f)
    }

    /// Runs `f` on every living object's name and what the kernel keeps of
    /// it, one record's lock at a time, as [`Capabilities::with_kept`]
    /// does: an object made or released meanwhile may be seen or not.
    pub fn each_kept(&self, mut f: impl FnMut(ObjectId, &mut K)) {
        self.objects.each(self.objects.made(), |id, _, kept| {
            if let Some(kept) = kept {
                f(id, kept);
            }
        });
    }

    /// [`Capabilities::each_kept`], but giving up (`None`) at the first
    /// lock held elsewhere rather than wait for it: for a debugger, which
    /// may have stopped the CPU that holds one. `f` may have seen some
    /// objects by then.
    pub fn try_each_kept(&self, mut f: impl FnMut(ObjectId, &mut K)) -> Option<()> {
        let made = self.objects.free.try_lock()?.made;
        for index in 0..made {
            let Some(record) = self.objects.record(index) else { continue };
            let mut inner = record.try_lock()?;
            let id = ObjectId { index, generation: record.generation() };
            if let (Some(_), Some(kept)) = (inner.entry, inner.kept.as_mut()) {
                f(id, kept);
            }
        }
        Some(())
    }

    // ---- One operation at a time ----
    //
    // Each takes the locks it needs and lets them go again. A delete or a
    // move tries the fast path first.

    /// How many slots `space` has.
    pub fn slots(&self, space: SpaceId) -> Result<u32, CapError> {
        self.in_space(space, |view| view.slots())
    }

    /// The flags of the slot `at`, which must hold a capability.
    pub fn slot_flags(&self, at: SlotRef) -> Result<u8, CapError> {
        self.in_space(at.space, |view| view.slot_flags(at))?
    }

    /// Sets the flags of the slot `at`, which must hold a capability, and
    /// returns them as they were.
    pub fn set_slot_flags(&self, at: SlotRef, flags: u8) -> Result<u8, CapError> {
        self.in_space(at.space, |view| view.set_slot_flags(at, flags))?
    }

    /// The first slot of `space` from `from` on that holds a capability
    /// and has any of the flags in `mask`, if one does.
    pub fn find_flagged(
        &self,
        space: SpaceId,
        mask: u8,
        from: u32,
    ) -> Result<Option<u32>, CapError> {
        self.in_space(space, |view| view.find_flagged(mask, from))
    }

    /// Which of the 64 slots of `space` from `first` on hold a
    /// capability, one bit each from bit 0; slots past the end count as
    /// empty.
    pub fn used_slots(&self, space: SpaceId, first: u32) -> Result<u64, CapError> {
        self.in_space(space, |view| view.used_slots(first))
    }

    /// The capability in `at`.
    pub fn get(&self, at: SlotRef) -> Result<Capability, CapError> {
        self.in_space(at.space, |view| view.get(at))?
    }

    /// [`SpaceView::get_to_cache`].
    pub fn get_to_cache(&self, at: SlotRef) -> Result<Capability, CapError> {
        self.in_space(at.space, |view| view.get_to_cache(at))?
    }

    /// The capability in `at`, if it may be used on a `kind` with the
    /// rights `needs` ([`Capability::check`]). Every use of a capability
    /// goes through this.
    pub fn lookup(
        &self,
        at: SlotRef,
        kind: ObjectKind,
        needs: Rights,
    ) -> Result<Capability, CapError> {
        self.in_space(at.space, |view| view.lookup(at, kind, needs))?
    }

    /// How many steps `at` is from a capability the kernel made, at most.
    pub fn depth(&self, at: SlotRef) -> Result<u8, CapError> {
        self.with_tree(&[at.space], |tree| tree.depth(at))
    }

    /// The slot of the capability `at` was derived from, if it was.
    pub fn parent(&self, at: SlotRef) -> Result<Option<SlotRef>, CapError> {
        self.with_tree(&[at.space], |tree| tree.parent(at))
    }

    /// Whether the slot `at` is marked as having relatives: it does if it
    /// has a parent or children, and may if its last relative in another
    /// space went (ADR 0369).
    pub fn is_linked(&self, at: SlotRef) -> Result<bool, CapError> {
        self.in_space(at.space, |view| view.is_linked(at))?
    }

    /// Whether a revoke of `at` is under way.
    pub fn is_revoking(&self, at: SlotRef) -> Result<bool, CapError> {
        self.with_tree(&[at.space], |tree| tree.is_revoking(at))
    }

    /// What the store knows about the kernel object `id`.
    pub fn object(&self, id: ObjectId) -> Option<ObjectInfo> {
        let entry = self.objects.entry(id)?;
        Some(ObjectInfo {
            kind: entry.kind,
            caps: entry.caps,
            children: entry.children,
            extent: entry.extent,
            watermark: entry.watermark,
            origin: entry.origin,
        })
    }

    /// [`SpaceView::create`].
    pub fn create(&self, at: SlotRef, kind: ObjectKind) -> Result<ObjectId, CapError> {
        self.in_space(at.space, |view| view.create(at, kind))?
    }

    /// [`SpaceView::create_with_rights`].
    pub fn create_with_rights(
        &self,
        at: SlotRef,
        kind: ObjectKind,
        rights: Rights,
    ) -> Result<ObjectId, CapError> {
        self.in_space(at.space, |view| view.create_with_rights(at, kind, rights))?
    }

    /// [`SpaceView::create_memory`].
    pub fn create_memory(&self, at: SlotRef, base: u64, pages: u64) -> Result<ObjectId, CapError> {
        self.in_space(at.space, |view| view.create_memory(at, base, pages))?
    }

    /// [`SpaceView::grant_range`].
    pub fn grant_range(
        &self,
        at: SlotRef,
        kind: ObjectKind,
        base: u64,
        len: u64,
    ) -> Result<(), CapError> {
        self.in_space(at.space, |view| view.grant_range(at, kind, base, len))?
    }

    /// [`TreeView::derive`].
    pub fn derive(&self, from: SlotRef, to: SlotRef, how: Derivation) -> Result<(), CapError> {
        self.with_tree(&[from.space, to.space], |tree| tree.derive(from, to, how))
    }

    /// [`TreeView::seal`].
    pub fn seal(&self, from: SlotRef, key: SlotRef, to: SlotRef) -> Result<(), CapError> {
        self.with_tree(&[from.space, key.space, to.space], |tree| tree.seal(from, key, to))
    }

    /// [`TreeView::unseal`].
    pub fn unseal(&self, from: SlotRef, key: SlotRef, to: SlotRef) -> Result<(), CapError> {
        self.with_tree(&[from.space, key.space, to.space], |tree| tree.unseal(from, key, to))
    }

    /// [`TreeView::retype`], recording nothing beside the store.
    pub fn retype(
        &self,
        memory: SlotRef,
        kind: ObjectKind,
        order: u32,
        to: SlotRef,
        count: u32,
    ) -> Result<(), CapError> {
        self.with_tree(&[memory.space, to.space], |tree| {
            tree.retype(memory, kind, order, to, count, &mut |_, _, _| Ok(()))
        })
    }

    /// Moves the capability in `from` to `to`, keeping its place in the
    /// derivation tree. Into another space it needs the `transfer` right.
    /// One with no relatives moves with only the two spaces' locks.
    pub fn move_cap(&self, from: SlotRef, to: SlotRef) -> Result<(), CapError> {
        if self.move_fast(from, to)? == Fast::Linked {
            self.with_tree(&[from.space, to.space], |tree| tree.move_cap(from, to))?;
        }
        Ok(())
    }

    /// The fast path of [`Capabilities::move_cap`]: both spaces' locks,
    /// lowest index first.
    fn move_fast(&self, from: SlotRef, to: SlotRef) -> Result<Fast<()>, CapError> {
        if from.space == to.space {
            return self.in_space(from.space, |view| view.move_cap(from, to))?;
        }
        if from.space.index == to.space.index {
            // Two generations of one space: at most one is live, so the
            // other name is stale. Locking both would take one lock twice.
            return Err(CapError::NoSuchSpace);
        }
        let low = from.space.index.min(to.space.index);
        let (first, second) = if from.space.index == low { (from, to) } else { (to, from) };
        let mut first_state = self.lock_space(first.space, 0)?;
        let mut second_state = self.lock_space(second.space, 1)?;
        let (src, dst) = if from.space.index == low {
            (&mut first_state, &mut second_state)
        } else {
            (&mut second_state, &mut first_state)
        };
        let (Some(src), Some(dst)) =
            (src.slots.get_mut(from.slot as usize), dst.slots.get_mut(to.slot as usize))
        else {
            return Err(CapError::NoSuchSlot);
        };
        let Some(cap) = src.cap else { return Err(CapError::Empty) };
        if dst.cap.is_some() {
            return Err(CapError::Occupied);
        }
        if !cap.rights.contains(Rights::TRANSFER) {
            return Err(CapError::MissingRights(Rights::TRANSFER));
        }
        let mut stale = false;
        let done = fast::move_unlinked(src, dst, &mut stale);
        if stale {
            self.bump_version();
        }
        Ok(done)
    }

    /// Deletes the capability in `at`. Capabilities derived from it stay,
    /// as children of its parent. If it was the last capability to its
    /// object, and nothing retyped from the object is left, the object
    /// goes too and `released` hears of it. One with no relatives goes
    /// with only its space's lock.
    pub fn delete(
        &self,
        at: SlotRef,
        released: &mut dyn FnMut(Released, Option<K>),
    ) -> Result<(), CapError> {
        if self.in_space(at.space, |view| view.delete(at, released))?? == Fast::Linked {
            self.with_tree(&[at.space], |tree| tree.delete(at, released))?;
        }
        Ok(())
    }
}
