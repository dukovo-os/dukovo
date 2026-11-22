<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0558. A poll wakes only for the objects it looks at

- Status: Accepted
- Date: 2026-11-22

## Context

`poll` kept one global count of changes. Every change to an object that
anything polled moved it and woke every thread waiting in `poll` on the
machine. Each of them looked at all its own objects again, found nothing
new, and slept. ADR 0550 gave each network socket its own waiters and left
this count as a separate change.

A probe in QEMU (an installed system, counters from `/processes/stat` and
`/processes/<pid>/status`, and a sampling profile) showed what it costs:

- Valkey with 8 clients: 27 to 31 thread switches per request over the
  system, in two runs. Twelve idle services, each asleep in `poll`, woke
  1.1 to 1.5 times per request each.
- PostgreSQL, `pgbench -S -c 8`: 95 switches per transaction. Every
  backend woke about twice per transaction, though each serves an eighth
  of them, and so did every service.
- In the profile of the Valkey run, `poll` took 14%, rescheduling 10% and
  wake-up IPIs 9%.

A9's QEMU run had Valkey at 13.6 to 15.6 times Linux's time per request,
PostgreSQL at 12.7 to 14.5 and PHP-FPM at 6.9.

Linux keeps a wait queue on each file, and a change wakes the entries on
that file's queue alone.

## Decision

- **Each polled object keeps the threads polling it.** Its `Polled` holds
  a list of them, once for each time a poll names the object, beside the
  event queues' watches. A change takes the object's lock and wakes the
  threads on its list, and takes no lock while nobody polls or watches it.
- **A flag on the thread says it must look again, and no wake-up is
  lost.** A poll that waits adds the thread to each object's list, clears
  the thread's `poll_changed` flag, takes a full barrier and looks. A
  change reads the object's count of pollers after the change, with a
  full barrier between, then under the lock sets each poller's flag and
  wakes it unless the flag was already set. If the look misses the change,
  the change sees the poller, finds the flag clear and wakes it, and a wake
  that comes before the sleep makes the sleep return at once. The model
  `kernel/radix/tla/Poll.tla` checks it.
- **Pollers leave the scheduler's wait table.** A poller sleeps until its
  flag is set, a signal or a stop reaches it, or its deadline passes. No
  poll enters the table, so a wake there no longer goes past the pollers.
- **A poll that does not wait adds itself nowhere.** A poll with a timeout
  of 0 and no signal mask, or one that finds something ready at its first
  look, keeps its cost.
- **A poll that waits may fail with ENOMEM.** It does so when an object's
  list is full and the memory to grow it cannot be had, as Linux's `poll`
  may. A list doubles and keeps its room, so a busy object stops
  allocating after its first polls.

## Consequences

- A change wakes the threads polling what changed. An idle service asleep
  in `poll` stays asleep while others work. The `bystanders` part of
  `userspace/posix/poll.c` checks it: a process asleep in `poll` on a
  pipe nothing writes does not wake while two others pass 2,000 bytes.
- A poll that waits takes each object's lock twice, to add the thread and
  to remove it, as Linux's `poll` does for each file.
- A change made in an interrupt handler (the keyboard, the mouse) still
  wakes its pollers, with the lock taken with interrupts masked.
- A `Polled`'s lock is taken under whatever lock the change's caller
  holds. Under it a change wakes threads and queues event queue watches;
  an event queue's own `Polled` holds no watches, so its lock is the last
  of them taken (docs/design/posix.md, Lock order).
- The scheduler's wait table is one list under one lock, as ADR 0550 says.
  With the pollers gone from it, what remains is measured next.
- The same probe, run before and after this change in QEMU without KVM,
  where only the ratios mean anything: Valkey with 8 clients went from
  1,291 to 8,197 requests a second, with 1 client from 830 to 1,993, and
  pipelined from 16,410 to 55,556; PostgreSQL's `pgbench -S -c 8` from 290
  to 502 transactions a second. Thread switches per Valkey request fell
  from 31 to 9 and IPIs from 7.2 to 1.9, and the idle services woke only
  on their own timers. The lab's server rows (ADR 0546) measure it against
  Linux under KVM.
