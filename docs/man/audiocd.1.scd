audiocd(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

audiocd - list, play and rip the music CD in an optical drive

# SYNOPSIS

*audiocd* [*list*] [*--drive* _name_]

*audiocd play* [*--drive* _name_] [*--output* _name_] [_track_...]

*audiocd rip* [*--drive* _name_] [*--wav*] [*--to* _directory_] [_track_...]

# DESCRIPTION

A music CD's tracks are not files, and the kernel mounts nothing of it
(ADR 0431): audiocd reads it through the drive (ADR 0447). It works the
drive *--drive* names, such as *cd0*, or else the first drive that holds a
music CD, or failing that a CD of data and audio both, as
_/kernel/block/<drive>/disc_ says (*radix-sysfs*(5)). A disc with data
tracks too, such as an Enhanced CD, is read for its tracks of audio.

*list*, or no command, lists the tracks: each one's number, how long it
plays and its title, with the disc's title and performer first, as the
disc's CD-Text gives them; a track performed by other than the disc's
performer is said to be by them. A disc without CD-Text has its tracks
named *Track 1* and on.
```
cd1: Lab Sessions, by Dukovo Lab
 1   0:05  First Light
 2   0:02  Static
2 tracks, 0:07
```

*play* plays the tracks named, or every track, through the default sound
output or the one *--output* names, as *audio*(1) plays a file: a
second at a time, at the rate the output plays nearest to the disc's
44,100 frames a second.

*rip* copies the tracks named, or every track, to files in _directory_
(the working directory unless *--to* names one), one a track, named by
the track's number and title: _01 First Light.flac_. A file is FLAC,
lossless, tagged with the track's title (*TITLE*), its performer or the
disc's (*ARTIST*), the disc's title and performer (*ALBUM*,
*ALBUMARTIST*), its number and the count of tracks (*TRACKNUMBER*,
*TRACKTOTAL*); or with *--wav*, a WAV file of the samples as they are.
Either holds every sample of the track bit for bit. A file is written as
_<name>.part_ and takes its name once it is whole, so a rip cut short
leaves no file that looks whole.

A track is named by its number, or a range of them, such as *2-5*. A
track holds what the disc's table of contents gives it: from its start
up to the next track's, the silence before the next one (its pregap)
with it. A data track is not played or ripped.

A sector that does not read is read again twice before audiocd gives up.
It reads as the drive gives the samples, as drives made since the late
1990s read audio where it is: it does not read each sector twice to
correct the drive's jitter, as cdparanoia does.

# OPTIONS

*-d*, *--drive* _name_
	The drive, by its name in _/devices_.

*-o*, *--output* _name_
	For *play*, the sound output, by its name in _/devices/audio_.

*--to* _directory_
	For *rip*, where the files go.

*--wav*
	For *rip*, WAV files, not FLAC.

# EXIT STATUS

0 when it did as asked; 1 when no drive holds a music CD, the drive named
has no disc or is not an optical drive, a track is not on the disc or is
data, a sector does not read, a file cannot be written, or the command
line has a mistake.

# FILES

_/devices/cd0_, _/devices/cd1_, ...
	The optical drives, read with *radix-block*(2)'s READ_TOC and
	READ_AUDIO.

_/kernel/block/<drive>/disc_
	What the disc in a drive is: *cd-rom audio* for a music CD, *cd-rom
	mixed* for a CD of data and audio.

# SEE ALSO

*audio*(1), *eject*(1), *radix-block*(2), *radix-sysfs*(5), *systools*(1)
