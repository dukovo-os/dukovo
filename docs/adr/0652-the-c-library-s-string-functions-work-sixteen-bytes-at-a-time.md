<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0652. The C library's string functions work sixteen bytes at a time

- Status: Accepted
- Date: 2026-11-23

## Context

On the OptiPlex, with both systems held at one clock, PostgreSQL's
select-only row takes 1.15 times Linux's time, its TPC-B row 1.33, Valkey
pipelined 1.21 and PHP's compute row 1.04. A profile of the select-only
run in QEMU, counting instructions (`-icount`), put 13% of the
instructions in user mode in the C library's string functions: memset
5.3%, memcmp 2.3%, strlen 2.2%, memcpy 1.5%, memchr 0.7%, strcmp, strncmp,
strchr and strcpy 0.2-0.4% each.

mlibc's portable versions (`options/ansi/generic/string.cpp`,
`options/internal/generic/essential.cpp`) look at one byte at a time in
strlen, memcmp, memchr, strchr, strchrnul, strncmp, strcpy, and memmove
when it copies downwards, and move eight in memcpy and memset. mlibc has
SSE2, SSE4.2 and AVX2 versions of strcmp, chosen by an ifunc, but the
Radix toolchain builds no ifuncs (`C++ compiler supports ifunc: NO` in
mlibc's configure step), so strcmp was the byte loop too. glibc picks
vector versions of all of them.

## Decision

- **x86-64 has its own memcpy, memmove, memset, strlen, memcmp, memchr,
  strchr, strchrnul, strncmp and strcpy, on SSE2**, which every x86-64
  processor has, so no ifunc is needed: mlibc patch 0101
  (`options/ansi/x86_64/essential.cpp` and `string.cpp`).
  `MLIBC_ARCH_HAS_STRING_FUNCTIONS` leaves out the portable versions.
  memcpy, memmove, memset and strlen are built into the dynamic linker as
  well, as `essential.cpp` is.
- **strcmp is mlibc's own `__mlibc_strcmp_sse2`** where ifuncs are not
  built, by an alias in `strcmp.cpp`.
- **No read crosses into a page the function had no business in.** A
  function that reads one string of unknown length loads 16 aligned
  bytes, which never cross a page, and masks off the bytes before the
  string. Functions that read two strings load 16 bytes unaligned only
  where both have 16 left in their page, and otherwise go a byte at a
  time to the end of the page; strncmp may read past its length only
  there. memchr reads past its length only within the aligned block of
  its last byte, and memcmp not at all.
- **Copies are memmove.** Up to 64 bytes, every byte is read into
  registers before any is written. Longer copies read their first and
  last pieces, copy 64 bytes a loop to aligned stores, upwards or
  downwards by which way the two overlap, and then write the pieces.
- **No `rep movsb` or `rep stosb`.** They are fast on most processors for
  large sizes, but QEMU's TCG counts each byte as an instruction, which
  would distort the instruction budgets and the instruction profiles the
  benchmarks are read with.

## Consequences

- In QEMU with `-icount`, select-only pgbench (8 clients, 2 CPUs) runs
  7,187 transactions a second, from 6,730-6,829 before, with 6% fewer
  instructions a transaction. On hardware a byte loop costs more than its
  instructions suggest, so the gain there should be at least as large.
- memcmp returns the difference of the first bytes that differ, as
  glibc's does, where the portable version returned -1 or 1. C promises
  only the sign.
- The posix boot test runs `userspace/posix/strings.c`: every function at
  every alignment 0-15 and length 0-64, and longer lengths stepped, each
  buffer between pages that fault on any access, memmove at every overlap
  both ways, against byte-at-a-time versions in the test. It finds each
  of the off-by-one mistakes tried while writing the functions.
- Every port rebuilds once, as for any change to mlibc. There is no ABI
  change. AVX2 versions would need ifuncs or a check at startup, and wait
  until a profile asks for them.
