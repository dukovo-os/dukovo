// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Kani proof harnesses for the fast path's work on slots (ADR 0369).
//!
//! The fast path cannot reach the derivation tree: a [`super::SpaceView`]
//! holds one space's slots and no part of the forest, so no link can
//! change through it, which the types show without a proof. What is left
//! to prove is that inside its space it acts only on the slots it names,
//! and only on slots with no relatives: a delete empties one slot, and a
//! move empties one and fills another, leaving every other slot of the
//! space as it was. A space of three slots is enough: the two a move
//! names and one it must leave alone. Each harness unwinds its loops at
//! most six times, which covers them, as Kani checks.

use super::fast::move_unlinked;
use super::slots::{Marks, Slot, two_slots};
use crate::cap::Capability;
use crate::object::{Object, ObjectId, Target};
use crate::proofs::any_kind;
use crate::rights::Rights;

/// Slots in the proofs' space.
const SLOTS: usize = 3;

/// Any capability, as far as these proofs care: the code under proof
/// moves a capability whole and never looks inside it, so its fields
/// need not make sense together, and leaving them unchecked keeps the
/// proofs fast.
fn any_contents() -> Capability {
    let object = Object {
        kind: any_kind(),
        target: Target::Object(ObjectId { index: kani::any(), generation: kani::any() }),
    };
    let rights = Rights::from_bits_truncate(kani::any());
    Capability { object, rights, badge: kani::any(), sealed_by: None }
}

/// Any slot: full or empty, with any marks.
fn any_slot() -> Slot {
    let cap = if kani::any() { Some(any_contents()) } else { None };
    let marks = Marks { flags: kani::any(), cached: kani::any(), linked: kani::any() };
    Slot { cap, marks }
}

/// Whether two slots hold the same capability and marks.
fn same(a: &Slot, b: &Slot) -> bool {
    a.cap == b.cap
        && a.marks.flags == b.marks.flags
        && a.marks.cached == b.marks.cached
        && a.marks.linked == b.marks.linked
}

/// Whether a slot is empty with clear marks, as emptying one leaves it.
fn blank(slot: &Slot) -> bool {
    slot.cap.is_none() && slot.marks.flags == 0 && !slot.marks.cached && !slot.marks.linked
}

/// A fast move of a capability with no relatives moves it from one slot
/// to the other, with no marks, and moves the store's version if a cache
/// may hold it. Of a linked capability it changes nothing and sends the
/// caller to the slow path. Either way, no other slot changes.
#[kani::proof]
#[kani::unwind(6)]
fn a_fast_move_changes_only_its_two_slots() {
    let mut slots: [Slot; SLOTS] = [any_slot(), any_slot(), any_slot()];
    let before = slots;
    let from: usize = kani::any_where(|&n| n < SLOTS);
    let to: usize = kani::any_where(|&n| n < SLOTS);
    // What `SpaceView::move_cap` checks first.
    kani::assume(from != to && slots[from].cap.is_some() && slots[to].cap.is_none());
    let was_stale: bool = kani::any();
    let mut stale = was_stale;

    let (src, dst) = two_slots(&mut slots, from, to);
    let done = move_unlinked(src, dst, &mut stale);

    for index in (0..SLOTS).filter(|&index| index != from && index != to) {
        assert!(same(&slots[index], &before[index]));
    }
    if before[from].marks.linked {
        assert!(matches!(done, super::Fast::Linked));
        assert!(same(&slots[from], &before[from]) && same(&slots[to], &before[to]));
        assert!(stale == was_stale);
    } else {
        assert!(matches!(done, super::Fast::Done(())));
        assert!(blank(&slots[from]));
        assert!(slots[to].cap == before[from].cap);
        assert!(slots[to].marks.flags == 0 && !slots[to].marks.cached && !slots[to].marks.linked);
        assert!(stale == (was_stale || before[from].marks.cached));
    }
}

/// A fast delete empties the one slot it names, which has no relatives,
/// and hands back its capability and whether a cache may hold it; no
/// other slot changes.
#[kani::proof]
#[kani::unwind(6)]
fn a_fast_delete_changes_only_its_slot() {
    let mut slots: [Slot; SLOTS] = [any_slot(), any_slot(), any_slot()];
    let before = slots;
    let at: usize = kani::any_where(|&n| n < SLOTS);
    // What `SpaceView::delete` checks first.
    kani::assume(slots[at].cap.is_some() && !slots[at].marks.linked);

    let taken = slots[at].take_unlinked();

    for index in (0..SLOTS).filter(|&index| index != at) {
        assert!(same(&slots[index], &before[index]));
    }
    assert!(blank(&slots[at]));
    assert!(taken.cap == before[at].cap);
    assert!(taken.cached == before[at].marks.cached);
}

/// Putting a new object's capability in an empty slot fills that slot
/// alone, with no marks: a new capability has no relatives, and no cache
/// can hold it yet.
#[kani::proof]
#[kani::unwind(6)]
fn a_new_capability_fills_only_its_slot() {
    let mut slots: [Slot; SLOTS] = [any_slot(), any_slot(), any_slot()];
    let before = slots;
    let at: usize = kani::any_where(|&n| n < SLOTS);
    kani::assume(slots[at].cap.is_none());
    let cap = any_contents();

    slots[at].put_unlinked(cap);

    for index in (0..SLOTS).filter(|&index| index != at) {
        assert!(same(&slots[index], &before[index]));
    }
    assert!(slots[at].cap == Some(cap));
    assert!(slots[at].marks.flags == 0 && !slots[at].marks.cached && !slots[at].marks.linked);
}
