<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0813. An idle CPU polls only while another CPU has work

- Status: Accepted
- Date: 2026-11-28

## Context

An idle CPU polls its request word for a while before it halts, so that
a wake-up from another CPU costs one write and neither a halt nor an
interrupt (ADR 0037). How long it polls is learnt from how its idle
periods ended (ADR 0116): a period that another CPU's wake-up ended
within 200 µs doubles the limit, from 50 µs up to 200 µs in a virtual
machine; a longer one, or one that the CPU's own interrupt ended, halves
it.

A desktop's frame is short work by turns on two CPUs and then a long
wait for the screen. The window's program draws and hands its buffer
over, the desktop on the other CPU composes it and shows it, and both
sleep until the screen's next refresh, 10 ms and more away. The short
periods between the turns keep the limit up, so each CPU also polled at
the start of the long wait, for the whole limit, with every other CPU
idle too and nothing left that could wake it. In a virtual machine that
is processor time of the host.

Counted on a Ryzen 9 9900X3D under KVM (`poll_ns` of `/processes/stat`,
which this change adds), the two CPUs polled for 259 to 291 µs a frame,
of the 859 to 952 µs the host ran for the frame, beside Linux's 548 to
598 µs for the same frame under labwc (ADR 0724). A Linux guest does not
poll before it halts unless it is told to.

## Decision

An idle CPU polls only while another CPU has work: a thread to run, or
one on its way to it. Its threads are what wakes a thread here.

- **The poll's loop looks at one other CPU each time round**
  (`radix_sched::haltpoll::Others`), in turn, reading its request word,
  whether it is in an idle period, and the count of runnable threads it
  publishes for placement. When it has seen every other CPU without
  work, once round, the poll ends and the CPU halts.
- **A CPU has work from when its idle period ends until the next
  begins.** A thread handed to an idle CPU is in its request word, then
  in neither the word nor the count while the CPU queues it, then in the
  count. A poll looks many times in that gap, and one look that finds no
  work ends it. So each CPU keeps a flag for its idle period beside the
  word: its idle thread clears it before it withdraws the request, and
  the poll reads the request first, so that every look sees one of the
  two.
- **A CPU alone has no one to wake it** and does not poll.
- **The limit is learnt as before** (ADR 0116). A poll that ends early
  for want of work is a period like any other: a long one halves the
  limit.
- **`/processes/stat` counts the time polled** (`poll_ns`), beside the
  polls that ended in a wake-up and the halts.

## Alternatives

- **Linux's rule that a shrunk limit below its first step becomes 0.**
  It ends polling sooner after one long period, and polling starts again
  as late: the disks' queues learn their own limits with the same rule
  (ADR 0111) and would lose the steps in between.
- **Telling the host not to poll for this CPU** (`MSR_KVM_POLL_CONTROL`),
  since the guest has. The host's poll is what makes a device's
  interrupt cheap for a CPU that does not poll for it (ADR 0116).
- **Two limits, one for a CPU with busy neighbours and one without.**
  The second would be learnt to be 0, which this is without learning.
- **Not polling at all in a virtual machine.** The rows of `ob bench`
  that wake a thread across CPUs are as fast as Linux's because of it
  (ADR 0037).
- **The word and the count alone as the sign of work.** Built first. Two
  threads that wake each other across CPUs with semaphores halted at
  every turn, 1.7 halts a round trip, because each CPU's poll saw the
  other between the word and the count: the rows were 1.31 and 1.49
  times Linux's on a runner of the project's checks, where they must be
  at or under its time.

## Consequences

- On the same machine the two CPUs poll for 63 to 66 µs a frame, and the
  host runs 743 to 749 µs for it. The frame's row goes from 1.50 and
  1.52 times Linux's to 1.22 and 1.33, and a step of a resize from 1.08
  and 1.11 to 1.00 and 1.02.
- A wake-up that an interrupt handler on an otherwise idle CPU sends to
  another idle CPU finds it halted where it found it polling: a disk's
  completion that wakes a thread on another CPU pays the interrupt. The
  benchmarks' rows that wake across CPUs do it from a running thread.
- What is still polled in a frame is the wait for the other CPU's turn,
  100 µs and more of its work: longer than the halt it saves. The limit
  does not yet tell a wake-up that comes in microseconds from one that
  comes when the other CPU's work is done.
- The loop reads one more cache line each time round, another CPU's
  load, which that CPU writes only as its count of threads changes.
