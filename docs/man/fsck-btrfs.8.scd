fsck-btrfs(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

fsck-btrfs, btrfsck - check a Btrfs filesystem without changing it

# SYNOPSIS

*fsck-btrfs* [*--check-data-csum*] [*--force*] _device_

# DESCRIPTION

fsck-btrfs checks the Btrfs filesystem on _device_, a disk or partition
in _/devices_ or a file, as btrfs-progs' *btrfs check --readonly* does,
by whose old name, *btrfsck*, it runs too (ADR 0305). The kernel's own
Btrfs crate reads it. It reads every tree block, every copy of each, and
finds:

- superblock copies, tree blocks and copies of them that are damaged,
  or that belong to another filesystem;
- extents whose references, back references or levels are not what the
  trees that use them say, and extents no tree uses;
- block groups, devices and the superblock whose bytes used are wrong,
  and chunks without their device extents;
- free space the free space tree gives that an extent takes, or space
  it leaves out that nothing takes;
- in each subvolume, names without their inodes or inodes without their
  names, wrong link counts, directory sizes and byte counts, and file
  extents that overlap;
- data without checksums, and with *--check-data-csum*, data that does
  not match its checksums, in each copy;
- subvolumes whose references do not match their names.

It prints each phase as it starts, as *btrfs check* does, then what is
wrong and a summary:

```
Opening filesystem to check...
Checking filesystem on /devices/vda2
UUID: 87848637-a4e7-46be-8bf2-20dd6b6c242a
[1/7] checking root items
[2/7] checking extents
[3/7] checking free space tree
[4/7] checking fs roots
[5/7] checking csums
[6/7] checking root refs
[7/7] checking quota groups skipped (not checked by Dukovo)
found 147456 bytes used, no error found
total tree blocks: 9
file data extents: 0
1 subvolumes, 2 directories, 0 other files
```

fsck-btrfs never writes. A filesystem with a second copy of what is
damaged is mended by *scrub*(8), with the filesystem mounted.

# OPTIONS

*--check-data-csum*
	Read every copy of the data too, and check it against its checksums.

*--force*
	Check a filesystem in use. What is being written as it is read may
	be found wrong.

*--readonly*, *-p*, *--progress*
	Taken: fsck-btrfs never writes, and always says each phase.

# DIFFERENCES FROM LINUX

- The exit status is *fsck*(8)'s, as e2fsck gives, where *btrfs check*
  gives 1 for a filesystem with problems.
- It does not mend: *--repair*, *--init-csum-tree*, *--init-extent-tree*
  and *--clear-space-cache* are not there, nor *-s*, *-b*, *-r* and
  *--chunk-root*, which check from other trees than the newest.
- Quota groups are not checked.

# EXIT STATUS

0
	Nothing is wrong.

4
	Something is wrong.

8
	The filesystem could not be checked: it cannot be read, holds no
	Btrfs, or is in use without *--force*.

16
	A mistake in the command line, or an option that would change the
	filesystem.

# SEE ALSO

*mkfs-btrfs*(8), *scrub*(8), *mkfs*(8)
