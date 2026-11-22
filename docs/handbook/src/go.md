<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Go

Go comes from the catalog as a bundle: Go's own commands, `go` and
`gofmt`, with the compiler, the linker and the standard library they
build programs with. It is the same Go that you get for other systems
from go.dev, built for Dukovo. Install it for yourself, or, as an
administrator, for everyone:

```
$ bundle install dev.go.go
$ elevate bundle install --for-everyone dev.go.go
```

A `.dkv` file installs too, as for any bundle (see
[Bundles](bundles.md)):

```
$ bundle install dev.go.go-1.27.4.dkv
```

The first time you run `go` or `gofmt`, Dukovo asks whether it may run
with your access, as for any command of the catalog's that does; answer
`A` to allow it always.

```
$ go version
go version go1.27.4 radix/amd64
```

Go finds its own files from where the bundle is installed, so nothing
sets `GOROOT`. The bundle takes about 280 MiB, most of it the standard
library's source and its tests.

## Building and running a program

A program in Go is built, run, tested and checked with the `go`
command, as on any system:

```
$ mkdir ~/hello && cd ~/hello
$ go mod init example.com/hello
$ cat >main.go
package main

import "fmt"

func main() { fmt.Println("Hello from Dukovo") }
$ go run .
Hello from Dukovo
$ go build -o hello . && ./hello
Hello from Dukovo
$ go vet ./...
$ go test ./...
?   	example.com/hello	[no test files]
```

A program is one file, which needs on Dukovo only what every program
needs: the dynamic linker and the C library, which are in every image.
Copy it to another Dukovo machine and it runs there.

The first build is slow: Go compiles the parts of the standard library
the program uses and keeps the result in its build cache. Later builds,
of this program or another, reuse it.

`gofmt` sets out source files in the layout all Go programs share:

```
$ gofmt -l .
main.go
$ gofmt -w main.go
```

`-l` lists the files whose layout differs, `-w` rewrites them, and `-d`
shows the differences. `go fmt ./...` does the same through the `go`
command. See go(1) and gofmt(1).

## Modules and the module proxy

A program that imports a package from another module has it fetched, by
default through the public module proxy, `proxy.golang.org`, and checked
against Go's checksum database. `go mod tidy` adds what the imports need
to `go.mod` and `go.sum`.

To build without the network, copy the dependencies into the program's
folder while you are online, and build from the copy:

```
$ go mod vendor
$ go build -mod=vendor
```

`GOFLAGS=-mod=vendor` makes it the default, and `GOPROXY=off` makes
every download an error.

## Where things go

| What | Where |
| --- | --- |
| The build cache | `~/.cache/go-build` (`GOCACHE`) |
| Downloaded modules | `~/go/pkg/mod`, read-only (`GOPATH`) |
| Programs from `go install` | `~/go/bin` |
| Go's own files | the bundle's folder, sealed |

`go env` shows every setting. `go clean -cache` empties the build cache
and `go clean -modcache` the module cache.

## cgo

cgo, which lets a Go program call C, is off on Dukovo: `go env
CGO_ENABLED` says `0`. Dukovo has no C compiler yet; one comes with
release 1.17 of the [roadmap](https://github.com/dukovo-os/dukovo/blob/main/docs/roadmap.md), with Clang. Until then
a package that needs C does not build on Dukovo. A program with C code
is built on a build machine with `ob`, which links it with Clang and
lld, and the result runs on Dukovo as any program does (ADR 0357).

## What it lacks

- The race detector, `go build -race`: there is no race runtime for
  Dukovo.
- The build modes that make a library or a plugin for a C program or
  another Go program to load: `-buildmode=c-archive`, `c-shared`,
  `shared` and `plugin`.
- A newer Go than the bundle's. `GOTOOLCHAIN` is `local`: no release of
  Go from go.dev knows Dukovo, so a module that asks for a newer Go
  fails instead of downloading one. A new release of Go comes as a new
  version of the bundle.
- Fuzzing, `go test -fuzz`, which needs shared writable mappings of
  files that Dukovo does not give yet.

Go's tests for the standard library run on Dukovo with this `go`. The
tests that fail on Dukovo for reasons that are known are in
`userspace/go/skips.txt` of Dukovo's repository, each with its reason.

## Troubleshooting

- **`go: go.mod requires go >= 1.28 (running go 1.27.4; GOTOOLCHAIN=local)`**:
  the module asks for a newer Go than the bundle's, and Go will not
  fetch one. Lower the `go` line of its `go.mod` if the module does not
  need the newer Go, or wait for a version of the bundle with it
  (`bundle update --check`).
- **`build constraints exclude all Go files in ...`** for a package that
  has C code: it needs cgo, which is off. See [cgo](#cgo).
- **`go` asks whether it may run with your access**: it is the first
  run, and every command of the catalog's that works on your files asks.
  Answer `A` to allow it always, `O` for this once.
- **`go build` is slow the first time**: Go compiles the standard
  library packages the program needs into the build cache. The next
  build is quick; `go build -x 2>&1 | grep -c /compile` counts the
  packages still compiled, and says `0` when the cache serves all of
  them.
- **A download fails**: `go env GOPROXY` shows where modules come from,
  and `GOPROXY=off` or `GOFLAGS=-mod=vendor` in the environment makes
  a build use only the files at hand.
- **`go: ... module lookup disabled by GOPROXY=off`**: the module is in
  neither the module cache nor `vendor`. Fetch it while online with
  `go mod download`, or `go mod vendor`.
