<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0359. A thread is gone from its process before its joiner wakes

- Status: Accepted
- Date: 2026-11-21

## Context

A thread that ends with the C library goes through its
`ThreadExitUnmapping` sysdep (mlibc patch 0091): in registers alone, it
discards its stack, sets the word `pthread_join` waits on, wakes the
joiner with `futex_wake` and calls radix-process(2)'s `exit_thread`. The
joiner could therefore return, and its caller go on, before the kernel
had taken the thread out of its process. In that gap the thread's number
still named it: `clock_gettime` of its CPU-time clock read its time
where ADR 0497 promises EINVAL. The `posix` boot test's `time` program
checks the clock right after the join, and failed in CI when the
joiner's CPU won the race.

On Linux the kernel, not the thread, clears the word and wakes its
waiters on the thread's way out (`clone`'s `CLONE_CHILD_CLEARTID`), but
before the thread leaves its thread group, so a narrower gap remains
there.

## Decision

- **radix-process(2) gains `exit_thread_waking(code, word)`.** It ends
  the calling thread as `exit_thread` does, and once its process no
  longer has it, sets the 32-bit word at `word` to 1 and wakes every
  thread waiting on it. A word out of line with 4 bytes, or one the
  thread cannot write, is left alone; the thread ends all the same. The
  process interface goes to version 12.
- **The C library ends a thread with it.** The sysdep discards its stack
  and makes the one call, in registers as before. The word is the TCB's
  `didExit`, so the TCB is freed only once the thread is gone.
- `exit_thread` stays, for a thread whose end nobody waits on.

## Consequences

- When `pthread_join` returns, no call finds the thread by its number,
  and its CPU-time clock is EINVAL at once.
- The kernel writes the word on the thread's way out, after its process
  has let go of it. If that was the process's last thread, or another
  thread ends the process meanwhile, nobody is left to wait: the write
  meets an emptied space, or one the CPU was flushed of first, and does
  nothing.
- The `posix` boot test's `time` program checks the clock of a joined
  thread at once, as before, and now always finds it gone.
