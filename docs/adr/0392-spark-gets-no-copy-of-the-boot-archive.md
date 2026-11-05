<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0392. Spark gets no copy of the boot archive

- Status: Accepted
- Date: 2026-11-05

## Context

ADR 0050 had the kernel copy the boot archive into frames of Spark's own
and map them read-only in Spark's address space, so that Spark v0 could
read the program it started from them. Since Phase 7, programs are read
through `/system`, which the kernel serves from the boot loader's copy
of the archive, and the copy in Spark's frames was read only to print
one log line, "spark: boot archive of N KiB with M files".

The archive is now about 197 MiB, so every machine kept two copies of
it: about 197 MiB of memory that nothing used, most of the 249 MiB a
freshly booted machine used. Several boot tests needed 512 MiB or more
only because of it.

## Decision

- The kernel no longer copies or maps the boot archive for any program.
  Spark reads its files through `/system`, as every program does.
- The start-up page keeps its `bootfs_address` and `bootfs_size` fields,
  both 0, so its layout and every program built against it stay as they
  are (radix-startup(2)).
- radix-rt's `BootFs`, which only Spark used, is removed, and Spark's
  log line goes with it. The kernel's own line, "Init: the boot archive
  is N KiB with M entries", still gives the archive's size.

## Consequences

- A booted machine uses about the archive's size less memory.
- The boot loader's copy stays for the machine's whole life, as it did
  before. Freeing it waits until programs are read from the installed
  disk instead of the archive.
- The loader window at `LOADER_BASE` is unused.
