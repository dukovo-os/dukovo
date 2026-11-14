<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0524. A large kernel allocation takes mapped pages when memory is in pieces

- Status: Accepted
- Date: 2026-11-13

## Context

The kernel heap serves a request over 8 KiB with a block of frames of its
own: physically contiguous, the smallest power of two that holds it, up
to 4 MiB. A block over 32 KiB never runs the out-of-memory policy
(Phase 4): running out of contiguous memory is not running out of
memory, so such a request fails as soon as no free block is that large.

Installing PHP's bundle, 20,235 files, for everyone on a machine of
2 GiB stopped with `no_memory`, and the kernel logged that writing back
to the disk had run out of memory. The allocation that failed was the
growth of a Btrfs block group's list of changes to its free space, which
held 8,192 changes after one commit's worth of writes: 192 KiB, a block
of 256 KiB. The machine had 1.5 GiB free, but no free block of 256 KiB;
400 MiB of the free memory was in blocks of 64 KiB. A server's memory
breaks up the same way as it runs, so any kernel table, list or buffer
that grows past 32 KiB could fail on a machine with plenty of memory
free.

Linux meets this with `kvmalloc`: a request that a physically
contiguous block cannot serve takes pages from anywhere, mapped one
after another into the kernel's address space. Its compaction and its
grouping of pages by mobility make large blocks likelier too.

No kernel code needs the heap's memory to be physically contiguous:
drivers take memory for devices from the frame allocator, and nothing
turns a heap address into a physical one.

## Decision

- **A large request falls back to an area of the mapping window.** A
  request over 8 KiB, aligned to a page or less, for any memory (not
  memory below 4 GiB), from a caller that may sleep, first asks for a
  block of frames without reclaiming anything for it. If no block that
  large is free, the heap takes a frame for each page, anywhere in
  memory, and maps them one after another in the kernel's mapping
  window, with an unmapped guard page below, as a stack has: an
  *area*. The caller gets one range of addresses either way. The frames
  come with reclaim, and at worst the out-of-memory policy, as any small
  request's do. A request whose frames run out gives back what it took.
- **The limit stays 4 MiB.** A request too large for the frame
  allocator's largest block still fails, which keeps a size a program
  controls from reaching all of memory through it.
- **Only callers that may wait for the other CPUs get areas.** Releasing
  an area flushes it from every CPU's TLB and waits for them, so the
  heap maps areas only for a caller that runs with interrupts on,
  outside an interrupt handler, holding no spin lock. Others keep to
  blocks.
- **An area freed where its caller may not wait is released later.**
  A CPU spinning for a lock the caller holds, with interrupts off, could
  not answer a TLB flush. Such a free puts the area on a list linked
  through the areas' own first bytes, and the next large request or free
  from a caller that may wait releases the list first, as does the
  heap's reclaim hook, which the frame allocator runs before it fails a
  caller that may sleep.
- **Frees of areas are checked.** The first frame of an area keeps the
  area's page count in its record's owner-defined word, and a free sets
  a bit there in one atomic step: a second free of the area, even at the
  same moment on another CPU, finds the bit and is reported as a double
  free, and so never reaches the list twice. A free of an address that
  is not the start of an area, or with the wrong size, is reported as a
  bad pointer and changes nothing.
- **The heap reports its areas.** The heap's statistics count areas and
  their bytes beside its blocks, and the `kdb` heap line shows them.
  Their frames belong to the heap, so `/processes/meminfo` counts them
  under `large_heap_blocks`.
- **Btrfs joins changes that follow on.** A block group's list of
  changes to its free space adds a change to the one before it when it
  takes or frees the range right after it, which is what taking space in
  order does. The install's lists stay a few changes long.

## Consequences

- Installing PHP for everyone on 2 GiB succeeds, and the `php` and
  `php-fpm` boot tests run on 2 GiB again.
- An area costs more than a block: a page-table entry and a TLB entry
  for each 4 KiB, where the direct map uses large pages, and a flush of
  every CPU's TLB when it is released. Only requests that would
  otherwise fail pay it.
- An area freed under a spin lock holds its memory until the next large
  request or free that may wait, or until memory runs low.
- Page tables made for the mapping window stay, as they did for stacks.
  A window whose free space is in too many pieces to take an area back
  keeps that area's addresses out of use, which costs only addresses.
- Memory still breaks up as it did. Grouping frames by how long they
  live, and moving pages to join free blocks, as Linux does, are a row
  in `docs/deferred.md`.
- The unit tests of `radix-mm` break memory into single frames on a
  host platform with a mapping window, and check that large requests get
  areas, that callers that may not sleep or may not wait and requests
  aligned to more than a page do not, that an area freed where its
  caller may not wait is released by the next caller that may wait or by
  the reclaim hook, that a request that fails gives back every frame and
  page of the window it took, and that wrong frees are reported. The
  `mm-stress` self-test breaks the kernel's memory into pieces and has
  every CPU make large requests at once, freeing one in three with
  interrupts disabled, with the real mapping window and TLB shootdowns.
