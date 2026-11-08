<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0314. Installing next to Windows

- Status: Accepted
- Date: 2026-11-07

## Context

Most computers Dukovo will meet came with Windows, and people want to keep
it. Phase 14 PR 4 adds the installer's last layout: *Install next to
Windows*, which the approved installer design shows (ADR 0402). Its two
hard parts are in place: shrinking Windows' NTFS without losing a file
(*resize-ntfs*, ADR 0312) and making the firmware's boot entries
(*efiboot*, ADR 0313). What is left is deciding when the layout is
offered, how much space each system keeps, where Dukovo's partitions go,
and how the computer then starts each system.

Linux's installers differ. Ubuntu's and Fedora's put their boot loader on
Windows' EFI system partition, which on many machines is 100 MiB and
nearly full, and which Windows' feature updates rewrite. Both rely on
`os-prober` to find Windows when the install writes GRUB's menu, so a
Windows installed later, or moved, is missing from it until someone runs
`update-grub` again.

## Decision

- **Offered for Windows that starts by UEFI.** A disk has Windows when it
  has a GPT, an EFI system partition holding Windows Boot Manager
  (`EFI/Microsoft/Boot/bootmgfw.efi`), and a basic data partition holding
  NTFS, or one BitLocker encrypts: the largest such partition is C:. The
  installer reads it with the kernel's NTFS crate, as *resize-ntfs* does,
  for the space its files use, the smallest it shrinks to and why it
  cannot shrink now. The stick must have been started by UEFI, since the
  layout needs a boot entry; under a BIOS the page says how to start it
  by UEFI. Windows that starts by a BIOS, from an MBR, is left to the
  manual layout.
- **It is refused, with what to do in Windows,** when C: is encrypted by
  BitLocker (turn BitLocker off or suspend it); when Windows hibernated
  or shut down with Fast Startup (turn Fast Startup off, shut down); when
  C: is marked to be checked (run `chkdsk C: /f`); when the check finds
  it damaged; and when *resize-ntfs* would refuse it for its own reasons.
  The page shows each as a warning under the layout, which cannot then be
  chosen; chosen, the layout shows the three checks passed: BitLocker
  off, Fast Startup off, Windows shut down fully.
- **How the space is shared.** Dukovo gets at least 20 GiB. Windows keeps
  10 GiB free beyond what its files use, or the smallest *resize-ntfs*
  can make C:, whichever is larger, so that Windows can still update.
  Dukovo's root gets at least 8 GiB beyond its EFI system partition and
  swap. The installer suggests 60 GiB, or the most there is, and the
  answers file names the size in `space_gib`.
- **Where the partitions go.** C: is shrunk in place, its start kept, to
  a whole MiB. In the space it gives back go Dukovo's own EFI system
  partition, *Dukovo boot* (512 MiB, FAT32), the swap asked for, and
  the root, which ends where C: did. Windows' EFI system partition, its
  reserved and recovery partitions and any free space elsewhere stay as
  they are. No BIOS boot partition is made: Windows started by UEFI.
- **The volume first, then the partition.** The installer checks C: with
  `resize-ntfs --check`, shrinks it with `resize-ntfs --size`, and checks
  it again; then rdisk's layout file shrinks the partition with its new
  `resize` key (*rdisk-layout*(5)) and adds Dukovo's. Stopped between
  the two, a volume smaller than its partition is still read whole by
  Windows. As after Linux's `ntfsresize`, C: stays marked to be checked,
  so Windows runs `chkdsk` once at its next start.
- **A boot entry of Dukovo's own.** GRUB for UEFI goes on *Dukovo boot*,
  at the fallback path too, and `efiboot` adds a UEFI boot entry,
  *Dukovo*, first in the firmware's boot order, after taking out any
  earlier install's entry of that name, as Linux's `grub-install` does.
  Windows Boot Manager's entry and files are not touched: the firmware's
  own boot menu still starts Windows directly.
- **GRUB's menu finds Windows at every start.** Under UEFI, the installed
  menu searches every partition for Windows Boot Manager; when it is
  found, the menu gains *Windows Boot Manager*, which chainloads it, the
  menu always shows, for at least 5 seconds, and the entry chosen last,
  Dukovo or Windows, is chosen again, from GRUB's environment block on
  the EFI system partition, which the installer writes for every layout.
  The menu also offers *UEFI firmware settings* where the firmware lets
  GRUB restart into them (`fwsetup`, whose module the UEFI image now
  holds). The guided and free-space layouts make no boot entry: their
  disk's fallback path starts GRUB when it is the only system, and an
  entry outliving a wiped disk would only clutter the firmware's list.

## Consequences

- People can keep Windows without preparing the disk in Windows first,
  beyond turning Fast Startup and BitLocker off, which the page tells
  them to do.
- Windows checks C: once after the install; that takes seconds on an
  SSD.
- Starting the installer again after a shrink finds C: marked to be
  checked, and refuses it until Windows has started once.
- Windows' updates cannot overwrite Dukovo's boot files, nor fill the
  EFI system partition it shares, since it shares none. The cost is
  512 MiB of the space given to Dukovo.
- The `install-beside` boot test lays a disk out as Windows does, with
  a 32 GiB C: of files, installs next to it under OVMF, and checks on
  the host that Windows' partitions and boot files are kept and that
  ntfs-3g finds C: sound at 12 GiB with every file; the disk then
  starts by the entry *Dukovo*, GRUB offers Windows, and the choice is
  remembered. Unit tests cover the rules, the refusals, the review and
  the layout file.
- The lab cannot yet test a real Windows: dukovo-os/lab has no Windows
  disk image, and Dukovo makes no NTFS from nothing. The lab test
  `install/next-to-windows` checks what the Disk page says on a disk
  with no Windows; the lab's own machines, once they hold Windows, test
  the rest.

## Alternatives considered

- **Sharing Windows' EFI system partition**, as Ubuntu and Fedora do.
  It saves 512 MiB, but the partition is often too small for GRUB and
  its fonts, and Windows' updates have replaced or deleted other
  systems' boot files on it.
- **`os-prober` at install time.** A menu written once misses a Windows
  added or repaired later; GRUB's `search` at each start costs a
  fraction of a second.
- **Starting Dukovo from Windows Boot Manager** with an entry `bcdedit`
  adds. That needs Windows running, and leaves Dukovo's start in
  Windows' hands.
- **Shrinking C: to its smallest.** Windows fails to update with too
  little free space; 10 GiB is what its feature updates ask for.
