<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0812. An idle CPU pauses its tick

- Status: Accepted
- Date: 2026-11-28

## Context

Every CPU's timer ticks a hundred times a second (ADR 0037). The tick
charges the running thread and ends its slice, balances the CPU's load
against the others', lets an RCU grace period pass, picks the CPU's
speed, tells the kernel log's readers of new records (ADR 0811), and on
the boot CPU samples the load averages. A CPU that runs nothing needs
none of that for itself, and each tick wakes it from its halt.

On a desktop with nothing moving, in a virtual machine of two CPUs, the
CPUs halted 261 times a second, 200 of them for a tick, where Linux
under labwc halted 93 times (ADR 0724's row for a second at rest, on a
Ryzen 9 9900X3D under KVM). Each halt and the interrupt that ends it
are exits to the host, about 21 µs of its processor time a time: the
row was 5.6 to 6.6 ms a second beside Linux's 2.2 to 3.6 ms. On a real
machine each tick takes the CPU out of whatever idle state it reached,
so a state whose target residency is over 10 ms never pays, and a
laptop at rest wakes every core a hundred times a second for nothing.
Linux has stopped the tick of idle CPUs since 2.6.21.

## Decision

An idle CPU pauses its tick as it halts, and the tick comes again as
the CPU leaves idle for a switch. The timer's deadline, the next
sleeper's, interrupts as ever.

- **Two ways to pause** (`Timer::tick_pause`). *At once*: the timer is
  written for the deadline alone, or disarmed, and written again for
  the tick as the CPU leaves idle. No tick wakes the CPU, for two writes
  of the timer, each an exit in a virtual machine. *Lazily*: nothing is
  written. The tick that is armed comes once more and is not armed
  again; a CPU that leaves idle before it has paid nothing.
- **Each CPU guesses which its next halt deserves**
  (`radix_sched::tickless::TickPause`). A halt is *long* from a quarter
  of a tick, about where the chance that a tick falls in it outweighs
  two writes. Whether each of the CPU's last three halts was long makes
  a pattern of eight, and a two-bit count for each pattern says whether
  the halt that followed it was long: a processor's two-level branch
  predictor, small enough to reason about. Guessed long, the tick
  pauses at once; guessed short, lazily. A machine at rest pauses at
  once from each CPU's sixth halt. A frame's pattern of short waits and
  one long is guessed right once it has come round three times. A
  thread that waits 50 µs for a disk never writes the timer for it.
- **A halted CPU rests, and grace periods do not wait for it.** The
  idle loop says that the CPU rests just before it halts
  (`CpuLocal::rest`), then passes a quiescent state for a grace period
  already under way. The interrupt that ends the halt ends the rest
  before its handler runs. A grace period starts with the CPUs that do
  not rest, and completes at once when all do. Both sides pass a full
  barrier, so a handler's reads come after what the grace period's
  caller took away (`docs/design/scheduler.md`, argument 11). While it
  rests the CPU takes the grace periods' own lock and nothing else: if
  its quiescent state completes a grace period, it ends its rest to
  wake who waited.
- **A CPU with a thread waiting wakes a resting CPU to pull it**
  (`balance::offer`), at each of its ticks: the next CPU up that rests
  and may run the thread, and every 8th tick the next one from a place
  that moves on, so that one CPU's failures to pull add up as before
  and no CPU is left unasked. Idle CPUs used to try at each of their own
  ticks.
- **The rest of the tick's work for an idle CPU**: the kernel log's
  readers are told as a CPU goes round its idle loop; a CPU whose speed
  the kernel picks counts its load from the moment it leaves idle
  (`pstate::restart`), not from its last tick; the boot CPU's tick comes
  back by itself for the load averages, every 5 seconds.
- **Boot checks that every CPU ticks first**, and ticks pause from then
  on. The kernel flag `tick=always` keeps them, as Linux's `nohz=off`
  does, for telling whether a fault is the pause's.

## Alternatives

- **Always pause at once.** A program that answers another across CPUs
  halts for tens of microseconds at a time; two writes of the timer at
  each halt are two exits in a virtual machine, where the benchmarks'
  rows are measured against Linux.
- **Always pause lazily.** One tick still wakes each CPU after it went
  idle: a frame's long wait would be cut by it every time, on both CPUs.
- **Guess from the next sleeper's deadline, as Linux's menu governor
  starts.** The deadline says how long the halt may last, not how long
  it will: a CPU of a desktop is woken by another CPU's work or a
  device far more often than by its own timer.
- **Guess with the idle states' governor** (ADR 0795). It learns which
  state's residency a halt reaches, from bins of the states the
  firmware names; a virtual machine has one state, and the question
  here has its own threshold.
- **A grace period that wakes every idle CPU**, or a thread that passes
  quiescent states for them. A burst of interrupts for each grace
  period on a large machine at rest, where a flag read does.
- **Keeping idle CPUs in every grace period and letting them time
  out.** A grace period would last until each idle CPU's next wake-up,
  which may be seconds.
- **Stopping the tick of a CPU that runs one thread too** (Linux's
  `nohz_full`). The thread's time is charged and its CPU balanced from
  the tick; that is a different change, for workloads that ask for it.

## Consequences

- On the same machine the two CPUs halt 62 to 64 times a second at
  rest, none of them for a tick, where they halted 261 times and
  Linux's halt 92 to 94 times in the same seconds.
- The row for a second at rest was 0.57 and 1.52 times Linux's in two
  runs, from 1.73 and 1.92. Both systems are still starting in the
  seconds the row measures, and each one's time there differs by half
  from run to run. In the eight seconds after, Dukovo's CPUs ran 1.5 to
  4.0 ms a second and Linux's 0.8 to 1.9 ms: what wakes Dukovo then is
  its services' own timers, some ten of them between once and ten
  times a second, and the look at the removable drives every two
  seconds (`docs/deferred.md`).
- The rows for a frame and a resize take the time at rest away from
  what they measure, and do not change with it.
- A real machine's idle CPUs stay in their idle states until something
  wakes them. The idle states' governor sees the true time to the next
  timer and may choose the deepest state for a CPU at rest.
- A CPU that leaves idle after its tick stopped writes the timer once
  more than before, and one that pauses at once writes it as it halts.
- Grace periods are shorter on a machine with idle CPUs: they no longer
  wait up to a tick for each.
- A waiting thread is pulled by an idle CPU within a tick, as before,
  now of the busy CPU's.
- The architecture traits have four more functions, with defaults that
  do nothing: an architecture without them keeps its tick and is waited
  for.
- A CPU whose speed the kernel picks through the firmware's P-states
  (ADR 0796) keeps the speed of its last tick while it rests, where it
  was set to the lowest at the first idle tick.
- Not yet run on a real machine: the lab's performance tests, beside a
  run with `tick=always`, are the check (`docs/deferred.md`).
