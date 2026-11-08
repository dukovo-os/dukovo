<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0416. The scaling rows are timed from one start

- Status: Accepted; amends [0409](0409-the-scaling-rows-are-gated-at-parity-with-linux.md)
- Date: 2026-11-08

## Context

`ob bench --scaling` times 1, 4 and 8 workers opening and closing files
at once on 4 CPUs, on Radix and on Linux in the same virtual machine,
and CI holds every row at Linux's time or under (ADR 0409). Each worker
timed itself from the moment it began to the moment it was done, and a
row was the slowest worker's time.

With 8 workers two share each CPU, and the scheduler lets them go one at
a time: the second begins a slice after the first, 3 ms on Radix and
about 2.3 ms on Linux. Which of the two then ends last, and how long
after the other, depends on where the ~37 ms of work each has falls in
the scheduler's turns. A worker that began first and happened to end
last was the slowest by up to the slice it had before the other began.

After ADR 0414 the EPYC 9V74 runners put the 8-process row at 0.99,
1.00 and 1.01 of Linux's time on three runs, with the 4-process row at
0.98 to 0.99. Each worker's start, end and CPU time, and each CPU's busy
and idle time, showed why:

- no CPU sat idle during the row, and other threads took about 0.15 ms
  of each CPU's ~75 ms, as on Linux;
- on every run Radix's last worker was done sooner after the common
  start than Linux's;
- Radix's first starter ended about 0.8 ms before its partner and
  Linux's about 1.5 ms, and Radix's sometimes ended after it.

A model of the fair queue (`radix_sched::fair`) run on the host gave
both tie rules and both slice lengths the same average for that last
quantity. What moved the row was where the work fell, not what it cost.

| 9V74 run | 4 processes | 8 processes, slowest worker | 8 processes, from one start |
|---|---:|---:|---:|
| 1 | 0.986 | 0.994 | 0.987 |
| 2 | 0.980 | 0.997 | 0.992 |
| 3 | 0.984 | 1.006 | 0.995 |

Timing from one start shows a delay that the old timing hid. On one of
three EPYC 7763 runners, at the start of 13 of 18 rows, one CPU's worker
began 26 to 35 ms after the others, its CPU busy all the while; on other
runners a CPU's first worker now and then began 2 to 3 ms late. The KVM
steal time the host reports for each CPU named it: in every late start
recorded, the worker itself held its CPU, and the host had stopped the
virtual CPU for nearly all of that time (3,013 µs of a 3,048 µs run,
2,999 µs of 3,064 µs). No other thread of Radix's ran.

The same loop on both systems, reading the time stamp counter after
each open and close, showed that the host stopped Radix's machine and
not Linux's. In the 4-worker rows, where all four virtual CPUs are busy,
Radix's workers lost 2 to 12 ms per three runs to steps of 0.2 ms or
more on the EPYC 7763 runners, and Linux's 0 to 1 ms; with one worker,
when the host had CPUs to spare, neither lost any. The host's work was
`ob`'s own. While a machine runs, `Vm::expect` looked at its serial
output every 50 ms, and each look copied and cleaned all of it, in a
build without optimizations. Radix's output, with GRUB's screen and the
kernel's boot messages, is 60 to 97 KB by the end, and a look took 1 to
5 ms of a host CPU; Linux boots `quiet`, its output is 15 KB, and a look
took 0.3 to 0.9 ms. With every host CPU running a virtual CPU, the host
took that time from one of them.

## Decision

- A row is the time from the moment the workers are let go together
  until the last of them is done, per open and close. The program that
  starts them reads the clock just before it lets them go, and each
  worker reads it when it is done. Radix and Linux run the same
  program, so both sides are timed the same way.
- Every row is timed so, including one worker's, where it is that
  worker's own time and its wake-up, so that "scaling" (a row's time
  over one worker's) compares like with like.
- `Vm::expect` cleans and searches a machine's serial output again only
  once the machine has printed more since its last look. A machine that
  prints nothing, as the benchmark does while its rows run, costs the
  host nothing.
- The gate stays at parity (`bench::GATE_RATIO`, 1.00) and its extra
  runs stay as ADR 0409 has them.

## Consequences

- Start order no longer moves a row: with 8 workers a row is the time
  the 8 workers' work takes, twice one worker's if nothing is shared.
- Anything that keeps a worker from starting now counts, on either
  side: a thread that holds a CPU when the workers are let go makes the
  row slower, as it makes a user's programs slower. So does a host that
  stops a virtual CPU for work of its own, and `ob` no longer gives it
  any while a row runs: with the change, Radix's 4-worker rows on two
  EPYC 7763 runners lost 0 to 1.2 ms per three runs to steps of 0.2 ms
  or more, and Linux's 0 to 4.2 ms. Stops that KVM's steal time does
  not show stay, on both sides: on an Intel 8573C runner Radix's
  4-worker rows lost up to 16 ms per three runs and Linux's up to 8 ms,
  with under 1.5 ms of Radix's counted as stolen. The gate's extra runs,
  which keep each side's fastest, absorb a run they slow.
- Every boot test and benchmark spends less of the host's CPU while it
  waits on a machine. What `expect` finds does not change: it searches
  the same output, only not again when nothing was added.
- Rows measured before this ADR are not comparable with rows after it
  for 4 and 8 workers; the one-worker rows are within microseconds.
