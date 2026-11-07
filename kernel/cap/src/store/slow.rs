// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The slow path: the tree lock and the locks of the spaces named
//! (ADR 0369).
//!
//! A [`TreeView`] holds the tree lock and the locks of up to
//! [`MAX_HELD`] spaces, taken in increasing order of index. It reaches
//! every slot's place in the tree, and what the slots of its own spaces
//! hold: an operation through it reads a capability from, fills or
//! empties only slots of those spaces, and changes only the links (and
//! depths and revoke marks) of other spaces' slots, which the tree lock
//! guards wherever the slot is.
//!
//! Revoking ([`Capabilities::revoke`]) holds the tree lock through a turn
//! and takes one space's lock at a time; creating, destroying and growing
//! a space take the tree lock and that space's.
//!
//! An operation that makes or releases objects takes their records'
//! indexes from the spares of the space whose slots it fills or empties,
//! and gives them back there ([`TreeView::indexes`]). The spares take the
//! object table's lock only to fill or empty themselves, unless the view
//! holds that lock throughout ([`Capabilities::with_all`]): for a caller
//! that takes a spin lock inside, where a mutex may not be taken. Each
//! object's record has a lock of its own, taken last.

use core::alloc::Allocator;

use alloc::vec::Vec;

use super::fast::move_unlinked;
use super::objects::{Entry, Free, Indexes};
use super::slots::{self, Forest, Link, Marks, Slot, SpaceState, Tree, Trees};
use super::{
    Capabilities, Extent, MAX_DEPTH, MAX_RETYPE, MAX_SLOTS, Progress, Released, SlotRef, SpaceId,
    bump, fill,
};
use crate::cap::{Capability, Derivation, carve};
use crate::error::CapError;
use crate::object::{ObjectId, ObjectKind};
use crate::rights::Rights;

/// The most spaces one slow-path operation names: sealing reads a
/// capability, a key and fills a slot, each perhaps in a space of its own.
pub const MAX_HELD: usize = 4;

/// Objects a retype names on the stack; more go in a vector on the heap.
const FEW: usize = 8;

/// What [`TreeView::unretype`] takes back: the memory retyped, how many
/// objects were counted to it, its watermark before, and the space whose
/// spares the objects' records came from.
struct Unretype {
    memory: ObjectId,
    count: u32,
    before: u64,
    space: u32,
}

/// What hears of each object a retype makes, with its name and extent,
/// and fills in what the kernel keeps of it ([`TreeView::retype`]).
pub type Record<'r, K> = dyn FnMut(ObjectId, Extent, &mut Option<K>) -> Result<(), CapError> + 'r;

/// A space a [`TreeView`] names: its index, and its state if it has one
/// (an index nothing was ever made at has none).
type Held<'v, A> = Option<(u32, Option<&'v mut SpaceState<A>>)>;

/// The tree and some spaces, locked: the slow path's view of the store.
pub struct TreeView<'v, A: Allocator + Clone, K = ()> {
    pub(super) store: &'v Capabilities<A, K>,
    pub(super) forest: &'v mut Forest<A>,
    pub(super) held: [Held<'v, A>; MAX_HELD],
    /// The object table's free list, when this view holds its lock
    /// throughout ([`Capabilities::with_all`]); otherwise the spaces'
    /// spares take the lock when they need it ([`TreeView::indexes`]).
    pub(super) table: Option<&'v mut Free<A>>,
    /// A capability a cache may hold left its slot: the store's version
    /// moves before the locks go.
    pub(super) stale: bool,
}

