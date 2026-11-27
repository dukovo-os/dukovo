<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0774. Short copies to and from programs go by words

- Status: Accepted
- Date: 2026-11-26

## Context

Every copy between the kernel and a program's memory on x86-64 was one
`rep movsb` (`radix_user_copy`), but for long copies whose destination
is just ahead of the source, which go from the end with loads and
stores, where Zen's 4K aliasing slows `rep movsb` (docs/design/ipc.md). On CPUs without fast short REP MOVSB, Skylake and
its kind among them, `rep movsb` takes some 30 cycles to start, more
than a copy of a few bytes. The lab's events idle row copies three
times a round, one byte, one byte and a 32-byte event, and its profile
on the OptiPlex put 11 % of the kernel's samples in the copy routine.
Linux copies under 64 bytes with a loop on such CPUs
(`rep_movs_alternative`).

## Decision

- **A copy of at most 63 bytes goes from the end**, through the same
  loads and stores as a long backward copy: single bytes until a
  multiple of 8 is left, then 32 bytes at a time, then 8. Each of them
  may fault and go again, as `rep movsb` may, so the fault handling
  (`fixup`) is the one the backward copy has.
- Longer copies are as before.

## Alternatives

- **Only on CPUs without fast short REP MOVSB**, as Linux chooses. A
  copy of a few bytes by a few moves is not slower where `rep movsb`
  starts fast, and one path is simpler to keep right.
- **A forward loop.** It would report a partial copy as `rep movsb`
  does when a fault ends it, but the kernel's copies succeed or fail
  whole, and the backward loop is already there with its fault handling.

## Consequences

- A system call that copies a few bytes in or out spends a few cycles
  on it, not some 30, on the OptiPlexes.
- A short copy that faults past its first page copies nothing before
  the fault, where `rep movsb` copied up to it; the copy fails either
  way.
