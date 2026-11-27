<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0624. A program given memory.jit may make its private memory writable and executable

- Status: Accepted
- Date: 2026-11-27

## Context

No page of a Dukovo program has ever been writable and executable at
once: `map_anonymous` and `protect` refuse write with execute, and the
Linux table's `mmap` and `mprotect` refuse PROT_WRITE with PROT_EXEC with
EACCES. That keeps code an attacker writes into memory from running
there, and it is why PHP's JIT and PCRE2's are built off.

The programs Phase 18 brings run their own compilers as they run: Node.js
and every JavaScript engine (V8 writes code into pages that are writable
and executable at once, and its flags that avoid them are slower and not
kept up), LuaJIT, the JVM, .NET, PCRE2's JIT and Python's coming one.
They cannot run under the rule as it is. The manifest reserved the
permission `memory.jit`, "Generate machine code as it runs", for the
release that gives such memory.

What must hold is that memory is writable and executable only where
someone chose it: a program a person runs with a JIT, a bundle granted
the permission, never a sandboxed program that did not ask, never a
file's pages, which another program may have written, and never shared
memory, which another process writes.

## Decision

- **A process may have memory.jit**, a flag of its own. With it, and only
  for its own address space, `map_anonymous` and `protect` give private
  anonymous memory read, write and execute at once (`UserAccess::
  ReadWriteExecute`, which the x86-64 page tables give as present,
  writable, user and not NX). The Linux table's `mmap` (private and
  anonymous) and `mprotect` pass it through. A file's pages, shared
  memory and memory files are never writable and executable, memory.jit
  or not: `protect` refuses it with missing_rights for any region that is
  not private anonymous memory, and `map_file` and shared `map_anonymous`
  never give it. The kernel's own pages never are.
- **Copy on write keeps the rule.** After `fork`, a private page that
  both sides may write is mapped without write in both until one writes
  it, which copies it; a page that also runs stays executable while it
  waits (`UserAccess::without_write`), so a forked JIT runs the code it
  had and writes its own copy.
- **The flag lasts.** `fork` copies it, `exec` keeps it, and nothing
  clears it; a process `create` makes starts without it. So a launcher
  gives it before the program runs, and every program that program
  starts has it too, as a JIT's helpers need.
- **The jit capability gives it** (object kind `jit`, interface `jit`,
  radix-jit(2)). Its `allow` gives memory.jit to a process the caller
  holds with the control right, but not to the caller's own process: a
  holder gives it to a program it starts. Its `state` says whether a
  process has it and whether it asked for such memory without it and was
  refused, which the kernel notes on the process (and logs, once) so that
  a launcher can say why its program failed. `/processes/<n>/status`
  says `jit: yes`, `refused` or `no`, and `ps -o jit` shows it.
- **Who holds the capability.** The kernel makes one and gives it to
  Spark with the other boot capabilities. Spark gives a copy to a service
  whose file says `memory.jit = true` (service.toml(5)), in `SLOT_JIT`,
  never memory.jit itself. authd's file asks; authd puts the capability
  in each session it starts (a sign-in, a help screen, the web console's
  terminal, SSH, a job) and in elevated commands. The share sessions'
  and NFS's workers and the web console's workers do not get it.
- **The launchers give the flag.** `bundle run` gives memory.jit to a
  sandboxed program whose bundle was granted the permission, which is
  now defined, given at install ("Make machine code as it runs, as
  JavaScript engines do"), and never puts the capability in a sandbox.
  `linux run --allow-jit` gives it to the Linux program it starts, and
  plain `linux run` says, when its program ended with a status other
  than 0 after it was refused such memory, that `--allow-jit` would have
  given it. Both refuse to start a program that requires it where no
  capability is held, as in a service.

## Consequences

- Node.js, LuaJIT, the JVM and .NET run as Linux programs through
  `linux run --allow-jit` and, as bundles, with the permission; a native
  JIT port can be built with its JIT on.
- A program the person runs from their shell holds the capability and
  could start another program with memory.jit; what the rule protects is
  every program that has not been given it, sandboxed programs first,
  whose memory stays as it was.
- `userspace/posix/linux/jit.c`, built with glibc and with musl, runs in
  the `linux-programs` boot test three ways: by its name, where every
  writable and executable mapping is EACCES; through
  `linux run --allow-jit`, where private anonymous memory runs the code
  written into it and over it, across `fork` (each side writing its own
  copy) and `execve`, and a file's pages and shared memory are EACCES
  still; and through plain `linux run`, which tells of the option. It
  passes on Linux too. The native C test `files` checks that a program
  without the flag has no such memory.
- Rows of docs/deferred.md: a native bundle command that runs with your
  access is not given memory.jit (it runs in the process that started it,
  which may not give the flag to itself), and no service is given it.
