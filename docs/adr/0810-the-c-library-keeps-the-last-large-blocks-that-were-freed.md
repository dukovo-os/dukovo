<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0810. The C library keeps the last large blocks that were freed

- Status: Accepted
- Date: 2026-11-28

## Context

The desktop's rows beside labwc (ADR 0724) had a frame of a window at
1.84 times Linux's cost. The program that draws the frame is the same
on both systems: it fills a block of 1.2 MiB, writes it to the window's
buffer and frees it, for every frame.

Counting the kernel's work over 600 frames showed 301 page faults a
frame on Radix. The C library's allocator (frigg's slab pool) maps
every block above 32 KiB by itself and unmaps it when it is freed. So
each frame mapped 300 pages, took a fault for each as it was filled,
and unmapped them.

glibc pays that once. The first free of a mapped block of up to 32 MiB
raises its threshold for mapping to that size, and blocks of the size
come from its heap from then on, with their pages.

A block allocated, freed and allocated again at one size is what any
program does that works on a picture, a file or a packet at a time.

## Decision

The slab pool keeps the last large blocks that were freed, mapped and
with their pages, and hands one out again to a request it fits
(`ports/mlibc/patches/0144-large-freed-blocks-kept.patch`).

- **At most four blocks, of 16 MiB in all**, counted with their
  mappings' padding. When a free would pass either, the block kept
  longest goes back to the system first.
- **A request takes the smallest kept block that holds it with at most
  a quarter as much again left over.** `malloc_usable_size` says what
  the block has. A request no kept block fits is mapped as before.
- **A block larger than 16 MiB is never kept**, nor one placed on an
  alignment above a page, whose mapping is laid out otherwise.
- **The kept blocks are the pool's**, under the lock it takes for every
  large block already. `calloc` clears a block it is handed, as it
  always did.

## Alternatives

- **glibc's rule: a threshold that rises, and a heap.** frigg has no
  heap that grows and shrinks at its top; its large blocks are mappings.
  Keeping some of them gets the same effect for the same programs.
- **Kept without bound, or up to glibc's 32 MiB a block.** Dukovo runs
  dozens of services, each with a C library of its own. What a
  program frees once and never asks for again would stay with it.
- **Keeping the mapping and giving the pages back.** The faults are the
  cost, not the mapping.
- **The kernel mapping several fresh pages at a fault.** It would make
  the faults fewer for every program, and take memory a program never
  touches. It can come by itself; a block that is used again should not
  fault at all.
- **Size classes for large blocks**, so that a block a little larger
  than the last fits where that was. `malloc_usable_size` and realloc's
  rule for a block that shrinks would change with it.

## Consequences

- A program that frees a large block and allocates one of its size
  again gets the same block, with no call to the kernel and no fault.
- A process may hold up to 16 MiB that it freed. `free` after a large
  block no longer gives its memory back at once.
- A use of a large block after it was freed no longer faults at once,
  as it does not for a small block.
- A window of 1920 by 1080 is 8 MiB a buffer, which fits. One of 3840 by
  2160 is just under 32 MiB, which does not: a program that draws so
  large a picture into fresh memory for every frame still pays for it,
  where glibc's threshold would rise that far.
