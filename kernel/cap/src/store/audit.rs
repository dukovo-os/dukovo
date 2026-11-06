// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Counting what the store holds, and checking every invariant of it.
//!
//! Both hold the tree lock throughout and take each space's lock in turn,
//! then the object table's: the answer is exact for a store nobody else
//! changes meanwhile, as in the tests, a debugger stop or a stress test
//! between its rounds. Each space's slots are copied out under its lock,
//! so a check that crosses spaces never holds two.

use alloc::vec::Vec;
use core::alloc::Allocator;

use super::slots::{Link, Slot, Tree};
use super::{Capabilities, Census, Inconsistency, MAX_DEPTH, MAX_SLOTS, SlotRef, SpaceId};
use crate::object::{ObjectKind, Target};

/// One live space's slots, copied out under its lock.
struct Copied<A: Allocator> {
    generation: u32,
    slots: Vec<Slot, A>,
}

impl<A: Allocator + Clone, K> Capabilities<A, K> {
    /// Counts what the store holds.
    pub fn census(&self) -> Census {
        self.count(true).unwrap_or_default()
    }

    /// Counts what the store holds, as [`Capabilities::census`] does, but
    /// gives up rather than wait for a lock: for a debugger, which may have
    /// stopped the CPU that holds one.
    pub fn try_census(&self) -> Option<Census> {
        self.count(false)
    }

    /// The census, waiting for each lock if `wait`, or giving up (`None`)
    /// at the first one held elsewhere.
    fn count(&self, wait: bool) -> Option<Census> {
        let forest = if wait { self.tree.lock() } else { self.tree.try_lock()? };
        let mut census = Census::default();
        for index in 0..forest.spaces.len() as u32 {
            let Some(space) = self.space(index) else { continue };
            let state = if wait { space.state.lock() } else { space.state.try_lock()? };
            if !state.live {
                continue;
            }
            census.spaces += 1;
            census.slots += state.slots.len() as u64;
            for (number, slot) in state.slots.iter().enumerate() {
                if let Some(cap) = slot.cap {
                    let tree = forest.tree(Link { space: index, slot: number as u32 });
                    census.caps[cap.kind().index()] += 1;
                    census.deepest = census.deepest.max(tree.depth);
                    census.revoking += u32::from(tree.revoking);
                }
            }
        }
        let objects = if wait { self.objects.lock() } else { self.objects.try_lock()? };
        for entry in objects.records.iter().filter_map(|record| record.entry.as_ref()) {
            census.objects[entry.kind.index()] += 1;
            if entry.origin.is_none()
                && let Some(extent) = entry.extent
            {
                census.memory_pages += extent.pages;
            }
        }
        Some(census)
    }

