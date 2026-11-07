<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0403. The C library keeps its own table of file descriptors

- Status: Accepted
- Date: 2026-11-07

## Context

POSIX gives each new file descriptor the lowest number that is free.
On Radix, descriptor _n_ is capability slot 64 + _n_, and the C library
chooses the number: it asked the kernel which slots were full
(`process.slots_used`, 64 slots a call), claimed the lowest empty one so
that no other thread of the program chose it too, and then made the call
that fills it. Every open, socket, pipe, dup and accept made that extra
system call first.

That call was about 1,450 of the 5,770 instructions of an `open` and
`close` from C, and it is most of why one worker of `ob bench
--scaling` took 1.30 times Linux's time (the plan for making every
scaling row beat Linux, 2026-11-06). Linux finds the lowest free
descriptor in a bitmap the kernel keeps per process, with no second
trip.

Two ways were weighed. The kernel could choose the slot, as Linux's
does, which changes the calls of every object that makes a descriptor
and their ABI. Or the C library could keep the table it was already
half keeping, its bits of claimed descriptors, and make them a bit per
taken descriptor. Chase chose the second, the C library's own table, on
2026-11-07.

## Decision

- The C library keeps a bit for each descriptor, set while it is taken
  (`radix::FdTable`). An open finds the lowest clear bit from its
  lowest allowed number, sets it with one atomic operation, and makes
  the call that fills the slot: one system call. A bit set by another
  thread first is skipped, so two threads never choose the same number.
  Close clears it once the kernel has emptied the slot.
- The table is read from the kernel's slots (`slots_used`) when the
  program starts, before its constructors run, and in a child after
  `fork`, which has the parent's descriptors but none of its other
  threads. A program starts with what `exec` left it, the descriptors
  without close-on-exec.
- The table also holds how many slots the capability space has. A
  number past them grows the space first (`process.grow`, which takes
  the capability tree's lock), as the old path did when every slot was
  full.
- `dup2` marks its number taken; descriptors received with
  `SCM_RIGHTS` are marked taken in the numbers the kernel chose for
  them; `pipe` and `socketpair`, which fill two slots in one call,
  keep each number the kernel says is full when the call answers
  `occupied`.
- The kernel stays the judge of what a slot holds. A capability that
  reaches a free slot some other way than through the C library makes
  the call that would fill it answer `occupied`; the C library then
  marks that number taken and chooses the next, as it did when two
  threads raced. No number is handed out twice.
- The dynamic linker opens files too, the libraries a program needs and
  `dlopen`'s, with its own copy of the system layer. A program that has
  one shares one table: the dynamic linker defines it
  (`__radix_fd_table`, exported), and the shared C library uses it. The
  dynamic linker reads it at its first open, before the C library has
  started. A static program, as every system program is, has one copy.
- Its bits are set and cleared with atomic operations alone, with no
  lock, so `open` and `close` stay async-signal-safe.

## Consequences

- An `openat` and `close` from C, as one worker of the scaling
  benchmark makes them, takes about 4,360 instructions instead of 5,770,
  counted by QEMU: a quarter fewer. The bit operations add about ten
  instructions in the C library; the kernel's `slots_used` call is gone.
- A capability emptied from outside the program, such as one revoked,
  stays taken in the table until the program closes its descriptor.
  Until then, new descriptors are the lowest free numbers after it.
  POSIX has no such case: a Linux descriptor is never emptied but by
  the program.
- A number taken some other way costs one failed call the first time
  the C library tries it, then none.
- Nothing in the kernel or the ABI changes: programs that use the
  capability slots directly, through libradix, keep doing so, and the C
  library finds their slots as it finds any other.
- radix-posix(7) describes the table.
