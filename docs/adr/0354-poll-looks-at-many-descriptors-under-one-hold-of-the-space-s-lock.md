<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0354. Poll looks at many descriptors under one hold of the space's lock

- Status: Accepted
- Date: 2026-11-21

## Context

`poll` on 1,001 pipes with one ready (`posix_poll_idle`, ADR 0353) took
80.5 µs on CI's runners against Linux's 6.36 µs, 12.7 times, and counted
under QEMU (`ob bench --instructions`) it ran 648,728 instructions, about
645 a descriptor. The row was reported but not gated until parity.

`poll` looked each descriptor up with `objects::find_object`. The CPU's
capability cache (ADR 0369) has 16 entries, indexed by slot, so past 16
descriptors nearly every look-up missed and filled its entry: the space's
mutex, the record's spin lock, a counted clone of the object, a weak
reference for the cache with the old one dropped, and the clone dropped
once the look was done. That is six to eight locked read-modify-writes and
two lock round trips a descriptor, and on x86 each is a full barrier, so
the look at one descriptor could not overlap the next.

Linux's `do_poll` indexes the descriptor table without counting a
reference while the table is not shared, and asks the file for its
events: no read-modify-write, and the loads of one descriptor overlap
the next's.

## Decision

**A capability keeps its object, and its space's lock keeps the
capability.** The store releases an object only once its last capability
has left its slot, and only a holder of a space's lock empties one of its
slots (the fast path, the slow path and each step of a revoke). So a
holder of space S's lock that finds a capability naming object X in a
slot of S knows that X lives, and that what the kernel keeps of X stays
in X's record, until it lets S go.

- **The store keeps where each object is.** `radix_cap` asks what it
  keeps of an object for an address (the trait `Located`), and its record
  holds it in an atomic word: set with the record locked before the first
  capability naming the object is placed, and cleared once the last has
  left its slot. A locked space's view reads it for a slot
  (`SpaceView::address`) without the record's lock. The store treats the
  address as an opaque word and stays free of `unsafe` code; its audit
  checks every record's address against what it keeps.
- **The kernel's address says what the object is.** The kernel's kept
  object (`Recorded`) gives the address of its file, notification or
  event queue, with the kind in the address's low three bits, or a kind
  alone for a directory or anything else. The kind comes from the kept
  object itself, never from the capability's kind, so a capability whose
  kind and object disagree cannot make one object read as another.
  `objects::peek` turns the address back into a reference, `&FileObject`,
  `&Notification` or `&EventQueue`, whose lifetime is the locked view's:
  three marked `unsafe` dereferences, each sound by the rule above.
- **`poll` of more than 16 descriptors looks at them in rounds, each
  under one hold of the space's lock** (`poll::look_space`). It peeks at
  each slot and asks the object for its events where that takes only
  atomics and spin locks: a pipe's end, a Unix socket, a terminal's side,
  a plain file or directory, a notification, an event queue. An object
  whose look may take a sleeping lock (a network socket, a device, the
  kernel log, a FUSE file) is held instead, with a counted reference
  taken under its record's lock (`objects::find_locked`), up to eight a
  round; the round then lets the space go, looks at those, and the next
  round goes on from the next entry. A space gone with its process makes
  every entry left invalid, as an empty slot is. A poll that waits holds
  every object it waits on the same way, under one hold of the space's
  lock.
- **Up to 16 descriptors keep the cache.** The cache's hits take no lock
  shared with other threads of the process, so a small poll keeps the
  per-descriptor look-up.

Locks, outermost first: the space's mutex, then a record's spin lock (in
`find_locked` only), as the store's order already is. Under the space's
mutex a look reads atomics and takes spin locks only (a Unix datagram
socket's connection), and allocates nothing; a sleeping lock is taken
only with the space let go, so a look adds no lock to the space's
order.

## Consequences

- `poll` on the 1,001 pipes takes no read-modify-write a descriptor, and
  the loads of one overlap the next's. Counted under QEMU it runs
  COUNT instructions, from 648,728. The row is gated like the rest from
  this change on, and `docs/deferred.md` no longer lists it.
- `select` and `pselect` gain the same, as the C library builds them on
  `poll`.
- Other threads of a process wait for the space's lock while one of
  them polls more than 16 descriptors: a round holds it for at most
  1,001 looks. A poll of more than 16 network sockets takes the lock
  once for every eight sockets, and each socket's look as before.
- The store's record grows by one word, and every insert, unmake and
  release writes it.
- The kernel crate gains a marked `unsafe` site, the peek's three
  dereferences.
- The posix boot test's `poll` program polls 45 entries that mix every
  kind of descriptor above, ready and not, past and within the table,
  and waits on 21 of them for a pipe and then a socket another process
  makes ready.

## Alternatives considered

- **A bigger capability cache.** One of 1,024 entries would cost each
  CPU 32 KiB, and a hit still takes a counted reference and drops it, two
  locked read-modify-writes a descriptor.
- **A counted reference to every object under one hold of the lock.**
  It saves the lock round trips, but each object's count is still taken
  and dropped: by the same measure about twice Linux's time.
- **The space's lock held while every object is looked at.** A network
  socket's look takes its state's and its stack's mutexes, which sleep:
  the space's mutex would become an outer lock of the network stack's,
  an order every path into the stack would have to keep, and the space
  would stay locked while a look waits.
- **The capability's kind to say what its object is.** It would need the
  store to keep the two in step forever; the kept object says what it is
  by itself.
