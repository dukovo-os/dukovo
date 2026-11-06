<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0307. An installed system starts from its slot, with Spark alone

- Status: Accepted
- Date: 2026-11-06

## Context

Phase 14a, its third part, and the plan's "Lean installed systems". The
boot loader loads the boot archive, the whole system of about 199 MiB,
into memory, and keeps it there for the machine's whole life: the kernel
serves `/system` from it when no disk holds a Dukovo root, and nothing
frees it (ADR 0392). A USB stick or a disc needs that, having no disk to
run from. An installed system does not: Spark mounts its root's
`@system.a` on `/system` (ADR 0137), and from then on every program is
read from the disk. Until now the image that has a root still loaded the
whole archive, so a machine running from its disk used about 199 MiB of
memory for nothing. Phase 14e's budget is 100 MiB for an idle machine,
counting what the boot loader keeps.

The updates design (section 5, approved 2026-11-05) puts each slot's
kernel and boot archive in the slot's own read-only subvolume, which
GRUB reads, so that a new slot's boot files appear at the same moment as
the rest of it, and an update writes nothing on the EFI system partition
but the slot state. The roadmap mounts the EFI system partition at
`/system/boot`.

## Decision

- **`ob` builds a startup archive** beside the boot archive,
  `out/sysroot/boot/startup`: Spark at `/system/commands/spark` and the
  directories Spark mounts on, nothing else. Spark needs nothing from
  `/system` until it has mounted the root.
- **The slot holds what GRUB starts**: `/system/startup/radix`, the
  kernel without its debug information, and `/system/startup/archive`,
  the startup archive. The folder is not under `/system/boot`, which the
  EFI system partition will cover. `ob` puts both into `@system.a`
  whenever it makes a root.
- **An installed system's EFI system partition holds GRUB and its menu**
  (`image/grub/installed.cfg.in`), and the microcode file if the image
  has one, but no kernel or archive. The menu finds the root with
  `search --fs-uuid` and loads `/@system.a/startup/radix` and
  `/@system.a/startup/archive` from it, the archive with the word
  `bootfs`, as the kernel looks for it. GRUB reads Btrfs and its zstd
  extents itself.
- **The kernel's command line names the root**, `root=UUID=<uuid>`, a
  flag for Spark, not the kernel, as `startup-trouble` is for the
  sign-in. Spark reads it from `/kernel/cmdline` and takes only the
  Btrfs whose superblock has that UUID as the root, so that a second disk
  with Dukovo on it, or an install stick, never becomes the system GRUB
  did not start. A Btrfs of two disks has one UUID, so a mirrored root
  still boots with one disk gone. If no disk holds the named root, Spark
  says so on the console, in two lines, and starts nothing: the startup
  archive has nothing to start. Without the flag Spark takes the first
  disk that holds a root, as before, which the USB stick's image relies
  on.
- **An installed system boots with `panic=10`**, as the updates design
  says, so that a machine nobody is at restarts by itself.
- **`ob image --installed`** makes such a disk,
  `out/dukovo-<version>-x86_64-installed.img`: the BIOS boot partition,
  the EFI system partition and the Dukovo root, the layout the installer
  will leave less its swap, and `ob run --installed` boots it as a SATA
  disk. The cloud images of Phase 14d start from it.
- **The boot tests `installed-uefi` and `installed-bios`** boot that
  disk under each firmware with 512 MiB of memory, check that the boot
  loader's archive is the startup archive, under a MiB, and that the
  idle machine, with its services running, uses less than 100 MiB,
  counting that archive. `installed-bios` boots a second time naming a
  root no disk holds, and Spark must take no other.

## Consequences

- An installed system keeps the startup archive, 863 KiB, where it kept
  the whole system, about 199 MiB now, so it starts with that much more
  free. On QEMU the idle machine uses 41 MiB, counting the archive,
  under Phase 14e's target of about 50.
- A slot changes its kernel with its programs; Phase 15a's A/B slots
  switch both with the slot state, and Rescue covers the machine whose
  root will not mount.
- Each root holds the kernel and the startup archive, about 11 MiB
  before zstd compresses them, and the USB stick's root carries them
  too, unused until the installer copies the system.
- GRUB 2.12 must read the Btrfs the kernel's crate writes, zstd extents
  included: the two boot tests check that it does, under both firmwares.
- The boot loader's copy of the startup archive is still never freed; at
  its size that no longer matters.
- The USB stick and the ISO still run the whole system from memory: they
  have no disk to run from.
