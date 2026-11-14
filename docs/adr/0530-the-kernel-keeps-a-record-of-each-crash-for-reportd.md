<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0530. The kernel keeps a record of each crash for reportd

- Status: Accepted
- Date: 2026-11-13

## Context

When a program crashes, the kernel's log has one line: the process, the
signal and the faulting address. That is enough to know something
crashed, not where. A problem report that says where needs the
crashing thread's registers, its stack, and which files it ran from, at
the moment of the crash, while its address space is still there.

Linux writes a core file, or pipes one to systemd-coredump or apport,
which keep whole memory images: often megabytes, holding whatever the
program held, passwords and documents included. Those images are then
kept on disk, and a report tool has to work out what of them may leave
the machine. A trace needs a small part of that: the registers, the
stack's words from the stack pointer up, and the mapped files'
addresses and paths, so that the call frame information in those files
can walk the stack.

## Decision

- **A crash record is made in the crashing thread, before its process
  ends** (`kernel/radix/src/user/crash.rs`). It is made for a signal
  whose default action dumps core, when the process has no handler for
  it (`signal::fault`, the kill path of a delivered SIGABRT, SIGSEGV,
  SIGBUS, SIGILL, SIGFPE, SIGTRAP or SIGSYS, and the kill of a thread
  whose handler had no stack to run on). Spark, and every process marked
  vital, is never recorded.
- **What a record holds** (`abi/crash.ridl`, radix-crashes(4)):
  - a header: the signal, its code and fault address; the process, the
    thread and their user and group; up to four ancestors; the wall
    time and the boot time; the thread's name; the program counter,
    stack pointer and frame pointer; the program's path, as `exec` was
    given it; and the name of the process's resource domain, which is a
    service's name for a service's process;
  - a mapping for each part of a file the process had mapped to run
    (executable mappings, up to 64): its addresses, its offset in the
    file and the file's path, as the process opened it;
  - the stack from the stack pointer up, to the end of its region, the
    first page that is not there, or 64 KiB.
- **Paths are kept with the open file.** A file object keeps the path
  it was opened by (`fs::Opened`), shared by its mappings, so a mapping
  can name its file without a lookup at crash time. A path longer than
  104 bytes keeps its start and is marked cut; a file passed in from
  another process has no path, and is marked so.
- **Records wait in a queue of 8, read by root from
  `/devices/crashes`.** A read takes the oldest record whole; a buffer
  too small fails with `invalid_argument`, and an empty queue blocks, or
  fails with `would_block` on a non-blocking descriptor; `poll` finds
  the device readable while one waits. A crash past the eighth is not
  kept, and the next record read counts it in `dropped`. The kernel's
  log line is written either way.
- **The kernel flag `crashrecords=off`** keeps none, for a machine where
  even a stack must not leave the process.
- **The record holds memory, so only root reads it**, and reportd
  (ADR 0531), the one reader, keeps nothing of the stack's bytes: only
  the trace it makes of them.

## Consequences

- reportd can unwind a crash of any program on the system with its own
  call frame information and name its frames, without core files and
  without ptrace.
- A record costs at most 74,544 bytes of kernel memory while it waits,
  eight at most. Memory too short for one keeps none, and the crash's
  log line still says what happened.
- Only x86-64 fills the registers: the aarch64 and riscv64 ports give
  zeros, and their records hold the mappings and the stack alone.
- Not yet, each a row of docs/deferred.md: the registers beyond the
  three a trace needs, and the other threads' stacks, which a deadlock's
  report would want.
- Tests: the `problem-report` boot test crashes a test program by
  SIGSEGV through three named functions, and by abort, and checks that
  reportd's trace names them; in `problem-report-access`, with reportd
  stopped, nine crashes leave eight records, a read with a buffer too
  small fails, and reportd, started again, counts the one dropped. reportd's units read records
  the tests lay out as the kernel does.

## Alternatives considered

- **Core files.** They hold all the memory a program had, which is far
  more than a trace needs and holds what the person never meant to
  share. Writing them also takes disk and time at the worst moment.
- **A ptrace-like attach from reportd at crash time.** It needs the
  process stopped and kept alive across a round trip to user space,
  and a way for reportd to read another process's memory; a record made
  in the thread itself needs neither.
- **Unwinding in the kernel.** The call frame information is in the
  program's files, and parsing it in the kernel is code the kernel does
  not need. The kernel copies, and user space reads.
