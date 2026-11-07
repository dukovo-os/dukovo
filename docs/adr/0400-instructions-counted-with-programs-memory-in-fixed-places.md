<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0400. Instructions are counted with programs' memory in the same places every run: `norandmaps`

- Status: Accepted
- Date: 2026-11-06

## Context

The instruction gate (ADR 0045) holds each operation on one CPU to a
budget: what it took when the budget was set, plus a tenth. It assumes a
count is the same on every run of the same code. Fork and wait was not.
Its count moved by some 2,600 instructions from boot to boot of one
build (155,362, 157,513 and 158,007 over three boots), and CI's run of
#233 counted 162,149, past its budget of 160,000, which #233 raised to
180,000.

The cause is user ASLR (ADR 0048). A program's stack, its image and its
anonymous memory go to random pages, and in some boots one of them
crosses the boundary of a page table, a page directory or more. A fork
then builds two or three more page tables for the child, and its exit
frees them, about 1,300 instructions each. A budget that allows for that
swing lets the same amount of real growth through unseen.

## Decision

- A kernel flag, `norandmaps`, puts each of those at the first of its
  places instead of a random one: the stack and the anonymous memory at
  the top of their windows, a position-independent image at the start of
  its window. Linux has a flag of the same name for the same purpose.
  The kernel says so in its log.
- `ob bench --instructions` gives the flag to both benchmark boots, so
  the gate counts the same page tables every run. Normal boots, the
  timed benchmarks under KVM and every other test keep random places.
- The places come from each CPU's generator (`user::random::place` and
  `place_bits`), as programs' random bytes do, not from a ChaCha20 block
  of the kernel's generator for each.
- The test image's command-line slot grows from 160 to 192 bytes, room
  for the POSIX benchmarks' command line with the flag in front.

## Consequences

- Fork and wait counts 153,856 on every run, and its budget comes down
  to 170,000.
- The gate no longer sees what a random layout costs. That cost is the
  same before and after a change, so it is not what the gate is for, and
  the timings under KVM still run with random places.
- A change that moves where the fixed places are (a window's top or
  start) can move the counts once, as a change of QEMU can (ADR 0045).
