<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0656. A capability leaving its slot empties only the caches of slots that share its word

- Status: Accepted
- Date: 2026-11-24

## Context

Each CPU keeps the capabilities its system calls found, sixteen of
them, each with the store's version number from before the lookup
(ADR 0077). The version was one number for the whole store, and it
moved whenever a capability some CPU had found left its slot, and again
when the object of one left the store. ADR 0077 left it there, and named
a version for each space as the next step if programs that open, use and
close descriptors in a loop showed that it mattered.

Servers are such programs. nginx accepts a connection, PHP-FPM accepts
the one nginx makes to it, and both close theirs when the request is
done; PostgreSQL's backend and its client do the same with what each
transaction opens. Each of those closes emptied every CPU's cache, so
the next system call of every program on every CPU, the listening
socket's `poll` and the pipes' reads included, went back to the store
and took its space's lock. A count of instructions under QEMU
(`-icount`) found 26 cache misses in each PostgreSQL transaction and
about 2,500 instructions for each descriptor a `poll` looked at, much
of it in those misses; on the OptiPlex, PHP-FPM behind nginx took 1.14
times Linux's time and a TPC-B transaction 1.28 times, after the work of
ADR 0650 to 0655.

## Decision

- **Each space has sixteen version words** (`radix_cap::SLOT_VERSIONS`),
  and a slot uses the one its number picks, modulo sixteen. Taking a
  marked capability out of a slot (a delete, a move, a revoke, or its
  space destroyed) moves that slot's word, under the space's lock, as
  the store's version moved before. A program's descriptors are mostly
  numbered below sixteen, so a close takes from the caches only what
  they kept of that descriptor, and nothing of any other program's.
- **The words live beside the space's lock** in the space table's
  blocks, which are made once and never move (ADR 0369), on cache lines
  apart from the lock, so a CPU's cache entry keeps a reference to its
  slot's word and checks it with one load (`SlotVersion::get`). Words
  are 64 bits, so no word comes back to a value an entry holds.
- **An object leaving the store moves nothing.** It leaves only once
  its last capability has left its slot, and a cache holds an object
  only behind a slot it found it in, whose word moved then; the kernel's
  mark on objects handed to a cache, and the version it moved, are gone.
- `/system/kernel/counters`' `cap_cache_flushes` is the sum of every
  word: how many times a capability a cache held left its slot.

## Alternatives

- **One version for each space.** A close in one program would leave
  the others' caches alone, but still empty every entry of its own: a
  PHP-FPM worker would look its listening socket and its pipes up again
  after every request it closes. Sixteen words cost 128 bytes a space,
  32 KiB for each block of 256 spaces.
- **A version in each slot.** The slots of a space move when it grows,
  so a CPU could not read one without the space's lock; a word per slot
  kept apart from the slots would be as many words as slots, made and
  given back as spaces grow and shrink.
- **Emptying the matching entry of each CPU's cache** when a
  capability leaves its slot. Each CPU's cache is its own, touched with
  preemption off and no lock (ADR 0040); another CPU writing into it
  would need a lock or an interrupt on every close.

## Consequences

- A close empties the caches only of slots in the same space whose
  number has the same remainder modulo sixteen. The unit tests check
  that a cache kept fresh by the words is never wrong on random
  sequences of every operation, and that a departure leaves other
  spaces' words and the space's other words where they were.
- Each space takes 128 bytes more, on two cache lines of its own.
