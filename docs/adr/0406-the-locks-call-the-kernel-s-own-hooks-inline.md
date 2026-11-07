<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0406. The locks call the kernel's own hooks inline

- Status: Accepted
- Date: 2026-11-07

## Context

After ADR 0405, opening and closing files from four or eight workers at
once beat Linux on CI's KVM runner (0.87), but one worker alone still
took longer: 1.03 times Linux's time with one thread (647 against 629
ns per open and close) and 1.05 with one process (644 against 611).

One worker shares nothing, so its time is the instructions it runs and
what they cost. Under QEMU (2,000 opens and closes on one CPU, main at
8017e1a) an open and close ran 3,884 instructions, with 14 locked
read-modify-writes, and made some twenty calls through tables of
function pointers:

- Every spin lock and RCU read section calls `preempt_disable` and
  `preempt_enable` through the kernel's `dyn Hooks`, and every mutex
  `current_thread`. `preempt_disable` is one instruction; the call
  loads its target from the table, and the caller saves the registers
  it keeps around a call it cannot see into.
- Every heap allocation and free holds interrupts off through the
  memory manager's `dyn Platform` (`enter` and `restore_interrupts`).
- Counting a reference to a mount on the CPU (ADR 0404) is a locked add
  on the CPU's own count, twice per open and close, although only that
  CPU writes its row.
- A directory's lookup hashes the name and compares it a byte at a time.

Linux's spin locks and preemption counts are inline code, and its
per-CPU counters (`this_cpu_add`) plain instructions on the CPU's own
data.

## Decision

**The locks call the kernel's own hooks by name.** In the kernel's build,
`radix_sync`'s spin locks and read sections call `radix_preempt_disable`
and `radix_preempt_enable`, and its mutexes `radix_current_thread`,
functions the kernel defines (`sched::hooks`) with the code of its
`Hooks`, which call the same functions, so the two ways agree; fat LTO
inlines them into each lock, as it does the mount count of ADR 0404 and
the CPU hold of ADR 0405. They are called only once the hooks are
registered, as the table's were. Host builds, whose tests register hooks
of their own, keep calling through the table, and so do the hooks the
locks seldom call (the lock-order checker's, a contended mutex's wait
and wake).

**The heap holds the CPU through `radix_sync::cpu::HeldCpu`.** In the
kernel's build the heap's allocation and free hold interrupts off and
read the CPU's number through `HeldCpu`, inline, instead of through the
platform's table; the kernel's platform does the same thing.

**A CPU changes its own counts with plain instructions.** The counts of
`radix_sync::cpu_ref` are changed only by their own CPU, held, with
interrupts off: the fast path already held it, and the slow way's drop
now holds it too, where before it changed the row of whatever CPU it
read, with an atomic add, and might have moved since. So the kernel
changes a count with a load and a store, which nothing can come between.
Host builds, whose threads share CPU numbers, keep the atomic add.
`kernel/fs/vfs/tla/MountRefs.tla` already took each change of a count as
one step.

**Names are hashed and compared a word at a time.** The bytes after a
name's last whole word go in as one more word, read in two loads that
between them cover them, not a step for each byte.

## Consequences

- Under QEMU an open and close runs 3,717 instructions (from 3,884) and
  12 locked read-modify-writes (from 14); every lock and allocation in
  the kernel saves its calls through a table.
- `radix_sync` declares three more symbols that a kernel build must
  define, beside `radix_count_here`, `radix_hold_cpu` and
  `radix_let_go_cpu`; only the `radix` kernel links `radix_sync` for a
  target without an operating system, and it defines them all.
- A kernel build whose locks registered other hooks than the kernel's
  would not reach them for preemption or the running thread; there is
  none, and the hooks' documentation says so.
