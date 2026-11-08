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

Timing from one start shows a stall that the old timing hid. On one of
three EPYC 7763 runners, at the start of 13 of 18 rows, one CPU ran
something other than its worker for 26 to 35 ms before the worker
began; Linux on the same runner never did.

## Decision

- A row is the time from the moment the workers are let go together
  until the last of them is done, per open and close. The program that
  starts them reads the clock just before it lets them go, and each
  worker reads it when it is done. Radix and Linux run the same
  program, so both sides are timed the same way.
- Every row is timed so, including one worker's, where it is that
  worker's own time and its wake-up, so that "scaling" (a row's time
  over one worker's) compares like with like.
- The gate stays at parity (`bench::GATE_RATIO`, 1.00) and its extra
  runs stay as ADR 0409 has them.

## Consequences

- Start order no longer moves a row: with 8 workers a row is the time
  the 8 workers' work takes, twice one worker's if nothing is shared.
- Anything that keeps a worker from starting now counts, on either
  side: a thread that holds a CPU when the workers are let go makes the
  row slower, as it makes a user's programs slower.
- Rows measured before this ADR are not comparable with rows after it
  for 4 and 8 workers; the one-worker rows are within microseconds.
