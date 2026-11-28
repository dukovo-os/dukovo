<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0798. A cross-CPU wake-up moves as few cache lines as it can, and the FPU is restored only when its registers lost the thread's state

- Status: Accepted
- Date: 2026-11-28

## Context

Two threads that pass a turn back and forth through futexes on two CPUs
(`posix_futex_cross_cpu`, POSIX semaphores in both C libraries) took
4300 ns a round trip on optiplex-1, a Dell OptiPlex 7040 with a Core
i5-6500 (Skylake), against 3086 ns for Linux on the same machine. The
same pair on one CPU was already faster than Linux. The gap was in what
a wake-up does across CPUs: each half of the round trip runs the waker's
futex wake, the hand-over to the idle CPU (ADR 0556), and the idle CPU's
switch to the woken thread, and every cache line that one CPU writes and
the other then reads crosses between them on the critical path.

A timing probe with marks along that path, built for the lab only,
showed where the time went. Some of it was work Linux does not do: the
switch saved and restored the vector registers on every switch, though
an idle CPU's idle thread never touches them and the woken thread comes
back to registers that still hold its state; the futex waiter held a
reference to its program side that the waker dropped; the futex methods
counted a reference to the caller's own address space; idle CPUs read
every other CPU's load to balance when no CPU had a thread waiting.
Some of it was lines crossing twice where once would do: the waker read
the idle CPU's request word's line to share, to check that the CPU was
idle, then took it again to own for the hand-over; the waker counted a
reference to the thread for the inbox and dropped the waiter's, two
locked writes to a line the woken thread's own CPU had written last; the
futex list's lock, its vector and the waiter in it were three lines.

ADR 0027 had the extended state switched eagerly on every switch.

## Decision

- **The vector registers are saved only for a context that has state of
  its own, and restored only when the CPU's registers no longer hold
  it.** A program's thread, or the FPU self-test's, has its own state; a
  kernel thread does not. Each CPU records whose state its registers
  hold, and each context the CPU that last held it, as Linux's
  `fpregs_state_valid` does, so a thread that blocks while its CPU idles
  comes back without a restore. The registers are still saved at every
  switch away from such a thread and restored before it runs on any CPU
  where they may have changed; nothing is switched lazily on a fault,
  which is what made lazy switching unsafe (CVE-2018-3665). XSAVEOPT's
  modified optimization tracks the last area the CPU restored from, so
  anything that writes a context's area restores from it afterwards.
  This supersedes ADR 0027's eager switch on every switch.
- **FS base and the address space are written only when they change.**
- **A futex waiter's woken flag is on its scheduler thread**, on the
  line the wake-up writes anyway, and the waiter holds no reference to
  its program side. The futex methods find the caller's own address
  space without counting a reference.
- **An idle CPU balances only while some CPU has a thread waiting**, as
  one shared word, written only when a CPU's queue goes from one thread
  to two or back, says.
- **Counters one CPU at a time writes are bumped with a load and a
  store**, not a locked add.
- **A futex waker gives the waiter's reference to the wake-up**
  (`wake_given`), which puts that reference in the queue or inbox the
  thread joins, so no reference is counted or dropped on the waker's
  CPU.
- **A futex list's lock and its oldest waiter share one cache line**,
  the waiter's thread held in one word (`ThinRef`); the rest wait in a
  second list that the first's lock covers. What a waiter on shared
  memory holds stays in its own frame until it is off the list.
- **A waker tries the idle CPU it would choose without reading that
  CPU's lines first** (`hand_over_to_idle`). If the thread last ran on
  another CPU that may run it and that placement would choose anyway if
  it were idle (the CPUs are not ranked, or it is among the fastest),
  the waker makes one compare-and-swap of that CPU's request word from
  exactly "watched, nothing asked, empty inbox" to the thread in the
  inbox with the switch asked for, with the thread's lock still held.
  The word's line then comes to the waker once, to own. If the swap
  fails, the waker chooses a CPU and places the thread as before.
- **An idle CPU that a wake-up ends goes straight to the switch**, not
  through the idle loop's checks again.
- **A benchmark row times the wake-up of a thread that is asleep**
  (`posix_futex_asleep_cross_cpu`): the pair of `posix_futex_cross_cpu`,
  but each thread holds the turn 5 µs before it passes it on, long
  enough for the other to be asleep in the kernel on either system, and
  the row leaves those holds out. A pair that passes the turn straight
  back finds it passed before it sleeps once wake-ups are quick enough,
  and then times no wake-up at all.

The scheduler's and the idle wake-up's TLA+ models
(`kernel/sched/tla/`) gain the hand-over under the thread's lock and the
swap that only succeeds on a watched, empty word.

## Consequences

The first part of this (the FPU, FS base, address space, futex flag,
balancing and counters) brought `posix_futex_cross_cpu` on optiplex-1
from 4300 ns to 3811 ns, against Linux's 3086 ns; the same-CPU futex,
pipe, signal and fork-wait rows stayed faster than Linux, at 0.73x to
0.89x.

With the rest, a wake-up reaches the other CPU quickly enough that the
pairs mostly find the turn passed back before they sleep: on optiplex-1
`posix_futex_cross_cpu` took 1369 ns against Linux's 2974 ns (0.46x),
with 20,000 switches in 50,000 rounds, and
`posix_futex_shared_cross_cpu` 1217 ns against 3208 ns (0.38x), with
307, where a pair that sleeps every round makes about 200,000.
`posix_futex_asleep_cross_cpu`, whose threads sleep every round (80,188
switches in 20,000), took 3478 ns against 6339 ns (0.55x). The
same-CPU rows stayed at 0.73x and 0.74x.

A context switch now depends on per-CPU and per-context records of whose
state the registers hold; the FPU self-test checks them with a thread
that moves between CPUs and must find its own registers on each arrival.
Code that writes a context's save area must keep the rule above, or a
thread could run on stale registers. A waker that hands over under the
thread's lock makes the idle CPU wait on that lock if it is quick enough
to reach it first, for the few instructions the waker has left.

The timing probe stays out of the tree; the marks it used are where to
look again if the gap grows.
