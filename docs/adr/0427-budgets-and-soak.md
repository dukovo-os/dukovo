<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0427. Budgets for the boot, memory and images, and the soak

- Status: Accepted
- Date: 2026-11-10

## Context

A system gets slower to start, heavier and larger a little at a time,
each change too small to notice, unless something holds it to a number.
The plan's Phase 14 sets three: an installed system starts in 10 seconds
from the boot loader to the sign-in, uses 256 MB of memory idle, and
ships in images of 4 GiB at most. The benchmarks already hold every
operation to Linux (ADR 0039); nothing held these.

Neither did anything watch a system that runs for days. A leak of a page
a minute, or a few blocks a round that are never freed, passes every
boot test, which ends in minutes, and shows only on a server weeks
after an update. The lab's soak tests run for twenty minutes.

## Decision

- **The boot.** The `installed-uefi` and `installed-bios` boot tests read
  the kernel's time on the console service's `tty1 is shown` line, its
  clock having started as GRUB handed over, and fail over 10 seconds
  under KVM, as CI runs them. Without KVM, under emulation, the time is
  only reported. The lab's `performance/boot-time` holds real machines
  to the same 10 seconds.
- **Memory.** `installed-uefi` already holds the idle installed system
  to under 100 MiB in QEMU, counting the startup archive (Phase 14e's
  first budget), which is well within 256 MB and stays. The lab's
  `performance/idle-memory` holds real machines, whose firmware and
  devices take more, to 256 MiB.
- **Images.** `ob image` fails when any image it writes, the cloud
  image's formats included, is 4 GiB or more: the most a FAT32 file can
  hold, so that every image copies to any stick or drive. Each image's
  size is printed against the budget.
- **The soak.** `ob soak` boots an installed system's disk in QEMU and
  runs rounds of mixed load until its time is up: 128 MiB of files
  written, copied, compared and removed on the root, 300 programs, and
  32 MiB over the network from a program of the host's. Each round
  records the memory in use, neither free nor the page cache, and the
  space the root's filesystem uses. A round whose load fails ends the
  soak, as does memory past twice the first round's; at the end the
  last quarter of the rounds is held to the second, after the caches
  have warmed, with a slack of a twentieth of the memory or 8 MiB and
  64 MiB of disk. Each release soaks for seven days on the lab's PC
  first (`--hours 168`, the default). CI soaks for five hours each week
  on main (`.github/workflows/soak.yml`), as GitHub stops a job at six,
  not on each pull request.

## Consequences

- A change that makes the boot slower than 10 seconds fails CI, as one
  over the idle budget did before, and one that makes an image 4 GiB or
  larger fails `ob image`, which CI runs too.
  Today the boot reaches tty1 in about 3.4 seconds even without KVM,
  and the USB stick's image is under 700 MiB.
- A leak shows within a week on main, and before a release in the lab;
  `rounds.csv` shows when it began.
- The soak runs in QEMU, with virtio's disk and network. A leak in a
  driver of real hardware shows in the lab's own soak tests, on its
  machines.
