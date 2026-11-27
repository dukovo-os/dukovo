<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0773. A full barrier is a locked instruction on x86-64

- Status: Accepted
- Date: 2026-11-26

## Context

A wake with nobody to wake takes a full barrier and one load
(`WaitCount::wake_all`), and so do a change to a polled object
(`Polled::changed`), an event queue's look at a watch it took off its
ring, a ring's completion and a waiter counting itself. Each pipe read
and write takes one, and an event queue's round of a write, a look and
a read, the lab's events idle row, takes four. They were
`fence(Ordering::SeqCst)`, which Rust makes an MFENCE on x86-64.

MFENCE orders non-temporal stores and cache-line flushes besides
ordinary memory, and costs some 33 cycles on Skylake, the lab's
OptiPlexes, against about 18 for a locked instruction, which is a full
barrier for ordinary memory; AMD's cores show a wider gap. Linux's
`smp_mb` on x86-64 has been a locked add to the stack since 4.10 for
that reason, and `smp_mb__after_atomic` nothing, as a locked
read-modify-write is a barrier already. The events idle row ran at 1.05
times Linux's time on the OptiPlex after ADR 0771, and at 1.89 under
QEMU on the lab PC's Zen 5.

## Decision

- **`radix_sync::barrier::full`** is the kernel's full barrier for
  ordinary memory: a locked OR of zero into the word at the stack
  pointer on x86-64, and `fence(SeqCst)` elsewhere and under Kani.
- **`radix_sync::barrier::after_rmw`** follows a sequentially
  consistent read-modify-write whose following loads need a full
  barrier: only a compiler fence on x86-64, a fence elsewhere.
- The barriers of the waits, `Polled`, the event queue, the rings and
  the scheduler's wake of a running thread use them. Code that orders
  device memory or write-combining stores, and the TLB shootdowns, keep
  their fences.

## Alternatives

- **Sequentially consistent loads after a sequentially consistent
  store**, with no fence: on x86-64 that makes the store an XCHG, a
  locked instruction too, but each site would need its other side's
  accesses to be sequentially consistent as well, which a lock-free
  count read with `Relaxed` is not; the barrier says what it does in
  one place.
- **Keeping `fence(SeqCst)`.** Rust's model knows the fence, not the
  instruction, but the kernel already relies on the hardware's model
  wherever it uses `asm!`, and the waits' protocol is checked in TLA+
  (`kernel/radix/tla/EventQueue.tla`) on barriers, not on Rust's
  orderings.

## Consequences

- A wake with nobody waiting, a pipe's read or write, and an event
  queue's look each take a cheaper barrier.
- The sync crate's unit tests run the store-buffering pattern the
  barriers guard, with each one, on two threads many times.
