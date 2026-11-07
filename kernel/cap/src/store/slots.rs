// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a slot holds, in two parts under two locks (ADR 0369).
//!
//! - A [`Slot`] lives in its space ([`SpaceState`]) under that space's
//!   lock: the capability, the slot's flags, the cache mark and the
//!   `linked` bit.
//! - A [`Tree`] lives in the [`Forest`] under the tree lock: the slot's
//!   links (parent, first child, siblings), its depth and its revoke mark.
//!
//! A slot whose `linked` bit is clear has no parent and no children, and
//! its [`Tree`] is [`Tree::NONE`]: no link leads to it, and nothing in the
//! forest needs changing to delete or move its capability. That is what
//! lets the fast path work with its space's lock alone, and why it never
//! even sees the forest.

use alloc::vec::Vec;
use core::alloc::Allocator;

use crate::cap::Capability;

/// A slot of a space by index, inside the store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Link {
    pub(super) space: u32,
    pub(super) slot: u32,
}

/// A slot's place in the derivation tree, under the tree lock. A slot
/// that is not linked keeps [`Tree::NONE`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Tree {
    pub(super) parent: Option<Link>,
    pub(super) child: Option<Link>,
    pub(super) prev: Option<Link>,
    pub(super) next: Option<Link>,
    /// More than the parent's depth; at most [`super::MAX_DEPTH`].
    /// Deleting a capability gives its children a parent nearer the root,
    /// so this is an upper bound on the distance to a root. 0 in a slot
    /// that is not linked, which is a root.
    pub(super) depth: u8,
    /// A revoke of this capability is under way. Deriving reads it on
    /// every capability above the source, in any space. Only a linked
    /// slot is marked: a revoke of a capability with no relatives has
    /// nothing to do, and the slot it marks stays linked until it ends.
    pub(super) revoking: bool,
}

impl Tree {
    pub(super) const NONE: Tree =
        Tree { parent: None, child: None, prev: None, next: None, depth: 0, revoking: false };
}

/// What a slot keeps beside its capability, under its space's lock.
#[derive(Clone, Copy, Debug)]
pub(super) struct Marks {
    /// The slot's own flags (Phase 8), which stay with the slot rather
    /// than the capability: the kernel's close-on-exec. Clear in an empty
    /// slot, and in one a capability was just put in.
    pub(super) flags: u8,
    /// Its capability may be in a cache outside the store: it was looked
    /// up to be cached. Taking it out of the slot then moves the store's
    /// version.
    pub(super) cached: bool,
    /// The slot may have a parent or children. Clear, it has neither (nor
    /// siblings, which only children have), and its [`Tree`] is
    /// [`Tree::NONE`], so its capability can be deleted or moved with
    /// only its space's lock. Set on both slots when a capability is
    /// derived, carried by a move, and cleared, with the tree lock held,
    /// when the slot is emptied or left alone in the tree. It may stay set
    /// on a slot whose last relative was in another space, which only
    /// costs the slow path.
    pub(super) linked: bool,
}

impl Marks {
    pub(super) const NONE: Marks = Marks { flags: 0, cached: false, linked: false };
}

/// One slot's part under its space's lock.
#[derive(Clone, Copy, Debug)]
pub(super) struct Slot {
    pub(super) cap: Option<Capability>,
    pub(super) marks: Marks,
}

impl Slot {
    pub(super) const EMPTY: Slot = Slot { cap: None, marks: Marks::NONE };

    /// Empties this slot, which is not linked, and returns what was in
    /// it: the start of a delete or a move of a capability with no
    /// relatives, which touches no other slot.
    pub(super) fn take_unlinked(&mut self) -> Taken {
        debug_assert!(!self.marks.linked, "taking a linked capability without the tree");
        let taken = Taken { cap: self.cap.take(), cached: self.marks.cached };
        self.marks = Marks::NONE;
        taken
    }

    /// Puts `cap` in this empty slot, with no relatives: a new object's
    /// original capability, or the end of a move.
    pub(super) fn put_unlinked(&mut self, cap: Capability) {
        // Only an empty slot, which no cache can hold, takes a capability.
        debug_assert!(self.cap.is_none(), "placing a capability in a full slot");
        self.cap = Some(cap);
        self.marks = Marks::NONE;
    }
}

/// Two different slots of one space, `slots[a]` and `slots[b]`, borrowed
/// apart so both can be written.
///
/// # Panics
///
/// If `a` and `b` are the same slot, or either is out of range.
pub(super) fn two_slots(slots: &mut [Slot], a: usize, b: usize) -> (&mut Slot, &mut Slot) {
    assert_ne!(a, b, "one slot taken twice");
    let (low, high) = slots.split_at_mut(a.max(b));
    if a < b { (&mut low[a], &mut high[0]) } else { (&mut high[0], &mut low[b]) }
}

/// What [`Slot::take_unlinked`] took out of a slot.
pub(super) struct Taken {
    pub(super) cap: Option<Capability>,
    /// A cache outside the store may hold the capability.
    pub(super) cached: bool,
}

/// A capability space, under its own lock.
///
/// It starts a cache line of its own, so the lock's word, before it, has
/// one too (ADR 0405): threads waiting for the lock read the word, and
/// would otherwise take this line from the thread that holds the lock
/// and reads it.
#[repr(align(64))]
pub(super) struct SpaceState<A: Allocator> {
    /// Moves on when the space is destroyed, so its old name stays wrong.
    pub(super) generation: u32,
    pub(super) live: bool,
    pub(super) slots: Vec<Slot, A>,
}

/// The tree part of every slot, by space and slot, and the list of free
/// space indexes: what the tree lock guards.
pub(super) struct Forest<A: Allocator> {
    pub(super) spaces: Vec<Trees<A>, A>,
    /// The first free space index.
    pub(super) free: Option<u32>,
}

/// The tree parts of one space's slots, one per slot.
pub(super) struct Trees<A: Allocator> {
    pub(super) slots: Vec<Tree, A>,
    /// The space's generation, as in its [`SpaceState`]: both change
    /// together, with both locks held, so a link into the space can be
    /// named with the tree lock alone.
    pub(super) generation: u32,
    /// The next free space index, while this one is free.
    pub(super) next_free: Option<u32>,
}

impl<A: Allocator> Forest<A> {
    pub(super) fn tree(&self, link: Link) -> &Tree {
        &self.spaces[link.space as usize].slots[link.slot as usize]
    }

    pub(super) fn tree_mut(&mut self, link: Link) -> &mut Tree {
        &mut self.spaces[link.space as usize].slots[link.slot as usize]
    }

    /// For the audit, which must not panic on a bad link.
    pub(super) fn try_tree(&self, link: Link) -> Option<&Tree> {
        self.spaces.get(link.space as usize)?.slots.get(link.slot as usize)
    }
}
