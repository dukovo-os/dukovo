<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0411. Every kernel function starts a cache line

- Status: Accepted
- Date: 2026-11-08

## Context

CI holds the open and close scaling rows to Linux's time (ADR 0409).
After the firmware's runtime services went in (ADR 0313), the one worker
rows on CI's AMD EPYC 7763 runners went over:

| Commit | One process, Radix | One process, Linux | Ratio |
|---|---:|---:|---:|
| Before ADR 0313 (two runs) | 575 and 590 ns | 601 and 606 ns | 0.96 and 0.97 |
| After ADR 0313 (two runs) | 612 and 614 ns | 610 and 603 ns | 1.004 and 1.02 |
| After ADR 0315 (one run) | 603 ns | 615 ns | 0.98 |

The one thread row moved the same way, from 0.94 to 0.98. Linux's time
stayed where it was on the same runners, and Radix's became 4 to 6 %
slower; after ADR 0313 that was the fastest of nine runs each time.

ADR 0313 changed none of the code that open and close run, and opening
and closing a file still takes 3,675 instructions. Built before and
after it, the kernel's hot functions for open and close
(`radix_syscall`, the directory and file methods, `install_file`) are
the same size in both, but each starts at a different place in its
64-byte cache line. `radix_syscall`, for one, started 48 bytes into its
line before and at the start of one after. The kernel is linked as one
unit (fat LTO, one codegen unit), so code added anywhere moves
everything after it, and LLVM starts functions only on a 16-byte
boundary on x86-64.

Zen 3, the EPYC 7763's core, fetches and caches decoded instructions by
aligned 32- and 64-byte windows. Where a hot function falls in those
windows can change how many it spans and how fast it runs. So a change to
the firmware's variables turned the open and close gate red, and the next
change to the kernel, to Btrfs's balance (ADR 0315), which open and close
do not run either, moved them again and brought the row back under.
Linux has a debug option that starts every function at a 64-byte
boundary for the same reason: so that a change in speed can be told
apart from a change in where the code fell.

## Decision

**Every kernel function starts a 64-byte cache line:**

- the kernel's Rust, `core` and `alloc` included, is built with
  `-Zmin-function-alignment=64` (`kernel::target_args` in
  `tools/xtask/src/kernel.rs`);
- uACPI's C is built with `-falign-functions=64`
  (`kernel/lib/uacpi/build.rs`);
- the assembly that runs after boot starts each of its blocks with
  `.balign 64`: the system call entry and the copies to and from user
  memory, the interrupt stubs, the guarded MSR read and the call into
  the firmware.

The boot code, which runs once, keeps the alignment it needs, and user
programs keep LLVM's default.

## Consequences

- Where a function falls in its cache lines depends only on its own
  code. Code added elsewhere in the kernel no longer moves a hot path,
  so a benchmark that changes after a commit changed because of what the
  commit did to the code it runs.
- The padding between functions is never run, so no instruction count
  changes.
- The kernel's code grows by the padding, from 5,237,239 to 5,380,743
  bytes: 140 KiB, or 2.7 %.
- It makes the speed of a hot path stable, not higher. If open and close
  sit badly in their lines, they stay that way until their own code
  changes, and ADR 0409's gate shows it.
- A change to a hot function itself can still move its loops and
  branches within its lines. The benchmarks measure that as before.
