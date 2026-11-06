<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0311. The free-space and manual layouts, and swap at every start

- Status: Accepted
- Date: 2026-11-06

## Context

Phase 14a, the fifth step of its third part. The installer installs by
the guided layout alone, which erases the disk (ADRs 0309, 0310). The
approved design's Disk step also offers the disk's free space and a
manual layout on an rdisk screen, and the stick's menu has *Boot from
first disk*, which no test started. The guided layout makes a swap
partition, but nothing turns it on: Dukovo has no fstab.

## Decision

- **One plan for every layout.** Each layout becomes the same thing, a
  change to a disk: whether it gets a new GPT, the partitions taken
  out, the partitions made (their numbers, first sectors and sizes in
  the disk's own sectors, and names), and those formatted, each with a
  use: the Dukovo root, the EFI system partition, swap, BIOS boot, or
  data, on Btrfs, exFAT or FAT32 or left empty. The installer's
  `plan` module works a change out on the disk's table in memory, with
  the engine `rdisk` writes tables with (ADR 0300), before anything is
  written; the install then writes it as a layout file
  (rdisk-layout(5)) with `rdisk --script`, and formats each partition
  by its use. The screens, the review and the install all work from
  that one change, so what the review lists is what rdisk writes.
- **The free-space layout** keeps the disk's table and every partition
  on it, and makes the guided layout's four in its largest free space.
  It needs a GPT disk; a disk with no table or an MBR is the guided
  layout's. It refuses a disk that starts a system already, one with an
  EFI system partition or a BIOS boot partition, as a disk Windows or
  Linux starts from has, and names the partition: a second EFI system
  partition, or GRUB over another system's boot code, is a choice to
  make on purpose, in Manual, or with *Install next to Windows* in the
  plan's next part.
- **The manual layout** is the answers' `changes`, one entry for each
  disk changed (install-answers(5)), which the rdisk screen makes or an
  answers file gives. The changes must leave one root and one EFI system
  partition, on the same GPT disk, which is the target and also holds
  the swap partition, if any: Spark turns on only the swap of the
  root's disk. A partition made or formatted for a use other than data
  gets that use's type, taken out and made again in its place under its
  number; a data partition keeps the type it has. A partition taken out
  or under a new table cannot be formatted. Swap is a partition of the
  changes, so `swap_gib` is refused with them.
- **The rdisk screen, as the design draws it**: a tab for each disk
  but the stick, a bar of the disk, its partitions with their file
  system, size, mount and change, and the last three changes made,
  numbered. N makes a partition in the largest free space, offering
  the EFI system partition first, then the root in the rest; D takes
  one out, or keeps it again, or drops a new one; F formats one; G lays
  out the guided partitions on the disk; U undoes; Tab shows the next
  disk; Enter checks the changes and goes on. Each change is worked out
  as rdisk will write it as it is made, so one that cannot be is
  refused with the reason. The design's M, a mount point for each
  partition, is left out: Dukovo has no fstab, the root and EFI system
  partition are where the system puts them, and Spark mounts every
  other filesystem at `/volumes/<partition>`, so the screen shows where
  a use goes instead of asking. ext4, which Dukovo reads but does not
  make, is not offered.
- **Swap at every start.** After it mounts the root, Spark turns on each
  partition of the root's disk whose type is Linux swap (its GPT GUID,
  or 0x82 in an MBR), as systemd's gpt-auto-generator does by the
  Discoverable Partitions Specification, and says so on the console.
  It reads the partition's first page first: one that does not end in
  `SWAPSPACE2` is left off, as when Linux hibernated into it, so that
  the memory Linux means to wake with is not written over. Swap on other
  disks belongs to their own systems and is left alone.
- **GRUB for the firmwares the disk can start.** A BIOS's GRUB finds
  its files on the disk's partition 2, its EFI system partition
  (rgrub-setup(8)), as the guided layout numbers it. The free-space
  layout numbers its EFI system partition 2 wherever it lies, the
  other partitions taking the first numbers free; on a disk that has a
  partition 2 already, it leaves out the BIOS boot partition. The rdisk
  screen numbers a new EFI system partition 2 where it can, and it and
  the install refuse a target with a BIOS boot partition whose
  partition 2 is something else. A disk with no BIOS boot partition
  gets GRUB for UEFI alone, which the review says.
- **Boot from first disk, tested.** Under UEFI, the stick's entry
  looked for `EFI/BOOT/BOOTX64.EFI` on `hd1`, taking the stick for
  `hd0` as a BIOS's GRUB does; but UEFI's GRUB numbers disks as the
  firmware lists them, which need not put the stick first, and may
  call a USB stick a floppy, `fd0`. It now looks on every disk, `hd0`
  to `hd7`, in the first eight GPT partitions of each, not four, as the
  manual layout can put the EFI system partition after others, and
  passes over one that holds the live system's kernel, `boot/radix`:
  the stick's own, or another Dukovo stick's. The partition it finds
  becomes GRUB's root before it is started, as GRUB's chainloader tells
  the loader it starts that it came from the root's disk: told the
  stick, the disk's GRUB read the stick's menu and started the stick
  again. Under a BIOS the stick is `hd0`, and the entry is as it was.
  A disk kind of the boot tests starts a case's second boot from the
  stick still there, with the menu's default changed to that entry, as
  a person who left the stick in would. Under UEFI that disk is in the
  firmware's boot order after the stick, as a machine's own disk is:
  OVMF makes ready only the disks QEMU's boot order names, and GRUB
  sees no other.
- **Two boot tests.** `install-free`, under a BIOS: the live shell
  gives the blank disk a data partition with a file, an answers file
  installs into the free space after it, and the second boot, by
  *Boot from first disk*, signs alice in, with the swap on and the file
  under `/volumes`. `install-manual`, under UEFI, by keys: the shell
  makes three partitions, and on the rdisk screen D takes out the
  second, F formats the third, and N makes the EFI system partition in
  the second's place, swap of a size typed and the root; the review
  lists each change, and the second boot, by *Boot from first disk*,
  starts the disk's UEFI GRUB.

## Consequences

- The free-space test found the partition 2 rule: its first run put
  the EFI system partition third, and rgrub-setup refused the disk.
- The screens tests found why an install by the screens now and then
  failed with EEXIST on a file of the release it was copying. The C
  library chose the lowest free descriptor for an open, and the kernel
  filled it only once the file was made; the screens' other thread,
  whose terminal library opens `/dev/tty` each time it draws, could
  take that descriptor in between, and the open, choosing again, found
  the file it had just made. The thread that chooses a descriptor now
  holds it until the kernel has filled it, other threads choosing the
  next (radix-posix(7)), and the POSIX threads test opens files with
  `O_EXCL` while another thread opens and closes descriptors.
- Dukovo installs beside data a disk holds, and the rdisk screen makes
  any layout rdisk can, with the checks the install needs made before
  anything is written.
- Every installed system with a swap partition uses it, whichever
  layout made it, with no file to keep in step with the disks.
- The screens and answers files cannot put the root on an MBR disk,
  as its type is a GPT one, nor mount a data partition elsewhere than
  `/volumes`.
- *Install next to Windows* and the mirror layout come with the plan's
  fourth part and later.
