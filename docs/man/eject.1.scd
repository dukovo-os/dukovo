eject(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

eject - unmount a volume, and eject a drive's medium, so it can be taken out

# SYNOPSIS

*eject* [*-v*] _volume_

# DESCRIPTION

eject unmounts a volume at _/volumes_, after the filesystem has written
out everything it holds, and removes its directory, so that its disk can
be pulled out safely (ADR 0163). The _volume_ is its label or name
under _/volumes_ (as *volumed*(8) or Spark mounted it), its path, or a
disk or partition by its name or its path in _/devices_; a disk's name
unmounts each of its partitions' volumes.

```
$ eject DUKOVOUSB
```

Once a volume is ejected, *volumed*(8) leaves its disk alone until it is
pulled out and plugged in again.

A drive whose medium comes and goes, such as a card reader's slot or a
stick that says its medium is removable (_/kernel/block/<drive>/removable_
is 1, ADR 0430), is ejected whole: eject unmounts every volume on its
medium, as util-linux's eject unmounts every partition of the disk, and
then has the drive let go of the medium and eject it (*radix-block*(2)'s
EJECT): a tray opens, and a card can be taken out. The drive stays,
empty, and *volumed*(8) mounts the next medium put in. A drive's name
ejects its medium with nothing of it mounted too.

```
$ eject SDCARD
$ eject sdc
```

A CD, DVD or Blu-ray drive is such a drive (ADR 0431). While its disc
is mounted, the drive keeps it in; eject unmounts the disc and opens the
tray, as pressing the drive's eject button has *volumed*(8) do (ADR
0432). Its name opens an empty drive's tray too. A drive *burn*(1) is
writing keeps its disc until the disc is written (ADR 0449).

```
$ eject HOLIDAY_2026
$ eject cd0
```

A floppy drive is such a drive too (ADR 0435), but has no motor to push
the disk out: eject unmounts the disk and has the drive let it go. The
drive is then empty, and stays empty, the disk still in it, until the
disk is taken out. A floppy drive is never polled for a disk: one put in
is found when the drive is looked at, as *floppy look* asks, or
*rdisk --reread*.

```
$ eject fd0
$ rdisk --reread /devices/fd0
```

A slot of an SD card reader on PCI, _mmcblk0_ and on, is such a drive
too (ADR 0437), and cannot push its card out either: eject unmounts the
card, and the reader lets it go and turns its power off. The slot is
then empty, the card still in it, until the card is taken out; one put
in is found within two seconds.

```
$ eject CAMERA
$ eject mmcblk0
```

# OPTIONS

*-v*, *--verbose*
	Say what is unmounted, and which drive's medium is ejected.

# EXIT STATUS

0 when every volume named was unmounted, and every drive's medium
ejected; 1 when none is mounted at _/volumes_ and nothing named is a
drive, when one could not be unmounted (a file on it is open, or what it
holds could not be written), when a drive could not eject (its medium or
a partition of it is in use outside _/volumes_, as swap or a volume
unlocked, its disc is being burned, or the drive did not answer), or for
a mistake in the command
line.

# SEE ALSO

*volumed*(8), *remount*(8), *findmnt*(8), *lsblk*(8), *floppy*(1), *burn*(1),
*radix-block*(2), *systools*(1)
