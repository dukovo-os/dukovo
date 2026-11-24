<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0616. A clock page and a vDSO read the time without a system call

- Status: Accepted
- Date: 2026-11-23

## Context

Programs read the clock very often: every log line, every timeout, every
request a server times. On Linux `clock_gettime`, `gettimeofday`, `time`
and `getcpu` are not system calls but functions of the vDSO, a small
shared object the kernel maps into each process, which reads the TSC and
a page of the kernel's clock data. glibc and musl look for it in the
auxiliary vector (`AT_SYSINFO_EHDR`) and call it; Go's runtime does too.
Radix answered each of them with a system call (the POSIX design note's
"clock_gettime is a system call"), which costs a few hundred nanoseconds
where Linux's vDSO costs tens, and linux-bench's `lx_clock_gettime` row
measures exactly that.

The kernel's clock is the TSC scaled to nanoseconds
(`((rdtsc - start) * ns_per_tick) >> 32`), and the wall clock is that
plus an offset, slewed by `adjtime` at most 500 ppm (radix-hal's
`WallClock`, a sequence lock with a writer flag).

## Decision

- **A clock page** (`radix_hal::clock::ClockPage`, 4096 bytes, page
  aligned) holds what a reader needs, at fixed offsets that the vDSO's
  `clock_page.h` checks at compile time: a sequence number, the counter's
  start and nanoseconds per tick, flags, and the wall clock's fields
  (whether it is known, its offset, and the slew's start and amount). It
  is embedded in the `WallClock`, so it lies in the kernel image and is
  written by the wall clock's writers under its writer flag, as a sequence
  lock: odd while written. The x86-64 HAL fills the counter's fields once
  it has calibrated the TSC, the kernel's own clock (`COUNTER`), with
  `RDPID` or `RDTSCP` in the flags when the CPU has them, and puts each
  CPU's index in `IA32_TSC_AUX` on every CPU.
- **The vDSO** (`kernel/lib/vdso`) is a C shared object built with the
  toolchain's clang and LLD: one read-and-execute segment at address 0,
  with the clock page 4096 bytes below it, no relocations, no writable
  data, both hash tables, versions `LINUX_2.6` and `DUKOVO_1`, and the
  soname `linux-vdso.so.1`. It exports `__vdso_clock_gettime`,
  `__vdso_gettimeofday`, `__vdso_time`, `__vdso_clock_getres` and
  `__vdso_getcpu`, with their plain names as weak aliases, as Linux's
  does, and `__dukovo_clock_read` for Dukovo's C library. A clock the
  page cannot answer (a process's or thread's CPU time, an unknown wall
  clock, a page without `COUNTER`) falls back to the system call by
  Linux's number. Its build checks the image (one loadable segment, no
  relocation or `DT_NEEDED`, the exported names exactly), and host tests
  map it and call each function against a page the HAL wrote.
- **`exec` maps both** at a free range below the top of anonymous memory:
  the clock page read-only, the vDSO read-and-execute, from frames the
  kernel keeps (`FrameObject::kept`), shared by every process and kept
  by `fork`. A program may unmap them, which costs it only its fast
  clocks, but not make them writable: protection changes are for
  anonymous memory only. A Linux program gets
  `AT_SYSINFO_EHDR`, and so does a Dukovo program, whose C library looks
  up `__dukovo_clock_read` by the vDSO's hash table and reads the clocks
  through it, falling back to the system call when it says no. Its
  dynamic linker lists the vDSO after the program (mlibc patch 0123), so
  that `dl_iterate_phdr` gives the program first, as glibc does, which
  Rust's backtraces take for the program.
- `kernel/radix/tla/ClockPage.tla` checks the page's sequence lock with
  two writers and a reader (48,479 states): a reader that finds the same
  even sequence before and after never returns a half-written page, nor
  one older than the last write that ended before it began. The check
  fails if the reader does not read the sequence again, or goes on when
  it is odd.

## Consequences

- `clock_gettime(CLOCK_MONOTONIC)` and `CLOCK_REALTIME`, `gettimeofday`,
  `time` and `getcpu` cost no system call for a Linux program or a Dukovo
  one. linux-bench's `lx_clock_gettime` row measures it, and is held to
  Linux's speed once `ob bench` runs linux-bench on Dukovo
  (docs/deferred.md).
- The page shows only what every process may know (the clock), and it is
  read-only; the TSC is readable from user mode already.
- Mapping the two does not count as mapping memory another space may
  map (ADR 0110's `may_share`): no space writes them, so a futex word in
  them is named by the space alone, and a program's own futexes go on
  being named without a look at its regions.
- `getcpu`'s node is always 0, as Radix has one memory node per machine
  for now; a program the kernel starts itself, without `exec`, has no
  vDSO and makes the system calls.
- Where programs may not read the counter (a clock the HAL has not
  calibrated, another architecture), nothing is mapped and no
  `AT_SYSINFO_EHDR` is given, so the C libraries make the system calls,
  as before.
- The clock page and the vDSO came to main ahead of the rest of Phase
  18, as servers read the clock several times a request (PostgreSQL 22
  times a TPC-B transaction, PHP-FPM about 16 times a request). Linux
  programs, and linux-bench's `lx_clock_gettime` row, come with Phase
  18's Linux personality (ADR 0610).
