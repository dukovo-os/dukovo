<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0291. Btrfs compresses a long write's next extents while it writes the last

- Status: Accepted
- Date: 2026-11-02

## Context

`ob bench`'s row "a 1 MiB file of text written, compressed (zstd) and
synced with 31 others" was 0.78 of Linux on main's AMD EPYC 7763
runners (3130 against 3997 µs a file) and 1.01 to 1.05 on the faster
runners pull request #176 drew three times (3755 against 3643 µs), with
nothing of #176's on that path. On the faster runners the disk's time
is most of a file's: Radix was idle on its CPU for 3540 of its 3755 µs.

ADR 0134 has a sync compress a file's dirty pages two 128 KiB extents
at a time, the second on another CPU (`radix_vfs::join`, ADR 0133),
then write the two, then compress the next two. The page cache handed
Btrfs 256 KiB at a time (`WRITE_RUN`, 64 pages), so a 1 MiB file took
four rounds of compressing and then writing, and the CPUs waited for
the disk while the disk waited for the CPUs. Linux's writeback
compresses a file's ranges on its worker threads and sends each
extent's write to the disk as it is made, without waiting for it, so
its compressing and its writing overlap.

## Decision

- **Longer runs for a backing that asks.** A page cache's backing may
  ask for more than `WRITE_RUN` pages at once
  (`Backing::write_run_pages`), up to `WRITE_RUN_MAX`, 256 pages
  (1 MiB). A run longer than 64 pages is listed on the heap, 8 bytes a
  page, rather than on the 16 KiB kernel stack; without memory for the
  list, the run is cut to 64 pages, as before.
- **Btrfs asks for 1 MiB** for a file whose data it compresses, and
  keeps 256 KiB for the rest.
- **Four packers, two to a side.** A filesystem has up to four packers
  (an encoder and a frame's pages each) instead of two, made at the
  first long compressed write and dropped by the shrinker with the
  others. A run of three extents or more uses all four as two sides of
  two: while one side's two extents are written (the transaction, the
  checksums and the disk), `radix_vfs::join` compresses the next two
  into the other side on another CPU. That side splits its two again
  with a nested join, onto whichever CPU's helper is free, the writer's
  own CPU among them while the writer waits for the disk. Then the
  sides swap.
- **Nothing on disk changes.** The extents are written in the file's
  order, each as ADR 0134 writes it, from the same frames; only when
  they are compressed moves.
- **Less memory, less overlap.** Without memory for the third packer, a
  run compresses two at a time as before; without memory for the
  second, one. A file that is not compressed, and a machine with one
  CPU, write as before.

## Consequences

- A compressed file of eight extents takes about one round of
  compressing and then four of whichever of writing and compressing is
  longer, rather than four of compressing and writing in turn. CI's
  Benchmarks job measures what that is on each runner.
- A compressed file's run that fails to be written leaves all its
  pages dirty, up to 1 MiB of them rather than 256 KiB, and the next
  sync writes them again.
- Two more packers are held while compressed writes go on, and given
  back when memory runs short (`drop_packers`).
- The host unit tests run a filesystem's joins on two threads for
  writable disks too, so the overlapped path is what
  `files_written_compressed_read_back_and_linux_restores_them` writes
  and `btrfs restore` reads back.