impl<A: Allocator + Clone, K> TreeView<'_, A, K> {
    // ---- Looking capabilities up ----

    /// The capability in `at`.
    pub fn get(&self, at: SlotRef) -> Result<Capability, CapError> {
        let link = self.link(at)?;
        self.slot(link).cap.ok_or(CapError::Empty)
    }

    /// The capability in `at`, if it may be used on a `kind` with the
    /// rights `needs` ([`Capability::check`]).
    pub fn lookup(
        &self,
        at: SlotRef,
        kind: ObjectKind,
        needs: Rights,
    ) -> Result<Capability, CapError> {
        let cap = self.get(at)?;
        cap.check(kind, needs)?;
        Ok(cap)
    }

    /// Which of the 64 slots of `space` from `first` on hold a
    /// capability, one bit each from bit 0; slots past the end count as
    /// empty.
    pub fn used_slots(&self, space: SpaceId, first: u32) -> Result<u64, CapError> {
        let state = self.space(space)?;
        let mut used = 0u64;
        for (bit, slot) in state.slots.iter().skip(first as usize).take(64).enumerate() {
            if slot.cap.is_some() {
                used |= 1 << bit;
            }
        }
        Ok(used)
    }

    /// The flags of the slot `at`, which must hold a capability.
    pub fn slot_flags(&self, at: SlotRef) -> Result<u8, CapError> {
        let link = self.full(at)?;
        Ok(self.slot(link).marks.flags)
    }

    /// Sets the flags of the slot `at`, which must hold a capability, and
    /// returns them as they were.
    pub fn set_slot_flags(&mut self, at: SlotRef, flags: u8) -> Result<u8, CapError> {
        let link = self.full(at)?;
        Ok(core::mem::replace(&mut self.slot_mut(link).marks.flags, flags))
    }

    /// How many steps `at` is from a capability the kernel made, at most.
    pub fn depth(&self, at: SlotRef) -> Result<u8, CapError> {
        let link = self.full(at)?;
        Ok(self.forest.tree(link).depth)
    }

    /// The slot of the capability `at` was derived from, if it was.
    pub fn parent(&self, at: SlotRef) -> Result<Option<SlotRef>, CapError> {
        let link = self.full(at)?;
        Ok(self.forest.tree(link).parent.map(|parent| self.slot_ref(parent)))
    }

    /// Whether the slot `at` is marked as having relatives.
    pub fn is_linked(&self, at: SlotRef) -> Result<bool, CapError> {
        let link = self.full(at)?;
        Ok(self.slot(link).marks.linked)
    }

    /// Whether a revoke of `at` is under way.
    pub fn is_revoking(&self, at: SlotRef) -> Result<bool, CapError> {
        let link = self.full(at)?;
        Ok(self.forest.tree(link).revoking)
    }

    // ---- Making objects ----

    /// [`super::SpaceView::create_recorded`], for a slow-path operation
    /// that makes an object too.
    pub fn create_recorded<T>(
        &mut self,
        at: SlotRef,
        kind: ObjectKind,
        rights: Rights,
        record: impl FnOnce(ObjectId, &mut Option<K>) -> Result<T, CapError>,
    ) -> Result<(ObjectId, T), CapError> {
        if kind.is_range() || kind.from_memory() {
            return Err(CapError::InvalidArgument);
        }
        let link = self.empty(at)?;
        let entry = Entry::new(kind, None, None);
        let store = self.store;
        let mut from = self.indexes(link.space);
        let recorded = store.objects.insert(&mut from, entry, record)?;
        let id = recorded.0;
        let mut cap = Capability::original(crate::object::Object {
            kind,
            target: crate::object::Target::Object(id),
        });
        cap.rights = rights.intersection(kind.rights());
        self.slot_mut(link).put_unlinked(cap);
        Ok(recorded)
    }

    // ---- Deriving ----

    /// Puts in `to` a capability derived from the one in `from`
    /// ([`Capability::derive`]), as its child. Into another space this
    /// also needs the `transfer` right.
    pub fn derive(&mut self, from: SlotRef, to: SlotRef, how: Derivation) -> Result<(), CapError> {
        let (src, dst, cap) = self.source_and_destination(from, to)?;
        let derived = cap.derive(how)?;
        self.may_derive(src, &cap, from.space != to.space)?;
        self.place(dst, derived, Some(src));
        self.store.objects.retain(&derived);
        Ok(())
    }

    /// Puts in `to` a copy of the capability in `from` sealed with the
    /// endpoint in `key` ([`Capability::seal_with`]), as its child.
    pub fn seal(&mut self, from: SlotRef, key: SlotRef, to: SlotRef) -> Result<(), CapError> {
        let key = self.get(key)?;
        let (src, dst, cap) = self.source_and_destination(from, to)?;
        let sealed = cap.seal_with(&key)?;
        self.may_derive(src, &cap, from.space != to.space)?;
        self.place(dst, sealed, Some(src));
        self.store.objects.retain(&sealed);
        Ok(())
    }

    /// Puts in `to` an unsealed copy of the sealed capability in `from`,
    /// if `key` holds the endpoint it was sealed with
    /// ([`Capability::unseal_with`]), as its child.
    pub fn unseal(&mut self, from: SlotRef, key: SlotRef, to: SlotRef) -> Result<(), CapError> {
        let key = self.get(key)?;
        let (src, dst, cap) = self.source_and_destination(from, to)?;
        let unsealed = cap.unseal_with(&key)?;
        self.may_derive(src, &cap, from.space != to.space)?;
        self.place(dst, unsealed, Some(src));
        self.store.objects.retain(&unsealed);
        Ok(())
    }

    /// Makes `count` objects of `kind` (memory, frames or DMA buffers) of
    /// 2^`order` pages each from the memory the capability in `memory`
    /// designates, and puts their capabilities, children of that one, in
    /// the `count` slots from `to`. It needs the `control` and `copy`
    /// rights. The memory goes back to the memory object when every object
    /// made from it is gone. `record` hears of each object made, and fills
    /// in what the kernel keeps of it, before its capability is placed; if
    /// it fails, nothing is made.
    pub fn retype(
        &mut self,
        memory: SlotRef,
        kind: ObjectKind,
        order: u32,
        to: SlotRef,
        count: u32,
        record: &mut Record<'_, K>,
    ) -> Result<(), CapError> {
        let src = self.full(memory)?;
        let cap = self.slot(src).cap.ok_or(CapError::Empty)?;
        if count == 0 || count > MAX_RETYPE {
            return Err(CapError::InvalidArgument);
        }
        let first = self.link(to)?;
        let end = to.slot.checked_add(count).ok_or(CapError::NoSuchSlot)?;
        if end as usize > self.space(to.space)?.slots.len() {
            return Err(CapError::NoSuchSlot);
        }
        for slot in to.slot..end {
            if self.slot(Link { space: first.space, slot }).cap.is_some() {
                return Err(CapError::Occupied);
            }
        }
        // Checks the rights and the kind; the name is filled in below.
        cap.retyped(kind, ObjectId { index: 0, generation: 0 })?;
        let Some(memory_id) = cap.id() else {
            return Err(CapError::InvalidArgument);
        };
        // The memory's pages are carved, and its count of children
        // raised, in one hold of its record's lock: a fast-path delete may
        // release an object retyped from it meanwhile, which lowers the
        // count (and, at zero, the watermark). The carve is the first
        // change, since it may refuse the retype; whatever refuses it
        // after takes the carve back ([`TreeView::unretype`]).
        let store = self.store;
        let objects = &store.objects;
        let carved = objects.update(memory_id, |parent| {
            let extent = parent.extent.ok_or(CapError::InvalidArgument)?;
            let (offset, watermark) =
                carve(extent.base, extent.pages, parent.watermark, order, u64::from(count))?;
            let before = parent.watermark;
            parent.watermark = watermark;
            parent.children += count;
            Ok((extent.base + offset, before))
        });
        let (base, before) = carved.ok_or(CapError::Empty)??;
        let undo = Unretype { memory: memory_id, count, before, space: first.space };
        if let Err(err) = self.may_derive(src, &cap, memory.space != to.space) {
            self.unretype(&undo, &[]);
            return Err(err);
        }
        // The names made, to place or to unmake: on the stack for a few,
        // as most retypes make (a page at a time), and on the heap for
        // more.
        let mut few = [ObjectId { index: 0, generation: 0 }; FEW];
        let mut many: Vec<ObjectId, A> = Vec::new_in(store.alloc.clone());
        let made: &mut [ObjectId] = if count as usize <= FEW {
            &mut few[..count as usize]
        } else {
            if many.try_reserve_exact(count as usize).is_err() {
                self.unretype(&undo, &[]);
                return Err(CapError::NoMemory);
            }
            super::fill(&mut many, count as usize, || ObjectId { index: 0, generation: 0 });
            &mut many
        };
        // Each object's record comes from the spares of the space its
        // capability goes to.
        for i in 0..count as usize {
            let extent = Extent { base: base + ((i as u64) << order), pages: 1 << order };
            let entry = Entry::new(kind, Some(extent), Some(memory_id));
            let keep = |id, kept: &mut Option<K>| record(id, extent, kept);
            match objects.insert(&mut self.indexes(first.space), entry, keep) {
                Ok((id, ())) => made[i] = id,
                Err(err) => {
                    self.unretype(&undo, &made[..i]);
                    return Err(err);
                }
            }
        }
        for (i, &id) in made.iter().enumerate() {
            let child = cap.retyped(kind, id)?;
            let link = Link { space: first.space, slot: to.slot + i as u32 };
            self.place(link, child, Some(src));
        }
        Ok(())
    }

    /// Takes back a retype that cannot finish: the objects in `made`,
    /// whose capabilities were not placed, and the carve. Only a retype
    /// raises a memory's watermark, and this one holds the tree lock, so
    /// the watermark it found is still the one to go back to; the objects
    /// it counted kept the memory's count of children above zero until
    /// now.
    fn unretype(&mut self, undo: &Unretype, made: &[ObjectId]) {
        let objects = &self.store.objects;
        for &id in made {
            objects.unmake(id, &mut self.indexes(undo.space));
        }
        objects.update(undo.memory, |parent| {
            parent.children -= undo.count;
            parent.watermark = if parent.children == 0 { 0 } else { undo.before };
        });
    }

    // ---- Moving and deleting ----

    /// Moves the capability in `from` to `to`, keeping its place in the
    /// derivation tree. Into another space it needs the `transfer` right.
    pub fn move_cap(&mut self, from: SlotRef, to: SlotRef) -> Result<(), CapError> {
        let src = self.full(from)?;
        let dst = self.empty(to)?;
        let cap = self.slot(src).cap.ok_or(CapError::Empty)?;
        if from.space != to.space && !cap.rights.contains(Rights::TRANSFER) {
            return Err(CapError::MissingRights(Rights::TRANSFER));
        }
        if self.forest.tree(src).revoking {
            return Err(CapError::Revoking);
        }
        if self.slot(src).marks.linked {
            self.move_linked(src, dst);
        } else {
            let mut stale = self.stale;
            let (from_slot, to_slot) = self.two_slots(src, dst);
            let _ = move_unlinked(from_slot, to_slot, &mut stale);
            self.stale = stale;
        }
        Ok(())
    }

    /// Deletes the capability in `at`. Capabilities derived from it stay,
    /// as children of its parent. If it was the last capability to its
    /// object, and nothing retyped from the object is left, the object
    /// goes too and `released` hears of it.
    pub fn delete(
        &mut self,
        at: SlotRef,
        released: &mut dyn FnMut(Released, Option<K>),
    ) -> Result<(), CapError> {
        let link = self.full(at)?;
        self.remove(link, released);
        Ok(())
    }

    // ---- Internals ----

    /// Where work on the slots of the space with index `space`, which
    /// this view holds, takes free object indexes from and gives them
    /// back to: the space's spares, and the table's own list too when this
    /// view holds the table's lock ([`Capabilities::with_all`]), since
    /// filling or emptying the spares would take that lock again.
    fn indexes(&mut self, space: u32) -> Indexes<'_, A> {
        let held = self.held.iter_mut().flatten().find(|(held, _)| *held == space);
        let Some(state) = held.and_then(|(_, state)| state.as_deref_mut()) else {
            unreachable!("object indexes for a space the view does not hold");
        };
        match self.table.as_deref_mut() {
            Some(free) => Indexes::Both(&mut state.spare, free),
            None => Indexes::Spare(&mut state.spare),
        }
    }

    /// The state of the space with index `index`, if this view holds it.
    fn state(&self, index: u32) -> Option<&SpaceState<A>> {
        self.held.iter().flatten().find(|(held, _)| *held == index)?.1.as_deref()
    }

    fn state_mut(&mut self, index: u32) -> Option<&mut SpaceState<A>> {
        self.held.iter_mut().flatten().find(|(held, _)| *held == index)?.1.as_deref_mut()
    }

    /// The state of `space`, which must be live.
    fn space(&self, space: SpaceId) -> Result<&SpaceState<A>, CapError> {
        let Some(state) = self.state(space.index) else {
            debug_assert!(self.names(space.index), "{space} was not named to the slow path");
            return Err(CapError::NoSuchSpace);
        };
        if !state.live || state.generation != space.generation {
            return Err(CapError::NoSuchSpace);
        }
        Ok(state)
    }

    /// Whether this view was asked for the space with index `index`.
    fn names(&self, index: u32) -> bool {
        self.held.iter().flatten().any(|(held, _)| *held == index)
    }

    /// The link for `at`, which must name a slot of a live space this
    /// view holds.
    fn link(&self, at: SlotRef) -> Result<Link, CapError> {
        if at.slot as usize >= self.space(at.space)?.slots.len() {
            return Err(CapError::NoSuchSlot);
        }
        Ok(Link { space: at.space.index, slot: at.slot })
    }

    /// The link for `at`, which must hold a capability.
    fn full(&self, at: SlotRef) -> Result<Link, CapError> {
        let link = self.link(at)?;
        if self.slot(link).cap.is_none() {
            return Err(CapError::Empty);
        }
        Ok(link)
    }

    /// The link for `at`, which must be empty.
    fn empty(&self, at: SlotRef) -> Result<Link, CapError> {
        let link = self.link(at)?;
        if self.slot(link).cap.is_some() {
            return Err(CapError::Occupied);
        }
        Ok(link)
    }

    fn slot_ref(&self, link: Link) -> SlotRef {
        let generation = self.forest.spaces[link.space as usize].generation;
        SlotRef { space: SpaceId { index: link.space, generation }, slot: link.slot }
    }

    fn source_and_destination(
        &self,
        from: SlotRef,
        to: SlotRef,
    ) -> Result<(Link, Link, Capability), CapError> {
        let src = self.full(from)?;
        let dst = self.empty(to)?;
        let cap = self.slot(src).cap.ok_or(CapError::Empty)?;
        Ok((src, dst, cap))
    }

    // What a slot of a held space holds. Every operation reads, fills or
    // empties only slots of the spaces it named; one that reached another
    // is a bug in this module, and it panics.
    fn slot(&self, link: Link) -> &Slot {
        let Some(state) = self.state(link.space) else {
            panic!("a slot of a space the slow path does not hold");
        };
        &state.slots[link.slot as usize]
    }

    fn slot_mut(&mut self, link: Link) -> &mut Slot {
        let Some(state) = self.state_mut(link.space) else {
            panic!("a slot of a space the slow path does not hold");
        };
        &mut state.slots[link.slot as usize]
    }

    /// Two different slots of held spaces, both writable.
    fn two_slots(&mut self, a: Link, b: Link) -> (&mut Slot, &mut Slot) {
        assert_ne!(a, b, "one slot taken twice");
        if a.space == b.space {
            let Some(state) = self.state_mut(a.space) else {
                panic!("a slot of a space the slow path does not hold");
            };
            return slots::two_slots(&mut state.slots, a.slot as usize, b.slot as usize);
        }
        let mut first = None;
        let mut second = None;
        for (index, state) in self.held.iter_mut().flatten() {
            if *index == a.space {
                first = state.as_deref_mut().map(|state| &mut state.slots[a.slot as usize]);
            } else if *index == b.space {
                second = state.as_deref_mut().map(|state| &mut state.slots[b.slot as usize]);
            }
        }
        match (first, second) {
            (Some(first), Some(second)) => (first, second),
            _ => panic!("a slot of a space the slow path does not hold"),
        }
    }

    /// Whether a capability made from `src` may be placed: not too deep,
    /// not below a revoke under way, and with `transfer` if it goes to
    /// another space.
    ///
    /// Deleting a capability moves its children nearer the root while
    /// their depths stay, so a depth may be more than the distance to the
    /// root. Too deep is judged by the distance, and the depths on the way
    /// from `src` to its root become their distances: otherwise a
    /// capability copied and deleted in turn, as a shell saves and
    /// restores a descriptor with dup, grew deeper each time until
    /// nothing more could be derived from it. This walks and writes only
    /// the tree, which the tree lock guards in every space.
    fn may_derive(&mut self, src: Link, cap: &Capability, elsewhere: bool) -> Result<(), CapError> {
        if elsewhere && !cap.rights.contains(Rights::TRANSFER) {
            return Err(CapError::MissingRights(Rights::TRANSFER));
        }
        // Depths fall strictly towards the root, so these end within
        // MAX_DEPTH steps, and the distance fits.
        let mut distance = 0u8;
        let mut revoking = false;
        let mut at = Some(src);
        while let Some(link) = at {
            let tree = self.forest.tree(link);
            revoking |= tree.revoking;
            at = tree.parent;
            distance += u8::from(at.is_some());
        }
        if distance >= MAX_DEPTH {
            return Err(CapError::TooDeep);
        }
        if revoking {
            return Err(CapError::Revoking);
        }
        // Each depth only falls, and to one more than its parent's new
        // one, so every depth stays more than its parent's.
        let mut depth = distance;
        let mut at = Some(src);
        while let Some(link) = at {
            let tree = self.forest.tree_mut(link);
            tree.depth = depth;
            depth = depth.saturating_sub(1);
            at = tree.parent;
        }
        Ok(())
    }

    /// Puts `cap` in the empty slot `link` as the first child of `parent`,
    /// and marks both slots linked.
    fn place(&mut self, link: Link, cap: Capability, parent: Option<Link>) {
        let Some(parent) = parent else {
            self.slot_mut(link).put_unlinked(cap);
            return;
        };
        // Only an empty slot, which no cache can hold, takes a capability.
        debug_assert!(self.slot(link).cap.is_none(), "placing a capability in a full slot");
        self.slot_mut(parent).marks.linked = true;
        let above = self.forest.tree_mut(parent);
        let depth = above.depth + 1;
        let next = above.child.replace(link);
        if let Some(next) = next {
            self.forest.tree_mut(next).prev = Some(link);
        }
        *self.slot_mut(link) =
            Slot { cap: Some(cap), marks: Marks { linked: true, ..Marks::NONE } };
        *self.forest.tree_mut(link) = Tree { parent: Some(parent), next, depth, ..Tree::NONE };
    }

    /// Moves a capability that may have relatives, from the full slot
    /// `src` to the empty slot `dst`, and the links that lead to it.
    fn move_linked(&mut self, src: Link, dst: Link) {
        let moved = *self.slot(src);
        // The flags stay behind: they belong to the slot. A cache may
        // hold the capability under its old slot, not its new one. The
        // bit goes with the links.
        *self.slot_mut(dst) =
            Slot { cap: moved.cap, marks: Marks { linked: moved.marks.linked, ..Marks::NONE } };
        *self.slot_mut(src) = Slot::EMPTY;
        self.stale |= moved.marks.cached;
        let node = core::mem::replace(self.forest.tree_mut(src), Tree::NONE);
        *self.forest.tree_mut(dst) = node;
        match node.prev {
            Some(prev) => self.forest.tree_mut(prev).next = Some(dst),
            None => {
                if let Some(parent) = node.parent {
                    self.forest.tree_mut(parent).child = Some(dst);
                }
            }
        }
        if let Some(next) = node.next {
            self.forest.tree_mut(next).prev = Some(dst);
        }
        let mut child = node.child;
        while let Some(c) = child {
            let tree = self.forest.tree_mut(c);
            tree.parent = Some(dst);
            child = tree.next;
        }
    }

    /// Takes the capability out of `link`: its children move up to its
    /// parent, in its place among its siblings, and its object loses a
    /// capability. Only `link`'s own slot, and its parent's if that is in
    /// the same space, are touched beyond the tree.
    pub(super) fn remove(&mut self, link: Link, released: &mut dyn FnMut(Released, Option<K>)) {
        let slot = core::mem::replace(self.slot_mut(link), Slot::EMPTY);
        let held = slot.cap.as_ref().and_then(Capability::id);
        self.stale |= slot.marks.cached;
        let node = core::mem::replace(self.forest.tree_mut(link), Tree::NONE);
        match (node.parent, node.child) {
            (Some(parent), Some(first)) => {
                let mut last = first;
                let mut child = Some(first);
                while let Some(c) = child {
                    let tree = self.forest.tree_mut(c);
                    tree.parent = Some(parent);
                    last = c;
                    child = tree.next;
                }
                self.forest.tree_mut(first).prev = node.prev;
                match node.prev {
                    Some(prev) => self.forest.tree_mut(prev).next = Some(first),
                    None => self.forest.tree_mut(parent).child = Some(first),
                }
                self.forest.tree_mut(last).next = node.next;
                if let Some(next) = node.next {
                    self.forest.tree_mut(next).prev = Some(last);
                }
            }
            (Some(parent), None) => {
                match node.prev {
                    Some(prev) => self.forest.tree_mut(prev).next = node.next,
                    None => self.forest.tree_mut(parent).child = node.next,
                }
                if let Some(next) = node.next {
                    self.forest.tree_mut(next).prev = node.prev;
                }
            }
            (None, first) => {
                // The children become roots, with no siblings.
                let mut child = first;
                while let Some(c) = child {
                    let tree = self.forest.tree_mut(c);
                    child = tree.next;
                    tree.parent = None;
                    tree.prev = None;
                    tree.next = None;
                }
            }
        }
        // The common case of a copy made in a process and deleted again:
        // the original is alone once more, and the fast path can have it.
        if let Some(parent) = node.parent
            && parent.space == link.space
        {
            self.unlink_if_alone(parent);
        }
        if let Some(id) = held {
            // The index goes to the spares of the space whose slot this
            // was.
            let store = self.store;
            store.objects.release(id, &mut self.indexes(link.space), released);
        }
    }

    /// Clears the `linked` bit of `link`, and makes its tree part
    /// [`Tree::NONE`], if it has no parent and no children left and no
    /// revoke of it is under way (a revoke clears its mark first, when it
    /// ends). `link`'s space must be held.
    pub(super) fn unlink_if_alone(&mut self, link: Link) {
        let tree = self.forest.tree(link);
        if tree.parent.is_none() && tree.child.is_none() && !tree.revoking {
            *self.forest.tree_mut(link) = Tree::NONE;
            self.slot_mut(link).marks.linked = false;
        }
    }
}

