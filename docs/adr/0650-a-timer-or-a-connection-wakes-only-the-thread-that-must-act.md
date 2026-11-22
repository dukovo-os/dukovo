<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0650. A timer or a connection wakes only the thread that must act

- Status: Accepted
- Date: 2026-11-22

## Context

A9's first step measured PHP-FPM at 6.9 times Linux's time per request.
ADR 0558 addressed part of the gaps of Valkey and PostgreSQL. A probe in
QEMU (counters from `/processes/stat`) found two more causes for PHP-FPM,
both wake-ups of threads that then had nothing to do:

- **The timer thread.** PHP sets `ITIMER_PROF` to 30 s at the start of
  each request and turns it off at its end. Each `setitimer` called
  `changed`, which woke the kernel's one timer thread, often on another
  CPU. The thread looked at every armed process and slept again. It was
  the top source of wake-up IPIs while FPM served. While any CPU-time
  timer was armed, the thread also woke every 10 ms (`CPU_TICK_NS`) to look
  at it: a 30 s timer cost 3,000 looks.
- **The accept herd.** FPM's four static workers all block in `accept` on
  one Unix socket. Every connection woke all four (`Listener::changed`
  called `wake_all` on one key); one took the connection and the others
  found the queue empty and slept again.

Linux wakes one exclusive waiter for each connection (a Unix `accept`
waits in `__skb_wait_for_more_packets`, a TCP one in
`inet_csk_wait_for_connect`, both exclusively), and checks CPU-time
timers at the tick that counts the time, so setting one wakes nothing.

## Decision

- **The timer thread says when it will look next.** `NEXT_LOOK` holds the
  time the thread sleeps until, or `u64::MAX` if only a wake ends its
  sleep. The thread stores it after each look, before its wait reads
  `CHANGES`.
- **A setter wakes the thread only if its timer is due sooner.** It moves
  `CHANGES`, then reads `NEXT_LOOK`, and wakes the thread only if the
  timer must be looked at before that time. A timer turned off, or set
  later than the thread already plans to look, wakes nobody. Both
  accesses are sequentially consistent, so either the thread's wait sees
  `CHANGES` move or the setter sees the time the thread sleeps until.
- **A CPU-time timer is looked at when it could be due.** CPU time is
  counted at ticks, so the thread cannot look sooner than `CPU_TICK_NS`
  from now. It looks once the process could have used what is left of the
  timer on every CPU at once, and no sooner than that tick: a far timer is
  looked at a few times as it nears, not at every tick. The thread no
  longer wakes every 10 ms for as long as a CPU-time timer is armed; a
  CPU-time timer still fires within a tick of its time.
- **A listener has one key for the accepts and one for the connects.** A
  connection that is queued, or put back by a failed accept, wakes one
  blocked `accept`. An `accept` that takes a connection wakes all the
  connects that wait for room in the backlog. `listen` with a new backlog
  and `close` wake both. A woken `accept` that a signal interrupted still
  takes a connection that waits, so a wake is not lost to it; it returns
  EINTR only when none waits.
- **An interval timer replaced while due sends its signal then.** With
  the thread looking only when it must, and CPU time moving at ticks, a
  timer can be due a while before the thread sends its signal.
  `setitimer` on such a timer sends the signal it owed at once, and gives
  as what was left the time to the expiry after (none for a timer
  without an interval). Before, a due timer read as a little way off, and
  mlibc's `getitimer`, which sets the timer again with what it read, moved
  it that little way on at each read: a program that reads its timer in a
  loop, as `time.c`'s `timers` part does, saw it fire only when a look of
  the thread fell between a tick and the next read, which the old wakes
  on every set made likely and the new rule makes rare.
- **The wait table can wake one waiter.** `wait::wake_one` wakes the
  waiter on a key that has waited longest, and `WaitCount::wake_one` does
  the same for a counted wait.

## Consequences

- Setting and turning off a far timer costs no wake-up and no IPI. The
  `timer_wakes` part of `userspace/posix/time.c` checks it: 2,000 rounds
  of setting `ITIMER_PROF` to 30 s and turning it off make under 1,000
  context switches in all, where each set used to cost at least 2. It
  also checks that a nearer timer set while the thread waits for a far one
  (an interval timer and a POSIX timer, on CPU time and on the real-time
  clock) still fires on time, and that a timer turned off does not fire.
- A connection wakes one of the processes blocked in `accept`. The
  `accepting` part of `userspace/posix/socket.c` checks it: four children
  block in `accept` on one socket and 500 connections make their
  voluntary switches total at most 750, where waking all four made about
  2,000.
- A setter that finds the thread asleep until a later time wakes it, so a
  timer set nearer than the thread's plan is never slept through. The
  model `kernel/radix/tla/Timers.tla` checks it, as `EveryTimerFires`: TLC
  1.7.4 finds no error in 572,429 distinct states, to depth 56, in 42 s.
  Each of three mutations makes `EveryTimerFires` fail: the setter reading
  `NEXT_LOOK` before it moves `CHANGES`, the thread writing `NEXT_LOOK`
  after its wait reads `CHANGES`, and the thread reading `CHANGES` after
  it looks.
- The accept rule needs no model of its own. Each queued connection
  issues one `wake_one` after it stores the count of waiting connections.
  A waiter checks that count under the wait table's lock before it sleeps
  (the `WaitCount` argument of ADR 0405, checked in `EventQueue.tla`). A
  woken waiter takes the connection unless another thread took it first,
  so every wake is used, or its connection is.
- A CPU-time timer still fires within a tick of its time: the thread
  cannot look before the process could have used what was left, and once
  less than a tick's worth on every CPU is left it looks at each tick, as
  before.
- TCP listeners keep their own waits (`inet.rs`), as does the stack's
  lock in TCP's poll readiness. Neither is changed.
- There is no ABI change. What programs see changes in one way: a due
  interval timer that `setitimer` replaces sends its signal at once, as
  on Linux, where it would have gone out at the tick.