    /// Checks every invariant of the store: the tree's links agree with
    /// each other, every slot with relatives is marked linked and every
    /// slot not linked has no place in the tree, depths grow down the tree,
    /// every capability grants no more than its parent, no capability
    /// holds a right its kind lacks or a badge its kind cannot carry, every
    /// capability names a live object of its kind, and every object's
    /// counts are right. Takes time in proportion to the store's size, and
    /// memory for a copy of every slot and two counts per object.
    pub fn audit(&self) -> Result<Census, Inconsistency> {
        let fail = |what, at| Inconsistency { what, at };
        let oom = || fail("out of memory for the audit", None);
        let forest = self.tree.lock();
        // Every space's slots, copied out one space at a time.
        let mut copies: Vec<Option<Copied<A>>, A> = Vec::new_in(self.alloc.clone());
        copies.try_reserve_exact(forest.spaces.len()).map_err(|_| oom())?;
        for (index, trees) in forest.spaces.iter().enumerate() {
            let state = self.lock_index(index as u32, 0);
            let copy = match state {
                Some(state) if state.live => {
                    if state.generation != trees.generation {
                        return Err(fail("a space's two generations differ", None));
                    }
                    if state.slots.len() != trees.slots.len() {
                        return Err(fail("a space's slots and tree parts differ in number", None));
                    }
                    let mut slots = Vec::new_in(self.alloc.clone());
                    slots.try_reserve_exact(state.slots.len()).map_err(|_| oom())?;
                    for &slot in &state.slots {
                        let _ = slots.push_within_capacity(slot);
                    }
                    Some(Copied { generation: state.generation, slots })
                }
                _ => {
                    if !trees.slots.is_empty() {
                        return Err(fail("a free space has slots", None));
                    }
                    None
                }
            };
            let _ = copies.push_within_capacity(copy);
        }
        let objects = self.objects.lock();
        let mut census = Census::default();
        let mut children_listed = 0u64;
        let mut non_roots = 0u64;
        let mut counts: Vec<(u32, u32), A> = Vec::new_in(self.alloc.clone());
        counts.try_reserve_exact(objects.records.len()).map_err(|_| oom())?;
        super::fill(&mut counts, objects.records.len(), || (0, 0));
        let slot_of = |link: Link| -> Option<&Slot> {
            copies.get(link.space as usize)?.as_ref()?.slots.get(link.slot as usize)
        };
        for (index, copy) in copies.iter().enumerate() {
            let Some(copy) = copy else { continue };
            census.spaces += 1;
            census.slots += copy.slots.len() as u64;
            let id = SpaceId { index: index as u32, generation: copy.generation };
            for (number, slot) in copy.slots.iter().enumerate() {
                let here = Link { space: index as u32, slot: number as u32 };
                let at = Some(SlotRef { space: id, slot: number as u32 });
                let tree = forest.tree(here);
                let Some(cap) = slot.cap else {
                    if *tree != Tree::NONE || slot.marks.linked {
                        return Err(fail("an empty slot has links", at));
                    }
                    continue;
                };
                let kind = cap.kind();
                census.caps[kind.index()] += 1;
                census.deepest = census.deepest.max(tree.depth);
                census.revoking += u32::from(tree.revoking);
                if tree.depth > MAX_DEPTH {
                    return Err(fail("a capability is deeper than MAX_DEPTH", at));
                }
                if !kind.rights().contains(cap.rights) {
                    return Err(fail("a capability holds a right its kind does not have", at));
                }
                if cap.badge != 0 && !kind.badged() {
                    return Err(fail("a capability carries a badge its kind cannot", at));
                }
                match cap.object.target {
                    Target::Object(id) => {
                        let Some(entry) = objects.entry(id) else {
                            return Err(fail("a capability names an object that is gone", at));
                        };
                        if entry.kind != kind || kind.is_range() {
                            return Err(fail("a capability names an object of another kind", at));
                        }
                        counts[id.index as usize].0 += 1;
                    }
                    Target::Range { base, len } => {
                        let limit = kind.range_limit();
                        if len == 0 || limit.is_none_or(|limit| base.saturating_add(len) > limit) {
                            return Err(fail("a capability has a bad range", at));
                        }
                    }
                }
                if let Some(sealer) = cap.sealed_by
                    && objects.entry(sealer).is_some_and(|entry| entry.kind != ObjectKind::Endpoint)
                {
                    return Err(fail("a capability is sealed by something not an endpoint", at));
                }
                // A clear bit lets a delete or a move skip the tree.
                if !slot.marks.linked && *tree != Tree::NONE {
                    return Err(fail("a slot that is not linked has a place in the tree", at));
                }
                match tree.parent {
                    None => {
                        if tree.prev.is_some() || tree.next.is_some() {
                            return Err(fail("a capability without a parent has siblings", at));
                        }
                    }
                    Some(parent) => {
                        non_roots += 1;
                        let (Some(above), Some(above_tree)) =
                            (slot_of(parent), forest.try_tree(parent))
                        else {
                            return Err(fail("a parent link leads nowhere", at));
                        };
                        let Some(parent_cap) = above.cap else {
                            return Err(fail("a capability's parent slot is empty", at));
                        };
                        if above_tree.depth >= tree.depth {
                            return Err(fail("a capability is no deeper than its parent", at));
                        }
                        if !cap.within(&parent_cap) && !objects.retyped_from(&cap, &parent_cap) {
                            return Err(fail("a capability grants more than its parent", at));
                        }
                    }
                }
                // The children list: each child names this slot as its
                // parent, and the list's links agree in both directions.
                let mut prev = None;
                let mut child = tree.child;
                let mut steps = 0u64;
                while let Some(c) = child {
                    let (Some(below), Some(below_tree)) = (slot_of(c), forest.try_tree(c)) else {
                        return Err(fail("a child link leads nowhere", at));
                    };
                    if below.cap.is_none()
                        || below_tree.parent != Some(here)
                        || below_tree.prev != prev
                    {
                        return Err(fail("a children list is inconsistent", at));
                    }
                    steps += 1;
                    if steps > u64::from(MAX_SLOTS) * copies.len() as u64 {
                        return Err(fail("a children list has a cycle", at));
                    }
                    prev = Some(c);
                    child = below_tree.next;
                }
                children_listed += steps;
            }
        }
        if children_listed != non_roots {
            return Err(fail("a capability is missing from its parent's children", None));
        }
        check_objects(&objects, &mut counts, &mut census)?;
        Ok(census)
    }
}

/// The object table's part of the audit: every retyped object lies in its
/// memory's used part, and every object's counts are those found.
fn check_objects<A: Allocator, K>(
    objects: &super::objects::Objects<A, K>,
    counts: &mut [(u32, u32)],
    census: &mut Census,
) -> Result<(), Inconsistency> {
    let fail = |what| Inconsistency { what, at: None };
    for record in &objects.records {
        if let Some(entry) = &record.entry
            && let Some(origin) = entry.origin
        {
            let Some(parent) = objects.entry(origin) else {
                return Err(fail("an object was retyped from memory that is gone"));
            };
            let (Some(own), Some(from)) = (entry.extent, parent.extent) else {
                return Err(fail("a retyped object has no extent"));
            };
            if own.base < from.base || own.base + own.pages > from.base + parent.watermark {
                return Err(fail("a retyped object lies outside its memory's used part"));
            }
            counts[origin.index as usize].1 += 1;
        }
    }
    for (record, &(caps, children)) in objects.records.iter().zip(counts.iter()) {
        let Some(entry) = &record.entry else { continue };
        census.objects[entry.kind.index()] += 1;
        if entry.origin.is_none()
            && let Some(extent) = entry.extent
        {
            census.memory_pages += extent.pages;
        }
        if entry.caps != caps || entry.children != children {
            return Err(fail("an object's counts are wrong"));
        }
        if caps == 0 && children == 0 {
            return Err(fail("an object outlived its capabilities"));
        }
        if entry.watermark > entry.extent.map_or(0, |extent| extent.pages)
            || (children == 0 && entry.watermark != 0)
        {
            return Err(fail("a memory object's watermark is wrong"));
        }
    }
    Ok(())
}
