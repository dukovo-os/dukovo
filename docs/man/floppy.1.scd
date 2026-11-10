floppy(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

floppy - list floppy drives, look for a disk in one, and format its disk

# SYNOPSIS

*floppy*

*floppy look* _drive_

*floppy format* [*-s* _size_] [*-n* _label_] [*--no-verify*] [*--no-filesystem*] _drive_

# DESCRIPTION

floppy works with the floppy drives on the PC's floppy disk controller
(ADR 0435) and USB floppy drives (ADR 0436), named *fd0*, *fd1* and on.
A _drive_ is its name or its path in _/devices_.

With no command, floppy lists each floppy drive, the sizes of disk it
formats (_/kernel/block/<drive>/formats_), what is in it, and where that
is mounted.

```
$ floppy
fd0: formats 1.44M and 720K; a 1.44M disk, mounted at /volumes/DRIVERS
```

# COMMANDS

*look* _drive_
	Has the kernel look for a disk in the drive, as *rdisk --reread*
	does (*radix-block*(2)'s REREAD_PARTITIONS), and says what it found.
	A floppy drive cannot say that a disk went in, so it is never
	polled: a disk put in is found when the drive is looked at, and
	*volumed*(8) then mounts it at its label. Looking needs no write
	right.

*format* _drive_
	Formats the disk in the drive, as Linux's fdformat and mkfs.fat do
	together: each of its 80 cylinders, both sides, a track at a time
	(*radix-block*(2)'s FORMAT_TRACK), which loses what the disk held.
	It then has the kernel look, reads the whole disk back, makes a FAT
	filesystem on it with the disk's own geometry, as DOS's format
	does, and mounts it at _/volumes_, at its label. A drive whose disk
	is mounted is refused: eject it first.

# OPTIONS

*-s*, *--size* _size_
	The size to format the disk as: *1.44M* for a high-density disk,
	*720K* for a double-density one (*1440K* and *720* are taken too).
	The drive's first size in _/kernel/block/<drive>/formats_ if not
	given.

*-n*, *--label* _label_
	The filesystem's label, up to 11 characters, as mkfs-fat's *-n*.

*--no-verify*
	Does not read the disk back once it is formatted.

*--no-filesystem*
	Formats the disk and reads it back, but makes no filesystem on it.

# EXIT STATUS

0 when the drives were listed, the drive looked at, or the disk
formatted, its filesystem made and mounted; 1 when the drive is not a
floppy drive, has no disk in it, its disk is write-protected or mounted,
a track could not be formatted or read back, the disk reads as another
size once formatted, or for a mistake in the command line.

# EXAMPLES

```
# floppy look fd0
fd0: a 1.44M disk
# floppy format -n BACKUP fd0
floppy: formatting the disk in fd0 as a 1.44M disk
floppy: reading the disk back
floppy: making a FAT filesystem on it
floppy: mounted the FAT filesystem of fd0 at /volumes/BACKUP
# floppy format -s 720K fd1
```

# SEE ALSO

*eject*(1), *rdisk*(8), *mkfs-fat*(8), *volumed*(8), *radix-block*(2),
*radix-sysfs*(5), *systools*(1)
