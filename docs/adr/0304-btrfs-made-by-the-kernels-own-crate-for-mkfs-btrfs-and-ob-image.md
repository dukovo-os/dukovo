<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0304. Btrfs made by the kernel's own crate, for mkfs-btrfs and ./ob image

- Status: Accepted
- Date: 2026-11-02

## Context

Phase 14a, its second part. The installer makes the Dukovo root, a Btrfs
of subvolumes (ADR 0137), and so does `./ob image`. Until now the image's
root came from btrfs-progs' `mkfs.btrfs --rootdir` (GPL-2.0) on the build
machine, a newer copy than Ubuntu 24.04's fetched when need be, and
`ob` then made every file root's by editing the inodes it wrote
(ADR 0213). Dukovo itself could mount a Btrfs and write it, but not make
one.

- Porting btrfs-progs brings its licence into the base system, and its
  `mkfs.btrfs` is 30,000 lines of C over its own copy of the kernel's
  tree code.
- The kernel's Btrfs crate already writes every tree a Btrfs has, with
  copy-on-write, checksums, compression and subvolumes (ADRs 0129 to
  0134 and 0138), and builds for the host for its tests.

## Decision

- **The crate makes a Btrfs** (`make`, its `make` feature), as
  `mkfs.btrfs` 6.17 makes one on one device by default, so that what
  Linux makes and what Dukovo makes are alike:
  - a system chunk of 8 MiB at 1 MiB, and a metadata chunk after it, both
    DUP; metadata of 256 MiB, or 1 GiB past 50 GiB, but at most a tenth
    of the device, as mkfs.btrfs sizes it; a data chunk of 8 MiB, single;
  - the root, extent, chunk, device, top subvolume, checksum, UUID, free
    space and data relocation trees, one leaf each, in generation 1, with
    the features mkfs.btrfs sets (the free space tree, no holes, extended
    references, skinny metadata);
  - the superblock at 64 KiB, and its copies at 64 MiB and 256 GiB where
    the device reaches them, with the first backup root filled;
  - 4 KiB sectors; nodes of 4 to 64 KiB; one copy of metadata or two;
    CRC-32C, xxHash64, SHA-256 or BLAKE2b; 109 MiB at least.
  Then the crate's writer, mounted on it, makes everything else, as it
  does on any Btrfs: subvolumes, files, compression.
- **mkfs-btrfs(8)** (`mkfs.btrfs`, and `mkfs -t btrfs`) in systools, on
  the disk engine (ADR 0300), takes mkfs.btrfs's options for one device
  and refuses a device that holds a filesystem or a table without `-f`.
- **`./ob image`** makes the root with the crate on the build machine:
  made, then mounted by the writer, which makes the subvolumes, writes
  the staging tree into them compressed with zstd, every file root's,
  and makes `@system.a` read-only. No btrfs-progs is needed to build an
  image, and nothing edits inodes afterwards.
- **Tested**: the crate's host tests compare what it makes with what
  mkfs.btrfs makes, tree by tree (`btrfs inspect-internal dump-tree` and
  `dump-super`), at several sizes and options, and Linux's `btrfs check`
  finds it whole, before and after the writer fills it. `ob`'s own test
  checks the root it writes the same way. The `btrfs-tools` boot test
  runs mkfs-btrfs on disks of 512-byte and 4 KiB sectors, and the host's
  `btrfs check` checks them. btrfs-progs stays a host tool of the tests:
  the oracle, not a builder.

## Consequences

- Dukovo makes its own root, and the installer can (Phase 14a, part 3).
- The images' roots are what an install makes, by the same code.
- Data is single, and one device is made: RAID comes from `device add`
  and `balance` (ADR 0159). `--rootdir`, `--subvol` and `-O`
  are not there; the writer does their work.
- The free space tree leaves out the superblock copies' ranges, which
  mkfs.btrfs leaves in and the kernel's allocator steps over; `btrfs
  check` finds both whole.
