<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0629. mremap moves a mapping's page-table entries, and never copies

- Status: Accepted
- Date: 2026-11-27

## Context

Linux's `mremap` changes a mapping's size, growing it where it is or
moving it. glibc's and musl's `realloc` of a block they mapped call it
with `MREMAP_MAYMOVE`, so a growing buffer never copies; V8 and other
garbage collectors move pages with it; userfaultfd's users move memory
with `MREMAP_DONTUNMAP`. Until now the Linux table shrank a mapping and
answered every growth with ENOMEM (ADR 0611), and the C libraries then
copied: a 1 GiB buffer that doubles copied 1 GiB, and touched all of it.

A mapping's pages live in the page tables (ADRs 0152 and 0563): each entry
holds the page's frame and access, a frame held without access under
`PROT_NONE`, or a slot in swap. A region in the address space's list says
what fills the pages not yet touched.

## Decision

- **A method of the address space.** `remap` (radix-address-space(2),
  version 5) takes the old range, the new size, `may_move`, `fixed` and
  `keep_old` (Linux's three flags) and an address. The Linux table checks
  what Linux checks, in Linux's order, and calls it; its errors are
  Linux's: EFAULT where no mapping holds the range, ENOMEM where it cannot
  grow and may not move, EINVAL for the rest. Dukovo's own programs may
  call it too.
- **Shrinking** needs only a mapping at the address, and unmaps the tail
  whatever holds it. **Growing** extends the region in place when it ends
  there and the pages after it are free; otherwise, with `may_move`, it
  moves to the fixed address (unmapped first), the hint, or where anonymous
  memory goes.
- **A move moves entries.** The architecture's `move_pages` moves each
  entry of the range to the new place in one step under the page-table
  lock, with an atomic exchange, so a CPU's accessed or dirty bit is not
  lost, and shoots the old range down once. The tables for the new place
  are made first, so nothing moves unless everything can. Frames, held
  frames and swap slots stay as they are: no page is copied or read in,
  every page stays charged to the space, shared pages keep their counts,
  and a page lent to a device keeps its pin. The region goes to the new
  place with its backing and file offset, which keeps a file's kept pages
  (ADR 0563) and a System V segment's attachment (ADR 0490) right.
- **`keep_old`** leaves the old place mapped as it was, empty: private
  memory only, as Linux before 5.13; a shared mapping is EINVAL.
- **An old size of 0** maps a shared mapping's pages a second time, as
  Linux does; for private memory it is EINVAL, as Linux's.
- The old range must lie in one region; a frame's, ring's or card's
  region cannot grow or move (EFAULT, as Linux's `VM_PFNMAP`).

## Consequences

- `realloc` of a large block grows without copying, in glibc and musl.
- linux-core's mremap case checks growth in place, moves across page
  tables, `MREMAP_FIXED`, `MREMAP_DONTUNMAP`, shared memory mapped twice,
  a fork after a move, and Linux's errors, on Linux and on Dukovo.
- Rows of docs/deferred.md: `MREMAP_DONTUNMAP` of shared memory, a range
  across regions Linux would have merged, and Dukovo's C library's own
  `realloc`, which copies until an mlibc patch calls `remap`.
