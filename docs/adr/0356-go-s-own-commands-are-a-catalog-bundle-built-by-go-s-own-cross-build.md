<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0356. Go's own commands are a catalog bundle, built by Go's own cross build

- Status: Accepted
- Date: 2026-11-21

## Context

ADR 0350 left open where Go's own commands, `go` and `gofmt`, live on
Dukovo: in `/system`, or elsewhere. ADR 0351 builds a Go toolchain that
knows Radix, but only on the build machine, where it builds Go programs
for the image and the tests.

`/system` is in the boot archive, which is held in memory on the live and
USB system and in most boot tests, about 200 MiB. Go's distribution is
about 280 MiB unpacked, in 15,672 files: the commands, the compiler and
linker, the standard library's source with its tests, and the
documentation. Adding it to `/system` would more than double what every
machine holds in memory, for a tool most machines never run. Python is a
catalog bundle as well (ADR 0495), and the catalog installs a bundle for
one person or for everyone (ADR 0370).

Go can build a release for a system itself: `make.bash -distpack`
builds the commands and the standard library, and writes the archive Go
ships for that system. It runs from another system's toolchain as a
cross build, which the go port's toolchain is.

## Decision

- **A port of the catalog, `ports/go-radix`, builds a bundle, `dev.go.go`
  ("Go")**, with the commands `go` and `gofmt`. It depends on the go
  port, whose toolchain builds it, and uses the same patches as the go
  port.
- **The bundle is what Go ships for a system.** The port runs Go's own
  `make.bash -distpack` as a cross build for `GOOS=radix`, with
  `CGO_ENABLED=0`, and unpacks the archive it writes into the bundle's
  folder: `bin` with `go` and `gofmt`, `pkg/tool/radix_amd64` with the
  programs the `go` command runs (the assembler, `cgo`, the compiler,
  `cover`, `fix`, the linker, `preprofile` and `vet`), `pkg/include`,
  `src` with the standard library's tests, `api`, `doc`, `lib`, `misc`
  and `test`. The bundle holds what Go chooses to ship and nothing else.
- **The `*.syso` files are removed.** They are prebuilt objects of the
  race detector and BoringCrypto for other systems, under other licenses,
  and nothing on Radix links them.
- **`go.env` says `GOTOOLCHAIN=local`.** Go's own file says `auto`, which
  lets the `go` command download a newer Go for a module that asks for
  one. No release of Go from go.dev knows Radix, so a download would give
  a toolchain that cannot build for Dukovo. With `local`, a module that
  asks for a newer Go fails with Go's own message. The port's build fails
  if the line is not there to change.
- **cgo is off on Dukovo.** The commands are built with `CGO_ENABLED=0`,
  which `make.bash` makes the default, as Dukovo has no C compiler until
  release 1.17. A program with C code is built by `ob` on the build
  machine (ADR 0357).
- **The `go` command finds its `GOROOT` from the path it runs at**, as
  Go's own does from the program's location. A bundle's command runs by
  the program's path in the bundle's folder, wherever the bundle is
  installed, and the kernel gives that path in the auxiliary vector
  (`AT_EXECFN`), as it does for Python (ADR 0495). Nothing sets
  `GOROOT`.
- **It runs with the access of the person who runs it**, as a command of
  the catalog that works on the person's files does, and the first run
  asks whether it may, with a warning (ADRs 0225, 0370). The files of
  the installed bundle are sealed. The `go` command keeps its build cache
  in `~/.cache/go-build`, its modules and installed programs in `~/go`.
- **The licenses** are Go's BSD-3-Clause, google/pprof's Apache-2.0 and
  its svgpan script's BSD-3-Clause, and the public-domain time zone
  database. Their notices are in the bundle.

## Consequences

- Go is installed with `bundle install dev.go.go`, for one person or, by
  an administrator, for everyone. Images carry no Go.
- The bundle takes about 280 MiB unpacked, and its `.dkv` file about
  72 MiB.
- Updates follow the go port's version: a new release of Go is a new
  version of both ports, as the patches are for one release's runtime.
- The boot test `go-toolchain` installs the bundle for Alice and checks
  that `go` asks, finds its `GOROOT`, builds, runs, vets and tests a
  module, serves a second build from the cache, and that `gofmt` sets out
  a file.
- Go's own test suite, in the bundle's `src`, runs on Dukovo with the
  bundle's `go` (ADR 0358).
- A module that asks for a newer Go than the bundle's fails, and so does
  a package that needs cgo, until Dukovo has a C compiler.
