resize-ntfs(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

resize-ntfs, ntfsresize - check an NTFS volume, and shrink it

# SYNOPSIS

*resize-ntfs* [*--info* | *--info-mb-only* | *--check*] [*--force*] _device_

*resize-ntfs* *--size* _size_ [*--no-action*] [*--no-windows-check*] [*--no-progress-bar*] _device_

# DESCRIPTION

resize-ntfs shrinks the NTFS volume on _device_, a disk or partition in
_/devices_ or a file, with the options of ntfs-3g's *ntfsresize*, by
whose name it runs too (ADR 0312). The kernel's own NTFS crate reads and
writes it. The installer shrinks Windows' volume the same way when it
installs Dukovo next to Windows.

A shrink keeps every file. It checks the volume first, then copies what
files hold past the new end into free space below it, each piece of a
file moved whole, and rewrites each file's record to say where. Windows'
own files move too: the master file table, its mirror, the journal and
the bitmap of clusters. Then the boot sector says the new size, and its
copy goes in the new last sector. It checks the shrunk volume again.

It shrinks the volume alone, as *ntfsresize* does, and says how many
sectors the partition should then have: shrink the partition to that
with *rdisk*(8), keeping its first sector. Until then the volume leaves
the rest of the partition unused, and Windows reads it as it is.

It refuses, changing nothing:

- a volume BitLocker encrypts: turn BitLocker off in Windows, and let
  it finish decrypting;
- a volume Windows hibernated with, or shut down with Fast Startup on;
  shut Windows down fully (hold Shift as you choose *Shut down*), or
  turn Fast Startup off in the Control Panel's power options;
- a volume marked to be checked, or with something left in its journal;
  start Windows, and let it check the volume;
- a volume the check finds problems on: run *chkdsk /f* in Windows;
- a size under the smallest it shrinks to, which *--info* says;
- a file in so many pieces that, moved, its list of pieces would no
  longer fit in its record: let Windows optimize (defragment) the drive.

A power cut during a shrink leaves every file where its record says, and
the volume marked to be checked: Windows' *chkdsk* gives back clusters
copied to but not yet used.

After a shrink the volume stays marked to be checked, as after
*ntfsresize*: Windows checks it at its next start. Until then, Dukovo
mounts it to read only, and does not shrink it again.

# OPTIONS

*-i*, *--info*
	Say the volume's size, the space its files use, and the smallest it
	shrinks to:

```
Device name        : /devices/nvme0n1p3
Cluster size       : 4096 bytes
Current volume size: 255466373120 bytes (255467 MB)
Current device size: 255466373120 bytes (255467 MB)
Checking filesystem consistency ...
Space in use       : 61022 MB (23.9%)
You might resize at 63576211456 bytes or 63577 MB (freeing 191890 MB).
```

*-m*, *--info-mb-only*
	Say only the smallest size, in MB, as *Minsize (in MB): N*.

*-c*, *--check*
	Check the volume, and say what is wrong: clusters two files hold,
	clusters a file holds that the bitmap calls free, and records that
	cannot be read. Clusters marked in use that no file holds are lost,
	which *chkdsk* gives back; they are counted but are not wrong.

*-s*, *--size* _size_
	Shrink the volume to _size_: the size of the partition it will
	fill, the copy of its boot sector in its last sector. _size_ is in
	bytes, or with *k*, *M*, *G* or *T*, powers of 1000, as *ntfsresize*
	reads it, or with *Ki*, *Mi*, *Gi* or *Ti* (*KiB*, *MiB*...), powers of
	1024.

*-n*, *--no-action*
	Work out what a shrink to *--size* would move, and write nothing.

*--no-windows-check*
	Leave the shrunk volume unmarked, so that Windows does not check it
	at its next start. *--check* checks it at once.

*-P*, *--no-progress-bar*
	Do not show how far the copying is.

*-f*, *--force*
	With *--info* or *--check*, read a volume in use, which may be
	written as it is read. It does not shrink a volume in use, or one
	Windows must check first.

*-b*, *--bad-sectors*, *-v*, *--verbose*
	Taken: bad clusters are always left out of a shrunk volume, and
	what it says is all it says.

# DIFFERENCES FROM LINUX

- It only shrinks: *--expand* is refused. A partition grown under a
  volume leaves the space after it unused.
- *--force* does not shrink a volume marked to be checked, or one
  Windows hibernated with.
- Each piece of a file past the new end is moved whole, so a shrink
  leaves files in fewer pieces, where *ntfsresize* moves them as they
  are.
- *--no-windows-check* is Dukovo's own.

# EXIT STATUS

0
	The volume was shrunk, or is as *--info* or *--check* says.

1
	It was not shrunk or could not be checked, the check found
	problems, or a mistake in the command line.

# SEE ALSO

*rdisk*(8), *blkid*(8), *mount*(8), *mkfs*(8)
