<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0369. Each capability space has its own lock, and one lock guards the tree

- Status: Proposed
- Date: 2026-11-06

## Context

The capability store (`radix_cap::Capabilities`) and the object registry
sit behind one mutex for the whole kernel (`kernel/radix/src/caps.rs`,
`caps::with`). Every change to any process's capabilities takes it.

`ob bench --scaling` has 1, 4 and 8 workers open and close a file each
at once on 4 CPUs, as threads of one process or as separate processes.
With the mutex fixed (ADR 0368), the run of #225 (KVM, EPYC 7763) gave:

| Workers | Dukovo | Linux | Ratio |
| --- | --- | --- | --- |
| 1 thread | 776 ns | 622 ns | 1.25 |
| 4 threads | 4.68 µs | 1.37 µs | 3.43 |
| 8 threads | 9.30 µs | 2.71 µs | 3.43 |
| 4 processes | 3.63 µs | 1.15 µs | 3.17 |
| 8 processes | 6.94 µs | 2.29 µs | 3.03 |

The counts beside it show where the time goes. Each open and close found
the store's mutex taken 2.7 times with threads and 2.1 times with
processes, and almost never slept: the workers take turns at one lock,
with its cache lines moving between CPUs at every turn. 4 workers each
holding the lock for most of one worker's 776 ns gives about 3 µs, near
what was measured. Processes do no better than threads, though they
share nothing: Linux's processes each lock their own file table, which
is why its 4 processes cost 1.15 µs and not 4 times 622 ns.

An open (`user::files::install_file`) puts a new object's capability in
an empty slot of the process's space and sets the slot's flags; a close
deletes that capability. Neither follows or changes a link of the
derivation tree: the new capability is a root with no relatives, and a
file nobody duplicated or passed on is still one when it is closed.
Only deriving (including granting, retyping, sealing and the copies
`fork` makes), revoking, and deleting or moving a capability
with relatives touch the tree, and the tree crosses spaces: a capability
derived into another process's space is linked to its parent in the
first.

## Decision

The store gets one lock per space and one lock for the tree, and an
operation takes only those it needs.

**The locks.**

- Each space's lock (S) guards what its slots hold: the capability, the
  slot's flags, the cache mark, and a new `linked` bit.
- The tree lock (T) guards each linked slot's place in the tree: its
  links (parent, first child, siblings), its stored depth and its revoke
  mark, which deriving reads on every capability above its source, in
  any space.
- The object table and the registry keep a lock of their own (O), taken
  last and only around a short step. Step 3 below replaces it with counts
  kept in each entry and free lists per CPU.
- The table of spaces only grows, in blocks that never move, so finding
  a space by its index takes no lock. Creating, destroying and growing a
  space take T and that space's lock, since growing moves its slots.

**The order.** T, then space locks in increasing order of space index,
then O, then the heap's and the frame allocator's locks as before. A
thread holding a space's lock never waits for T. The second space lock
an operation takes is taken with `lock_nested(1)`, so lockdep sees two
subclasses of one class in one order.

**The `linked` bit.** A slot's bit is set or cleared only with its space
locked. A clear bit means the slot has no parent and no children (and so
no siblings).

- A derive (or grant, retype, seal or unseal) sets it on both slots.
- A move carries it to the new slot.
- Emptying a slot clears it. A delete also clears it on the parent, when
  the parent is in the same space and is left with no relatives: the
  common case of a copy made in one process and deleted again.
- It may stay set on a slot with no relatives, when the slot's last
  relative was in another space. That is safe: a set bit only sends an
  operation to the slow path, which may clear it there.

**The fast path** takes only the space's lock (the locks of both spaces,
lowest first, for a move between spaces):

- creating a capability for a new object in an empty slot;
- deleting, or moving to an empty slot, a capability whose slot has a
  clear bit and no revoke mark;
- looking up, and reading or setting slot flags.

It reads the bit with the space locked, so it acts on a slot nobody can
link meanwhile: linking a slot needs the same lock. It changes only its
own slots and follows no link. If it finds the bit set, it lets go and
starts again on the slow path.

**The slow path** takes T, then the locks of the spaces whose slots it
reads a capability from, fills or empties. Deriving in all its forms,
deleting or moving a linked capability, and destroying a space take it.

**Revoking** holds T through each turn (up to 64 deletions, as now) and
one space's lock at a time: the capability's, to set the revoke mark, to
look at it again at the start of each later turn (as `continue_revoke`
does) and to clear it; or a leaf's, to delete it. The walk down the tree
follows links under T alone. Between turns it lets go of everything, as
it lets go of the one lock today.

**Which lock guards which field.** Each slot keeps its fields in two
parts. The capability, flags, cache mark and `linked` bit are read and
written with the slot's space locked. The tree part (links, depth,
revoke mark) is read and written with T held while the slot is linked,
and with the space locked while it is not: no link leads to an unlinked
slot, so no thread holding only T reaches it, and the bit changes only
with the space locked. A revoke holds both when it sets or clears its
mark, and the slow path holds both for the slots it fills or empties.
The store gives out no `&mut` to a whole space or to the slots of more
than one.

**The cache version.** The version the per-CPU capability caches check
(ADR 0077) is already atomic. An operation that empties a cached slot
moves it before it lets go of that slot's lock, as it does before letting
go of the one lock today.

**The interface.** `caps::in_space(space, ..)` runs a fast-path
operation, and returns that the slot is linked when it is; the caller
then runs it again through `caps::with_tree(spaces, ..)`, which takes T
and the named spaces. `caps::with` keeps today's one lock until its
callers have moved.

**The model.** `kernel/cap/tla/CapabilityLocks.tla` takes the store's
operations from `Capabilities.tla` and runs them on threads that take
these locks: the fast path changing only its own slots, the slow path as
one step under its locks, and revokes in turns. TLC checks that all of
`Capabilities.tla`'s invariants and step properties still hold, that a
clear bit means no relatives, that a fast path acts only on an unlinked
slot whose space it holds and changes nothing else, that the leaf a
revoke deletes is still a leaf, and that the lock order holds, with
deadlock checking on.

**The steps** (from the plan of #215):

1. This ADR and the model (no kernel change).
2. A refactor with no change in behavior: the store's state split into
   what each space holds and what is shared, under the one lock still;
   the Kani proofs, fuzzing and property tests pass unchanged.
3. The object table: counts in each entry, free lists per CPU.
4. The split itself: the call sites move from `caps::with` to
   `caps::in_space` or `caps::with_tree`, the `linked` bit and the
   locks go in, `audit()` checks the bit, and new Kani proofs show the
   fast path never changes a link or a slot outside its own.
5. A boot test that derives, revokes, moves and deletes across processes
   on 4 CPUs with `audit()` between rounds; the benchmark run again; the
   design document updated; this ADR accepted.

## Consequences

- Opens and closes in different processes no longer take turns: each
  takes its own space's lock and, until step 3, the object table's
  briefly. Threads of one process still take turns at their space's
  lock, as Linux's threads do at their file table's.
- Deriving, revoking, and deleting or moving linked capabilities still
  take turns, at T, and take one or two locks more than now. They are
  rarer than opens and closes: `fork`, copies, passing capabilities over
  IPC, and revokes.
- A fast path that finds its slot linked takes its space's lock twice.
  A bit left set after a relative in another space went costs the same,
  once per operation on that slot.
- The store's code gets harder: slots with fields under two locks need
  interior mutability, which Kani and the audit must cover (step 4).
- Lockdep checks the order at run time; the TLA+ model checks that the
  order and the fast path's check are enough for the tree to stay whole.
