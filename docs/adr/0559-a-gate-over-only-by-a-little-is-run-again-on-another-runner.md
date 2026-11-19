<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0559. A performance gate over only by a little is run again on another runner

- Status: Accepted
- Date: 2026-11-19

## Context

The performance gate holds every row to parity with Linux in the same
virtual machine (ADR 0107). A row over it after three runs is measured
again, up to six more times, each side keeping its fastest run, so that
a run the host slowed is set aside.

That answers noise that falls on a different run each time, and not
noise that falls on the same side in every run. A runner's host can do
that for the whole job: on some Intel runners (Xeon 6975P-C, 8370C) a
wake-up across CPUs costs the guest more on one system than on the
other, and the shared-memory channel's round trip on two CPUs stayed
over the gate in all nine runs (1.09 on one pull request, 1.05 on two
that changed only lab tests), while the instruction budgets counted the
same instructions for it as on runs that passed. Each such red was read
by hand and the job run again, which lands on another machine, and it
passed. A red gate that a rerun turns green teaches people to rerun it,
which is how a real loss gets through.

## Decision

- `ob bench --gate --defer-close` measures as `--gate` does. If, after
  the last run, every row still over the gate is within `CLOSE_RATIO`
  (1.10) times Linux's time, it names those rows in
  `out/bench/close.txt` and passes. A row further over, or one not
  measured, fails at once, as before.
- CI's bench job runs the gate that way, warns on the run with the
  rows' names, and a second job, "Linux parity, on another runner",
  then runs `ob bench --gate` on a fresh runner, set up the same way
  from the same caches. Its verdict is the gate's, and it hands nothing
  on.
- `GATE_RATIO` stays 1.00. The gate passes only when one runner, from
  start to end, measures every row at parity or under.

## Consequences

- A loss of a few percent is in Radix's every run on every machine, so
  the second runner fails it too, and the pull request is red as before;
  it only learns that an hour later. A loss of more than a tenth fails
  on the first runner.
- A host that slows one side of a row on one machine no longer fails a
  pull request, unless the second runner is a machine of the same kind
  that does the same. That is a real limit: GitHub picks the machine,
  and two Intel 6975P-C runners in a row will still fail the gate. The
  answer there remains making the row faster on those machines.
- The second job takes about as long as the first, an hour with its
  reruns, and runs only when the first passed its gate on to it. The
  required check, "CI passed", waits for neither.
- The nightly run on main does the same, so a close row on main is run
  twice before it shows as red.
