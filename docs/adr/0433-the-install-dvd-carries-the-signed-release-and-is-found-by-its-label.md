<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0433. The install DVD carries the signed release and is found by its label

- Status: Accepted
- Date: 2026-11-09

## Context

ADR 0308 made the install stick: the live system on its EFI system
partition, and the signed release the installer installs on its
DUKOVO_LIVE partition, which Spark mounts read-only and which the
installer finds by its partition type. The ISO ran the live system but
carried no release, having no partition for one, so a machine that
starts only from a DVD could try Dukovo but not install it.

ADR 0431 and ADR 0432 brought optical drives on AHCI, IDE and USB, and
ISO 9660 and UDF to read their discs. 11h's last step for discs of data
is the live system from a real DVD, its release on the disc, and the
install from it.

A disc has no partition table: the ISO's El Torito boot images are
inside its ISO 9660 volume, not partitions the kernel lists. What marks
a disc is its volume's label, in the primary volume descriptor at
sector 16, which `xorriso -V` sets.

## Decision

- **The ISO carries the release in its `release` directory.** `ob image
  --iso` stages the signed release as for the stick, and xorriso grafts
  the repository in whole at `/release`: `metadata/`, `targets/` and
  `files/` as the update server will serve them (ADR 0308). Rock Ridge
  keeps the files' names and modes. The disc is the same release the
  stick carries, signed with the same keys.
- **The disc is labelled DUKOVO_LIVE**, as the stick's Btrfs is. An ISO
  with no release, such as most boot tests' images, keeps its version's
  label.
- **Spark takes the install DVD as the live medium**: a drive whose
  `/kernel/block/<drive>/disc` file says it is an optical drive, and
  whose disc's primary volume descriptor has the label DUKOVO_LIVE. Like
  the stick's partition, it is mounted read-only and is never the root,
  and with `live` on the command line it is the only volume mounted.
  Only a disc counts: a disk whose volume is labelled so is not the
  medium the system started from.
- **The installer finds the release on the disc** the same way, at
  `/volumes/<drive>/release`, or at `/volumes/DUKOVO_LIVE/release` when
  volumed mounted the disc after it was put in again. The drive is the
  medium's disk, never offered to install onto; optical drives are left
  out of the disks it lists altogether. Its screens name the disc where
  they would name the stick, and its last line says to remove the
  install disc. A disc holds no setup file: one comes from the web, and
  the Done page offers none to save.
- **`ob run --media ide-cdrom`** boots the ISO from a drive on an IDE
  controller's secondary channel, as an older PC has it, besides
  `cdrom` on AHCI.

## Consequences

- The ISO carries the system twice, as the boot archive the live system
  runs and as the release, and ISO 9660 does not compress: it grows by
  the release's size, about 220 MiB, to some 450 MiB, and still fits
  a CD.
- The boot test `live-dvd` boots the ISO under UEFI from an AHCI DVD
  drive beside a disk holding a Dukovo root: Spark takes no root and
  mounts only the disc, read-only, and `system-release check` passes on
  the disc's release, every file read from the disc. `install-dvd`
  boots it under a BIOS from an IDE DVD drive and installs onto a blank
  disk from the disc's release, and the disk then starts alone. `ob
  lab` builds the DVD beside the stick, and the lab test
  `install/from-dvd` installs from it by the installer's screens.
- A damaged or changed disc is refused file by file before anything is
  written, as a stick is.
- The eject button of the drive holding the DVD unmounts and ejects it
  (ADR 0432) unless the installer is reading it, which keeps it in.
- A real DVD drive and a burned disc are to be tried in the lab when it
  has them; writing the disc from Dukovo is 11h's later step.

## Alternatives considered

- **A partition table on the ISO** (isohybrid), with a DUKOVO_LIVE
  partition beside the ISO 9660 volume. A disc's drive reads the ISO
  9660 volume, not a table; the table would serve only a DVD image
  written to a stick, which the stick's own image already does better.
- **Finding the disc by a file on it,** such as `release/metadata`. Any
  disc could carry such a file; the label is what the image sets on
  purpose, and Spark reads one sector for it instead of mounting first.
- **A disc with only the release,** the live system booting from
  elsewhere. A machine that starts only from a DVD would need two discs.
