<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0306. GRUB built from its source, and put on disks by our own tools

- Status: Accepted
- Date: 2026-11-02

## Context

Phase 14a, its second part. Until now `ob image` ran grub-mkrescue,
which copies the build machine's GRUB, every module its distribution
built, into a hybrid ISO that is also a GPT disk (ADR 0010), and the
Dukovo root went after its four partitions (ADR 0137). The image's GRUB
was whichever version the build machine had, the license manifest could
only point at the distribution's source package for it, and Dukovo had
no way to put GRUB on a disk itself, which the installer needs.

## Decision

- **GRUB 2.12 is built from the GNU project's tarball** by `ob`
  (`tools/xtask/src/grub.rs`): fetched once, from ftp.gnu.org or
  Ubuntu's byte-identical copy, checked against its pinned SHA-256,
  configured with no translations, fonts, themes or tools that need
  other libraries, and built with the build machine's GCC, flex, Bison
  and gawk for a PC's BIOS (`i386-pc`) and for UEFI (`x86_64-efi`) into
  `.cache/grub/2.12`. Its source is not changed; the one file the
  release tarball leaves out that its build reads,
  `grub-core/extra_deps.lst`, is written as GRUB's repository has it.
- **Each image's GRUB holds only these modules**, which `grub-mkimage`
  puts in its image, so it loads none from the disk: the GPT and MBR
  tables, FAT, Btrfs and ISO 9660, Multiboot2, the environment block,
  `search`, the serial port, the graphical terminal and video, the menu
  and its commands (`normal`, `test`, `echo`, `configfile`, `boot`), and
  the firmware's video and disks (`vbe` and `biosdisk` under a BIOS,
  `efi_gop` and `efi_uga` under UEFI).
- **`ob image` makes a GPT disk image with Dukovo's own code**, a USB
  stick's: partition 1 a BIOS boot partition of 1 MiB holding GRUB's
  `core.img`, partition 2 the EFI system partition, FAT32, made by
  mkfs-fat's engine and filled by the kernel's FAT crate with the kernel,
  its modules, the license manifest, `boot/grub/grub.cfg` and UEFI's GRUB
  at `EFI/BOOT/BOOTX64.EFI`, and partition 3 the Dukovo root. GRUB's
  `boot.img` is in the first sector, beside the protective MBR the UEFI
  specification asks for, so the stick boots under either firmware.
  GRUB for a BIOS finds its files at `(,gpt2)/boot/grub`, the disk's
  second partition; UEFI's on the partition it was loaded from.
- **`ob image --iso` makes an ISO for optical media** with xorriso, El
  Torito booting GRUB's `cdboot.img` and a `core.img` for a CD under a
  BIOS, and a FAT image holding UEFI's GRUB, which finds the disc by the
  UUID GRUB reads from its time of making. It is not a disk image too:
  the disk image is for sticks. The boot tests from a CD boot one.
- **rgrub-setup(8)** in systools puts GRUB on a disk on Dukovo, from the
  images of it the system carries in `/system/boot/grub` (`boot.img`,
  `core.img`, `BOOTX64.EFI`, made by `ob`): for a BIOS, `boot.img` in the
  disk's first sector and `core.img` in its BIOS boot partition, as
  grub-bios-setup puts them (the disk engine's `grub`, with GRUB 2's
  `boot.h` layout and no Reed-Solomon codes); with `--efi`,
  `BOOTX64.EFI` in an EFI system partition's FAT, unmounted, written by
  the kernel's FAT crate, which the disk engine's `esp` uses. Both
  `ob image` and rgrub-setup use the same code.
- **The license manifest lists GRUB 2.12** with the tarball's download
  location and SHA-256, its `COPYING`, and where its build recipe is,
  which with Dukovo's source is its corresponding source.

## Consequences

- Every image boots the same GRUB, whatever the build machine has; the
  build machine needs GCC, flex, gawk and Bison instead of GRUB's
  packages, and the first build takes some two minutes more, which CI
  caches.
- Dukovo can make its installer's disks bootable itself (Phase 14a's
  third part).
- The image's root partition is now its third, not its fifth.
- GRUB for a BIOS looks for its files on the disk's second partition,
  which Dukovo's own layouts give it; rgrub-setup refuses a disk whose
  second partition is not an EFI system partition, and MBR disks.
- Images are not yet bit-for-bit reproducible: their GPT, filesystems and
  GRUB's images hold random UUIDs and the build machine's GCC's code.
