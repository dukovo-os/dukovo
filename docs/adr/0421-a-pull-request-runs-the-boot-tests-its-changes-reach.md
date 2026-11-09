<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0421. A pull request runs the boot tests its changes reach

- Status: Accepted
- Date: 2026-11-09

## Context

There are 158 boot tests. By `tools/xtask/src/test/times.txt` they take
about 105 minutes on CI's machines, plus three minutes to pack the
catalog's bundles for the tests that install them. Every pull request
ran all of them, split between five jobs (#235), each of which also
spends about 12 minutes setting up and building the system before its
first test. Two days after the split, new tests had brought each job
back to about 45 minutes, against a goal of under 30 minutes for a pull
request's CI. Adding machines does not hold the time down for long, and
the account's runners are shared by every run, so more jobs also wait
longer for a runner.

Most of a pull request's boot tests cannot fail because of it: a change
to the installer cannot break the WireGuard test. But the changes that
reach everything are common. Of the 60 pull requests merged before this
one, 44 changed the kernel, the system call interface, the C library,
Spark, the test harness or CI, which every boot stands on; 14 changed
only the lab's tests or writing, and 2 only user-space programs.
Running every test for the first group would leave most pull requests
at 45 minutes.

Main's runs, which ran every test, were cancelled by the next merge, so
on a busy day main's full results were seldom recorded.

## Decision

- `ob test boot --affected <commit>` runs the boot tests that the
  changes since `<commit>` reach (the commit where HEAD left it, the
  working tree's changes and new files included). Each changed file
  counts in one of four ways, the first that fits:
  - A path no boot test reads (`NOTHING` in
    `tools/xtask/src/test/affected.rs`): Markdown, the ADRs, the
    handbook, the TLA+ models, the lab's tests, other jobs' tools. It
    adds no test.
  - A test's own file, `tools/xtask/src/test/cases/<name>.rs`: that test.
  - A path that tests name in `tools/xtask/src/test/covers.txt`: those
    tests. If the path is also in `CORE` (the kernel, `abi/`, the C
    library and the Rust runtime, Spark and the system's defaults, the
    harness and the image, the toolchain, CI), the broad sample too.
  - Any other path: the broad sample, since nothing says what uses it.
- `covers.txt` names, for each test, what it exercises beyond the boot
  every test makes: its programs and test programs, the commands it
  types, the drivers and filesystems it uses. A unit test checks that
  every line names a test and every path exists.
- The broad sample (`SAMPLE`) is about 14 minutes of tests: each way
  of booting, the kernel's self-tests, Spark, the services and the
  console, the `posix` suite (the C library and every command), a
  filesystem of each kind, the network, SSH and TLS, accounts, USB and
  an installed system. A unit test holds it within two machines' share.
- Whenever any boot test runs, so do the smoke tests (`uefi-usb`,
  `bios-cdrom`, `spark`) and every test `covers.txt` does not name.
- `--plan` prints how many machines the chosen tests need, one for each
  ten minutes of tests by `times.txt`, at most five, in the form
  `$GITHUB_OUTPUT` takes.
- In CI, the job "Choose the checks" builds ob and runs
  `./ob test boot --affected HEAD^1 --plan` on a pull request, where
  `HEAD^1` is main's commit under the merge commit. The boot jobs are a
  matrix of that many shards, each running
  `./ob test boot --affected HEAD^1 --shard <n>/<parts>`; with no test
  to run there is no boot job. A push to main, a manual run and the
  nightly run use all five shards and every test.
- Main runs every boot test after each merge. Its runs are no longer
  cancelled by the next merge: a merge waits for the run before it, and
  of the runs waiting only the newest is kept, as it holds the others'
  changes. CI also runs on main every night at 03:11 UTC, every job but
  the benchmarks, which `bench.yml` runs each night already.
- Pull requests never wait for main's runs. When main's run fails, the
  next pull request fixes it.

## Consequences

- A pull request that changes only writing or the lab's tests runs no
  boot job. One that changes a program runs its tests and the smoke
  tests, usually on one machine. One that changes the core runs the
  sample and the tests that name its paths, usually on two machines.
  Over the 60 pull requests merged before this one, 11 would have run
  no boot job, 3 one job of two minutes of tests, 29 two jobs of 6 to 8
  minutes of tests each, and 17 three to five jobs of 7 to 10 minutes,
  where each of the five jobs ran 21 minutes of tests; a job also spends
  about 12 minutes setting up and building.
- A test a pull request leaves out still runs after the merge, so a
  break it would have caught shows on main's run within the hour, or in
  the nightly run, after the pull request has merged. The fix comes in
  the next pull request.
- A new test with no line in `covers.txt` runs on every pull request
  that runs boot tests, until it gets one: a new test's pull request
  adds its line.
- "Choose the checks" now builds ob before the boot jobs start, about a
  minute with its cache.
- Main's runs queue instead of cancelling one another, which uses more
  runner time on days with many merges.
