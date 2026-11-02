rgrub-setup(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

rgrub-setup - put GRUB on a disk, for a BIOS or for UEFI

# SYNOPSIS

*rgrub-setup* [*-d* _directory_] _disk_

*rgrub-setup* *--efi* [*-d* _directory_] _partition_

# DESCRIPTION

rgrub-setup puts GRUB 2.12 on a disk, as GRUB's *grub-install* does,
from the images of it the system carries in _/system/boot/grub_, which
*ob*(1) builds from GRUB's source with only the modules Dukovo needs
(ADR 0306). _disk_ and _partition_ are names in _/devices_, such as
_vda_, or files.

For a BIOS, it puts GRUB's _boot.img_ in the boot code of the first
sector of _disk_, keeping the protective MBR's table and disk signature,
and GRUB's _core.img_ in its GPT's BIOS boot partition (type
21686148-6449-6E6F-744E-656564454649, *bios-boot* to *rdisk*(8)), as
*grub-bios-setup* puts them, without Reed-Solomon codes. That GRUB finds
its files in _/boot/grub_ of the disk's second partition, which must be
its EFI system partition.

With *--efi*, it puts UEFI's GRUB in the FAT of the EFI system
partition _partition_, as _EFI/BOOT/BOOTX64.EFI_, the file firmware
starts a disk by when it has no boot entry of its own for it, in place
of the one there. The system leaves EFI system partitions unmounted
(*volumed*(8)): rgrub-setup writes the FAT with the kernel's own FAT
code, and refuses a partition in use. That GRUB finds its files in
_/boot/grub_ of the partition it was started from.

GRUB's files there, _grub.cfg_ above all, are not rgrub-setup's to
write: Dukovo's installer writes them.

```
# rgrub-setup vda
Installed GRUB for a BIOS on /devices/vda: boot.img in its first sector, core.img (59 sectors) in its BIOS boot partition, 1, from sector 2048
# rgrub-setup --efi vda2
Installed GRUB for UEFI on /devices/vda2: EFI/BOOT/BOOTX64.EFI (1172 KiB)
```

# OPTIONS

*--efi*
	Put UEFI's GRUB on the EFI system partition _partition_ instead.

*-d*, *--directory* _directory_
	Take GRUB's images, _boot.img_, _core.img_ and _BOOTX64.EFI_, from
	_directory_ instead of _/system/boot/grub_.

# DIFFERENCES FROM LINUX

- It writes no boot entry to the firmware's variables, as
  *grub-install* does with *efibootmgr*: UEFI's GRUB goes where firmware
  looks on its own, as *grub-install --removable* puts it.
- It takes no _--target_: *--efi* chooses UEFI, and the images are
  Dukovo's own GRUB's, for x86-64 PCs.
- GRUB for a BIOS goes only on a disk with a GPT and a BIOS boot
  partition, not in the gap an MBR leaves before its first partition.

# EXIT STATUS

0
	GRUB was put on the disk.

1
	It was not: the images or the disk could not be read or written, the
	disk is a partition, has no GPT, no BIOS boot partition as large as
	_core.img_, or no EFI system partition second, the partition holds no
	FAT or is in use, or the command line was wrong.

# SEE ALSO

*rdisk*(8), *mkfs-fat*(8), *dukovo-boot*(7), *volumed*(8)
