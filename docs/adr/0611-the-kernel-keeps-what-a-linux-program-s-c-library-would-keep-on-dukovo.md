<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0611. The kernel keeps what a Linux program's C library would keep on Dukovo

- Status: Accepted
- Date: 2026-11-22

## Context

Radix leaves some of what Linux's kernel keeps for a program to the C
library (ADR 0060 and the POSIX design note): Dukovo's mlibc keeps the
working directory as a string and resolves relative paths from it, picks
the lowest free descriptor, keeps a cursor for each directory it lists,
places the break, and writes a thread's number where `pthread_create`
asks. A Linux program brings its own C library, which expects the kernel
to do all of that (ADR 0610), and its calls must still end in the
methods a Dukovo program calls.

## Decision

- **The working directory** is the process's `PosixState::directory`: an
  absolute path with no links or `..`, as `real_path` gives it. A relative
  path from `AT_FDCWD` is joined to it in a kernel buffer of `PATH_MAX`
  (4096) bytes and resolved beneath `SLOT_ROOT`; an absolute one beneath
  `SLOT_ROOT` with its slashes stripped; one from a directory descriptor
  beneath that slot, and again from the root by the descriptor's path when
  the walk leaves the descriptor's directory, as mlibc's `at` does. The
  directory methods take a path in kernel memory as well as in the
  program's (`Paths::Given`), so no method's checks move. `chdir` and
  `fchdir` store the directory's real path. `exec` keeps it both ways: a
  Dukovo program's exec block carries it to a Linux program, and a Linux
  program's exec gives it to a Dukovo program as its start-up directory.
- **A new descriptor** is the lowest free slot from `SLOT_FD_BASE` plus the
  call's minimum, below the soft `RLIMIT_NOFILE`: EMFILE at the limit.
  Another thread can take the slot first; the method then fails with
  `occupied` and the table takes the next, a few times at most.
  `O_CLOEXEC` and `FD_CLOEXEC` are the slot's close-on-exec flag, which
  `exec` already honours.
- **A directory's listing cursor** is its open description's offset, the
  one a file's reads move. `getdents64` gives `.` and `..` first, as
  mlibc does, and then the filesystem's entries, each with the cursor
  after it as its `d_off`; `lseek` sets it, and copies of the descriptor
  share it, as Linux's do.
- **The break** is the process's (`LinuxProcess::brk`): it starts at the
  page after the main program's last loadable segment, grows by mapping
  read-write anonymous memory with `FIXED_NO_REPLACE` (so it never covers
  another mapping) and shrinks by discarding. Its lock is taken before
  any address-space lock.
- **A thread's Linux words** are the thread's (`LinuxThread`): `clone`'s
  `CLONE_CHILD_SETTID` word, which the new thread writes itself before its
  first instruction, as Linux's `schedule_tail` does, so the write meets
  the new thread's memory and protections and no other; the
  `CLONE_CHILD_CLEARTID` or `set_tid_address` word, which the kernel sets
  to 0 after the thread has left its process, waking a waiter, as a C
  library's join expects; and the robust list's head. `CLONE_PARENT_SETTID`
  is written in the caller's memory before the new thread can run.
- **Thread numbers** are the kernel's, and a process's first thread has
  the process's number, so `gettid` is `getpid` in a single-threaded
  program, as on Linux.
- **`clone`** makes a thread when it shares what a Linux thread shares
  (`CLONE_VM`, `CLONE_THREAD`, `CLONE_SIGHAND`, with any of `CLONE_FS`,
  `CLONE_FILES`, `CLONE_SYSVSEM`, `CLONE_SETTLS` and the three word flags),
  and a process with the kernel's `fork` when it shares none of them; a
  mix Radix cannot give is EINVAL. The new task starts with the caller's
  registers, rax 0, on the stack it was given.
- **`vfork`** (and `CLONE_VFORK`) gives the child a copy of the memory,
  not the memory itself, and the parent waits until the child has run
  `exec` or begun to end, as Linux's parent waits. A program may only call
  `exec` or `_exit` in a `vfork` child, so it cannot tell the copy from
  the original.
- **`madvise(MADV_DONTNEED)` and `MADV_FREE`** call a new method of the
  address space, `forget` (radix-address-space(2), version 3): the pages'
  frames go and the mapping stays, so a page of private anonymous memory
  touched again reads zeros, as on Linux. Dukovo's C library can use it
  for the same advice.
- **`mremap`** shrinks a mapping in place; a mapping that would grow or
  move is ENOMEM, which glibc's and musl's `realloc` answer by copying.

## Consequences

- A Linux program sees the working directory a Dukovo program left it,
  and the other way round, and picks up descriptors as Linux numbers them.
- A `vfork` child that writes to memory before its `exec` writes to its
  copy. glibc's `posix_spawn` tells its parent that the `exec` failed that
  way, so on Dukovo it returns 0 for a program that is not there, and the
  child ends with status 127, as glibc's older `posix_spawn` did; musl's
  tells it through a pipe and returns the error.
- What the kernel keeps for a Linux program is all in
  `kernel/radix/src/user/linux/state.rs`, beside what every process has.
- The robust list's walk when a thread ends and the futex calls beyond
  waits and wakes came with [0617](0617-futexes-take-bit-sets-requeue-and-wake-op-and-a-linux-thread-s-robust-list-is-walked-at-its-end.md).
  `mremap` growth is a row of docs/deferred.md, for the seccomp and JIT
  step of Phase 18.
