<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0409. The open and close scaling rows are gated at parity with Linux

- Status: Accepted
- Date: 2026-11-07

## Context

`ob bench --scaling` (#215) times opening and closing files with 1, 4
and 8 workers at once on 4 CPUs, as threads of one process and as
processes of their own, next to Linux in the same virtual machine. It
was a report only: it measured whether the capability store's one lock
was worth splitting (ADR 0369), and CI ran it with `continue-on-error`.

Since then:

- the store's lock was split (ADR 0369);
- a mount is counted on each CPU (ADR 0404);
- open and close hold their space's lock only for the slot (ADR 0405);
- the locks, the heap and the system call entry call the kernel inline
  (ADR 0406);
- each object record has cache lines of its own (ADR 0407).

With all of that, every row beats Linux on CI's KVM runners:

| Workers | EPYC 9V45 | EPYC 7763 |
|---|---:|---:|
| 1 thread | 0.92 | 0.93 |
| 4 threads | 0.70 | 0.83 |
| 8 threads | 0.67 | 0.85 |
| 1 process | 0.90 | 0.96 |
| 4 processes | 0.96 | 0.92 |
| 8 processes | 0.98 | 0.94 |

Each is Radix's time over Linux's, from #249's runs.
The goal (Chase, 2026-11-07) was that every row beat Linux and that CI
then hold it there, as `ob bench --gate` holds every other operation
(ADR 0107).

## Decision

- `ob bench --scaling --gate` fails unless, on every row, Radix's
  slowest worker takes at most Linux's time: `bench::GATE_RATIO`, 1.00,
  the same parity as ADR 0107. A row neither side printed counts as
  over.
- While a row is over after the first runs, it boots Radix and Linux in
  turn again, up to `bench::MORE_RUNS` (six) more times, each side
  keeping its fastest run of every row, and stops once no row is over,
  as ADR 0107 does.
- `--gate` needs Linux, so it does not go with `--no-linux`. Without
  KVM Linux's side does not run, and the gate fails.
- CI's benchmark job runs `./ob bench --scaling --gate`, without
  `continue-on-error`. Like the rest of the benchmark job it stays out
  of the checks main's ruleset requires: a red one is read by hand.

## Consequences

- A change that makes open and close slower than Linux's with any
  number of workers, or one worker's slower than Linux's, turns the
  benchmark job red.
- The step takes about a minute when every row is under the gate after
  three runs. Each extra run adds about 20 seconds.
- The closest rows are within 10 % of Linux: one worker, and four and
  eight processes. A runner whose noise always falls on Radix's side
  can fail them; the answer, as in ADR 0107, is to make open and close
  faster, never to give them room.
