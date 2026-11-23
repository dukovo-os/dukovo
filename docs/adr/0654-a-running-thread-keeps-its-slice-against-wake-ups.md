<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0654. A running thread keeps its slice against wake-ups, and each CPU's spans are worked out once

- Status: Accepted
- Date: 2026-11-23

## Context

On the OptiPlex, with both systems held at one clock, PostgreSQL's
TPC-B-like row takes 1.33 times Linux's time and its select-only row
1.15. A run of the TPC-B-like row in QEMU (`-icount`, 2 CPUs, eight
clients on two pgbench threads, the lab row's options) switched threads
24.8 times a transaction, 10.6 of them taking the CPU from a thread that
could still run, and sent 3.1 interrupts between CPUs.

- **Every wake-up of a thread whose earlier deadline made it the fair
  pick took the CPU at once.** A pgbench thread that sent a query woke
  the server process on its own CPU and was switched out before it got to
  its next wait, and before it sent the queries of its other
  connections. Linux has not done this since 6.6: its `RUN_TO_PARITY`,
  on by default, lets the running task finish the slice it was picked
  with, while it is still eligible, before a waking one takes the CPU.
- **Each look at a CPU's levels worked them out again from every CPU's
  place.** `Topology::span` looked at every CPU, `levels` worked out
  four spans, and a waking thread with no idle CPU to go back to
  (`select_cpu`) read the levels and the spans again: about 1,400
  instructions a wake-up with two CPUs, 13.6 wake-ups a transaction.
  Balancing does it for each CPU of a level: `find_busiest` works out the
  span of the level below for every CPU of the level it looks at, at each
  timer tick's attempt and each time a CPU goes idle. On a two-socket
  server of 88 hardware threads that is about 10,000 instructions a
  wake-up and over 150,000 for each idle CPU's attempt.

## Decision

- **Run to parity.** The fair queue remembers that its running entity
  has not used up the slice it was picked with (`protected`, set by
  `set_running`). While that holds and the entity is eligible,
  `should_preempt` says no: a woken entity waits until the running one
  blocks, yields (which gives the slice up), uses its slice up (its
  deadline passes and `charge` gives it another), or runs ahead of the
  queue's virtual time. Real-time threads still take the CPU at once, and
  an idle CPU still switches to any thread.
- **`Topology::finish` works out each CPU's span at each level, and which
  levels add CPUs to the one below, once.** `span` and `levels` read
  them; a change of the CPUs or their places forgets them, and without
  memory for them they are worked out each time, as before. The kernel
  finishes its topology when the scheduler starts. `Topology` keeps them
  in memory from the allocator it is given, as the run queues do.

## Alternatives

- **No wake-up preemption for fair threads at all**, as Linux's
  `WAKEUP_PREEMPTION` off. A thread that wakes would wait up to a slice
  even behind a thread that has run past its share, which EEVDF's bound
  on lag rests on not happening.
- **Work out spans from a group number per CPU and level.** It costs a
  comparison per CPU rather than one per CPU and level, but still grows
  with the machine; the spans themselves are 32 bytes each, 32 KiB for
  256 CPUs.
- **Keep the spans in the topology as a fixed array.** The topology is
  built on the stack of the boot CPU before it goes into its static, and
  32 KiB more there is more than that stack is sure to have.

## Consequences

- In QEMU, the TPC-B-like run switches 22.4 times a transaction rather
  than 24.8, preempts 8.3 times rather than 10.6, and sends 2.1
  interrupts between CPUs rather than 3.1; it runs 1,091 transactions a
  second, from 1,079. On hardware a switch and an interrupt cost more
  than their instructions, so the gain there should be larger.
- A fair thread that wakes may wait for the running thread for up to a
  slice (3 ms), as on Linux, when the running thread is still owed time.
  Each thread still gets its share: the protection ends as soon as the
  running thread is no longer eligible, so the lag bound of
  `docs/design/scheduler.md` holds as before.
- A wake-up that looks for an idle CPU, and a balancing attempt, cost the
  same however many CPUs share a level, apart from the look at each CPU's
  load that they need anyway.
- The host tests check that a finished topology answers every span, level,
  placement and balancing question as the worked-out one does, for random
  places, and that a change of place forgets what was worked out.
