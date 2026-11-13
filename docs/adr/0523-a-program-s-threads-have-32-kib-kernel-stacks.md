<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0523. A program's threads have 32 KiB kernel stacks

- Status: Accepted; amends [0058](0058-kernel-stack-frame-budget.md)
- Date: 2026-11-13

## Context

Every thread had a 16 KiB kernel stack (ADR 0058), and a program's
system calls run on its thread's. PHP's test suite, run whole in a
virtual machine, ended in a double fault on a kernel stack's guard
page. A `write` to a pipe copied from the program's memory, the copy
faulted on a page of a file the program had mapped and not yet read, and
the fault read the page in on the spot, through a compressed Btrfs
extent: the file cache, the extent's decoder, the checksum tree, the
block cache and the disk's queue, down to the sleep that waits for the
disk. The fault's part alone is about 10 KiB; with the system call above
it, the thread slept with less than 4 KiB left.

The lock checker's boot tests look for such paths: a thread that sleeps
with less than 4 KiB of its stack left logs a warning, which fails the
test. Here the warning itself overflowed the stack. Printing it, with
the log, the serial port and the lock checker's own frames under the
sleeping call, takes about 3 KiB, which is what such a thread may not
have.

Any system call that copies from a program's memory can meet the same
fault, and some go deeper than a pipe's before they copy: a write to a
file goes through its filesystem first. ADR 0058 holds each frame to
2 KiB but noted that it bounds frames, not chains.

## Decision

- **A program's threads have 32 KiB kernel stacks**
  (`sched::USER_STACK_PAGES`), with the same unmapped guard page below.
  Kernel threads keep 16 KiB: none runs a system call, and those that read
  files start near the top of their stacks.
- **The stack warning is printed later, from a thread with room.** With
  the lock checker on, the scheduler notes the first thread that sleeps
  with less than 4 KiB left: its name, what it had left, its stack's size
  and a backtrace, taken without printing anything. The next sleep, by
  any thread, that has 4 KiB or more left prints it, as "Threads:
  `python3` slept with 3656 bytes of its 16 KiB kernel stack left, at:"
  and the backtrace. The boot tests fail on it as before.

## Consequences

- A program's thread costs 16 KiB more kernel memory, charged to its
  resource domain with the rest of its stack: nine pages in all, where it
  was five. A thousand threads take 16 MiB more.
- The python boot test's path, a write to a pipe whose copy faults into
  a compressed file, now sleeps with about 19 KiB left, where it had 3656
  bytes.
- A warning about a thread's stack can no longer overflow the stack it
  warns about. It reaches the log a moment later, after another thread's
  sleep, and names the thread it is about.
- The `python` boot test, whose root is a compressed Btrfs, maps
  smb(1), a program of the root it never runs, writes every page of it
  to a pipe, each read in by the write's copy, and checks the bytes.
  With 16 KiB stacks it fails on the stack warning.
