<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0351. ob builds Go from source with Dukovo's patches, and builds Go programs in an environment of its own

- Status: Accepted
- Date: 2026-11-20

## Context

Go programs for Dukovo need a Go toolchain that knows `GOOS=radix`
(ADR 0350), which no Go release does: it is Go's source with Dukovo's
patches. Only a release of Go builds Go (Go 1.27 needs Go 1.24.6 or
later), and a build machine may have none, or an older one. The
toolchain runs on the build machine, as Rust's standard library for
Radix is built there (`ports/rust-std`), and never goes into an image.

Go's `go` command reads a dozen settings from its environment and from
a file of its own: the target, flags, the proxy and checksum database
modules come from, a workspace, telemetry, and the toolchain itself,
which it may download and switch to. A build that takes any of them
from the machine it runs on can differ from one machine to the next.

## Decision

- **A toolchain port, `ports/go`**, builds Go 1.27.4 from its source at
  the release's commit, with `ports/go/patches` applied, by Go's own
  `make.bash`, into `out/toolchain/go`. The tree there is a whole Go
  tree but for the repository's history, the compiler's own tests, the
  API lists and the documentation. When the port has patches, its build
  then compiles the whole standard library for Radix, so a patch that
  does not compile fails the port.
- **The release that builds it is named in the recipe**, in a
  `[bootstrap]` table: the oldest release that will do (`minimum`), and
  a release to download, the zip of Go's toolchain module from the Go
  proxy, pinned by its SHA-256 and size, with the folder in it that
  holds the tree. ob uses the tree `GOROOT_BOOTSTRAP` names when it is
  set, and refuses it, naming the version it found, when its `VERSION`
  is older than the minimum or no release's. Without the variable, ob
  downloads the pinned zip once into `target/ports/sources`, refuses it
  unless its size and SHA-256 match, unpacks it and makes its programs
  executable, as a zip keeps no modes.
- **ob builds Go programs in an environment of its own**
  (`tools/xtask/src/golang.rs`), with none of the build machine's:
  `GOOS=radix`, `GOARCH=amd64`, `GOAMD64=v1`, `CGO_ENABLED=0`, the
  port's tree as `GOROOT`, `GOTOOLCHAIN=local`, `GOTELEMETRY` and
  `GOENV` off, `GOFLAGS=-trimpath -buildvcs=false`, and `GOPROXY`,
  `GOSUMDB` and `GOWORK` off, so that nothing is fetched. The build
  cache, module cache and `GOPATH` are in `target/go`.
- **The `go-runtime` boot test** builds the test program of
  `userspace/go` and the test binaries of 22 packages of Go's standard
  library that way, puts them on a disk with each package's files and
  test data, and runs them on Dukovo. `userspace/go/skips.txt` lists the
  tests and subtests left out, each with the feature Radix lacks and the
  change that brings it; a line without a reason is an error.

## Consequences

- Building the toolchain takes about two minutes, once for each change
  to the port, and needs nothing but a Go release or the network once.
- A new Go release is a new commit, version and bootstrap pin in the
  recipe, and the patches carried to it; the recipe's `series` keeps
  `ob ports outdated` to patch releases of 1.27, which take the same
  patches.
- A Go program built by ob is the same on every build machine: no
  setting of the machine's reaches the build, and no path or version
  control state is written into the program.
- Programs with dependencies need them in the repository
  (`-mod=vendor`), as nothing is fetched while building.
