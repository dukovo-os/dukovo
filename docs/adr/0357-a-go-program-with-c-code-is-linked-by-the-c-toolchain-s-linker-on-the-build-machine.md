<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0357. A Go program with C code is linked by the C toolchain's linker, on the build machine

- Status: Accepted
- Date: 2026-11-21

## Context

cgo lets a Go program call C and C call Go: the `cgo` tool turns a
package's C preamble into C files that the C compiler builds, and Go
code that calls them on the system stack. ADR 0350 left it out. Every
Go program on Radix already calls the C library through Go's own
linker, which imports the C library's functions from `libc.so` by name;
it does not link C objects, nor the C library's start files that a
program with C code begins in.

Dukovo has no C compiler of its own until release 1.17 (the roadmap's
Clang). Its C toolchain is on the build machine: Clang and lld behind
the wrapper `x86_64-radix-cc` that `ob` makes for the ports (ADR 0071),
whose `-dynamic` mode links a position-independent program that
`/system/libraries/ld.so` starts and that needs `libc.so`, as every Go
program is.

Building Go's standard library and its cgo tests with cgo for Radix
found three things the C side lacks:

- **Go's cgo resolver.** With cgo, Go's `net` package resolves names
  through the C library's `getaddrinfo`, and looks names up with
  `res_search` or `res_nsearch`. mlibc has neither, so `net` did not
  compile.
- **The bounds of a thread's stack.** `runtime/cgo` asks
  `pthread_getattr_np` for them on glibc, and otherwise lets the runtime
  estimate them from the stack pointer, as on musl. mlibc's
  `pthread_getattr_np` prints "Implementation is incomplete!" on each
  call.
- **`SIGSTKSZ` by default.** A C file that names no standard saw
  `sigaltstack` but not `SIGSTKSZ` or `SS_DISABLE`, which glibc and musl
  show by default; mlibc showed them only under `_XOPEN_SOURCE`. Go's
  cgo test `sigaltstack.go` did not compile.

## Decision

- **cgo is supported for `radix/amd64`.** Go's table of platforms says
  so, and a Go build with `CGO_ENABLED=1` and a C compiler builds a
  package's C code.
- **A program with C code is linked by the C toolchain's linker.** For
  Radix with cgo, Go's linker always links externally: it writes the Go
  code as one object and runs the C compiler as the linker, which links
  it with the package's C objects, the C library's start files and
  `libc.so`. Go programs without C are linked by Go's own linker, as
  before (ADR 0350).
- **`ob` builds a Go program with C code on the build machine.** A Go
  build that asks for cgo gets `CGO_ENABLED=1` and, as `CC`, the
  wrapper in its `-dynamic` mode; the C flags are Go's own default. Every
  other Go build of `ob` keeps cgo off (ADR 0351).
- **Go's commands on Dukovo build without cgo**, as they find no C
  compiler there, until Dukovo has one.
- **`net` resolves names in Go also in a program with cgo**, as it does
  without: the same resolver, reading the same files, in every Go
  program on Radix.
- **`runtime/cgo` takes the estimate for a thread's stack**, as on musl,
  and threads made by Go with cgo are the C library's pthreads, as they
  are without.
- **mlibc shows `SIGSTKSZ`, `MINSIGSTKSZ`, `SS_ONSTACK` and
  `SS_DISABLE` under `_DEFAULT_SOURCE`**, as it shows `sigaltstack`
  (`0100-signal-stack-sizes-by-default.patch`, meant for upstream).
- **Go's own cgo tests run on Radix.** The `go-runtime` boot test runs
  the test binaries of `cmd/cgo/internal/test` (calls both ways,
  callbacks from threads C made, signals, `errno`, the C library's
  functions), `cmd/cgo/internal/testtls` (C's thread-local variables
  beside the runtime's) and `runtime/cgo`, built by `ob` with cgo.

## Consequences

- A catalog app can be a Go program with C code: `ob` builds it, and it
  needs on Dukovo only what every Go program needs, the dynamic linker
  and `libc.so`. The C libraries of Dukovo's ports are static, so the C
  code a program links of theirs is in the program.
- A person who builds Go on Dukovo builds programs in Go alone. A
  package that needs C fails to build there with Go's own error, until
  Dukovo has a C compiler (`docs/deferred.md`).
- Go's linker and the C toolchain's place the runtime's thread-local
  word each in their own way: Go's at -8(FS), lld wherever the program's
  TLS block puts it beside C's variables. The runtime reaches it through
  its relocation either way, and `cmd/cgo/internal/testtls` checks both
  kinds of variable on several threads.
- `-race`, `-buildmode=c-archive`, `c-shared`, `shared` and `plugin`
  stay unsupported on Radix: the race detector has no runtime for it,
  and the build modes, which make Go code for a C program or another Go
  program to load, are not enabled for Radix in Go's table of
  platforms.
