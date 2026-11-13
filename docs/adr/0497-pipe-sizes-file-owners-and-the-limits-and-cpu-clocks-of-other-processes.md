<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0497. Pipes have sizes, descriptions have owners, and limits and CPU clocks reach other processes

- Status: Accepted
- Date: 2026-11-13

## Context

CPython's test suite, which the lab runs whole (ADR 0495), found calls
on files, processes and clocks that fail on Radix and work on Linux, and
that programs use as a matter of course:

- `fcntl` with `F_GETPIPE_SZ` and `F_SETPIPE_SZ` failed with EINVAL.
  Programs that move a lot through a pipe, and Python's `subprocess`,
  ask for a larger one; every pipe held 64 KiB and nothing could change
  that.
- `F_GETOWN`, `F_SETOWN`, `F_GETOWN_EX` and `F_SETOWN_EX` failed with
  EINVAL, as a description had nowhere to keep the process that `SIGIO`
  and `SIGURG` are for.
- `RLIMIT_FSIZE` was kept for programs to read and enforced by nothing:
  a file grew past it, and a program that lowers it to protect itself
  from a runaway write was not protected.
- `prlimit` of any process but the caller's failed with ENOSYS, so a
  supervisor could not read or set the limits of a process it runs.
- `sched_getaffinity` and `sched_setaffinity` of a negative number
  failed with EINVAL, where Linux's say ESRCH.
- `pthread_getcpuclockid` always failed with ENOENT and
  `clock_getcpuclockid` with ENOSYS, and `clock_gettime` refused every
  negative clock id, so a program could not tell how much CPU time
  another thread or a child had used.
- `fstatvfs` of a descriptor that is not a directory's failed with
  ENOSYS, though `statvfs` of its path worked.
- `fopen` with a mode that does not start with `r`, `w` or `a` opened
  the file for reading, and `fdopen` took the descriptor, where glibc
  fails both with EINVAL.

## Decision

- **A pipe has a size.** It starts at 64 KiB, as on Linux. `F_SETPIPE_SZ`
  takes a size, rounds it up to a power of two of at least one page, and
  gives the size it set; `F_GETPIPE_SZ` gives the size. The size is read
  as an `unsigned int`, as Linux reads it, so a negative one is a size
  past 2^31 and EINVAL. The largest pipe is 1 MiB, the default of
  Linux's `pipe-max-size`, and growing one past it is EPERM.
  A pipe does not shrink below what it holds (EBUSY). What the pipe
  holds comes out in the order it went in, and the readers and writers
  that wait, and `poll`, are told of the room a larger pipe has. Both
  ends and a named pipe have the one size. Anything but a pipe is
  EBADF.
  radix-file(2)'s `pipe_size` is the call.
- **A description has an owner**: a thread, a process or a process
  group, and none by default. The owner is kept with the description,
  so that every descriptor `dup` or `fork` makes of it, and no other,
  has it, and must be there when it is set (ESRCH). One that has ended
  reads as none. A directory has no owner (EINVAL). radix-file(2)'s
  `owner` is the call; the C library turns `F_SETOWN`'s sign and
  `F_SETOWN_EX`'s type into its kind.
- **`RLIMIT_FSIZE` is enforced by the kernel**, for a write to a file
  and for a file made longer (`ftruncate`, `fallocate`): one that would
  take the file past the soft limit is cut short at it. One that starts
  at the limit or past it sends the writer SIGXFSZ, which ends it unless
  it ignores or catches the signal, and fails with EFBIG. A pipe, a
  socket and a terminal have no size, and the limit does not touch them.
  Each process keeps the soft limit where a write reads it without the
  POSIX state's lock.
- **`prlimit` reaches any process the caller may signal**, as `kill`
  does (ADR 0214): its own, its descendants', and those of its session
  that act for the same user. radix-process(2)'s `limits_of` takes the
  process and the same arguments `limits` does, and `limits` stays for
  the caller's own. A process the caller may not signal is EPERM, one
  that is not there ESRCH. Another process's `RLIMIT_NOFILE` can be read
  and not set: the C library keeps that limit in the process itself.
- **`sched_getaffinity` and `sched_setaffinity` of a negative number are
  ESRCH**; `sched_getscheduler` of one stays EINVAL, as on Linux.
- **CPU-time clocks have ids.** The ids are glibc's: a process's is
  `(~pid << 3) | 2` and a thread's `(~tid << 3) | 6`, which are negative
  numbers, so that no other clock's id is taken for one. radix-process(2)'s
  `cpu_time` reads the time of a thread of the caller's process or of any
  process that is there; a thread of another process, and a thread or
  process that has gone, are EINVAL for `clock_gettime`, as on Linux.
  `clock_getres` gives 1 ns.
- **`fstatvfs` of any descriptor tells of its filesystem**, as `statvfs`
  of its path does; radix-file(2)'s `filesystem_info` is the call, which
  the capability of a file takes as well as that of a directory.
- **`fopen` and `fdopen` refuse a mode that does not start with `r`, `w`
  or `a`**, with EINVAL and without making a file or taking the
  descriptor. Characters after the first that the library does not know
  are left out.

## Consequences

- The CPython tests these gaps held back run in the lab's suite again,
  and their lines leave `lab/tests/services/python-skips.txt`. The test
  of `futimens` of a descriptor open for reading only stays there, as
  a rule and not a gap: a capability that only reads cannot change what
  is said of its file (ADR 0213).
- A pipe's buffer is kernel memory that no resource domain is charged
  for, so one process can hold up to 1 MiB for each pipe it makes. Linux
  caps what a user's pipes hold (`pipe-user-pages-soft`); the same cap
  here waits for kernel memory to be charged to domains, and is a row in
  `docs/deferred.md`.
- An owner is kept and given back, and nothing is sent to it: signal-driven
  I/O (`O_ASYNC`, `SIGIO` and `SIGURG`) is not part of this decision.
- A write to a file opened with `O_APPEND` finds the end just before it
  writes, and the limit is checked against that, as on Linux; a second
  writer that grows the file between the two is not stopped by it.
- Another process's `RLIMIT_NOFILE` cannot be set. A supervisor that
  wants a child to have fewer descriptors sets the limit before the
  child runs, as `setrlimit` in the child itself or before `exec`.
- CPU-time clock ids carry the number of a thread or a process in 28
  bits; a number past 2^28 gives ENOENT from `pthread_getcpuclockid`
  and `clock_getcpuclockid`.
- The `posix` boot test checks each of these: `pipe` the sizes, the
  order of what a resize moves, EBUSY and EPERM, and the room a larger
  pipe gives; `files` the owners, `fstatvfs` and the modes of streams;
  `limits` `RLIMIT_FSIZE` with SIGXFSZ and EFBIG, `prlimit` of a child
  and of process 1, and affinity of a negative number; `time` the
  CPU-time clocks of threads and children.
