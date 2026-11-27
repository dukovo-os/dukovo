<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0548. A request of little urgency goes at the default while its thread holds a lock another waits for

- Status: Accepted
- Date: 2026-11-23

## Context

A disk request takes room by its process's I/O class and level (ADR 0093):
the idle class only once no other request waits or is in flight in any
lane, and each best-effort level only while no more urgent one waits.
While domains have disk weights, the requests that wait for their turn go
by the same rank first, then by weight (ADR 0476). A disk that never
empties therefore never serves the idle class, and one whose queue is
always full of the default level never serves levels 5 to 7.

That would only slow the work that asked for it, but the kernel's locks
pass the wait on. Btrfs holds its transaction's lock through every read of
its trees and of a file's data, writes too, so that a change cannot edit a
block under a reader. A thread of the idle class that reads a file holds
the lock while its request waits, and every other thread that opens,
reads or writes a file of that filesystem waits for the lock behind it.

The lab's `storage/io-weight` test showed it on a laptop. Three services
read its NVMe disk raw, 48 threads in all, so that it never emptied. The
start had proved the release, so Rescue's rewrite began (ADR 0543), which
reads the running slot's files at idle priority. From then on nothing
that touched the root's filesystem went on: Spark logged nothing more,
`service status` never answered, and the test ran out its time, while the
kernel went on dropping packets at the firewall.

Linux's BFQ scheduler serves its idle class after `BFQ_CL_IDLE_TIMEOUT`,
a fifth of a second without serving it, and mq-deadline serves a request
of a lower priority once it has waited `prio_aging_expire`, ten seconds.
Linux's own filesystem locks are finer than one per filesystem, so the
inversion is narrower there, but it is known there too.

## Decision

- **A lent rank.** A request less urgent than the default, the idle
  class's or best effort's at level 5 to 7, goes at the default,
  best-effort level 4, while its thread holds a mutex that another thread
  waits for (`sched::wait::holds_wanted_lock`, from the scheduler's table
  of waiters). The thread waiting for the lock waits for the request too,
  so the request goes at least as a request of a process that set nothing.
  The kernel's own threads ask at that rank already.
- **A grace for the idle class.** An idle-class request that has waited a
  fifth of a second for the disk to be free goes at the default too, as
  BFQ has it, so that the idle class is never shut out for as long as a
  disk is busy. Best effort's levels keep their order without a grace.
- **Looked at while waiting.** A request less urgent than the default
  that waits, for room in its lane or for its turn among the domains'
  shares, looks again every 10 ms at the rank it is lent, as a thread may
  come to wait for its lock after it began to wait, and moves to that
  rank's count, or its domain's share at that rank, when it changes.
- **A boot test, `idle-reader-lock`.** Two services of different weights
  read a throttled virtio disk raw, so that it never empties; beside an
  idle-class `cat` of a file on the disk's Btrfs, a best-effort `cat` of
  another ends within 30 seconds, and the idle-class one within two
  minutes.

## Consequences

- A program at idle priority can no longer hold up the filesystems it
  reads, whatever the disk's load; Rescue's rewrite and an `ionice -c idle`
  backup are safe beside a busy disk.
- An idle-class request on a disk that stays busy waits a fifth of a
  second, then competes as the default: a busy disk gives the idle class
  a few requests a second for each of its threads.
- The rank lent is the default, not the waiter's own: a real-time waiter
  behind such a lock waits as long as the default's requests do. Lending
  the waiter's own rank is a later change, which the waiter table already
  allows.
- Requests less urgent than the default that wait wake every 10 ms: a few
  hundred wake-ups a second on a busy disk, for work that is meant to be
  slow.
