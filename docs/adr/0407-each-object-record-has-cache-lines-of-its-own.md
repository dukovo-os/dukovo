<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0407. Each object record has cache lines of its own

- Status: Accepted
- Date: 2026-11-07

## Context

After ADR 0406, `ob bench --scaling` had one worker beat Linux on CI's
KVM runners: 0.93 to 0.97. But on one runner type (AMD EPYC 9V74), four
processes opening and closing files at once took 1.00 times Linux's
time, and eight took 1.01. On another (EPYC 9V45) they took 0.97 and
0.96.

Four processes share nothing they open. Each opens a file of its own,
in a directory of its own, through a capability space of its own. So
their time should grow with the workers only as Linux's does, from what
the CPUs share in hardware. Radix's grew more: 1.73 times one worker's
time, against Linux's 1.68.

QEMU's view of which cache lines CPUs pass between them, over 4,000
opens and closes in the four process row, found one line that moved
0.57 times per open and close. The total was 0.78; the rest was the
timer. That line held the records of two objects in the capability
store's object table, the file descriptions of two workers on two CPUs.
Each record is 160 bytes in the kernel, and records sit side by side in
blocks of 512, so neighbours share the line where one ends and the next
begins. Every open locks and fills its record, and every close locks
and empties it. Each took the line from the other CPU, although the two
objects have nothing to do with each other.

Linux keeps each `struct file` in cache lines of its own: its slab cache
is made with `SLAB_HWCACHE_ALIGN`.

## Decision

**A record of the object table starts a cache line and fills whole
ones:** `Record` in `kernel/cap/src/store/objects.rs` is
`#[repr(align(64))]`. A unit test checks that its size is a multiple of
the line.

## Consequences

- No two records share a cache line, whichever CPUs use their objects.
- A record takes 192 bytes in the kernel instead of 160, and a block of
  512 records takes 96 KiB instead of 80. Blocks are made only as
  objects need them; a system with ten thousand objects holds 20 blocks,
  about 1.9 MiB instead of 1.6.
- An open and close touches no more lines than before: a record of 160
  bytes that does not start a line spans three or four, and one of 192
  bytes that does spans three.
