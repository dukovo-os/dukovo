<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0617. Futexes take bit sets, requeue and wake-op, and a Linux thread's robust list is walked at its end

- Status: Accepted
- Date: 2026-11-23

## Context

Radix's futexes (radix-address-space(2), Phase 8) wait on a word and wake
its waiters, which is what Dukovo's C library needs. glibc and musl use
more of Linux's futex: `FUTEX_WAIT_BITSET` with an absolute deadline, on
the monotonic or the realtime clock, for every timed wait;
`FUTEX_CMP_REQUEUE` in condition variables, to move waiters from the
condition's word to the mutex's instead of waking them all to fight for
it; `FUTEX_WAKE_OP` in older condition variables; and robust mutexes,
whose holder's death the kernel must report to the next locker through
the thread's robust list. ADR 0611 left all but plain waits and wakes for
this step, with the robust list kept but never walked.

## Decision

- **Bit sets** (radix-address-space(2) version 4): `futex_wait_set` waits
  with a bit set, and `futex_wake_set` wakes only the waiters whose sets
  share a bit with its own; `futex_wait` and `futex_wake` use all bits.
  The deadline is a time from now, or with `absolute` a time on the
  monotonic clock or, with `realtime`, on the wall clock, which is turned
  into a monotonic time once, as the wait begins.
- **`futex_requeue`** wakes up to `count` waiters of one word and moves
  up to `moved` others to wait on a second, where a wake of that word
  finds them; with `compare` it first checks that the first word holds
  `expected` (would_block, EAGAIN, if not). Both words may be private or
  shared, each named as `futex_wake` names it. It locks both waiter lists,
  the lower one in the table first, and does all its work under them, so
  a waiter that checked either word before cannot sleep through it. A
  waiter's list is the one its thread names, which only a requeue
  holding both lists changes, so a waiter leaving for a timeout or a
  signal looks for itself in the list it is in now.
- **`futex_wake_op`** changes the second word by Linux's encoded
  operation in one atomic step and wakes waiters of the first word, and of
  the second if its old value compares true.
- **Robust lists** (`user/linux/robust.rs`): when a Linux thread ends, or
  runs `exec`, the kernel walks the list `set_robust_list` named, as
  Linux's `exit_robust_list` does, at most `ROBUST_LIST_MAX` (2,048)
  entries and the pending one last: each word the thread still owns gets
  `FUTEX_OWNER_DIED`, keeping `FUTEX_WAITERS`, and one waiter wakes if
  there were any, for the C library to give `EOWNERDEAD`. A word that
  cannot be read or written, or is not 4-byte aligned, ends the walk.
- **The Linux table** turns `futex`'s operations into these methods, and
  the operations' decoding, `FUTEX_WAKE_OP`'s arithmetic and the robust
  word's change are `radix_linux::futex`'s, with host tests. The
  priority inheritance operations are ENOSYS, as on a Linux built without
  them.
- `kernel/radix/tla/FutexRequeue.tla` checks the requeue with three
  waiters, one or two of them interrupted, and a move that may wake a
  waiter for want of memory (34,995 and 135,081 states): no wake-up is
  lost, a wait ends as woken exactly when a wake or a requeue took the
  waiter off a list, a waiter is in one list at most and its bucket names
  it, and every wait ends. The check fails if a waiter that leaves does
  not read its bucket again once the list is locked.

## Consequences

- glibc's and musl's condition variables, timed waits and robust mutexes
  work as on Linux; `linux-programs`' `core.c` covers bit sets, absolute
  deadlines on both clocks, requeue with and without compare, wake-op and
  a robust mutex whose holder dies.
- Differences from Linux, rows of docs/deferred.md: a realtime deadline
  does not move when the wall clock is set during the wait; an unknown
  wake-op comparison is refused before the word is changed, where Linux
  changes the word first; private and shared waits of the same word are
  not told apart; an entry of a robust list marked for priority
  inheritance has its word marked but wakes no one.
- Dukovo's C library has no robust mutexes, so natively nothing walks a
  list.
