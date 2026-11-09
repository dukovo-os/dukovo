volumed(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

volumed - mount the volumes plugged in after boot by their labels

# SYNOPSIS

*volumed*

*volumed --list*

# DESCRIPTION

volumed mounts each disk or partition plugged in after boot at
_/volumes/_*label*, its filesystem's label, as udisks does on Linux for a
desktop's removable media (ADR 0163). Spark runs it as the *volumed*
service. A volume whose filesystem has no label is mounted at
_/volumes/_*name*, its name in _/devices_, as Spark mounts the disks
there at boot; one whose label another volume has is mounted at
_/volumes/_*label*-*name*. A label's slashes become underscores.

It mounts FAT, exFAT, NTFS, Btrfs, ext4, UDF and ISO 9660 as Spark does
at boot (*spark*(8)): FAT, exFAT and NTFS are tried first, UDF and ISO
9660 (discs, and the images of them on USB sticks) last, and a
filesystem the
disk or Radix cannot write (such as an NTFS that Windows hibernated
with) is mounted read-only. A disk with partitions has each of them
mounted, and an EFI system partition is left alone, as Windows, macOS
and udisks leave it. An install stick's DUKOVO_LIVE partition, with the
signed release on it, is always mounted read-only (ADR 0308). A volume BitLocker locked is left for *unlock*(8),
and the volume it unlocks, _/devices/_*name*-unlocked, is mounted as
one plugged in (ADR 0240).

A partition that appears on a disk that was there already was made by
partitioning it (*rdisk*(8)), not plugged in: volumed leaves it
unmounted, for its maker to put a filesystem on (ADR 0301):

```
volumed: vda3 is new on vda, which was partitioned again; left unmounted
```

When a disk is pulled out, volumed unmounts what is mounted at
_/volumes_ from it and its partitions, including what Spark mounted
there at boot, and removes the directories. Unmount a volume with
*eject*(1) before pulling its disk out, so that what it holds is
written first.

A drive whose medium comes and goes, such as a card reader's slot
(_/kernel/block/_*drive*_/removable_ is *1*, ADR 0430), stays when its
medium comes out; Spark leaves it alone at boot when it has none. Its
_media_changes_ count moves each time a medium goes in or comes out, and
when it has, volumed takes the drive's old partitions as gone and its
new ones as plugged in, though a partition of the new medium may have an
old one's name: a card put in is mounted as a stick plugged in, never
taken as partitioned again, and one taken out has its volumes unmounted.
volumed reads the counts before and after it lists _/kernel/block_, and
lists again if they moved between, so that a medium swapped while it
looked is not missed.

```
volumed: a medium went in sdc
volumed: mounted the FAT filesystem of sdc1 at /volumes/SDCARD
volumed: sdc1 is gone; unmounted /volumes/SDCARD
volumed: sdc has no medium in it now
```

A CD, DVD or Blu-ray drive (ADR 0431) is such a drive, named *cd0* and
on, and volumed says what disc went in it from its _disc_ file. A disc
of data has no partitions: the drive itself is mounted, read-only, at
the disc's label. A music CD or blank disc is not read, and only said:

```
volumed: a disc (dvd-rom) went in cd0
volumed: mounted the UDF filesystem of cd0 at /volumes/HOLIDAY_2026, read-only
volumed: cd0 has no disc in it now
volumed: a music CD went in cd0; its tracks are not files, and it is not mounted
volumed: a blank disc (dvd+r) went in cd0; nothing to mount
volumed: a disc went in cd0, but it cannot be read
```

volumed takes the disks there are when it starts as done: Spark has
mounted them. It looks at _/kernel/block_ (*radix-sysfs*(5)) every
second for disks and partitions that come or go, and writes a line of
each change, which Spark puts in the kernel log:

```
volumed: watching for volumes; 5 disks and partitions now
volumed: sdb is gone; unmounted /volumes/sdb
volumed: mounted the FAT filesystem of sdb at /volumes/DUKOVOUSB
volumed: sdc is a locked BitLocker volume; `unlock sdc` unlocks it
```

volumed also keeps the links in _/devices/disk_ that name each disk and
partition by its filesystem's UUID and label, and its GPT entry's GUID
and name (ADR 0303; *radix-files*(7)): at its start, and whenever disks
and partitions come or go.

Its service file gives it _/devices_, _/system/live/disk_, where those
links are, and _/volumes_, and nothing else of the tree
(*service.toml*(5)). Spark hands it _/volumes_ as descriptor 3
as well, as *files.mount* says and *MOUNT_PLACES* names, since nothing
is mounted through a service's view; volumed mounts and unmounts through
it, and through the root when *MOUNT_PLACES* names nothing, as for
*eject*(1).

Until Dukovo has users, every program reaches _/volumes_ as it reaches
the rest of the root; once it has them, the user logged in at the
console is given each volume volumed mounts.

# OPTIONS

*--list*
	Print each volume mounted at _/volumes_, with its device and
	filesystem, and exit.

# EXIT STATUS

*volumed --list* exits 0, or 1 if the mounts cannot be read. As a
service, volumed does not exit.

# SEE ALSO

*eject*(1), *remount*(8), *unlock*(8), *spark*(8), *findmnt*(8), *blkid*(8),
*rdisk*(8), *radix-files*(7), *systools*(1)
