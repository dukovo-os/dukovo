// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The object table: an entry for each kernel object, with the counts that
//! say when it goes.
//!
//! Every space shares the table, so it has a lock of its own (O in ADR
//! 0369), taken after the tree's and the spaces' and only around a short
//! step. Nothing here looks at a slot.

use alloc::vec::Vec;
use core::alloc::Allocator;

use super::{Extent, Released, bump};
use crate::cap::Capability;
use crate::error::CapError;
use crate::object::{ObjectId, ObjectKind};

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

#[derive(Clone, Copy, Debug)]
pub(super) struct Record {
    pub(super) generation: u32,
    pub(super) entry: Option<Entry>,
    /// Next free record, while this one is free.
    next_free: Option<u32>,
}

/// The table itself, which the store keeps behind its lock.
pub(super) struct Objects<A: Allocator> {
    pub(super) records: Vec<Record, A>,
    /// The first free record.
    free: Option<u32>,
}

impl<A: Allocator> Objects<A> {
    pub(super) fn new(alloc: A) -> Self {
        Objects { records: Vec::new_in(alloc), free: None }
    }

    /// Counts one more capability to the object `cap` names, if any.
    pub(super) fn retain(&mut self, cap: &Capability) {
        if let Some(id) = cap.id()
            && let Some(entry) = self.entry_mut(id)
        {
            entry.caps += 1;
        }
    }

    /// Drops a capability's hold on the object `id`, and the object if
    /// that was the last hold, and so on up the memory it was retyped from.
    /// `released` hears of each object that goes, before its record can be
    /// used again: whatever the kernel keeps beside the table at the same
    /// index is gone by then.
    pub(super) fn release(&mut self, mut id: ObjectId, released: &mut dyn FnMut(Released)) {
        let Some(entry) = self.entry_mut(id) else { return };
        entry.caps -= 1;
        loop {
            let Some(entry) = self.entry(id).copied() else { return };
            if entry.caps > 0 || entry.children > 0 {
                return;
            }
            let record = &mut self.records[id.index as usize];
            record.entry = None;
            record.generation = bump(record.generation);
            released(Released {
                kind: entry.kind,
                id,
                memory: if entry.origin.is_none() { entry.extent } else { None },
            });
            let record = &mut self.records[id.index as usize];
            record.next_free = self.free;
            self.free = Some(id.index);
            let Some(origin) = entry.origin else { return };
            let Some(parent) = self.entry_mut(origin) else { return };
            parent.children -= 1;
            if parent.children == 0 {
                parent.watermark = 0;
            }
            id = origin;
        }
    }

    pub(super) fn entry(&self, id: ObjectId) -> Option<&Entry> {
        let record = self.records.get(id.index as usize)?;
        if record.generation != id.generation {
            return None;
        }
        record.entry.as_ref()
    }

    pub(super) fn entry_mut(&mut self, id: ObjectId) -> Option<&mut Entry> {
        let record = self.records.get_mut(id.index as usize)?;
        if record.generation != id.generation {
            return None;
        }
        record.entry.as_mut()
    }

    /// Makes room for `count` more objects, counting the records on the
    /// free list and the spare capacity, so that as many calls to
    /// [`Objects::insert`] cannot fail.
    pub(super) fn reserve(&mut self, count: usize) -> Result<(), CapError> {
        let mut free = self.records.capacity() - self.records.len();
        let mut at = self.free;
        while let Some(index) = at {
            free += 1;
            at = self.records[index as usize].next_free;
        }
        if free < count {
            self.records.try_reserve(count - free).map_err(|_| CapError::NoMemory)?;
        }
        Ok(())
    }

    /// A new object with `entry`, in a free record if there is one.
    pub(super) fn insert(&mut self, entry: Entry) -> Result<ObjectId, CapError> {
        let index = match self.free {
            Some(index) => {
                let record = &mut self.records[index as usize];
                self.free = record.next_free.take();
                record.entry = Some(entry);
                index
            }
            None => {
                let index = u32::try_from(self.records.len()).map_err(|_| CapError::NoMemory)?;
                self.records.try_reserve(1).map_err(|_| CapError::NoMemory)?;
                let record = Record { generation: 1, entry: Some(entry), next_free: None };
                // Cannot fail: the capacity was reserved.
                let _ = self.records.push_within_capacity(record);
                index
            }
        };
        Ok(ObjectId { index, generation: self.records[index as usize].generation })
    }

    /// Takes back an object [`Objects::insert`] just made, whose original
    /// capability was never placed: nothing was told of it, so nothing is
    /// told of its going.
    pub(super) fn unmake(&mut self, id: ObjectId) {
        let Some(record) = self.records.get_mut(id.index as usize) else { return };
        if record.generation != id.generation || record.entry.is_none() {
            return;
        }
        record.entry = None;
        record.generation = bump(record.generation);
        record.next_free = self.free;
        self.free = Some(id.index);
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
        for _ in 0..self.records.len() {
            match origin {
                Some(above) if above == memory => return true,
                Some(above) => origin = self.entry(above).and_then(|entry| entry.origin),
                None => return false,
            }
        }
        false
    }
}
