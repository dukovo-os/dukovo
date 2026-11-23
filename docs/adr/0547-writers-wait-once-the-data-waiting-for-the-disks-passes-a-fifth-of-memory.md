<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0547. Writers wait once the data waiting for the disks passes a fifth of memory

- Status: Accepted
- Date: 2026-11-23

## Context

A write to a file copies the data into the file's page cache, marks the
pages dirty and returns (ADR 0055). The writeback thread writes them to the
disk when it looks, every five seconds, and Btrfs commits when its changes
are 30 seconds old or over 64 MiB, which is checked only at those looks
(ADR 0130). Nothing held back a program that wrote faster than that. Clean
pages can always be dropped, but dirty ones cannot until they are written,
so a program that writes quickly fills memory with them.

The lab's `stress/disk-full` test showed it on all three machines: `dd
if=/dev/zero bs=1M` into the lab account's home filled memory in half a
second, and the out-of-memory policy stopped the domain with the most memory
in the lowest class, `authd`'s, which holds the SSH sessions, `dd` with them.
Any program that copies a large file, restores a backup or loads a database
does the same.

Linux holds writers to two limits of dirty data, over the memory that
could hold it: past `dirty_background_ratio` (10%) the flusher threads
start writing back, and past `dirty_ratio` (20%) a writer sleeps in
`balance_dirty_pages` until the flushers have brought the dirty data down,
for a time it works out from each disk's measured speed.

## Decision

- **A count of the dirty pages.** Each file's page cache counts the pages
  it makes dirty and clean, as it did, and adds those of a file on a disk
  to one count for the whole system, `radix_vfs::cache::dirty_pages`. A
  page counted once is taken out once: when it is written, when the file
  is cut short or a range of it replaced, and when the cache goes with it
  still dirty, as with a file removed before its pages were written. Pages
  of files with nothing below them, tmpfs's, are not counted: they have
  nowhere to go, and the out-of-memory policy and swap deal with them.
- **The background limit, a tenth of memory.** After a write to a file
  (`user::files::write`, when it holds no lock), the kernel looks at the
  count. Over a tenth of the memory it manages, it asks the writeback
  thread to write every filesystem back at once (`Vfs::sync_all`), however
  new its changes, unless an ask waits already. The thread also does so at
  each of its looks while the count is over the limit.
- **The limit writers wait at, a fifth of memory.** Over a fifth, the
  writer waits until the count is under it, looking again every 100 ms and
  asking for another write-back each time the last one is done. The write
  itself has been made: a writer goes over the limit by at most one write.
  The writeback thread never waits for itself.
- **A disk that takes nothing.** A writer waits no longer once the count
  has not gone down for 30 seconds, as when a disk fails every write, and
  the kernel log says so once (`Files: N pages wait to be written and none
  went in 30 s; writers go on`). A thread asked to stop stops waiting
  within 100 ms. Each later write over the limit waits again, so memory
  then fills by a write every 30 seconds, not at the speed of memory.
- **Reclaim in a thread of its own.** Clean pages can be dropped, but
  only by a thread that holds no cache's lock, which was the writeback
  thread: while it wrote a fifth of memory back, seconds on a slow disk,
  no clean page went, and the out-of-memory policy, which waits a fifth of
  a second for a reclaim, stopped a domain. A reclaim thread now drops
  them beside it, as Linux's kswapd does beside its flushers: when asked,
  and when less than a 32nd of memory is free at its look each second. It
  passes over a cache whose lock is held, as while it is written back or
  while a writer that holds it waits for memory, so it never waits.
- **Writers look at free memory.** Once the dirty pages are held, what a
  writer wrote stays cached clean, and a writer that fills memory with it
  runs memory out while it holds its own file's cache, whose clean pages
  are then the ones the reclaim thread cannot take. So each time the
  caches have grown by a 256th of memory, a writer looks at free memory
  after its write, with no lock held, and when less than a 32nd is free
  it waits for the reclaim thread, a second at most.
- **What can be seen.** `/processes/meminfo` gives `dirty`, as Linux's
  `Dirty`, and kdb's `mem` the dirty pages and how many writers waited and
  gave up.
- **A boot test, `dirty-limit`.** In 512 MiB, `dd` fills a Btrfs of 768
  MiB from the console: `dirty` stays under a fifth of memory and a write,
  nothing runs out of memory, `dd` ends with no space left, a write fits
  again once the file goes, and Linux's `btrfs check` finds the filesystem
  sound.

## Consequences

- A program that writes faster than its disk runs at the disk's speed once
  a fifth of memory is waiting, as on Linux, and the rest of the system
  keeps its memory.
- The limits are shares of the memory the kernel manages, not of what is
  free or could be freed as Linux's are, and the same for every disk: a
  slow USB stick and a fast NVMe disk share one count, so a writer to the
  fast disk can wait for the slow one's pages. Linux sets each device's
  share of the limit from its measured write speed; doing the same is a
  later change, which the count per disk would need first.
- A write to a tmpfs file also waits while the disks' dirty data is over
  the limit, though its own pages are not counted.
- A writer that fills the caches drops their older clean pages as it
  goes, a quarter at a time, as Linux's reclaim does; the reclaim thread
  looks at free memory once a second, which takes the frame allocator's
  locks.
- Only `write` makes pages dirty: a program's mapping of a file is its own
  copy, and a shared writable one is refused (radix-posix(7)). Shared
  mappings of files, when they come, are to be held to the limit where
  their pages are first written, as Linux does in its page faults.
