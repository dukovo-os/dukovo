<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0555. A CPU's request to switch is one word in a cache line of its own

- Status: Accepted
- Date: 2026-11-21

## Context

An idle CPU polls for a while before it halts (ADR 0037, ADR 0551): it
spins on its `need_resched` flag, and a waker on another CPU that finds it
polling writes the flag instead of sending an interrupt. The flag sat in
the last cache line of the CPU's scheduler state, with:

- `polling`, which the waker read;
- `called`, which the waker wrote to mark a wake-up from another CPU;
- `wake`, which the waker wrote for a CPU in MWAIT;
- `running` and `rt_prio`, the load every enqueue publishes;
- `online`, which every choice of CPU reads;
- the idle period's statistics, which the CPU writes as it leaves idle.

So one wake-up wrote that line four or five times, in separate accesses,
while the polling CPU kept reading it: each write had to take the line
back from the poller. On CI's Intel Xeon 6975P-C runners a cache line
takes some 170 ns to move between cores. Timestamps on that path put the
waker's part of a wake-up at 524 ns and the woken CPU's part, from seeing
the request to running the thread, at 950 ns.

The protocol between waker and poller also rested on two flags, each side
writing one and reading the other, with sequentially consistent accesses,
which a change to either side's order would break (the TLA+ model
`IdleWake` checked that it did).

## Decision

- **The request is one 32-bit word, alone in a 64-byte line**
  (`radix_sched::Resched`, the first field of the CPU's scheduler state).
  It holds three bits:
  - *pending*: a switch is asked for, the lowest bit, which
    `Cpu::idle_until` checks before MWAIT or HLT;
  - *watched*: the idle CPU polls the word or waits in MWAIT on its line,
    so a waker need send no interrupt;
  - *remote*: the request came from another CPU, which the idle-poll
    governor learns from (ADR 0116).
- **A remote wake-up is one atomic read-modify-write**: it sets *pending*
  and *remote* and returns the old word. If *watched* was clear, the waker
  sends the reschedule IPI. The `wake` word is gone: MWAIT monitors the
  request word's own line.
- **The idle CPU's steps on the word are atomic too:**
  - going idle forgets *remote* and reads *pending* in one step;
  - starting to poll, or to wait in MWAIT, sets *watched* and reads
    *pending* in one step;
  - stopping clears *watched* and reads *pending* in one step.

  Either the waker's write comes first and the idle CPU sees it, or it
  comes after and the waker sees the word unwatched. The TLA+ model
  `IdleWake` now models the word and checks that no wake-up is lost with
  polling, MWAIT and HLT. A version whose idle CPU halts with *watched*
  still set fails it.
- **The switch clears the word with a plain store.** Only the CPU itself
  does that, never while it watches the word. A remote request it
  overwrites is for a thread already on the run queue, which the switch
  picks from. The old flag was cleared the same way.

## Alternatives

- **Pad the old flags apart.** Each would get a line of its own, but a
  wake-up would still write three lines and read a fourth, and the
  protocol would still rest on the order of separate accesses.
- **Have the woken CPU put the thread on its own run queue**, as Linux
  does for an idle CPU (a wake list). The waker would then touch only the
  thread and the word, and the run queue's lines would stay with their
  CPU. This saves more cache-line moves, but the thread would be ready
  and on no run queue for a while, which every path that moves a ready
  thread would have to allow for. It is the next step if the two-CPU rows
  need it.

## Consequences

- A remote wake-up writes the idle CPU's request line once. On a CI
  runner with AMD EPYC 9V74 CPUs, with each vCPU pinned to a core of its
  own for the measurement and both versions measured in the same job:
  - five of the six two-CPU rows took 1-6% less time than before;
  - the futex row was unchanged;
  - the one-CPU rows were unchanged.
- Asking the CPU itself to switch is now an atomic OR rather than a store.
  Going idle is an atomic AND, where it was a store and a load. Each is a
  few cycles on a line the CPU owns.
- `Cpu::idle_until` returns at once if the word's lowest bit is set,
  rather than if the word is non-zero.
- The scheduler's state grows to a whole number of cache lines, 640 bytes
  per CPU.
