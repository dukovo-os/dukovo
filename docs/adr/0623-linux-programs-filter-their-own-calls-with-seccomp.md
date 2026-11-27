<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0623. Linux programs filter their own calls with seccomp

- Status: Accepted
- Date: 2026-11-27

## Context

Many Linux programs limit their own system calls before they run code
they do not trust: Chrome's and Firefox's sandboxes, OpenSSH's privilege
separation, systemd's services, container runtimes such as runc and
crun (which install Podman's and Docker's profiles), Node's and Python's
sandboxing libraries, and anything built on libseccomp. They do it with
seccomp: a classic BPF program, given to `seccomp(2)` or
`prctl(PR_SET_SECCOMP)`, that the kernel runs on each call before the
call is made and whose value says what happens to it. A program that
finds no seccomp either refuses to start or runs without its sandbox,
and the Linux table answered both with ENOSYS.

The kernel already runs classic BPF for packet captures (ADR 0346, in
radix-net). seccomp needs the same machine with Linux's stricter checks,
a cache that skips the filters for calls they allow by number alone, and
the rules of Linux's `kernel/seccomp.c` for chains, threads, `fork` and
`execve`.

## Decision

- **radix-bpf** (`kernel/lib/bpf`), a `no_std` crate with host tests,
  Kani proofs and a fuzz target, holds the one classic BPF machine:
  radix-net's packet filters run on it with a frame as input, and seccomp
  filters with the call's `struct seccomp_data`. A seccomp program is
  checked as Linux's `bpf_check_classic` and then `seccomp_check_filter`
  check it, in that order: 1 to 4,096 instructions, only the opcodes
  seccomp lists (no `MOD`, no load of a byte, a half word, at an index or
  of a header length, and `LD W ABS` only of an aligned word inside the
  64-byte record), no division by a constant 0 or shift by 32 or more,
  jumps inside the program, each scratch word stored on every path before
  it is read, and a `RET` last. It runs in at most one step per
  instruction, as jumps only go forward. Its length is counted as Linux
  counts it, in the instructions Linux's conversion to eBPF makes
  (`Program::converted_len`, measured against Linux 6.18), so that a
  chain fills up where it does on Linux.
- **The cache.** `Program::always_allows` finds the calls a program
  allows whatever their arguments, as Linux's `seccomp_cache_prepare`
  (5.11) does, by running it with only the number and the arch known; a
  chain's set is its filters' sets met. A thread keeps its chain's set in
  eight atomic words, so a call it allows costs one load more than a call
  made with no filter, which costs one load and a branch.
- **Each Linux thread** has its mode (off, strict, filter), its chain of
  filters (newest first, each holding the one before it, so that the
  threads and children that share filters share their memory), the
  chain's set, and `no_new_privs`. A thread started by a Linux thread
  takes all four before it runs, so `clone`, `fork` and `execve` keep
  them, as Linux's `copy_seccomp` does.
- **The check** runs before the table's calls and before those it
  forwards to linuxd (ADR 0618). Strict mode allows `read`, `write`,
  `exit` and `rt_sigreturn` and ends the thread with SIGKILL for any
  other call. In filter mode every filter runs and the strictest action
  wins, ties to the newest, as Linux's `seccomp_run_filters` decides.
  `ALLOW` makes the call and `LOG` logs it too; `ERRNO` fails it with the
  data as the errno, at most 4,095; `TRAP` skips it, leaves its number in
  rax, and forces SIGSYS with `si_code` SYS_SECCOMP, `si_errno` the data,
  `si_call_addr` the address after the `syscall`, `si_syscall` and
  `si_arch`, which ends the process if SIGSYS is blocked, ignored or has
  no handler; `USER_NOTIF` and `TRACE` fail the call with ENOSYS, as with
  no listener and no tracer; `KILL_THREAD` ends the thread with SIGSYS,
  or the process when it is the last thread; `KILL_PROCESS` and any
  action Linux does not know end the process with SIGSYS and a crash
  record. A kill and `LOG` are logged always, the other refusals when
  the filter was installed with `SECCOMP_FILTER_FLAG_LOG`, at most ten
  lines in five seconds.
- **Installing** follows Linux's order of checks, so that a program that
  probes what the kernel has learns what it would on Linux: unknown flags
  EINVAL, the record EFAULT, the length EINVAL, no `no_new_privs` EACCES
  (no Linux program holds CAP_SYS_ADMIN on Dukovo), the program EFAULT,
  ENOMEM or EINVAL, a thread in strict mode EINVAL, a chain past 32,768
  instructions ENOMEM. With `TSYNC` every thread of the process takes the
  new chain and `no_new_privs`, under the process's thread list lock,
  which a starting thread's inheritance also takes; a thread in strict
  mode, or whose chain the caller's did not grow from, stops it, and the
  answer is that thread's number, or ESRCH with `TSYNC_ESRCH`. A
  per-process lock orders installs. `SECCOMP_GET_ACTION_AVAIL` names the
  eight actions Linux knows. A listener for `USER_NOTIF` is not made:
  `NEW_LISTENER`, `WAIT_KILLABLE_RECV` and `SECCOMP_GET_NOTIF_SIZES` are
  EINVAL, as on a kernel without them, which is how libseccomp and the
  container runtimes find out.
- **A filtered program may not start a Dukovo program**: `execve` of one
  fails with EACCES, as the filter speaks Linux's call numbers and the
  program would leave it behind.
- **What others see.** `/processes/<n>/threads` gives each thread's
  `seccomp` (`off`, `strict` or `filter:`_n_) and `no_new_privs`
  (radix-proc(5)), from which linuxd's `/proc/<pid>/status` prints
  `NoNewPrivs`, `Seccomp` and `Seccomp_filters`.

## Consequences

- Linux programs that sandbox themselves run with their sandbox on.
  `userspace/posix/linux/seccomp.c`, built with glibc and with musl, runs
  in the `linux-programs` boot test and passes on Linux as on Dukovo:
  every action, the SIGSYS record and the handler's frame, the errno
  clamp, arguments, chains, `TSYNC` and its refusals, strict mode,
  inheritance through threads, `fork` and `execve`, a call linuxd
  answers, the refused program shapes and the chain's limit. Run by its
  name it has no `/proc`, so it runs through `linux run` with `--status`
  as well, where it checks the three status lines.
- radix-bpf's host tests run libseccomp's output for a small allow list
  and for Podman's default profile, and the conversion lengths measured
  on Linux; its proofs show that a checked program always ends, that the
  cache skips only calls the program allows, and that a filter added
  never makes a chain less strict.
- Differences from Linux, rows of docs/deferred.md: no `USER_NOTIF`
  listener, so a container runtime that asks for one runs without it; no
  tracer for `TRACE`; the log is the kernel log's `seccomp:` line, not an
  audit record, and its set of logged actions cannot be changed
  (`/proc/sys/kernel/seccomp/actions_logged`); `SECCOMP_FILTER_FLAG_SPEC_ALLOW`
  changes nothing, as every thread has the same speculation mitigations.
