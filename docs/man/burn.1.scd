burn(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

burn - write a CD, DVD or Blu-ray disc

# SYNOPSIS

*burn* [*--drive* _name_] [*--blank*] [*--test*] [*--eject*] [*--label* _label_] [*--udf*] _image_|_folder_

*burn --audio* [*--drive* _name_] [*--blank*] [*--test*] [*--eject*] _file_...

*burn blank* [*--drive* _name_] [*--full*]

*burn info* [*--drive* _name_]

# DESCRIPTION

burn writes a disc in an optical drive that records (ADR 0449): a CD-R
or CD-RW, a DVD-R, DVD-RW, DVD+R, DVD+R DL, DVD+RW or DVD-RAM, or a BD-R
or BD-RE. It works the drive *--drive* names, such as *cd0*, or else the
first drive that holds a disc of those kinds, as
_/kernel/block/<drive>/disc_ says (*radix-sysfs*(5)).

burn writes what it is given:

- an _image_, such as an ISO, as it is;
- a _folder_, made into an image as it is written (as *mkiso*(1) makes
  one, with Rock Ridge and Joliet, and UDF with *--udf*), labelled
  _label_ or else with the folder's name;
- with *--audio*, a CD of music: each _file_, WAV or FLAC, is a track, in
  the order given, converted to 44,100 frames a second of 16-bit stereo,
  padded with silence to a whole sector and to four seconds at least.

Each kind of disc is written its own way: a CD of data a track at a time,
a CD of music all at once, with two seconds of silence before its first
track; a DVD-R or DVD-RW incrementally; a DVD+R, DVD+R DL or BD-R in
sequence; a DVD+RW, DVD-RAM or BD-RE in place, formatted first if it is
not. A disc of data is closed once written, so that any drive reads it:
nothing more is added to it. A rewritable disc that holds something is
written only with *--blank*, which erases it first; a DVD+RW, DVD-RAM or
BD-RE is written over without asking, as a disk is. Buffer underrun
protection is turned on wherever the drive has it.

While it writes, burn says how far it got every tenth of the way on
standard error, and the kernel holds the drive: it does not look at the
disc, its tray stays locked, and *eject*(1) is refused. Once written, the
kernel has looked at the disc afresh by the time burn ends, and
*volumed*(8) mounts a disc of data at its label as it mounts any disc
put in; with *--eject*, the disc comes out first. The last line, on standard
output, says what was burned:
```
cd1: burned 825 sectors on the CD-R
```

*burn blank* erases a CD-RW or DVD-RW: its table of contents, quickly, or
every sector with *--full*. It formats a DVD+RW, DVD-RAM or BD-RE afresh.

*burn info* says what disc is in the drive, whether it is blank, written
or closed, and the room it has:
```
cd1: CD-R, blank, 737MB free
cd1: DVD+RW, formatted, 4.8GB, erasable
```

# OPTIONS

*-d*, *--drive* _name_
	The drive, by its name in _/devices_.

*--blank*
	Erase a rewritable disc that holds something before writing it.

*--test*
	Have the drive go through the write with its laser off: nothing is
	written, and the disc stays blank. CDs, DVD-Rs and DVD-RWs have
	test writes; other discs do not.

*--eject*
	Eject the disc once it is written.

*--label* _label_
	For a _folder_, the disc's label: in upper case and at most 32
	characters in ISO 9660's own names, as given in Joliet's (cut to 16)
	and UDF's (cut to 30).

*--udf*
	For a _folder_, a UDF tree beside ISO 9660's, for players that read
	only UDF, as DVD and Blu-ray players do.

*--audio*
	Burn a CD of music from WAV and FLAC files.

*--full*
	For *burn blank*, erase every sector.

*-q*, *--quiet*
	Do not say how far the write got.

# EXIT STATUS

0 when it did as asked; 1 when no drive holds a disc burn writes, the
disc is of a kind burn cannot write as asked (music on a DVD, or a test
write of a DVD+R), it holds something and is not erasable or *--blank* was
not given, what is burned does not fit, a file cannot be read, the drive
fails a command, or the command line has a mistake. The drive's failure
is said with its sense key, ASC and ASCQ.

# NOTES

A drive without buffer underrun protection that runs out of data while
it writes spoils the disc. Drives made since the early 2000s have it.

CD-Text is not written on a CD of music. Ogg Vorbis and Opus files are
not read.

# FILES

_/devices/cd0_, _/devices/cd1_, ...
	The optical drives, written with *radix-block*(2)'s MMC_COMMAND,
	which needs the write right to the drive.

_/kernel/block/<drive>/disc_
	What the disc in a drive is: *cd-r blank*, *dvd+rw data* and the
	like.

# SEE ALSO

*audiocd*(1), *eject*(1), *mkiso*(1), *radix-block*(2), *radix-sysfs*(5),
*systools*(1), *volumed*(8)
