<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0730. Programs forbid unsafe code and call the system through sysapi

- Status: Accepted
- Date: 2026-11-25

## Context

Rust's guarantees about memory hold only as far as no `unsafe` block
breaks them. The coding standard has always kept `unsafe` to the trusted
computing base, but in user space it let the programs on Rust's standard
library make their C library calls themselves. On the main branch that
was some 780 lines with `unsafe` across fourteen crates: about 475 in
`systools`, 80 in `nukesh`, 150 in the services and tests (`consoled`,
`sshd`, `smbd`, `nfsd`, `smb`, `nfs`, `acl`, the installer and four
test programs) and 80 in the Linux tools of the lab (`tools/lab-writer`,
`tools/linux-bench`).

Many were the same few calls, each copied with its own `SAFETY:`
comment: five copies of the kernel's `syscall` instruction, pipes,
random bytes, the host's name, descriptors passed over a socket
(SCM_RIGHTS). Several rested on what the whole program does rather than
on the block itself: `from_raw_fd` on a number a parent handed down,
which is sound only if nothing else in the program takes that number;
`fork` in a process that might have started a thread; `set_var` while
another thread might read the environment; and signal handlers, each
written by hand, that must do only what is async-signal-safe. A review
of one block cannot check any of these.

## Decision

- **Only the trusted computing base uses `unsafe`.** In the kernel it is
  the crates docs/coding-standard.md names (the architecture crates, the
  locks, the device access library, the memory manager, uACPI's binding,
  the kernel crate, and the vDSO's host tests). In user space it is
  `libradix` and the runtime `radix-rt` for the programs without the
  standard library, the user-mode tests `usertests`, which make raw calls
  on purpose, and **sysapi** for the programs on the standard library.
- **Every other crate forbids it**, in each of its targets: its library,
  its programs, its tests, benchmarks and examples, and its build script,
  each of which Rust compiles as a crate of its own, start with
  `#![forbid(unsafe_code)]`.
- **`ob lint` checks it.** `tools/xtask/src/unsafe_code.rs` asks Cargo
  for the targets of every workspace (the build tools, the kernel, user
  space, the programs and the fuzz targets) and reads the top of each
  target's root file, before its first item: every target of a crate that
  is not on the list of the trusted computing base must start with the
  line. A crate added to a workspace is checked from its first commit,
  without a list to update. A crate on the list that no workspace has any
  more is an error too, so the list stays the list of what is trusted.
- **sysapi** (`userspace/programs/sysapi`) puts each call the standard
  library has none for behind a safe function, by the rules that make the
  whole-program cases sound in one place:
  - Descriptors are `OwnedFd`, taken as `impl AsFd`; a function that
    makes one returns it owned and closed on exec.
  - A descriptor a parent handed down is claimed once: `fd::handed_down`
    takes a number that is open and not closed on exec, and marks it
    closed on exec as it takes it. Every descriptor the program opens
    itself is closed on exec already (the standard library's and
    sysapi's), so a number is claimed at most once, and never one the
    program holds. Descriptors 0 to 2, which the standard library's
    handles use, are taken with `fd::take_standard`, which moves what they
    are open on to a new descriptor and leaves them on the null device.
  - A fork and a change to the environment count the process's threads
    first (the kernel's count on Dukovo, `/proc/self/task` on Linux) and
    are refused while there is more than one. A child of a fork in a
    process with one thread may do anything before it runs its program.
  - There is one signal handler. It sets the flag of a `Caught` the
    program keeps in a `static`, and writes a byte to a pipe a loop waits
    on if asked, keeping errno as it was; the program reads the flag in
    its own time.
  - The kernel's own calls go through one `Syscalls` (`kernel::SYS`), and
    a device's requests with a buffer through `kernel::control`, which
    gives the kernel the buffer's address and length.
  - A list of descriptors to `poll` borrows each one for as long as it is
    waited on.

  A program that needs a call sysapi lacks adds it there, with a test,
  instead of making it itself.
- **The crates that still make their own calls are listed apart**:
  `nukesh`, `tools/lab-writer` and `tools/linux-bench`. The
  check passes over them, and fails once one of them forbids `unsafe` in
  every target, so that it comes off the list. The list is empty once
  they have moved to sysapi.

## Consequences

- A fault in memory in a program can come only from sysapi, the
  runtime, the C library or the kernel. Review of `unsafe` code is
  review of those, and every other change is checked by the compiler.
- The copies are gone: one `syscall` instruction for the programs, one
  set of SCM_RIGHTS code, one wait on a descriptor, one lookup of an
  account. A program's call costs a function call more at most, against a
  system call.
- consoled starts each session's program with the standard library's
  `exec`, which gives it the default action for SIGPIPE and an empty
  signal mask, as login(1) gives a session on Linux; its sessions
  inherited SIGPIPE ignored before. A worker that takes its connection or
  pipe from its standard input leaves the input open on the null device.
- `radix_abi`'s generated wrappers are safe functions, and those that
  pass the kernel a buffer take its address as an integer, so a crate
  that forbids `unsafe` can still give the kernel an address of its own
  making. The kernel reads and writes only the calling program's memory
  through it, so a wrong address can spoil only that program's own
  memory, as an `unsafe` block could. sysapi's `control` is the safe way
  to pass a buffer; docs/deferred.md keeps the change to the generator.
