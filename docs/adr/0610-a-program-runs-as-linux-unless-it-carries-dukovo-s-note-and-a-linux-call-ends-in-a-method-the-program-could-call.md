<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0610. A program runs as Linux unless it carries Dukovo's note, and a Linux call ends in a method the program could call

- Status: Accepted
- Date: 2026-11-22

## Context

Phase 18 runs programs built for Linux on Dukovo, unchanged. The first
step is a static x86-64 Linux program: one built with glibc or musl
linked in, a Rust program for `x86_64-unknown-linux-musl`, a Go program
for `linux/amd64`. Such a program reaches the kernel only through the
`syscall` instruction, with Linux's numbers, Linux's arguments and
Linux's records. Radix's own programs use the same instruction for
Radix's twelve calls, with other numbers.

A layer outside the kernel cannot serve those calls. A C library or a
preloaded shim does not see them: a static program carries its own C
library, and Go makes its calls itself. Trapping each call into another
process (as a debugger does) costs two context switches a call.

Inside the kernel, the calls could be served by Linux-shaped subsystems
of their own, a second file layer, a second process layer. That doubles
the code that checks what a program may do, and every difference between
the two is a way around a check. Radix's methods already do what Linux's
calls do for a POSIX program (Dukovo's C library is built on them), and
each makes its checks on the capability it is called on.

`exec` must also tell the two kinds of program apart. The ELF header does
not: most Linux programs say "System V" in `EI_OSABI`, as Radix's do. A mark
outside the file (an attribute, a launcher command) is lost when one
Linux program starts another by its path. Radix's programs, though, all
come from Dukovo's toolchain, which can mark them.

## Decision

- **The personality is chosen at `exec`, from the main program.** A
  program gets Radix's calls if a `PT_NOTE` segment of its main ELF file
  holds Dukovo's note (name `"Dukovo"` with its NUL, type 1
  `NT_DUKOVO_ABI`, a 4-byte description holding the interface version,
  1), or if its `PT_INTERP` is exactly `/system/libraries/ld.so`. Every
  other x86-64 program is a Linux program. The interpreter's own notes do
  not decide, and the programs the kernel starts itself are Radix's.
  `radix-elf` reads the note (`Abi::of`), so the kernel and the build tool
  decide the same way.
- **Dukovo's toolchain writes the note.** mlibc's `crt1.S` (so every C,
  C++ and Rust program linked with mlibc) and libradix (Radix's programs
  without a C library) carry it in `.note.dukovo.abi`; `userspace/link.ld`
  keeps it in a `PT_NOTE` segment. Go programs for Radix name the dynamic
  linker. `ob build` reads each program of the system image and of each
  bundle it packs as `exec` would and fails, naming each one, if one would
  run as Linux.
- **The process holds the choice** (`LinuxProcess::on`), set by `exec`
  before the new program's thread starts and copied by `fork`.
  `syscall::handle` looks at it first, one load of a word it already
  reads, and sends a Linux program's calls to the Linux table
  (`kernel/radix/src/user/linux/`).
- **Linux's calling convention.** The number is in rax and the arguments
  in rdi, rsi, rdx, r10, r8 and r9; the result, or a negated error number,
  goes back in rax; every other register comes back as it went in, but
  rcx and r11, which `sysret` takes.
- **A Linux call ends in a method the program could call itself.** The
  table finds the capability a call means in the program's own slots (a
  descriptor n is slot `SLOT_FD_BASE + n`, a path is resolved from
  `SLOT_ROOT` as ADR 0611 says, the process is `SLOT_PROCESS`, the address
  space `SLOT_SPACE`), turns Linux's arguments into the method's, calls
  it through the same lookup a program's `cap_call` takes, and turns the
  answer into Linux's. Where a native method is itself a kernel function
  that a C library wraps (`fork`, `exec`, a futex wait), the table calls
  that function with the same checks. Where Linux's record is not the
  method's (a path joined to the working directory, a file's status, a
  socket address, the descriptors a message carries, a `pollfd` array a
  `select` becomes), the table does the method's own work with the
  record in kernel memory: the directory methods take `Paths::Given`, and
  `status_given`, the socket methods' `*_of` functions and `poll_entries`
  are each the body of a method, which the method itself calls around
  the caller's records, so they check the capability's kind and rights as
  it does. The table never maps memory into the program to hold a record
  of its own, and the program's buffers go to a socket as they are,
  several of them as one (`Buffers`). So a Linux program has no
  authority a Dukovo program in its place would not have.
- **Errors** become error numbers as Dukovo's C library makes them
  (`radix_linux::errno::from_status`, a port of its `to_errno`), with a
  call's own remaps where Linux answers otherwise.
- **A call a signal interrupts** fails with EINTR or runs again as the
  method's `interrupted` status says, through the path a Radix call takes
  (`signal::call_failed`), so SA_RESTART works as Linux's ERESTARTSYS and
  ERESTARTNOHAND do. A signal's frame is Linux's `rt_sigframe`, and
  `rt_sigreturn` restores it from the stack pointer of the call.
- **What a Linux program is told at start.** No start-up page; the
  auxiliary vector carries the process's user and group numbers in
  `AT_UID`, `AT_EUID`, `AT_GID` and `AT_EGID`, the CPU's features as
  Linux gives them in `AT_HWCAP` (CPUID leaf 1's EDX), `AT_HWCAP2` 0, and
  the largest signal frame in `AT_MINSIGSTKSZ`. `uname` says `Linux`,
  release `6.1.0-dukovo`, version `#1 SMP Dukovo` and Dukovo's version,
  machine `x86_64`, and the machine's name as the C library reads it.
- **What is refused for good** is refused as Linux refuses a program
  without the privilege: ENOSYS for io_uring, BPF, perf events, ptrace,
  kexec and kernel modules; EPERM for mounting, `pivot_root`, `reboot`,
  swap, and setting the time or the machine's name; EPERM for a `clone`
  that asks for a new namespace. A number the table does not know is
  ENOSYS, as on a Linux too old for it, which C libraries handle.
- **Linux's numbers, flags and records** are in `radix-linux`
  (`kernel/lib/linux`), a host-tested crate with no unsafe code; the
  kernel's table holds only the calls.

## Consequences

- Static Linux programs run: the `linux-programs` boot test runs one
  built with glibc and one built with musl through threads, `fork`,
  `vfork`, `execve` both ways between Linux and Dukovo programs, memory,
  signals, futexes, directories and descriptors. The calls a later step
  brings (`epoll`, `eventfd`, `timerfd`, `signalfd`, `memfd`, `pidfd`,
  the vDSO) answer ENOSYS until then, as on an older Linux.
- A Linux call costs a Radix call's path plus one flag and a translation.
  linux-bench's `lx_` rows measure the same static program on Linux and
  on Dukovo.
- A program built for Dukovo by another toolchain, without the note,
  runs as Linux and fails at its first call. The build check keeps such
  a program out of the image and the catalog; anyone building outside
  them uses Dukovo's toolchain, which writes the note.
- `/system/libraries/ld.so` run directly as a program has no note and
  runs as Linux: nothing in Dukovo runs it so.
- radix-trace(2) does not record a Linux program's calls yet
  (docs/deferred.md).
- Dynamic Linux programs, with their own loader and libraries, need the
  Linux view of the file tree (`/proc`, `/sys`, `/dev`, `/lib`), which is
  a later step of Phase 18, as are seccomp and namespaces.
