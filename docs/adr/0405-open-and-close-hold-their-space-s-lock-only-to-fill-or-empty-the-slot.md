<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0405. Open and close hold their space's lock only to fill or empty the slot

- Status: Accepted
- Date: 2026-11-07

## Context

Threads of one process share its capability space, so every open and
close they make takes that space's lock (S, ADR 0369). After the C
library kept its own descriptor table (ADR 0403) and processes stopped
sharing lines (ADR 0404), four threads opening and closing files on four
CPUs still took 1.68 times Linux's time on CI's KVM runner (1,231 against
733 ns per open and close), and eight took 1.72. Each open and close
found S taken 1.13 times.

Under QEMU (4,000 opens and closes of the 4-thread row, main at
acd7ea1), the threads passed 15.1 cache lines between them per open and
close:

| Line | Moves per op | Why it is shared |
| --- | ---: | --- |
| S's word and the first fields it guards | 6.4 | Threads waiting for S read its word and take the line from the thread that holds it |
| Object records | 3.0 | The record one thread's close frees is the next one another thread's open takes, from the space's spare indexes |
| The rest of the space's state | 1.9 | The spare indexes and the slot table, written under S |
| The process's account | 1.9 | Every open charges it and every close gives back, from all four CPUs |
| The kernel's mutex counters | 1.4 | Global counters, bumped on every contended mutex |

And S is held long. Of the 4,435 instructions of one open and close,
about 900 run with S held: about 200 to open, and about 700 to close.
The close's include taking the object's record lock and giving its index
back (about 100), and closing the file, freeing its memory and giving
back its charge (about 380), none of which needs S: the store runs its
`released` callback with the operation's locks held.

Linux holds its descriptor table's lock (`files->file_lock`) only to
pick a descriptor and to empty one; the file is made before and put
after (`fd_install`), and charged to its memory cgroup from a stock kept
on each CPU.

## Decision

**A fast-path create makes its object before it takes S.** The record
is taken from a spare index and filled in (the kernel's `record`
callback, which puts the file object there) with no lock but the
record's own, counting the capability about to be placed. Then S is
taken only to check that the slot is empty, place the capability and set
the slot's flags. Nobody can name the object before its capability is
in the slot, so nobody can find it without its object, as before. If the
slot was full, the record is given back after S has gone.

**A fast-path delete gives back its capability's count after S.** S is
held only to check the slot and take the capability out. Then the
record's count goes down under the record's lock, and if that was the
object's last capability the object is released: the `released`
callback runs with no lock of the store's held, and the kernel closes
the file, frees it and gives back its charge there.

So while a fast-path create or delete is under way, an object's count
may stand one above the capabilities that name it, never below; a
record is free only once nothing names its object. Once no operation is
under way every count is exact, and the audit, which already needs a
store nobody changes meanwhile, still checks them exactly. The slow path
and revokes keep changing counts inside their steps, with their locks
held, as before. `kernel/cap/tla/CapabilityLocks.tla` now models each
record's count and the fast path's two steps on either side of S: it
checks that every count is the capabilities naming its object plus the
operations under way that hold one (`CountsAccounted`), and that a free
record's object is named by nothing (`FreeRecordsNameNothing`), beside
every invariant of the tree it checked before.

**Spare object indexes are kept for each CPU, not for each space.** Each
CPU keeps up to eight, on a line of their own, which only that CPU
touches, with interrupts off (`radix_sync::cpu::HeldCpu`, whose two
functions the kernel defines and fat LTO inlines, as with the mount
counts of ADR 0404); it fills and empties them from the table's list
under the table's lock (O), four at a time, as each space did. A record
freed on a CPU is the next that CPU makes, so its lines stay on that
CPU. Host builds, whose threads may share a CPU number, take a spin lock
around each CPU's spares instead.

**S's word has a line of its own,** apart from the state it guards, so
threads waiting for S no longer take that state's line from the holder;
and the kernel's mutex counters are kept for each CPU and added up when
read.

**Each thread charges its process's account from a stock of its own.**
A thread opening a file takes the file's bytes from its stock, charging
the account a batch (1 KiB) more only when the stock runs out; closing a
file of its own process puts the bytes back in the stock, giving the
account back all but a batch once the stock would hold more than two. A
thread gives its stock back when it ends. The account still counts
every byte charged, its threads' stocks among them, and its domain the
pages for them, with up to two batches more for each thread; a charge
whose object is closed by another process, or by the kernel, goes back
to the account itself, as now.

## Consequences

- An open holds S for the slot's check, the capability and its flags,
  and a close for the slot's check and taking the capability: about a
  sixth of the 900 instructions before. A record and a thread's stock stay
  on their CPU, and S's word and the counters stop moving the lines
  beside them.
- A fast-path create that finds its slot full has made and unmade its
  object for nothing; the C library picks free descriptors (ADR 0403),
  so that is rare.
- `released` runs with no store lock held after a fast-path delete, and
  with the operation's locks held on the slow path and in revokes. The
  kernel's callback works either way: it never calls back into the
  store.
- An object's count, read while a fast path is under way, may be one too
  many for a moment. Nothing decides by a count but the release of the
  record, which happens only when it reaches zero.
- A process's account may hold up to two batches (2 KiB) more for each
  of its threads, given back when the thread ends; `/processes`'s
  `kernel_memory` counts them.
- If threads still take longer than Linux's after this, the next step is
  a lock for each slot, so that open and close take no space-wide lock
  at all (the plan's PR 4).
