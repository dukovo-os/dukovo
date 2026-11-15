<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0560. CI builds the ports once a run, and runs a failed boot test again

- Status: Accepted
- Date: 2026-11-14

## Context

A pull request's CI took about an hour a round in November 2026, against
a goal of 30 minutes. Two causes made most of that hour.

The ports were built again in most jobs of most runs. Only main's build
job saved them to the cache, at its end, some 45 minutes after a merge,
and it built them from scratch first. Every job that needed them (the
build job, the unit tests, each boot shard, the instruction budgets and
the benchmarks) restored the newest cache main had saved and built what
it lacked. A merge that changed `abi/` or the C library, as most of
Phase 16's did, changes every port's stamp, so for the hour after it
every pull request's run built all the ports, about 30 minutes, in each
of those jobs at once: a boot shard spent 22 to 36 minutes in `./ob
ports` before its first test, and the instruction budgets 33 of its 50.
A pull request that changed `abi/` itself built them in every job of
every run, its re-runs and new pushes included, since nothing a pull
request built was saved.

A boot test that failed once failed its shard, and so the run: the
power-off hang (seven sightings, cause not yet found), a race in the
certificates test and others each cost a whole round, an hour, though a
second run passed.

## Decision

- A job "Ports" runs `./ob ports` once for the whole run, after the
  scope job. Every job that builds on the ports needs it and restores
  the cache it names; it builds nothing when that cache is whole.
- Its cache key names the clang version, then a hash of `ports/`,
  `abi/` and `rust-toolchain.toml`, then one of `tools/`. It restores
  the exact key, else the newest cache of the same ports and `abi/`,
  else the newest of any. ob's stamps then decide what to build.
- When every port was up to date, the jobs restore the cache it
  restored, and nothing is saved. When a port was built, the job saves
  the cache under its own key, on a pull request too: its next push and
  its re-runs find it, and on main the pull requests after it find it
  within minutes of the merge, not at the end of main's run.
- The build container's job keeps its own cache of the ports built in
  the container, and saves it the same way: whenever a port was built,
  on a pull request too.
- The ports job names the rust-std port's stamp, and the Rust caches
  of the jobs that build programs on Rust's standard library (build,
  boot, instructions, bench) add it to their keys. ob throws away a
  build in `target/programs` made against another std, and a Rust
  cache keyed only by `Cargo.lock` kept the build of whichever std it
  was first saved with, so every job built those programs afresh,
  about six minutes each.
- Main's runs restore the ports as a pull request's do. The nightly run
  builds them from scratch, so a port that builds only on top of an
  older build of itself is still found within a day, as the nightly
  container run (ADR 0330) also does.
- `ob test boot --retry` runs a boot test that fails once more, and
  fails it only if it fails again. A test that passes the second time
  is reported as `FLAKY`, named with its first failure in
  `out/test/flaky.txt`, and its first serial log is kept as
  `boot-<name>-first.log`. CI's boot jobs pass `--retry`, show each
  flaky test as a warning on the run and in its summary, and upload the
  logs, so that the flaky test is fixed rather than lived with.

## Consequences

- A pull request whose ports main has already saved spends about a
  minute on them in each job instead of 25 to 35, and its run takes
  about 30 minutes. One that changes `abi/` or a port waits
  about 30 minutes for the ports job once, then reuses its cache.
- One job builds the ports instead of up to fourteen, which leaves
  those runners to other runs.
- The jobs that need the ports start after the ports job, about five
  minutes later when the ports are up to date.
- A test that fails one time in two passes CI. The warning on the run
  is what keeps it from being forgotten; a test that is flaky often
  shows up on most runs.
- A pull request's own ports cache takes room in the repository's
  cache, which GitHub trims by age when full; one is saved only when
  the pull request's ports differ from every cache it could restore.
- The first run after the rust-std port changes starts those jobs' Rust
  caches empty, and builds the host's crates as well as the programs;
  later runs on the same std reuse both.
