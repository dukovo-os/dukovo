<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0556. A thread woken for an idle CPU is handed to it, and that CPU queues it

- Status: Accepted
- Date: 2026-11-21

## Context

A thread woken on one CPU for another was queued by the waker: it took
the other CPU's run-queue lock, wrote that CPU's queue and its published
load, then asked the CPU to switch (ADR 0555). The woken CPU then took
the same lock and read the same lines to pick the thread. Each of those
lines crossed between the two CPUs twice per wake-up, once each way, one
access at a time. On CI's Intel Xeon 6975P-C runners a line takes some
170 ns to move between cores, and timestamps on the path put the waker's
part of a one-way wake-up at 524 ns and the woken CPU's part, from seeing
the request to running the thread, at 950 ns. Two threads that answer
each other across CPUs, through a pipe, a socket or a futex, pay that
twice per round trip.

When the CPU is idle, it has nothing to do but queue the thread itself,
and its own queue's lines are already in its cache.

## Decision

- **A thread woken for another CPU that is idle goes in that CPU's
  inbox** (`hand_over`). The waker makes the thread ready under its lock
  as before and picks the CPU as before; if that CPU runs nothing, the
  waker lets go of the thread's lock and, with interrupts masked, hands
  the thread over. A CPU that is busy, or the waker's own, still gets
  the thread on its queue directly.
- **The inbox is in the request word.** The word grows to a machine word:
  its upper bits hold the address of the newest thread handed over, and
  each thread holds the address of the one before it (`handed_next`).
  Handing over is one compare-and-swap that puts the thread in, asks for
  the switch, marks the request remote and reads whether the word is
  watched, so the waker writes nothing of the idle CPU's but that one
  line, and sends the reschedule IPI only if the word was unwatched, as
  before. The inbox holds a counted reference to each thread.
- **Every switch begins by taking the word** (`take_handed`): one
  exchange, only when the word is non-zero, withdraws the request and
  empties the inbox. The CPU then queues the threads oldest first, each
  under its own lock and then the run queue's, with the class and
  affinity its state holds then, before taking any other lock. A thread
  whose affinity changed while it waited goes where placement picks.
- **A thread woken as it blocks still chooses again if a switch was
  asked for.** Taking the word withdrew the request, maybe for a thread
  just queued from the inbox, so `reschedule(Block)` that finds
  `wake_pending` set carries on as if preempted rather than returning.
- **An idle CPU with a switch pending does not balance.** A thread
  handed over is on no queue yet, so pulling from other CPUs would find
  this one idle and take work it does not need.
- **Paths that move a ready thread allow for one on no queue.** Changing
  a thread's policy or affinity while it waits in an inbox finds it on no
  queue; the CPU that takes it queues it by what its state then says.

The TLA+ model `IdleWake` now models the inbox, threads queued
directly, and a CPU that runs a thread, and checks that no wake-up is
lost, that every thread handed over runs, that no switch takes the word
while the idle thread watches it, and that a request a switch took is
acted on. Seeding three bugs breaks the right property each time: a
waker that ignores an unwatched word, a block that drops the threads it
took, and a block that carries on after taking a request. `Scheduler`
models the inbox per CPU and checks that a ready thread is on exactly one
queue or inbox, or on its way to one.

## Alternatives

- **Hand over to busy CPUs too**, as Linux does when the CPUs share no
  cache. The thread would then wait for the busy CPU's next switch
  rather than being queued at once, and a wake-up that should preempt
  would need its own path. Two CPUs that answer each other leave each
  other idle; the busy case is left as it was.
- **Queue the thread without its lock on the idle CPU**, as Linux's wake
  list does. Its class and affinity would then have to live outside the
  lock too, read by every enqueue. The thread's lock line moves to the
  idle CPU once, which the measurement includes.
- **Keep the run queue's lines apart instead**, padding its lock, its
  queues and its published load into lines of their own. Each would
  still cross twice per wake-up.

## Consequences

- A wake-up for an idle CPU writes one of that CPU's lines, and that CPU
  takes its run queue's lock and lines from its own cache.
- Every switch reads the request word first, a load of a line the CPU
  owns, and exchanges it only when it is non-zero.
- A ready thread may for a moment be on no queue: `/processes` shows it
  ready, and the balancer cannot move it until it is queued.
