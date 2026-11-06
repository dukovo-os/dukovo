// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The fast path: what one space's lock is enough for (ADR 0369).
//!
//! A [`SpaceView`] holds one space's lock and nothing of the tree, so
//! nothing done through it can follow or change a link: it reads and
//! changes the slots of its own space, makes objects, and deletes or
//! moves capabilities whose slots are not linked. A slot that is linked
//! makes it answer [`Fast::Linked`]; the caller then lets go of the space
//! and asks again through [`Capabilities::with_tree`].

use core::alloc::Allocator;

use super::objects::{Entry, Objects};
use super::slots::{Slot, SpaceState, two_slots};
use super::{Capabilities, Extent, Released, SlotRef, SpaceId};
use crate::cap::Capability;
use crate::error::CapError;
use crate::object::{Object, ObjectId, ObjectKind, Target};
use crate::rights::Rights;

/// What a fast-path operation did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use]
pub enum Fast<T> {
    /// It is done.
    Done(T),
    /// The slot is linked, so nothing was done: the slow path must do it.
    Linked,
}

/// One space, locked: the fast path's view of the store.
pub struct SpaceView<'v, A: Allocator + Clone, K = ()> {
    pub(super) store: &'v Capabilities<A, K>,
    pub(super) id: SpaceId,
    pub(super) state: &'v mut SpaceState<A>,
    /// A capability a cache may hold left its slot: the store's version
    /// moves before the lock goes.
    pub(super) stale: bool,
}

