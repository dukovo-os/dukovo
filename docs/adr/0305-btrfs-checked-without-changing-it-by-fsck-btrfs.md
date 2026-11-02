<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0305. Btrfs checked without changing it, by fsck-btrfs

- Status: Accepted
- Date: 2026-11-02

## Context

Phase 14a, its second part. An installer that has just made a root, a
person whose disk misbehaves, and a test that has written one, all ask
whether a Btrfs is whole. Linux asks `btrfs check` (GPL-2.0), which
Dukovo does not have. The kernel's scrub (ADR 0150) reads every block
against its checksum and mends a bad copy from a good one, but it does
not check that the trees agree with each other: that every extent has
the references the trees say, that the free space is free, that names
and inodes match.

## Decision

- **The crate checks a Btrfs** (`check`, its `check` feature) as `btrfs
  check --readonly` does, in its seven phases, and never writes:
  - every superblock copy, and every copy of every tree block, by its
    checksum, its address and its filesystem's UUID; a bad copy is
    reported and the good one read past;
  - the extent tree against every tree that uses it: each tree block's
    and data extent's reference count, back references and level, and
    extents that nothing uses; the bytes used of each block group, each
    device and the superblock; each chunk against its device extents;
  - the free space tree against the extents;
  - in each subvolume, names against inodes both ways, link counts,
    directory sizes, byte counts, entry types and overlapping file
    extents;
  - that data has its checksums, and with `--check-data-csum`, that each
    copy of it matches them;
  - subvolumes' references against their names.
- **fsck-btrfs(8)** (`btrfsck`) in systools prints each phase as `btrfs
  check` does, then what is wrong and a summary, with fsck(8)'s exit
  status (0, 4, 8, 16), as fsck-fat does (ADR 0302). It refuses a
  filesystem in use unless `--force`, and every option that would
  change one.
- **Tested** on the host: what mkfs.btrfs makes with several options,
  what the crate makes, and what its writer writes (subvolumes, a
  snapshot, links, compressed files), `check` finds whole as `btrfs
  check` does; damage made on purpose (a directory's size, a block
  group's bytes used, a tree block's references, a device's bytes, a
  link count, a data checksum, one copy of a tree block), `check` finds
  as `btrfs check` does. The `btrfs-tools` boot test checks the disks
  mkfs-btrfs made.

## Consequences

- Dukovo checks its own Btrfs, and the installer can check the root it
  made before it reboots.
- It does not mend: scrub mends what a second copy can, and the rest
  needs a backup. `--repair` is not there.
- Quota groups are not checked; the seventh phase says so.
