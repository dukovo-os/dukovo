<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0350. Go programs call the C library and link to it dynamically, as on OpenBSD

- Status: Accepted; Go's network poller uses the event queue since [0353](0353-an-event-queue-watches-many-descriptors-as-kqueue-does.md); Go's own commands are a catalog bundle since [0356](0356-go-s-own-commands-are-a-catalog-bundle-built-by-go-s-own-cross-build.md); cgo links through the C toolchain since [0357](0357-a-go-program-with-c-code-is-linked-by-the-c-toolchain-s-linker-on-the-build-machine.md), and Go's own test suite runs on Dukovo since [0358](0358-go-s-own-test-suite-runs-on-dukovo-in-a-ci-job-of-its-own.md)
- Date: 2026-11-20

## Context

Phase 17 ports Go. On most systems Go's runtime and its `syscall`
package make system calls directly, by number. Radix has no such
numbers: its system calls are capability calls (`abi/*.ridl`), and its
POSIX interface is the C library's Radix layer (ADR 0068). Go already
works this way on OpenBSD, macOS, Solaris and AIX, where every system
interface is a C library function.

Go's own linker can link a program to a shared C library without cgo,
as it does on OpenBSD; it cannot link a static C library into a
program. The dynamic linker and the shared C library are in every image
(ADR 0071). The kernel's signal frame is Linux's (ADR 0062), mlibc's
Radix headers take Linux's numbers and structures, and anonymous memory
is committed only when touched (ADR 0060).

## Decision

- **Go knows Dukovo as `GOOS=radix`**, a Unix (the `unix` build tag),
  on amd64. The port is a patch series on Go's source, in
  `ports/go/patches`.
- **Every system interface is a C library function.** The runtime calls
  each through an assembly trampoline on the system stack, as on
  OpenBSD, and `//go:cgo_import_dynamic` imports it from `libc.so`. The
  `syscall` package's functions are made the same way, from `//sys`
  lines, by `mksyscall.pl -radix`. There are no system call numbers:
  `syscall.Syscall` and `syscall.RawSyscall` return `ENOSYS`.
- **A Go program is dynamically linked.** Go's linker writes an ELF
  program whose interpreter is `/system/libraries/ld.so` and which
  needs `libc.so`. The default build mode is PIE, so the kernel places
  each program at random; `-buildmode=exe` also works. Programs start at
  `_rt0_amd64_radix`, which hands the C library's `__mlibc_entry` the
  runtime's C entry point, so the C library is set up before Go's
  runtime starts.
- **The goroutine pointer is in thread-local storage at -8(FS).** The
  dynamic linker puts the main program's TLS block first, right below
  the thread pointer, so Go's linker turns the initial-exec access to
  the runtime's 8-byte block into a fixed offset. Threads are the C
  library's pthreads, each with its TLS.
- **Locks wait on a new C library function, `futex()`**, declared in
  `<sys/futex.h>` with OpenBSD's interface:
  `int futex(volatile uint32_t *addr, int op, int val, const struct
  timespec *timeout, volatile uint32_t *addr2)`. `FUTEX_WAIT` (1)
  sleeps while `*addr == val` until woken, a signal (`EINTR`) or the
  relative `timeout` (`ETIMEDOUT`), and fails with `EAGAIN` if
  `*addr != val`; `FUTEX_WAKE` (2) wakes up to `val` waiters and returns
  how many. `FUTEX_PRIVATE_FLAG` (128) is accepted and changes nothing,
  since Radix's futexes belong to one address space. Any other operation
  fails with `ENOSYS`; `addr2` is not used. It is Radix's own layer
  (`sysdeps/radix/generic/futex.cpp`), on the address space's futex
  calls.
- **Memory given back is mapped anew.** mlibc refuses `MADV_DONTNEED`,
  so the runtime returns unused memory by mapping fresh anonymous memory
  over it with `MAP_FIXED`. The kernel replaces the old pages in one
  step, and they read as zeros when next touched.
- **Signals use the C library's `sigaction`**, which supplies the
  restorer, and Linux's `ucontext` layout. The C library's signal set
  is 128 bytes; the runtime keeps the first 64 signals' word, as on
  Linux. Signals 32 to 34 are the C library's own (thread cancellation,
  timers and one reserved), and the runtime leaves them to it. The
  runtime preempts a goroutine by sending SIGURG to its thread with
  `tgkill` and the thread's ID, and profiles with `setitimer`'s
  `ITIMER_PROF`.
- **No descriptor is polled yet.** The runtime's network poller takes no
  descriptor, so every file and socket is used in blocking mode, as Go
  does wherever its poller cannot take a descriptor. Phase 17's event
  queue and networking replace it.
- **The `syscall` package's types and constants are mlibc's**, made
  from its headers with `cgo -godefs` and `mkerrors.sh`; the error and
  signal texts come from mlibc's source, since a program for Radix
  cannot run on the build machine. New processes are made with the C
  library's `fork` and `execve`, as on macOS and OpenBSD. Radix has no
  `ptrace`, so `SysProcAttr.Ptrace` fails with `ENOSYS`, and no abstract
  Unix socket names.

## Consequences

- Go programs depend on the shared C library's ABI, its exported
  functions and their structures. A change to mlibc that breaks them
  breaks every Go program; the `go-runtime` boot test runs a Go program
  and a list of the standard library's tests to catch it.
- Go's own tests find where Radix differs from Linux. The C library's
  `mmap` now fails with `ENOMEM`, as Linux's does, for a size larger than
  a program's addresses, where the kernel's answer meant `EINVAL`. The
  tests that cannot pass yet are listed in `userspace/go/skips.txt`, each
  with its reason, and what they wait for has a row in
  `docs/deferred.md`.
- A system call costs a C library call more than a raw one, as on
  OpenBSD and macOS.
- cgo needs no other model: C code a Go program links shares the same
  C library.
- The patch series is carried to each Go release. Another architecture
  needs its own entry, trampolines and type tables.
- A Go program is a dynamically linked program. Where Go programs may
  live in `/system`, where ADR 0005 links programs statically, is
  decided when the first one goes there.
