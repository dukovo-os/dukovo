<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0413. Open and close copy no path, and methods read their words in place

- Status: Accepted
- Date: 2026-11-08

## Context

CI holds the open and close scaling rows to Linux's time (ADR 0409). On
the AMD EPYC 9V74 runners the process rows sat at Linux's time, with
nothing to spare (main at 1a95729):

| Row | Radix | Linux | Ratio |
|---|---:|---:|---:|
| One process | 440 ns | 451 ns | 0.98 |
| Four processes | 748 ns | 757 ns | 0.99 |
| Eight processes | 1,487 ns | 1,477 ns | 1.01 |

The workers no longer share cache lines: under QEMU the four and eight
process rows move at most 0.008 lines between CPUs per open and close,
down from 0.78 before ADR 0407. So the gap is what each open and close
costs. Under QEMU (2,000 opens and closes of the one process row) one
ran 4,178 instructions: 3,640 in the kernel and 538 in the C library.

Of the C library's 538:

- 126 made the path: `resolve` measured it and copied it into a
  4 KiB buffer, though the kernel takes a path beneath a directory
  descriptor, or an absolute one past its slashes, as the program wrote
  it.
- 81 found and claimed the lowest free descriptor (ADR 0403): a call
  into `radix.cpp`, which claimed the bit with a compare-and-swap loop,
  and a destructor and a note about directory paths, each a call of
  its own.
- 30 turned the open flags into the kernel's, a test for each of nine
  flags, though most opens ask for none of them.
- 25 of `open_rights`'s 64 saved and restored registers, as a call of
  its own.
- Close made three calls into `radix.cpp` where one does.

On the kernel's way into a method, `cap_call`'s eight data words were
copied twice: out of the argument registers into `CapCallArgs`, and
then by each method into its arguments, with the words past the
message's count zeroed.

## Decision

**A path the kernel can take as written is not copied.** `radix::Path`
holds a pointer to its bytes: the program's own string for a path beneath
a directory descriptor, an absolute one past its slashes, or one
relative to the working directory when that is the root, and its own
buffer only for the paths `resolve` makes, joined to the working
directory's path or to a directory descriptor's (ADR 0089). A `Path` is
never copied. A thread that changes the string while another opens by
it changes what the kernel is given, as with any buffer a system call
reads; the kernel never resolves a path above the directory it starts
from.

**Claiming a descriptor takes one locked instruction, and closing one
call.** `free_slot` tests and sets the descriptor's bit alone, which
x86-64 does with one locked bit-test-and-set, and forgets the
descriptor's directory path itself: every new descriptor, a pipe's and a
socket pair's too, starts without one. The claim's destructor is inline.
`close` makes one call, `radix::close_fd`.

**The common open flags skip the translation.** Flags with no more than an
access mode and close-on-exec map to the kernel's with one test.
`open_rights` is inlined into `open_path`.

**Methods read the data words in the argument registers.**
`methods::call` and `methods::call_capability` take the eight words by
reference to where the system call left them; each method still makes
its own copy with the words past the count zeroed. A message to an
endpoint copies them, as before.

## Consequences

- Under QEMU an open and close runs 4,044 instructions (from 4,178):
  433 in the C library (from 538) and 3,611 in the kernel (from 3,640),
  of which `radix_syscall` runs 402 (from 432).
- On the EPYC 9V74 runners the process rows went to 0.93, 0.95 and
  0.98 (from 0.98, 0.99 and 1.01): Radix's eight process row took
  1,452 ns against Linux's 1,478, from 1,487 against 1,477. On the
  EPYC 7763 runners they were 0.92 to 0.94, 0.87 to 0.88 and 0.87 to
  0.89, and on the Xeon Platinum 8573C 0.94, 0.71 and 0.70.
- A descriptor that a pipe, a socket pair or an accepted connection
  takes has no directory path left from an earlier descriptor of that
  number, as an opened one never had.
- The posix test `files` opens by each form of path: absolute, with
  extra slashes, the root, relative to the root and to another working
  directory, and beneath a directory descriptor, with `.` and `..`.
