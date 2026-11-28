<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0799. A fault maps the pages around it that are there already, as Linux's fault-around does

- Status: Accepted
- Date: 2026-11-28

## Context

On optiplex-1, a Dell OptiPlex 7040 with a Core i5-6500, two rows of the
lab's performance tests stayed slower than Linux after the cross-CPU
wake-up was made cheaper (ADR 0798): `server_postgresql_select` (16.9 µs
a transaction against 16.4 µs) and `posix_fork_exec_wait` (283 µs a
round against 272 µs).

PostgreSQL's select row took 151,000 page faults in its 80,000
transactions, where Linux took 27,000. pgbench connects its eight
clients again for every pass, so each pass has eight new backends, and
each touches the shared buffers its transactions read for the first
time: about two pages a transaction, each a fault of its own. Linux
takes one fault for a run of them. Its fault-around
(`do_fault_around`, `fault_around_bytes`, 64 KiB by default) maps, on a
fault that reads a page of a file or of shared memory, the other pages
of the aligned 64 KiB around it that are in its page cache already and
that the process does not map yet, as one batch.

A program's code is the other case. `exec` of a program in the system's
read-only slot finds its pages kept for every space that runs it
(ADR 0563), but each page of its code and read-only data that it
touches is still a fault of its own, where Linux maps up to 16 of its
page cache's pages at a fault. `posix_fork_exec_wait`'s `fork` alone
was faster than Linux's (67 µs against 77 µs), so the time it lost was
in `exec` and the new program's start.

ADR 0118 turned down mapping 16 pages at a fault for anonymous memory
that is not there yet: that would make pages, and charge a program for
memory it never touched. Pages that are there already cost no memory to
map.

## Decision

- **A fault that does not write a page of a shared mapping, or a page
  of a file whose pages are kept, also maps the pages around it that are
  there already**: those of the aligned run of 16 pages (64 KiB) that
  holds it, within its region, that the space holds nothing for yet, as
  Linux's fault-around does. A page of a shared mapping is there once
  some space made it; a page of a file is there once it is kept, and
  only whole pages are kept. Nothing is made, read from a file or
  zeroed for them, and a page past a memory file's end is never one of
  them, so a fault there is SIGBUS still.
- **A write fault maps only its own page.** A shared mapping's pages are
  mostly made by its first writer, whose neighbours are not there yet,
  and so for Linux.
- **Each page mapped is charged to the space**, as a fault on it would
  be. If the space's domain has no room for them all, none is mapped;
  the fault itself is resolved all the same.
- **The pages are found and mapped in batches**: the page tables are
  walked once to find which of the run hold nothing (`empty_pages`), the
  mapping's or file's chunks are locked once to find which of those are
  there (`SharedPages::peek_run`, `ProgramPages::take_run`), and the
  page tables are walked once more to map them (`fill_pages`), mapping
  only an entry that still holds nothing. An entry that held nothing
  was cached by no CPU, so no shootdown is needed. A page that holds a
  frame, a frame without access or a swap slot is left as it is.
- **A kept page mapped this way is shared as one a fault took**: it is
  read-only in the space and copied at its first write after
  `mprotect`, and the regions' end frees it when no space maps it any
  more.

## Alternatives

- **Map the run on every fault, a write's too.** A first writer filling
  a shared mapping in order would walk the run and lock its chunks at
  every fault and find nothing there; Linux does not either.
- **A larger run.** Linux's default is 64 KiB, and a larger run maps
  more pages a program may never read, each charged to it and walked
  by every `fork` and exit.
- **Map the pages one at a time with `set_entry`.** Two walks of the
  page tables and a lock of the chunks for each page, where the batch
  takes three in all.
- **Large pages for shared memory.** Linux's shared memory has none by
  default either (`shmem_enabled` is `never`), and PostgreSQL's
  `huge_pages` is off in the lab, as on most installations.

## Consequences

- A process that reads memory another process made, or runs a program
  another process ran, takes a fault for each run of 16 pages rather
  than for each page, as on Linux.
- A process is charged for the pages around a fault that it did not
  touch, if they were there already: its resident memory counts them,
  as Linux's does.
- `posix/shm` checks that 64 pages read through a second mapping take
  four or five faults, that both mappings reach the same memory, and
  that a page past a cut end is SIGBUS right after a read of the last
  page; `posix/files` checks the same for a file whose pages are kept,
  with its partial last page, and the copy at a write after
  `mprotect`. The x86-64 self-test checks that `empty_pages` tells
  empty entries from mapped, held and swapped ones and that
  `fill_pages` fills only those it is asked to and that are empty.
