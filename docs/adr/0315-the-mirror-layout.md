<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0315. The mirror layout

- Status: Accepted
- Date: 2026-11-07

## Context

A server with two disks should keep running when one of them dies. The
answers file has had a `mirror` key since ADR 0309, and the approved
installer design offers *Mirror two disks (Btrfs RAID1)* on the Disk
page, with a *Mirror onto* list, and says: "Both disks are erased. If
one fails, Dukovo keeps running from the other and tells you." Phase 14
PR 5a builds it.

Radix already reads and writes Btrfs RAID1, mounts it with a disk
missing and warns (ADR 0153), adds disks and converts profiles (ADR
0159), and rebuilds onto a new disk (ADR 0155). `mkfs-btrfs` makes a
filesystem on one device only (ADR 0304).

Linux's installers differ. Fedora's Anaconda makes a Btrfs RAID1 with
`mkfs.btrfs -d raid1 -m raid1` over both disks, but puts the EFI system
partition on one disk, or on an mdadm RAID1 of metadata 1.0, which the
firmware reads as plain FAT and which goes out of sync whenever the
firmware writes to it. Debian's installer mirrors with mdadm under any
filesystem and leaves the boot loader on the second disk to the
administrator. Ubuntu Server's installer offers mdadm RAID1 and puts
an EFI system partition on each disk, which `grub-install` then keeps
in step.

## Decision

- **Each disk is laid out as the guided layout lays out one**: a new
  GPT with a 1 MiB BIOS boot partition, a 512 MiB EFI system partition
  as partition 2, swap of the size asked for, and the root. The two
  roots are the same size, the most the smaller disk holds, in whole
  MiB; the larger disk's space beyond is left free, as a RAID1 could
  not use it.
- **Either disk starts the computer alone.** GRUB goes on both, for a
  BIOS and for UEFI, at UEFI's fallback path, with the same menu, which
  names the root by its Btrfs UUID: the two roots share it, so either
  disk's GRUB finds the root on whichever disk is there, and reads it
  from one disk as GRUB's Btrfs reads a RAID1. No UEFI boot entry is
  made, as for the guided layout: the firmware tries each disk's
  fallback path in turn, and an entry naming a dead disk would only be
  skipped.
- **The root is made on one disk and mirrored onto the other.** The
  installer makes the root with `mkfs-btrfs` on the target, mounts it,
  adds the second disk's root partition with `device add`, and converts
  the new filesystem's three chunks to RAID1 with `balance start
  -dconvert=raid1 -mconvert=raid1`, before anything is copied. The
  balance then moves a few MiB, and every chunk the copy writes after
  is RAID1. Linux's `btrfs check` finds the result whole from both
  disks, and from each alone.
- **A balance keeps a data chunk.** The new root's one data chunk is
  empty, and moving it left the filesystem with no data chunk and no
  profile to make one of, so the first write failed with no space.
  Linux's balance makes a data chunk first when the data has nothing to
  move (`chunk_reserved` in `__btrfs_balance`). Radix's balance now
  makes one before it moves the last data chunk, when there is no
  other, in the profile it converts data to. A chunk that holds data
  would get one as its data moves, so a balance makes no chunk it would
  not have made before but an empty filesystem's.
- **Each disk has its own swap.** Spark turns on the swap of the disk
  it found the root on (ADR 0311), so the swap goes with whichever disk
  is left. Swap is not mirrored: a disk that dies under a running
  system takes with it what was swapped out to it.
- **The second disk is chosen on the Disk page**: *Mirror onto* lists
  every disk but the target and the stick, and the layout cannot be
  chosen without one. Its note says *recommended for servers* when a
  disk of the target's size is there, and *the smaller disk sets the
  size* otherwise. Both disks are warned of in red when they hold
  something, as the guided layout warns of one. The review shows each
  disk erased and its partitions, each root as *Btrfs RAID1, at /*.
  The answers name the second disk in `mirror`.
- **The install's step** *Mirroring the root on both disks (Btrfs
  RAID1)* runs between making the filesystems and the subvolumes, and
  logs both commands and the balance's output.
- **When a button has the focus, a page shows its end.** The Disk page
  of a mirror is longer than the design's 31 rows, and its warnings come
  last; Tab reaches the buttons after every field, so the warnings show
  before Next.

## Consequences

- A two-disk server keeps running, and starts again, with either disk
  dead, and the kernel log says that a disk is missing (ADR 0153).
- Rebuilding onto a new disk is by hand for now: rdisk gives it the
  partitions, `replace` rebuilds the root onto it (ADR 0155), and
  `rgrub-setup` puts GRUB on it.
- The EFI system partitions are not kept in step after the install.
  GRUB's menu names the root by UUID and the slots by their names, so
  an update that changes neither leaves both right; an update that
  changes the menu must write both (Phase 15).
- GRUB's environment block is on each disk's EFI system partition, so
  the entry chosen last is remembered on the disk that started.
- The `install-mirror` boot test installs onto two blank disks under
  OVMF and checks on the host each disk's layout, GRUB and menu, and the
  RAID1 with Linux's `btrfs check` from both disks and from each; then
  the first disk is taken out and the second put in its place, beside a
  blank disk: the second's own GRUB starts, the kernel warns of the disk
  missing, and alice signs in and writes a file. Unit tests cover the
  layout's sizes, the answers and the screens.
- The lab gives a test one disk beside the stick, so its test
  `install/mirror-needs-two` checks that the Disk page offers the
  mirror and says it needs a second disk; an install onto two of a lab
  machine's disks waits for the lab to give a test two.

## Alternatives considered

- **`mkfs-btrfs` over both disks**, as Linux's `mkfs.btrfs` makes a
  RAID1. It needs a second way of making the chunk tree, the device
  tree and each disk's superblock in the kernel's Btrfs crate, for what
  one balance of an empty filesystem does with code the boot tests
  already prove. `mkfs-btrfs` may learn it later, for itself.
- **mdadm-style software RAID under the root.** Radix has no block
  RAID, and Btrfs RAID1 checks every copy by checksum, so it reads the
  good copy where a block RAID cannot tell which one is good.
- **One EFI system partition, on the target.** The computer would then
  not start with the target dead, which is the case a mirror is for.
- **An EFI system partition on a RAID1 of both disks**, as Anaconda can
  make. The firmware writes it as plain FAT on one disk, behind the
  RAID's back, and the copies drift.
