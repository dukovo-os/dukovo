<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: Build, test and documentation tools
| [Go](https://go.dev) 1.27.4 by The Go Authors, built from source with its own make.bash and patched for Radix in `ports/go`, with a Go 1.24.6 or later to build it: the host's, or Go's own toolchain module that ob downloads, checks against its pinned SHA-256 and unpacks | Building programs in Go for Radix, and the Go programs and standard library tests the `go-runtime` boot test runs; never part of an image | BSD-3-Clause |
