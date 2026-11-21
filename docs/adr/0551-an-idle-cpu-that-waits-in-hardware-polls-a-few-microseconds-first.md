<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0551. An idle CPU that waits in hardware polls a few microseconds first

- Status: Accepted
- Date: 2026-11-20

## Context

An idle virtual CPU without MWAIT polls before it halts (ADR 0037, ADR
0116), so that a waker on another CPU writes a word instead of sending
an interprocessor interrupt. A CPU on bare metal, or one with MWAIT, did
not: it went straight to MWAIT or HLT.

`ob bench`'s POSIX benchmark "futex wake and wait each way, two
processes, two CPUs" took 4,031 ns on a Dell OptiPlex 7040 (an i5-6500,
bare metal, MWAIT in C1) against Linux's 3,080 ns, 1.31 times as long.
Each round trip wakes a CPU from MWAIT twice, and leaving C1 costs a
microsecond or so each time. The same row under KVM, where the CPUs
already poll, is not affected.

Linux's cpuidle has a POLL state. Its governor picks it instead of C1
when it expects a wake-up within C1's target residency (2 µs on Skylake),
so a CPU that another thread answers within a few microseconds catches
the wake-up at once.

## Decision

- **Every idle CPU polls before it waits.** The idle loop polls for the
  CPU's limit, which is 0 until an idle period shows polling would help,
  and only then waits with MWAIT or HLT. A waker that sees `polling`
  writes a word, as it already does for a virtual CPU.
- **The limit has two sets of bounds** (`radix_sched::haltpoll::Bounds`).
  A virtual CPU without MWAIT keeps ADR 0037's: up to 200 µs, growing
  first to 50 µs. Any other CPU, which waits in hardware, polls for up to
  8 µs, growing first to 2 µs: a few times what waking from MWAIT's
  lightest state costs, and 2 µs being Skylake's C1 target residency.
- **The rule is the same** (ADR 0116): the limit doubles for an idle
  period that another CPU's wake-up ended after the limit and within the
  maximum, and halves for one that lasted longer than the maximum or that
  the CPU's own interrupt ended. A CPU that waits only for its devices
  or for the timer learns to wait in hardware at once.
- `HaltPoll::new` and `with_limit` keep the virtual bounds, so the
  disks' lanes (ADR 0365) are unchanged. `HaltPoll::within` takes the
  bounds.

## Alternatives

- **Poll for as long as the virtual CPU does.** An idle CPU on bare
  metal would then spin for up to 200 µs, which costs power and a
  sibling thread's time, for a wake-up that costs a microsecond or two.
- **An idle governor that predicts each idle period, as Linux's menu and
  teo governors do.** The rule of ADR 0116 already learns from the
  CPU's recent idle periods; one rule with two sets of bounds is less to
  get right than a second policy.

## Consequences

- A thread that another CPU's thread answers within a few microseconds
  finds its CPU polling, and is woken by a write, without entering MWAIT.
  The lab's next run on the OptiPlex shows how much of the futex row's
  1.31 times this recovers; the CI rows, which run under KVM, do not
  change.
- A CPU that other CPUs wake within 8 µs does not reach MWAIT's C1 then,
  and uses more power while it polls: at most 8 µs each time it goes
  idle, and none once its idle periods are longer or its own interrupts
  end them.
- The TLA+ model `kernel/sched/tla/IdleWake.tla` lets a CPU with MWAIT
  poll first too, so TLC checks all four combinations: MWAIT or HLT,
  polling first or not.
- This changes ADR 0037's statement that nothing changes on bare metal
  and with MWAIT.
