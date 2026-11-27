<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0628. Linux's own results judge the Linux view on LTP's system call tests and generated seccomp filters

- Status: Accepted
- Date: 2026-11-27

## Context

Dukovo's kernel answers Linux's system calls itself (ADR 0610) and hands
the rest to linuxd (ADR 0618). Its boot tests check the calls that the
programs it runs make, and the trace guard of the `linux-node` test checks
that Node.js and npm make no call the table lacks. Neither says how close
each call's answers are to Linux's in the cases those programs did not
happen to try: the errors a call gives for bad arguments, the flags it
refuses, the limits it keeps. Those are what the next program trips on.

Two suites test Linux's calls one by one. LTP, the Linux Test Project,
has a program for each case of its system call tests, about 1,500 of
them, which report pass, fail or "does not apply" (TCONF) through their
exit status. gVisor's `test/syscalls` holds some 300 test programs built
with Bazel, whose dependencies the build fetches from the network as it
goes. Both are written for Linux and both depend on the machine they run
on: a case that needs root, a device or a kernel option passes on one
Linux and not on another. Both are GPL-2.0 (LTP) or Apache-2.0 (gVisor)
programs, built for Linux.

seccomp (ADR 0623) is a language: a filter is a program, and what Dukovo
makes of it must be what Linux makes of it, instruction by instruction.
radix-bpf's own tests check its checks and interpreter against Linux's
rules as written; they cannot show that the rules were read right.

## Decision

- **`ob test linux-suite`.** A run of its own, as `ob test go-suite` is
  (ADR 0358), on the same machine harness: it builds, runs on the build
  machine's Linux, then boots Dukovo and runs the same programs there.
- **LTP at a pinned release.** ob fetches LTP at the commit of its
  release 20260930 and builds `testcases/kernel/syscalls` with the build
  machine's C compiler and glibc, without NUMA's and TI-RPC's libraries.
  A program that would load a library the Linux base does not have is
  refused before anything runs. LTP is built and run only: it is never
  put in an image, a bundle or the catalog.
- **Linux is the standard.** `suite-run` (`userspace/linux/suite-run.c`)
  runs every case of LTP's `runtest/syscalls` on the build machine, as the
  ordinary user who runs ob (ob refuses root), one at a time, each in a
  process group of its own with LTP's environment, its runtime shortened
  to a tenth and a limit of 3 minutes, above the limits LTP's library
  keeps itself. The cases that pass there are the measure; the rest are
  listed in the report and not judged.
- **Dukovo must match.** The cases that passed run on Dukovo, in a
  machine with 4 CPUs and 4 GiB, through the same `suite-run` under
  `linux run`, with the programs copied to the person's home. Each must
  pass. A case that does not, and that is not fixed, is named in
  `tools/xtask/src/test/linux-suite-skips.txt` with a reason that quotes
  words of its row of `docs/deferred.md`: ob refuses a skip without such
  a row, or one that names no case, so a skip goes when its work is done.
- **`/dev/shm` is a memory filesystem of the run's own.** LTP's library
  keeps its results in a file of `/dev/shm` that each case's processes
  map shared and writable, as `shm_open` and `sem_open` do for other
  programs. The kernel shares the pages of a memory filesystem's files
  alone (ADR 0490), and the view's `/dev/shm` was a folder of the
  person's `/tmp`, so every such mapping failed with ENODEV.
  radix-directory(2)'s `memory_filesystem` (58) makes a memory filesystem
  on a mount in no tree and gives a capability to its root, as
  `serve_fuse` does (ADR 0619), and `linux run` attaches it at
  `/dev/shm`: private to the run, as ADR 0622 has it, and what a
  container runtime gives each container, as a container on Linux has a
  tmpfs of its own there. Its pages come from the room the memory files
  share, half of memory, so that however many runs make one they hold no
  more together.
- **The seccomp differential.** `seccomp-diff`
  (`userspace/posix/linux/seccomp-diff.c`) makes 2,000 filters from a
  seed: those Linux takes, of every instruction seccomp allows, and one in
  ten spoiled in one of ten ways Linux refuses. A child process installs
  each and makes four calls, and the program prints a line for each
  filter: whether it was taken, what each call returned, how the child
  ended. Its lines on Dukovo must be its lines on Linux, one for one.
- **CI.** `.github/workflows/linux-suite.yml` runs it for a pull request
  that changes the Linux layer: the kernel's Linux table and libraries,
  radix-bpf, linuxd, `linux run`, the Linux base, the suite's programs
  or the suite itself. Its report, the results on both systems and the
  logs of the cases that failed on Dukovo are kept as an artifact.
- **Times.** On a build machine with 4 CPUs, LTP's build takes about 2
  minutes and the Linux run 8: 1,552 cases, of which 848 pass as an
  ordinary user, 662 do not apply and 42 fail, break, time out or were
  not built. The cases that pass take 6 and a half minutes of those. The
  Dukovo machine may run for 1 hour 45 minutes, 75 of them for the cases,
  so that cases that hang until LTP ends them do not stop the run.
- **gVisor's tests later.** gVisor's `test/syscalls` join the run with
  Phase 18's runtime work (`docs/deferred.md`): their Bazel build fetches
  its toolchains and libraries from the network as it goes, which a
  build that must give the same programs each time cannot rely on, and
  LTP and the differential cover the calls the Linux view answers today.

## Consequences

- Each difference from Linux on a case that passes there is a fix or a
  named skip with its row, and the pull request that adds skips says how
  many.
- A change to the Linux layer costs a CI job of its own, limited to 220
  minutes with the ports built, which runs beside the boot tests and gates
  the merge.
- The run's verdict depends on the runner's Linux for which cases count:
  a newer runner kernel that passes more cases adds them to the measure,
  and a skip for a case that no longer passes on Linux is harmless.
- LTP's runtest list and programs change with its releases; moving the
  pin is a change of `LTP_COMMIT` and of the skips the new cases need.
