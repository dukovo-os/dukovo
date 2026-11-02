mkfs-btrfs(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

mkfs-btrfs, mkfs.btrfs - make a Btrfs filesystem

# SYNOPSIS

*mkfs-btrfs* [*-L* _label_] [*-n* _nodesize_] [*-m* dup|single] [*--csum* _type_] [*-U* _uuid_] [*-b* _size_] [*-f*] [*-q*] _device_

# DESCRIPTION

mkfs-btrfs makes a Btrfs filesystem on _device_, a disk or partition in
_/devices_ or a file, with the options of btrfs-progs' *mkfs.btrfs*, by
whose name it runs too (ADR 0304). The kernel's own Btrfs crate makes
it, the one that mounts it, and makes what *mkfs.btrfs* 6.17 makes on
one device by default: a system chunk and a metadata chunk of two copies
each, a data chunk of one, the free space tree, and the top subvolume's
directory. Linux's *btrfs check* finds it whole, and Linux mounts it.

The metadata chunk is 256 MiB, or 1 GiB on a device of more than 50 GiB,
and at most a tenth of the device, as *mkfs.btrfs* sizes it. A Btrfs
needs 109 MiB at least. The superblock is written at 64 KiB, and its
copies at 64 MiB and 256 GiB where the device reaches them. The marks of
any other filesystem on the device are wiped, as *wipefs* wipes them.

mkfs-btrfs refuses a device in use, and a device that holds a filesystem
or a table of partitions already, unless *-f*. Once it has written, the
links of _/devices/disk_ follow the new label and UUID (ADR 0303). It
prints what it made:

```
Label:              data
UUID:               87848637-a4e7-46be-8bf2-20dd6b6c242a
Node size:          16384
Sector size:        4096
Filesystem size:    1.00GiB
Block group profiles:
  Data:             single       8.00MiB
  Metadata:         DUP         51.19MiB
  System:           DUP          8.00MiB
Checksum:           crc32c
Device:             /devices/vda2
```

# OPTIONS

*-L* _label_, *--label* _label_
	The filesystem's label: up to 255 bytes.

*-n* _nodesize_, *--nodesize* _nodesize_
	The size of a tree block, in bytes or with *K*: 4 KiB, 8, 16, 32 or
	64 KiB. 16 KiB unless it is given.

*-s* _sectorsize_, *--sectorsize* _sectorsize_
	The sector size, which can only be 4096 bytes, the default.

*-m* _profile_, *--metadata* _profile_
	*dup*, two copies of metadata, the default; or *single*, one.

*-d* _profile_, *--data* _profile_
	*single*, the only one there is.

*--csum* _type_, *--checksum* _type_
	The checksum of every block: *crc32c*, the default, *xxhash*,
	*sha256* or *blake2*.

*-U* _uuid_, *--uuid* _uuid_
	The filesystem's UUID, instead of a random one.

*-b* _size_, *--byte-count* _size_
	Make the filesystem this size, in bytes or with *K*, *M*, *G* or
	*T*, from the device's start.

*-f*, *--force*
	Make the filesystem over another, or over a table of partitions.

*-q*, *--quiet*
	Print nothing but errors.

*-K*, *--nodiscard*
	Taken: nothing is discarded.

*-V*, *--version*
	Print the version.

# ENVIRONMENT

*SOURCE_DATE_EPOCH*
	The time to give the top directory, for a reproducible filesystem.

# DIFFERENCES FROM LINUX

- One device: the RAID profiles are made by adding devices with
  *device*(8) and converting with *balance*(8).
- Data is single; *--mixed*, *-O*, *-R*, *--rootdir*, *--subvol*,
  *--shrink*, *--compress* and *--inode-flags* are not there. *./ob
  image* writes the Dukovo root's files with the kernel's Btrfs crate.
- Nothing is discarded, and the summary is shorter.

# EXIT STATUS

0 when the filesystem was made; 1 when it was not, and for a mistake in
the command line.

# SEE ALSO

*fsck-btrfs*(8), *mkfs*(8), *rdisk*(8), *blkid*(8), *subvolume*(8)
