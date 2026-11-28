<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0811. The kernel log's readers are woken from the tick, not by a poll

- Status: Accepted
- Date: 2026-11-28

## Context

A read of `/devices/kmsg` waits for the next record. The log cannot
wake the reader as it takes the record: a record may be logged with any
lock held, even in an interrupt, and waking takes the wait table's
lock. So a kernel thread looked for new records every 10 ms while the
device was open, and woke the readers when there were any.

logd keeps the device open on every machine. The thread therefore woke
a CPU a hundred times a second for as long as the system ran, whether
anything was logged or not. On a desktop with nothing moving, in a
virtual machine of two CPUs, that was 100 of the 346 times a second a
CPU left its idle state, beside 200 timer ticks and 46 wake-ups of
programs' own timers (ADR 0724's row for a second at rest).

## Decision

The thread sleeps until there is a record, and the timer's tick tells
it.

- **The log's next sequence number is also kept in an atomic**
  (`radix_klog::next_seq`), read without the log's lock, which a CPU
  may hold while it writes a record to a console.
- **Every CPU's tick compares it** with the number the readers were
  last woken for (`kmsg::look`): three loads when nothing was logged.
  When they differ and the device is open, it wakes the watcher's
  thread, which an interrupt handler may do: that takes only the
  thread's and a run queue's locks.
- **The watcher wakes the readers and `poll`**, as before, and sleeps
  again. A record logged between its look and its sleep makes the next
  tick wake it once more.

## Alternatives

- **Waking the readers from the tick.** The wait table's lock is taken
  by threads around other locks with interrupts on; an interrupt
  handler that took it could wait for the thread it interrupted.
- **Waking the watcher from the log itself.** A record is logged from
  inside the scheduler too, with a run queue locked.
- **A longer poll.** Fewer wake-ups for nothing, and a slower `dmesg
  -w` and journal for it.

## Consequences

- A machine that logs nothing is not woken for its log.
- A reader still hears of a record within a tick of it, on a CPU that
  ticks. A CPU that stops its tick while idle must look as it goes
  idle, since what it logged last has no tick after it.
