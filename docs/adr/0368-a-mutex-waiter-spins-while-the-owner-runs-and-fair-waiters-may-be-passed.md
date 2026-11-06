<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0368. A mutex waiter spins while the owner runs, and fair waiters may be passed

- Status: Accepted
- Date: 2026-11-06

## Context

`ob bench --scaling` (#215) has 1, 4 and 8 workers open and close a
file each at once on 4 CPUs. In the run of #221 (KVM, Xeon Platinum
8573C), one thread took 720 ns per open and close and Linux 632 ns;
4 threads took 46.0 µs against Linux's 1.84 µs, and 8 threads 216.9 µs
against 3.53 µs. Processes did little better (34.3 µs and 76.0 µs).
Taking turns at one lock would cost about 4 times one worker's time
with 4 workers, not 64.

Every open and close takes the capability store's mutex several times.
A contended `radix_sync::Mutex` queued its waiter at once and slept,
and the owner letting go handed the mutex straight to the first sleeper
(no barging, ADR 0027). So once two threads met at the lock, every
critical section after that waited for a sleeper to wake: a switch out,
a wake-up, a switch in, and in a virtual machine an exit to the host on
the way. The lock was held, in effect, for the wake-up, not for the
store's work, and the queue never drained (a convoy). The counts
`ob bench --scaling` prints since #221 show it. With 4 threads, each
open and close found the store's lock taken 4.35 times (a counter on
that lock alone, in #221's first run), slept for a mutex 4.30 times, and
every sleep ended in a handoff (4.30), with 8.6 switches; with 8 threads,
12.4 sleeps and handoffs and 18.7 switches. With one worker all of these
are 0, and the capability cache was never flushed.

Linux's mutex spins while the owner runs on a CPU (optimistic spinning)
and lets a running thread take the lock before a woken waiter
(stealing), handing the lock over only to a waiter that has waited too
long.

## Decision

- A contended waiter spins first, in `mutex_wait`, while the owner is the
  thread on some CPU (`sched::is_running`, which reads each CPU's
  `current` word with no lock): up to 512 checks with 8 pauses between,
  from some microseconds to a fifth of a millisecond as the CPU's
  `pause` is quick or slow. The kernel is preemptible outside spin
  locks, so a spinner still gives way at a tick to anything more urgent. It takes the mutex if it finds no
  owner, keeping the waiters bit. It queues and sleeps as before once
  the owner does not run, or the spin ends.
- Letting go of a mutex with waiters (`mutex_wake`) hands it straight to
  the most urgent waiter, as before, when that waiter is real-time or has
  lost the mutex 4 times (`LOSSES_BEFORE_HANDOFF`). Otherwise it clears
  the owner, keeps the waiters bit only if others remain queued, and
  wakes that waiter to try again: a spinner may take the mutex first.
  The waiter keeps its place in the queue when it queues again, so the
  oldest fair waiter is the one woken each time, and counts a loss.
- While a mutex is let go to its waiters, they record no owner
  (`NO_OWNER`), since there is none to lend a priority to. A waiter that
  becomes real-time meanwhile is woken to queue again behind whoever
  took the mutex, lending to it.
- `kernel/radix/tla/Mutex.tla` models the word, the spin, the queue and
  both ways of letting go. TLC checks that one thread at a time holds
  the mutex, the bit is set exactly while someone is queued, a real-time
  waiter is handed the mutex, a fair one loses it at most the allowed
  number of times, and every thread gets it every time it asks.
- `/processes/stat` counts let-gos (`mutex_wakes`) beside the mutexes
  found taken, the sleeps and the handoffs it counted already; the
  `sched-mutex`
  boot test runs three fair threads per CPU and a FIFO thread on one
  mutex, and checks both ways of letting go happen, no increment is
  lost, every thread gets through and nobody waits long.

## Consequences

- A critical section no longer waits for a wake-up while threads that
  run are ready to take the lock. Fair waiters may be passed by threads
  that never slept, at most 4 times each before the mutex is handed to
  them, so no waiter starves.
- Real-time waiters keep the handoff, so the bound on priority inversion
  (docs/design/scheduler.md, property 8) holds as it was: nobody takes a
  mutex between its owner and a real-time waiter.
- A waiter burns up to a fifth of a millisecond of CPU spinning on an
  owner that runs a long critical section; a sleeping or preempted owner
  ends the spin at once.
- A waiter woken to try again that loses wakes for nothing: that costs
  what every wake-up cost before, at most 4 times per wait.
- The capability store's one lock still serializes the opens and closes
  of all processes; ADR 0369 splits it.