impl<A: Allocator + Clone, K> SpaceView<'_, A, K> {
    /// The space.
    pub fn space(&self) -> SpaceId {
        self.id
    }

    /// How many slots the space has.
    pub fn slots(&self) -> u32 {
        self.state.slots.len() as u32
    }

    /// The capability in `at`.
    pub fn get(&self, at: SlotRef) -> Result<Capability, CapError> {
        let index = self.index(at)?;
        self.state.slots[index].cap.ok_or(CapError::Empty)
    }

    /// The capability in `at`, as [`SpaceView::get`] gives it, for a cache
    /// outside the store to keep: from now on the store's version moves
    /// once it leaves the slot (deleted, revoked, moved, or its space
    /// destroyed). Nothing else changes a capability in a slot, so until
    /// then the cached copy is what the slot holds.
    pub fn get_to_cache(&mut self, at: SlotRef) -> Result<Capability, CapError> {
        let index = self.index(at)?;
        let slot = &mut self.state.slots[index];
        let cap = slot.cap.ok_or(CapError::Empty)?;
        slot.marks.cached = true;
        Ok(cap)
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

    /// The flags of the slot `at`, which must hold a capability.
    pub fn slot_flags(&self, at: SlotRef) -> Result<u8, CapError> {
        let index = self.full(at)?;
        Ok(self.state.slots[index].marks.flags)
    }

    /// Sets the flags of the slot `at`, which must hold a capability, and
    /// returns them as they were.
    pub fn set_slot_flags(&mut self, at: SlotRef, flags: u8) -> Result<u8, CapError> {
        let index = self.full(at)?;
        Ok(core::mem::replace(&mut self.state.slots[index].marks.flags, flags))
    }

    /// The first slot from `from` on that holds a capability and has any
    /// of the flags in `mask`, if one does.
    pub fn find_flagged(&self, mask: u8, from: u32) -> Option<u32> {
        let found = self
            .state
            .slots
            .iter()
            .enumerate()
            .skip(from as usize)
            .find(|(_, slot)| slot.cap.is_some() && slot.marks.flags & mask != 0);
        found.map(|(index, _)| index as u32)
    }

    /// Which of the 64 slots from `first` on hold a capability, one bit
    /// each from bit 0; slots past the end count as empty.
    pub fn used_slots(&self, first: u32) -> u64 {
        let mut used = 0u64;
        for (bit, slot) in self.state.slots.iter().skip(first as usize).take(64).enumerate() {
            if slot.cap.is_some() {
                used |= 1 << bit;
            }
        }
        used
    }

    /// Whether the slot `at` is marked as having relatives: it does if it
    /// has a parent or children, and may if its last relative in another
    /// space went.
    pub fn is_linked(&self, at: SlotRef) -> Result<bool, CapError> {
        let index = self.full(at)?;
        Ok(self.state.slots[index].marks.linked)
    }

    // ---- Making objects (the kernel only) ----

    /// Makes a new kernel object of `kind` (an address space, thread,
    /// process, endpoint, notification or resource domain) and puts its
    /// original capability, with every right of its kind, in `at`.
    pub fn create(&mut self, at: SlotRef, kind: ObjectKind) -> Result<ObjectId, CapError> {
        self.create_with_rights(at, kind, kind.rights())
    }

    /// Makes a new kernel object of `kind`, as [`SpaceView::create`] does,
    /// but with only `rights` (those of them its kind has) on its original
    /// capability: for an object the kernel makes on a holder's behalf,
    /// which must get no more than the holder may have (a file a directory
    /// capability opens).
    pub fn create_with_rights(
        &mut self,
        at: SlotRef,
        kind: ObjectKind,
        rights: Rights,
    ) -> Result<ObjectId, CapError> {
        self.create_recorded(at, kind, rights, |_, _| Ok(())).map(|(id, ())| id)
    }

    /// Makes a new kernel object as [`SpaceView::create_with_rights`]
    /// does, and runs `record` with its name and the registry, in the same
    /// hold of the object table's lock, before its capability is put in
    /// `at`: for the kernel to record what the name stands for. If `record`
    /// fails, the object is unmade and nothing is put in `at`.
    pub fn create_recorded<T>(
        &mut self,
        at: SlotRef,
        kind: ObjectKind,
        rights: Rights,
        record: impl FnOnce(ObjectId, &mut K) -> Result<T, CapError>,
    ) -> Result<(ObjectId, T), CapError> {
        if kind.is_range() || kind.from_memory() {
            return Err(CapError::InvalidArgument);
        }
        let index = self.empty(at)?;
        // Nobody can name the object before its capability is in the slot,
        // which this space's lock keeps from everyone else.
        let recorded = make_recorded(&mut self.store.objects.lock(), kind, record)?;
        let id = recorded.0;
        let mut cap = Capability::original(Object { kind, target: Target::Object(id) });
        cap.rights = rights.intersection(kind.rights());
        self.state.slots[index].put_unlinked(cap);
        Ok(recorded)
    }

    /// Runs `f` on what the kernel keeps beside the object table, with the
    /// table's lock held after this space's ([`Capabilities::with_registry`]).
    pub fn with_registry<T>(&self, f: impl FnOnce(&mut K) -> T) -> T {
        self.store.with_registry(f)
    }

    /// Makes a memory object for `pages` pages from page frame `base`,
    /// which the kernel hands over until the store releases them, and puts
    /// its original capability in `at`.
    pub fn create_memory(
        &mut self,
        at: SlotRef,
        base: u64,
        pages: u64,
    ) -> Result<ObjectId, CapError> {
        if pages == 0 || base.checked_add(pages).is_none_or(|end| end > 1 << 40) {
            return Err(CapError::InvalidArgument);
        }
        let index = self.empty(at)?;
        let extent = Some(Extent { base, pages });
        let id = self.store.objects.lock().insert(Entry::new(ObjectKind::Memory, extent, None))?;
        let cap =
            Capability::original(Object { kind: ObjectKind::Memory, target: Target::Object(id) });
        self.state.slots[index].put_unlinked(cap);
        Ok(id)
    }

    /// Puts in `at` an original capability for `len` interrupt lines, I/O
    /// ports or pages of device registers from `base`.
    pub fn grant_range(
        &mut self,
        at: SlotRef,
        kind: ObjectKind,
        base: u64,
        len: u64,
    ) -> Result<(), CapError> {
        let Some(limit) = kind.range_limit() else {
            return Err(CapError::NotARange);
        };
        if len == 0 || base.checked_add(len).is_none_or(|end| end > limit) {
            return Err(CapError::OutOfRange);
        }
        let index = self.empty(at)?;
        let cap = Capability::original(Object { kind, target: Target::Range { base, len } });
        self.state.slots[index].put_unlinked(cap);
        Ok(())
    }

    // ---- Moving and deleting what has no relatives ----

    /// Deletes the capability in `at` if its slot is not linked, and its
    /// object with it if that was the object's last capability (and
    /// nothing retyped from it is left), which `released` hears of.
    pub fn delete(
        &mut self,
        at: SlotRef,
        released: &mut dyn FnMut(Released, &mut K),
    ) -> Result<Fast<()>, CapError> {
        let index = self.full(at)?;
        if self.state.slots[index].marks.linked {
            return Ok(Fast::Linked);
        }
        let taken = self.state.slots[index].take_unlinked();
        self.stale |= taken.cached;
        if let Some(id) = taken.cap.as_ref().and_then(Capability::id) {
            self.store.objects.lock().release(id, released);
        }
        Ok(Fast::Done(()))
    }

    /// Moves the capability in `from` to the empty slot `to` of the same
    /// space, if its slot is not linked. The flags stay with the slot.
    pub fn move_cap(&mut self, from: SlotRef, to: SlotRef) -> Result<Fast<()>, CapError> {
        let src = self.full(from)?;
        let dst = self.empty(to)?;
        if src == dst {
            return Err(CapError::Occupied);
        }
        let (src, dst) = two_slots(&mut self.state.slots, src, dst);
        Ok(move_unlinked(src, dst, &mut self.stale))
    }

    // ---- Internals ----

    /// The index of `at` in this space.
    fn index(&self, at: SlotRef) -> Result<usize, CapError> {
        if at.space != self.id {
            return Err(CapError::NoSuchSpace);
        }
        if at.slot as usize >= self.state.slots.len() {
            return Err(CapError::NoSuchSlot);
        }
        Ok(at.slot as usize)
    }

    /// The index of `at`, which must hold a capability.
    fn full(&self, at: SlotRef) -> Result<usize, CapError> {
        let index = self.index(at)?;
        if self.state.slots[index].cap.is_none() {
            return Err(CapError::Empty);
        }
        Ok(index)
    }

    /// The index of `at`, which must be empty.
    fn empty(&self, at: SlotRef) -> Result<usize, CapError> {
        let index = self.index(at)?;
        if self.state.slots[index].cap.is_some() {
            return Err(CapError::Occupied);
        }
        Ok(index)
    }
}

/// Moves the capability in `src` to the empty slot `dst`, if `src` is
/// not linked: a move that touches no other slot. Moving a capability a
/// cache may hold marks the store stale.
pub(super) fn move_unlinked(src: &mut Slot, dst: &mut Slot, stale: &mut bool) -> Fast<()> {
    if src.marks.linked {
        return Fast::Linked;
    }
    let taken = src.take_unlinked();
    *stale |= taken.cached;
    if let Some(cap) = taken.cap {
        dst.put_unlinked(cap);
    }
    Fast::Done(())
}

/// Makes a new object of `kind` in `objects`, and has `record` record it
/// in the registry in the same hold of the table's lock; unmakes it if
/// `record` fails.
pub(super) fn make_recorded<A: Allocator, K, T>(
    objects: &mut Objects<A, K>,
    kind: ObjectKind,
    record: impl FnOnce(ObjectId, &mut K) -> Result<T, CapError>,
) -> Result<(ObjectId, T), CapError> {
    let id = objects.insert(Entry::new(kind, None, None))?;
    match record(id, &mut objects.registry) {
        Ok(recorded) => Ok((id, recorded)),
        Err(err) => {
            objects.unmake(id);
            Err(err)
        }
    }
}
