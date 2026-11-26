<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0777. A read that empties a pipe leaves its watches for reading alone

- Status: Accepted
- Date: 2026-11-26

## Context

A pipe tells its pollers and the event queues' watches of it about each
read and each write (`Polled`), and each watch it tells is put on its
queue's ring for the queue's next look (ADR 0353). A read that takes the
pipe's last byte puts the watches for reading on their rings too,
although a look then finds the pipe unreadable and reports nothing: a
watch for reading on a pipe's end is ready only while the pipe holds
bytes or no end writes it, and a read changes neither the writers nor,
once the pipe is empty, what it holds.

An event queue's wait for one ready pipe among a thousand idle ones
(`posix_events_idle`) writes a byte, looks, and reads it. The read took
the `Polled`'s lock, interrupts masked, to queue the watch, and the
write took it again to find the watch queued already. The row took 1.02
times Linux's time on the lab's OptiPlex 7040. Linux's pipes wake the
readers' wait queue, where epoll's watches for reading are, only on a
write, and the writers' only on a read that found the pipe full.

## Decision

- **A read that leaves a pipe empty while some end still writes it
  tells only the pollers and the watches for writing** (`Pipe::emptied`,
  `Polled::emptied_by_rmw`). It wakes the pipe's waits as every read
  does. The next write, or the last writer's going, which each tell
  every watch, makes the pipe readable again.
- **A `Polled` counts its watches for writing apart**, so that the read
  takes no lock while there are none and nobody polls.
- **Only a pipe of its own, plain or named, does so.** The pipes of a
  Unix socket's connection hold the socket's watches, which may be ready
  for what the socket's other pipe or its shutdowns say.
- What a look reports is unchanged: a read that leaves bytes is still a
  change an `EV_CLEAR` watch reports, and a read after the writer has
  gone still reports the end. The `events` POSIX test checks both.

## Alternatives

- **Not telling the watches for reading about any read, as Linux's
  epoll.** An `EV_CLEAR` watch would no longer be reported for a read
  that leaves bytes, which it is now: a change a program could see.
- **Checking each watch's readiness before putting it on its ring.** A
  look does that already, and the check needs the watched description,
  whose reference costs what the ring does.

## Consequences

- A read that empties a pipe watched only for reading, as servers'
  event loops watch theirs, makes one barrier and three loads after its
  copy, and takes no lock with interrupts masked.
- Each change to a `Polled`'s watches counts its watches for writing
  again, under its lock.
- On OptiPlex 1, with ADR 0776 in the same build, the wait for one
  ready pipe among 1,000 idle ones (`posix_events_idle`) went from 1.02
  times Linux's time to 0.99 and serverbench's from 1.00 to 0.95, with
  4.5% fewer kernel cycles in serverbench's (lab runs
  20261126-034703-163a and 20261126-044239-f0ad). Linux's own time for
  the first ranged from 624 to 651 ns between boots that week.
