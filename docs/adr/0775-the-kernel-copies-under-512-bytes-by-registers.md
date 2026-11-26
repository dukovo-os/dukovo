<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0775. The kernel copies under 512 bytes by registers

- Status: Accepted
- Date: 2026-11-26

## Context

The kernel's `memcpy` was the one `compiler_builtins` gives every Rust
program on a target without ERMS in its features, as the kernel's is:
`rep movsb` up to the destination's next 8-byte boundary, `rep movsq`
for the quadwords and `rep movsb` for the bytes left. On CPUs without
fast short REP MOVSB (FSRM), Skylake and its kind among them, each of
the three takes some 30 cycles to start, however few bytes it moves.
The kernel copies short runs all the time: a packet's headers and its
few bytes of payload, a socket's addresses, and every move of a value
too large for the compiler to make by registers, which it makes by a
call to `memcpy`. On the lab's OptiPlex, `memcpy` had about a tenth of
the kernel's samples that were not idle in the nginx connect row, much
of it under the loopback interface's lock, and the row took 1.01 to
1.02 times Linux's time per connection. Linux copies by registers on
such CPUs (`memcpy_orig`). ADR 0774 made the same change for the
copies to and from programs below 64 bytes.

## Decision

- **On x86-64 the kernel has a `memcpy` of its own** (`copy.rs`), which
  takes the place of `compiler_builtins`' (whose memory functions are
  defined weak so that a program may). It is built only for the kernel's
  target, never into a host test.
- **Under 512 bytes it copies by general-purpose registers**, with whole
  loads and stores that may overlap, which a `memcpy`'s source and
  destination never do: the first and last 8, 4 or 2 bytes, or one byte,
  under 16; the first and last 16 up to 32; 32 at a time from the
  start, then the last 32, up to 511.
- **From 512 bytes it copies by `rep movsq`**, from the destination's
  next 8-byte boundary, with the first and last 8 bytes moved by
  registers, so one `rep` starts, not three.
- **The `copy` self-test** (`selftest=copy`, a boot test) has `memcpy`
  copy each size up to 1,100 bytes from and to each place in a quadword,
  and checks the copy and the bytes around it.

## Alternatives

- **`rep movsb` alone for every size, as `compiler_builtins` does with
  ERMS.** One start instead of three, but still 30 cycles for a copy of
  a few bytes where FSRM is missing.
- **Choosing by CPU at boot, as Linux's alternatives do.** On a CPU with
  FSRM, `rep movsb` starts fast, but the register copies are no slower
  for short runs there, and one routine is simpler to keep right.
- **`memmove` and `memset` too.** The profiles put no time in them; they
  stay `compiler_builtins`'.

## Consequences

- A short copy in the kernel costs a few cycles, not some 30 to 90, on
  the OptiPlexes.
- The kernel's `memcpy` is assembly the kernel keeps, with its own test,
  instead of a library's.
- Copies of 512 bytes or more start one `rep` instead of three.
- On OptiPlex 1, with the rings that wrap without dividing in the same
  build, nginx's new connection went from 1.01 to 1.02 times Linux's
  time to 1.00, and serverbench's wait for one ready pipe among idle
  ones from 1.02 to 1.00 (lab runs 20261126-022946-4834 and
  20261126-034703-163a).
