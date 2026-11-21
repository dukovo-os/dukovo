<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0358. Go's own test suite runs on Dukovo, in a CI job of its own

- Status: Accepted
- Date: 2026-11-21

## Context

The `go-runtime` boot test runs the tests of 28 packages of Go's standard
library on Dukovo: the runtime, the operating system's side of the
library, cgo and a few that work files, text and numbers hard. `ob`
builds their test binaries on the build machine (ADR 0351), so the test
checks the runtime and the library, but not Go's own commands on Dukovo:
`go test` compiling, linking and running a test binary, its build cache,
its file locks, the programs it starts.

With the Go bundle (ADR 0356), Dukovo has those commands, and Go's
distribution carries the tests of every package of its standard library.
Run on Dukovo with the bundle's `go`, they check the library and the
commands together, as Go's own builders check a platform. They are a few
hundred packages; compiling, linking and running them all is thought to
take an hour or more even with KVM, where a CI shard of the boot tests
runs its share of them in about 30 minutes, and has 45.

Scheduled CI runs are off for now: no run of this repository's CI starts
on a timer.

## Decision

- **`ob test go-suite` runs the suite.** It boots Dukovo from the test
  image in a machine with 4 CPUs and 6 GiB of memory, as Go's linker
  builds each binary in memory and `go test` links four at once, and with
  the root grown to 12 GiB for the bundle and the build cache. Alice
  installs the bundle and runs `go test -short` of every package of `std`
  that has tests, each test binary limited to 15 minutes.
- **The build machine lists the packages.** Before the boot, `ob` asks
  the go port's `go` for the packages of `std` that have tests, for
  Radix, and writes them, with the skips and the script, to the disk the
  bundle is on.
- **The skips are the boot test's.** `userspace/go/skips.txt` names the
  tests the suite skips as it names those `go-runtime` skips, each with
  the feature Radix lacks and the change that brings it. A line written
  `suite:package` is one only the suite skips. A package with skips is
  tested in a `go test` of its own, as `-skip` applies to every package
  of a call; the others share one, which runs them in parallel.
- **The run passes when every package does.** A package passes when `go
  test` says `ok` for it. The script prints each package that failed with
  the lines that say why, and the count; its whole log is saved as
  `out/test/go-suite.log`, also when a wait runs out.
- **CI runs it in a job of its own,** `.github/workflows/go-suite.yml`,
  with five hours for the ports and the run. `ci.yml` calls it for a pull
  request that changes the go port, the Go bundle, the Go test programs
  or the code that builds and runs them, and the pull request is green
  only when it passes. Anyone may start it by hand on any branch.
- **It has no schedule** while scheduled runs are off. Main's runs leave
  it out, as it takes too long for each change.

## Consequences

- Go's own commands are tested on Dukovo as a person uses them, on every
  change to Go, by Go's own tests.
- A pull request that changes Go takes an hour or more longer to go green
  and uses a runner that long. The waits are estimates: CI's timings will
  tune them.
- A change elsewhere, such as to the kernel or the C library, that breaks
  a test only the suite runs is found by the next run: the next pull
  request that changes Go, or a run started by hand. The `go-runtime` and
  `go-toolchain` boot tests still run on the changes they cover.
- A test that fails on Radix is fixed, or skipped in `skips.txt` with the
  feature it needs and the change that brings it; nothing is skipped
  because it fails.
