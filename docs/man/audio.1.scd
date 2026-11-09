audio(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

audio - the sound outputs: list them, set the volume, test and play

# SYNOPSIS

*audio* [*list*]

*audio info* [_output_]

*audio volume* [_percent_ | *+*_n_ | *-*_n_]

*audio mute* [*on* | *off* | *toggle*]

*audio output* [_name_]

*audio test* [_output_]

*audio play* [*--output* _name_] _file_...

# DESCRIPTION

audio works the sound outputs of _/devices/audio_ (ADR 0320): one for each
sound card the kernel drives, such as *hda0* for a computer's built-in
sound (Intel HD Audio, ADR 0322) or *virtio0* for a virtual machine's
virtio sound card. _default_ leads to the one in use. It needs root's
access, as the outputs are root's.

*list*, or no command, lists the outputs, the one in use marked *\**,
with what each is, its volume and whether a program plays on it (or the
screen reader speaks):
```
* hda0       Realtek HD Audio      70%
  virtio0    virtio sound card     70%
```

*info* says more of one output, the default one unless named: its
volume on each channel, and whether the kernel scales the samples for it
(an output with no volume of its own, as a virtio card), the channels and
rates it plays, who plays on it, how much it has played, and how many
times it ran out of samples while a program played (an underrun, heard as
a gap).

*volume* prints the volume, or sets it: a percent, or *+*_n_ or *-*_n_
from the volume now. It is the volume of every output, kept in
*audio.toml*(5), and setting it unmutes. *mute* mutes or unmutes, or with
no argument does the other; the volume is kept for after. audiod(8) is
told by the file; audio also sets the outputs at once.

*output* prints the output in use, or makes _name_ the one, kept in
*audio.toml*(5) for after the next start.

*test* plays a test tone on an output, the default one unless named:
440 Hz on the left channel for 0.6 seconds, then 660 Hz on the right,
saying which as it does, so that one hears both speakers and which is
which. It ends once the tones have played.

*play* plays WAV files, one after another, on the default output or the
one *--output* names: PCM of 8, 16, 24 or 32 bits, or 32-bit floats, of
any rate and channels. audio converts the samples to the 16-bit ones the
kernel takes, maps the channels (mono to both sides, more to fewer) and
changes the rate to the nearest the output plays.

One program plays on an output at a time: while one plays, *test* and
*play* fail with *Device or resource busy*. The screen reader takes the
output from any of them.

# EXIT STATUS

0 when it did as asked; 1 for an output there is not, a file it cannot
read or play, a busy output, or a mistake in the command line.

# FILES

_/devices/audio_
	The outputs (*radix-files*(7)).

_/system/state/audio/audio.toml_
	The output, volume and mute (*audio.toml*(5)), which only root
	changes.

# SEE ALSO

*audio.toml*(5), *audiod*(8), *radix-files*(7), *systools*(1)
