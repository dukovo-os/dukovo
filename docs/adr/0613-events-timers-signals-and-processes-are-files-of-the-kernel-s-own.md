<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0613. Events, timers, signals and processes are files of the kernel's own

- Status: Accepted
- Date: 2026-11-23

## Context

Linux turns four things a program waits for into descriptors, so that one
`poll` or epoll wait covers them with its sockets: a counter other threads
add to (`eventfd`), a timer (`timerfd`), the signals pending for a thread
(`signalfd`) and the end of a process (`pidfd`). Event loops, init
systems, language runtimes and service managers use them. Radix has
counters, timers and process ends of its own (notifications, POSIX
timers, `wait`), but none of them is a file a descriptor can name, and
the event queue and `poll` watch files.

## Decision

- **Each is a node of the pipes' filesystem** of character device kind,
  as a tunnel's end is (`fs/tunnel.rs`), made by a method of the process
  (radix-process(2) version 13): `event_counter`, `timer_file`,
  `signal_file` and `process_file`, each putting the file in a slot the
  caller names, with the non-blocking and close-on-exec flags. The file
  object recognises the node (`FileObject::opened_as`), so reads, writes,
  `poll`, the event queue and `close` reach it as they reach a pipe. What
  the four have alike is `fs/own_file.rs`.
- **An event counter** (`fs/counter.rs`) is one atomic count. An 8-byte
  write adds to it and waits while the sum would pass `2^64 - 2`; an
  8-byte read takes all of it, or 1 with `semaphore`, and waits while it is
  0. It is readable while the count is not 0 and writable while a write of
  1 fits. A read whose buffer cannot be written puts what it took back.
- **A timer file** (`fs/timer_file.rs`) has a clock (realtime, monotonic
  or boottime), an expiry and an interval, which radix-file(2)'s
  `timer_set` and `timer_get` (version 14) set and read. Its expiries are
  counted, not queued: a read takes the count. One kernel thread, `timer
  files`, sleeps until the first armed timer is due and counts its
  expiries, as `posix::timer`'s thread does for a process's POSIX timers
  (ADR 0650); a read counts what is due itself first, and `poll` and
  `timer_get` count on a copy, so only the thread and a read change the
  count and every change wakes the readers. A realtime timer keeps its
  expiry as a time of day, so setting the clock moves it; setting the
  wall clock (radix-clock(2)'s `set`, or Hyper-V's time sync) cancels the
  timers armed absolute with `cancel_on_set`, whose next read fails once
  with the new error `cancelled` (ECANCELED).
- **A signal file** (`fs/signal_file.rs`) has a mask, which
  `set_signal_mask` changes. A read takes the signals of the mask pending
  for the reading thread, its own before its process's, as `signal_wait`
  takes them (`signal::take_pending`), and writes each as Linux's 128-byte
  `struct signalfd_siginfo`, as many as fit. The signals are the reader's,
  not the file's: a poll finds the file readable while one is pending for
  the thread that polls, and a process's `polled` is told whenever a
  signal is queued for it.
- **A process file** (`fs/process_file.rs`) holds its process weakly and
  its number, which Radix never gives again. It is readable once the
  process has exited, and hung up once it has been waited for; its reads
  and writes fail. `process_of` gives the number and whether it exited,
  for `pidfd_send_signal` and `waitid(P_PIDFD)`, which the table turns into
  the process's `kill` or `signal_queue` and a `P_PID` wait. A process
  file is closed by `exec` whatever its flags say, as a pidfd is.
- **The Linux table** (`user/linux/kernel_files.rs`) gives `eventfd2`,
  `timerfd_create`, `timerfd_settime`, `timerfd_gettime`, `signalfd4`,
  `pidfd_open`, `pidfd_send_signal`, `CLONE_PIDFD` and `waitid`'s `P_PIDFD`,
  with Linux's errors in Linux's order; the flags and records are
  `radix_linux::kernel_files`'s, with host tests. Dukovo's C library makes
  the same calls (`sysdeps/radix/generic/kernel_files.cpp`, mlibc's
  `linux-eventfd`, `linux-signalfd` and `linux-timerfd` options and its
  `pidfd_open` calls, which mlibc patch 0121 takes out of its Linux
  option).

## Consequences

- No new kind of object: every one is a file, so the boot report's count
  of object kinds stays as it was, and each is closed, passed over a Unix
  socket and inherited as any file is.
- Polling a signal file costs a signal's sender one more change of the
  process's `polled`, two loads while nothing polls.
- Differences from Linux, rows of docs/deferred.md: the files have no
  `anon_inode:` names in the process's view; `ssi_uid` is 0, as the native
  record of a signal keeps no sender's user; `PIDFD_THREAD`, the thread
  flags of `pidfd_send_signal` and `TFD_IOC_SET_TICKS` are not there;
  `pidfd_getfd` is EPERM, as `ptrace` is; the alarm clocks are EPERM, as
  for a program without `CAP_WAKE_ALARM`; Dukovo's C library copies a
  record without the kernel's EFAULT check.
- The `posix` boot test runs `kernelfiles.c` natively and under
  `linux-programs`; it passes on Linux first.