impl<A: Allocator + Clone, K> Capabilities<A, K> {
    /// Runs `f` with the tree lock and the locks of `spaces` held: the
    /// slow path, for everything that follows or changes a link. A space
    /// named twice is locked once; one that does not exist is named and
    /// not locked, and `f` finds it gone.
    ///
    /// # Panics
    ///
    /// If `spaces` names more than [`MAX_HELD`] spaces.
    pub fn with_tree<R>(
        &self,
        spaces: &[SpaceId],
        f: impl FnOnce(&mut TreeView<'_, A, K>) -> R,
    ) -> R {
        self.slow_path(spaces, false, f)
    }

    /// [`Capabilities::with_tree`], with the object table's lock held too,
    /// after the spaces' (ADR 0369's order): for a caller that takes a spin
    /// lock inside `f` and works on the store under it, where no mutex may
    /// be taken. The kernel moves a message's capabilities with an
    /// endpoint's lock held. Such a caller must not release objects under
    /// its spin lock, since what an object does when it goes may sleep.
    ///
    /// # Panics
    ///
    /// If `spaces` names more than [`MAX_HELD`] spaces.
    pub fn with_all<R>(
        &self,
        spaces: &[SpaceId],
        f: impl FnOnce(&mut TreeView<'_, A, K>) -> R,
    ) -> R {
        self.slow_path(spaces, true, f)
    }

    fn slow_path<R>(
        &self,
        spaces: &[SpaceId],
        hold_objects: bool,
        f: impl FnOnce(&mut TreeView<'_, A, K>) -> R,
    ) -> R {
        assert!(spaces.len() <= MAX_HELD, "the slow path names at most {MAX_HELD} spaces");
        let mut indexes = [u32::MAX; MAX_HELD];
        let mut named = 0;
        for space in spaces {
            if !indexes[..named].contains(&space.index) {
                indexes[named] = space.index;
                named += 1;
            }
        }
        indexes[..named].sort_unstable();
        let mut forest = self.tree.lock();
        // In increasing order of index, the second and later as nested
        // locks of the same class.
        let mut guards = [const { None }; MAX_HELD];
        for (nth, (&index, guard)) in indexes[..named].iter().zip(guards.iter_mut()).enumerate() {
            *guard = self.lock_index(index, nth as u8);
        }
        let mut held = [const { None }; MAX_HELD];
        for ((&index, guard), held) in indexes[..named].iter().zip(guards.iter_mut()).zip(&mut held)
        {
            *held = Some((index, guard.as_deref_mut()));
        }
        // The object table's lock comes last of the store's.
        let mut table = hold_objects.then(|| self.objects.lock());
        let mut view = TreeView {
            store: self,
            forest: &mut forest,
            held,
            table: table.as_deref_mut(),
            stale: false,
        };
        let result = f(&mut view);
        if view.stale {
            self.bump_version();
        }
        result
    }

    /// Deletes every capability derived from the one in `at`, in every
    /// space, keeping that one; at most `budget` of them in this call.
    /// Returns [`Progress::More`] when the budget ran out: until a call
    /// returns [`Progress::Done`], nothing can be derived from the
    /// capability or from those below it, and it cannot be moved.
    ///
    /// The turn holds the tree lock throughout, and one space's lock at a
    /// time: the capability's, to mark it and, at the end, to unmark it;
    /// and each deleted capability's, to empty its slot.
    pub fn revoke(
        &self,
        at: SlotRef,
        budget: usize,
        released: &mut dyn FnMut(Released, Option<K>),
    ) -> Result<Progress, CapError> {
        self.revoke_turn(at, budget, true, released)
    }

    /// Goes on with a revoke of the capability in `at` that
    /// [`Capabilities::revoke`] left at [`Progress::More`]. The caller let
    /// go of the store in between, so the capability may have gone (it was
    /// deleted, and another may be in its slot now, whose descendants
    /// nobody asked to revoke), or another revoke of it may have finished:
    /// either way the slot has no revoke mark, and nothing is left of what
    /// was asked, so this is [`Progress::Done`].
    pub fn continue_revoke(
        &self,
        at: SlotRef,
        budget: usize,
        released: &mut dyn FnMut(Released, Option<K>),
    ) -> Result<Progress, CapError> {
        self.revoke_turn(at, budget, false, released)
    }

    fn revoke_turn(
        &self,
        at: SlotRef,
        budget: usize,
        first: bool,
        released: &mut dyn FnMut(Released, Option<K>),
    ) -> Result<Progress, CapError> {
        let mut forest = self.tree.lock();
        let top = {
            let mut state = match self.lock_space(at.space, 0) {
                Ok(state) => state,
                Err(_) if !first => return Ok(Progress::Done),
                Err(err) => return Err(err),
            };
            let view = TreeView::one(self, &mut forest, at.space.index, &mut state);
            let top = match view.full(at) {
                Ok(top) => top,
                Err(_) if !first => return Ok(Progress::Done),
                Err(err) => return Err(err),
            };
            if first {
                // With no relatives there is nothing to revoke, and only a
                // linked slot is ever marked.
                if !view.slot(top).marks.linked {
                    return Ok(Progress::Done);
                }
                view.forest.tree_mut(top).revoking = true;
            } else if !view.forest.tree(top).revoking {
                return Ok(Progress::Done);
            }
            top
        };
        // Down the tree under the tree lock alone: every slot below `top`
        // is linked, so its links are the tree lock's.
        let mut deleted = 0;
        let mut cursor = top;
        loop {
            if let Some(child) = forest.tree(cursor).child {
                cursor = child;
                continue;
            }
            if cursor == top {
                break;
            }
            if deleted == budget {
                return Ok(Progress::More);
            }
            // Below `top`, so it has a parent.
            let parent = forest.tree(cursor).parent.unwrap_or(top);
            let Some(mut state) = self.lock_index(cursor.space, 0) else {
                panic!("a link leads to a space that was never made");
            };
            let mut view = TreeView::one(self, &mut forest, cursor.space, &mut state);
            view.remove(cursor, released);
            if view.stale {
                self.bump_version();
            }
            deleted += 1;
            cursor = parent;
        }
        // Nothing is left below it: unmarked, and unlinked if it has no
        // parent either.
        let Some(mut state) = self.lock_index(top.space, 0) else {
            panic!("a revoke's capability is in a space that was never made");
        };
        let mut view = TreeView::one(self, &mut forest, top.space, &mut state);
        view.forest.tree_mut(top).revoking = false;
        view.unlink_if_alone(top);
        Ok(Progress::Done)
    }

    // ---- Spaces ----

    /// A new capability space with `slots` empty slots (1 to [`MAX_SLOTS`]).
    ///
    /// # Panics
    ///
    /// Never in practice: only if the block a space index was just given
    /// room in has gone, which nothing does.
    pub fn create_space(&self, slots: u32) -> Result<SpaceId, CapError> {
        if slots == 0 || slots > MAX_SLOTS {
            return Err(CapError::InvalidArgument);
        }
        let mut array = Vec::new_in(self.alloc.clone());
        array.try_reserve_exact(slots as usize).map_err(|_| CapError::NoMemory)?;
        fill(&mut array, slots as usize, || Slot::EMPTY);
        let mut trees = Vec::new_in(self.alloc.clone());
        trees.try_reserve_exact(slots as usize).map_err(|_| CapError::NoMemory)?;
        fill(&mut trees, slots as usize, || Tree::NONE);
        let mut forest = self.tree.lock();
        let index = match forest.free {
            Some(index) => {
                let entry = &mut forest.spaces[index as usize];
                let next = entry.next_free.take();
                entry.slots = trees;
                forest.free = next;
                index
            }
            None => {
                let index = u32::try_from(forest.spaces.len()).map_err(|_| CapError::NoMemory)?;
                self.make_room_for(index)?;
                forest.spaces.try_reserve(1).map_err(|_| CapError::NoMemory)?;
                let entry = Trees { slots: trees, generation: 1, next_free: None };
                let _ = forest.spaces.push_within_capacity(entry);
                index
            }
        };
        let Some(mut state) = self.lock_index(index, 0) else {
            panic!("a space's block went");
        };
        state.slots = array;
        state.live = true;
        let generation = state.generation;
        forest.spaces[index as usize].generation = generation;
        Ok(SpaceId { index, generation })
    }

    /// Deletes every capability in `space` (as [`Capabilities::delete`]
    /// does, so capabilities derived from them elsewhere stay, under the
    /// deleted ones' parents) and frees the space.
    pub fn destroy_space(
        &self,
        space: SpaceId,
        released: &mut dyn FnMut(Released, Option<K>),
    ) -> Result<(), CapError> {
        let mut forest = self.tree.lock();
        let mut state = self.lock_space(space, 0)?;
        let mut view = TreeView::one(self, &mut forest, space.index, &mut state);
        let len = view.space(space)?.slots.len() as u32;
        // Sixty-four slots at a time, the full ones found in one pass over
        // the space's slots: looking each slot up through the view cost a
        // process's exit some 3,000 instructions. A removal empties only
        // its own slot, but each is checked again before it goes.
        for first in (0..len).step_by(64) {
            let mut used = view.used_slots(space, first)?;
            while used != 0 {
                let link = Link { space: space.index, slot: first + used.trailing_zeros() };
                used &= used - 1;
                if view.slot(link).cap.is_some() {
                    view.remove(link, released);
                }
            }
        }
        if view.stale {
            self.bump_version();
        }
        // Its spare object indexes go back to the table.
        self.objects.give_back(&mut state.spare);
        state.slots = Vec::new_in(self.alloc.clone());
        state.live = false;
        state.generation = bump(state.generation);
        let next = forest.free;
        let entry = &mut forest.spaces[space.index as usize];
        entry.slots = Vec::new_in(self.alloc.clone());
        entry.generation = state.generation;
        entry.next_free = next;
        forest.free = Some(space.index);
        Ok(())
    }

    /// Grows `space` to `slots` empty slots in all (at most
    /// [`MAX_SLOTS`]); a space never shrinks, so asking for fewer than it
    /// has changes nothing. Returns how many it has. Growing moves the
    /// space's slots, so it holds the tree lock too: nobody follows a link
    /// into them meanwhile.
    pub fn grow_space(&self, space: SpaceId, slots: u32) -> Result<u32, CapError> {
        if slots > MAX_SLOTS {
            return Err(CapError::InvalidArgument);
        }
        let mut forest = self.tree.lock();
        let mut state = self.lock_space(space, 0)?;
        let more = (slots as usize).saturating_sub(state.slots.len());
        let trees = &mut forest.spaces[space.index as usize].slots;
        state.slots.try_reserve_exact(more).map_err(|_| CapError::NoMemory)?;
        trees.try_reserve_exact(more).map_err(|_| CapError::NoMemory)?;
        let len = state.slots.len() + more;
        fill(&mut state.slots, len, || Slot::EMPTY);
        fill(trees, len, || Tree::NONE);
        Ok(len as u32)
    }
}

impl<'v, A: Allocator + Clone, K> TreeView<'v, A, K> {
    /// A view of the tree and one space's state, for the store's own use.
    fn one(
        store: &'v Capabilities<A, K>,
        forest: &'v mut Forest<A>,
        index: u32,
        state: &'v mut SpaceState<A>,
    ) -> Self {
        let mut held = [const { None }; MAX_HELD];
        held[0] = Some((index, Some(state)));
        TreeView { store, forest, held, table: None, stale: false }
    }
}
