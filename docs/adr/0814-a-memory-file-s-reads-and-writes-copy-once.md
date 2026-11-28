<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0814. A memory file's reads and writes copy once, between the program and the file's pages

- Status: Accepted
- Date: 2026-11-28

## Context

A memory file (`/devices/shm`, `memfd_create`, POSIX shared memory)
keeps its bytes in page frames of its own. A program that does not map
it reads and writes it by position, and a window's program hands the
desktop its pixels that way: `window-test cost` writes each frame's
1.2 MB to the buffer's file and the desktop maps it (ADR 0724).

`pread` and `pwrite` reach a file through `radix_vfs::Out` and `In`,
which copy between the caller's memory and a kernel slice. The memory
file's pages are not a slice the kernel holds: it reaches a frame through
`Arch::read_phys` and `write_phys`. So the file copied through 256 bytes
of the kernel's stack: user memory to the stack and the stack to the
frame, 256 bytes at a time. Every byte was copied twice, and each 256
bytes paid a call through `dyn In`, the checks and the speculation
barrier of a copy from user memory, and a check that the frame is RAM.

Timed on a Ryzen 9 9900X3D under KVM, with the time of each program
read from `/processes` on both systems, the window's program took 221 to
232 µs of processor time a frame on Dukovo and 121 to 123 µs on Linux,
whose `pwrite` to a memfd copies once into the page cache's pages. The
program is the same on both, and nearly all of its time is that write.

## Decision

A memory file's `read_at` and `write_at` copy once, straight between the
program's memory and the file's frames.

- **`Out::put_frames` and `In::get_frames`** (`radix_vfs`) copy between
  the caller's memory at an offset and `len` bytes of physical memory.
  Their default says the caller's memory is not one this can be done
  for, and the file then copies through its buffer as before: a kernel
  slice, or a pipe's.
- **A program's memory can.** `UserOut` and `UserIn` pass the range to
  `UserMode::copy_frames_to_user` and `copy_user_to_frames`, which check
  that the range is user memory and the frames RAM, once, and copy with
  the routine every copy to and from user memory uses, through the
  direct map. A fault in the program's memory ends the copy and gives
  `EFAULT`, as it does for any other file.
- **The file copies a page's worth at a time**, since its pages need not
  be next to each other, holding a share of the page and no lock while
  it copies, as before. What a short or failed copy returns is
  unchanged: the bytes of the pages copied whole.

## Alternatives

- **A larger buffer.** A page instead of 256 bytes saves the calls, not
  the second copy, and a page is too much for the kernel's stack.
- **Mapping the caller's pages into the kernel.** One copy too, with a
  page-table walk and an unmap for each call, where the direct map
  already reaches the file's frames.
- **Changing the program to map its buffer.** Most programs with a
  window do, and their toolkits write pixels into the mapping. A
  program may still write by position, and Linux makes that cost one
  copy.

## Consequences

- The window's program takes 93 to 115 µs a frame on the same machine,
  beside Linux's 96 to 129 µs.
- `read`, `write`, `pread` and `pwrite` on any memory file are faster
  alike, POSIX shared memory included, for any length over a few bytes.
- The architecture trait has two more functions. RISC-V's and AArch64's
  fail, as their other copies to and from user memory do until they run
  programs.
- The POSIX test `shm` writes and reads across pages at odd offsets,
  across a page that is not there, and from memory that is not the
  program's (`copies`).
