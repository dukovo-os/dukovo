<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: Build, test and documentation tools
| [Go](https://go.dev) 1.27.4 by The Go Authors, built from source with its own make.bash and patched for Radix in `ports/go`, with a Go 1.24.6 or later to build it: the host's, or Go's own toolchain module that ob downloads, checks against its pinned SHA-256 and unpacks | Building programs in Go for Radix, and the Go programs and standard library tests the `go-runtime` boot test runs; never part of an image | BSD-3-Clause |
| [Go](https://go.dev) 1.27.4 by The Go Authors, built from source for Radix by its own make.bash with `-distpack` in `ports/go-radix`: its commands, compiler, linker and standard library with its tests, with [google/pprof](https://github.com/google/pprof) (Apache-2.0) and its svgpan script (BSD-3-Clause) in `go tool pprof`, and the time zone database; the prebuilt `*.syso` objects of the source are removed | The `dev.go.go` bundle of the catalog, with Go's own commands, `go` and `gofmt`, for building Go programs on Dukovo (ADR 0356) | BSD-3-Clause, Apache-2.0 for pprof, BSD-3-Clause for svgpan |
