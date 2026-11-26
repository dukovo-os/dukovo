<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0776. Spin locks mask interrupts inline

- Status: Accepted
- Date: 2026-11-26

## Context

ADR 0406 had the locks call the kernel's preemption and thread hooks by
name, so that fat LTO inlines them, rather than through the table of
`dyn Hooks`. A spin lock taken with interrupts masked (`lock_irq`) still
masked and unmasked them through the table: two calls to functions of
three instructions and one (`pushf`, `pop`, `cli`; `sti`), with the
caller's registers saved around each.

Interrupt handlers change what pollers and event queues watch, so every
`Polled` and every event queue's ring is such a lock, as are the wait
table and the scheduler's queues. An event queue's wait for one ready
pipe among a thousand idle ones (`posix_events_idle`) takes four of them
per round, a write, a look and a read, and took 1.02 times Linux's time
on the lab's OptiPlex 7040 with every other server and IPC row at or
under Linux's.

## Decision

- **In the kernel's build, `radix_sync`'s spin locks mask and unmask
  interrupts through `radix_disable_interrupts` and
  `radix_restore_interrupts`**, functions the kernel defines
  (`sched::hooks`) with the code its `Hooks` run, which call the same
  functions, so that the two ways agree, as ADR 0406 has for preemption.
- Host builds keep calling through the table, as their tests register
  hooks of their own.

## Alternatives

- **Leaving the calls.** A few cycles each, on the paths that wake
  threads and queue events.
- **Masking interrupts in the lock itself, without the hooks.** The
  locks would need to know the architecture, which they leave to the
  kernel.

## Consequences

- Taking and letting go of a spin lock with interrupts masked makes no
  call.
- The kernel must define the two functions, as it does the hooks of
  ADR 0406; a build without them does not link.
- In a profile of the event queue's wait on OptiPlex 1, the samples in
  the two calls, about a hundred in three passes, are gone (lab run
  20261126-044239-f0ad); ADR 0777 gives what the two changes saved
  together.
