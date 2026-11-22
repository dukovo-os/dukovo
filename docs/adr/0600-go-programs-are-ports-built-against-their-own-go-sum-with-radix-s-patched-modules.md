<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0600. Go programs are ports, built against their own go.sum with Radix's patched modules

- Status: Accepted
- Date: 2026-11-22

## Context

Go builds for Radix (ADRs 0350 and 0351), and Go's own commands are a
bundle of the catalog (ADR 0356). Programs written in Go are not: `ob`
built only its own test programs in Go, with `GOPROXY=off` and no
modules. A program from elsewhere names its modules in `go.mod` and pins
each by hash in `go.sum`, and Go downloads them from its module proxy.

Some of those modules do not build for Radix. `golang.org/x/sys` and
`golang.org/x/term` know no `radix`; a terminal library calls `ioctl`
through package syscall, whose `Syscall` is ENOSYS on Radix, as on
OpenBSD, since programs call the C library; a program reads Linux's
`/proc`. Each needs a patch, and a patched module is no longer the code
`go.sum` pins.

Go has two ways to put another copy in a module's place. Workspace mode
(`go.work`) writes the hashes of what it downloads into `go.work.sum`
without checking them against anything, so a workspace build is not
pinned. A `replace` in `go.mod` is: Go checks every module it downloads
against `go.sum` and fails on one `go.sum` does not name. Go also refuses
a replacement whose `go` line is newer than the program's.

## Decision

- **A Go program is a port** whose build.sh runs `$GO_RADIX`, the
  `x86_64-radix-go` that `ob` writes into the toolchain's `bin`, as it
  writes `x86_64-radix-cargo`. The port's source is the program's
  repository at a pinned commit, and its build writes the output of `go
  version -m` for each program it installs to `$PORT_INFO/modules.txt`.
- **`x86_64-radix-go` builds against the program's own `go.sum`.** It
  writes `.radix-go.mod` beside the program's `go.mod`, the same file
  with a `replace` for each patched module that `go.sum` names, and runs
  Go with `-modfile` pointing at it, `-mod=readonly`, `GOWORK=off`,
  `GOSUMDB=off`, `GOTOOLCHAIN=local`, `-trimpath` and `-buildvcs=false`,
  for `GOOS=radix GOARCH=amd64 GOAMD64=v1` with cgo off. `go.sum` is the
  only pin: a module it does not name stops the build ("missing go.sum
  entry"), and nothing is written to it. The patched copies are reached
  through a link, `.radix-go`, beside `go.mod`, so that the paths
  recorded in the program are relative and the build is reproducible
  (ADR 0535). `--host` first builds for the build machine instead, for a
  generator a program runs as it builds.
- **The patched modules are one toolchain port, `go-modules`.** Its
  recipe names each module by path and version with the SHA-256 of the
  module proxy's zip, which the proxy never changes, and its license.
  `ob` downloads the zip once, checks it, unpacks it, and applies
  `patches/<path, / as _>@<version>.patch`. A module is replaced at the
  version each program selects, so that no program's set of modules
  changes. `golang.org/x/sys` v0.50.0 and `golang.org/x/term` v0.48.0 take
  the place of every version, as programs need them at many versions and
  their interfaces only grow; their patched `go.mod` files name the
  oldest Go their code compiles with, 1.22, and x/term's requires
  nothing, so that Go accepts them for programs that name an older Go.
  When a program's `go` line is older still, the wrapper raises it in
  `.radix-go.mod` and adds `godebug default=go<its line>`, so that the
  program keeps the defaults of the Go it names. A program whose `go.mod`
  replaces a module `go-modules` patches stops the build, with both
  named.
- **x/sys for Radix calls the C library**, as on OpenBSD: its functions
  are generated from `//sys` lines by `mksyscall.go -libc` and `mkasm.go`,
  and its constants and types by `mkerrors.sh` and `cgo -godefs` against
  mlibc. Its `Syscall` family returns ENOSYS, as package syscall's does,
  and is plain Go: x/sys's assembly jumps to package syscall's, which Go's
  linker refuses on Radix, where package syscall pushes no link name for
  it.
- **A module without x/sys that needs `ioctl`** (creack/pty) reaches it
  through package syscall's `ioctlPtr`, whose link name Radix's syscall
  package pushes.
- **Licenses and the SBOM come from `modules.txt`.** Each module the
  programs link is a package of the bundle: a patched one with its
  recipe's license and its patched tree's notices, any other with its
  notices and the license its files state, read by phrase from the
  module cache. A license text `ob` cannot read, or one outside the
  catalog's list, fails the bundle unless the recipe states the module's
  license in `[go-licenses]`, with where it is stated. Go's standard
  library and runtime, in every program, are a package too.
- **`ob security scan` reads each Go program's `go.mod`** at the commit
  its recipe pins, as it reads a Rust program's `Cargo.lock`, so that CI's
  daily scan, which builds no port, sees every module: it asks OSV about
  the program's own module at its tag, each module `go.mod` requires (as
  its replacements name them), each patched module, and `stdlib` at Go's
  version, in OSV's Go ecosystem. Like a `Cargo.lock`, `go.mod` also
  names what the program's tests need.
- **Not patched:** file watching (fsnotify), which needs kqueue's
  `EVFILT_VNODE` (ADR 0353), and the clipboard, which needs a display
  server. Both modules take their "unsupported" paths, which their
  programs already handle.

## Consequences

- Caddy, micro, fzf, lazygit and duf are bundles of the catalog. Their
  boot tests are `caddy` and `go-commands`.
- A new version of a program is a new commit in its recipe; a module it
  newly selects that `go-modules` patches at another version needs a new
  patched copy at that version, and the build says so by failing on Go's
  own error for that module.
- The module cache is `target/ports/sources/go-mod`, which CI keeps with
  the ports' sources.
- Every program gets x/sys and x/term at the patched versions. A program
  that needs a newer x/sys than the patched one fails to build until the
  patch moves to that version.
