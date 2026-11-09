<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Sound

Each sound output the kernel drives is a file in `/devices/audio`:
`hda0` for a computer's built-in sound (Intel HD Audio, in nearly every
PC since 2004), `usb0` for a USB headset, speaker or DAC, and `virtio0`
for a virtual machine's virtio sound card. `/devices/audio/default`
leads to the one in use. audiod keeps the outputs as
`/system/state/audio/audio.toml` says: which one plays, how loud, and
whether it is muted. The full references are audio(1), audiod(8),
audio.toml(5) and ADRs 0320 to 0323.

## Listing and testing

```
# audio
* virtio0    virtio sound card     70%
# audio test
left: 440 Hz on virtio0
right: 660 Hz on virtio0
```

`audio test` plays a tone on the left speaker and then a higher one on
the right, so you hear both, and which is which. `audio info` says what
an output plays, how much it has played, and how often it ran out of
sound while a program played (an underrun, heard as a gap).

## Built-in sound

A computer's built-in sound plays through its speakers, its headphone
jack and its line out at once, as the computer's maker wired them, and
plugging in headphones turns the speakers off within a quarter of a
second; the kernel's log says each time what plays (`dmesg | grep
Audio:` as root):

```
Audio: hda0 is an HD Audio codec at 0000:00:1f.3 (Realtek HD Audio 10ec:0256, card 1028:0a6e): speaker 0x14 from 0x02, headphones 0x21 from 0x02; 44100 to 192000 frames a second, volume on the codec, polled
Audio: hda0 plays through the headphones
```

Its volume is the sound chip's own, in its steps. The microphone, line
in, and sound over HDMI or DisplayPort are not played or recorded yet:
a graphics card's own HD Audio, which only has HDMI, is listed by
`lspci` and left alone.

## USB headsets and speakers

A USB headset, speaker or DAC plays as soon as it is plugged in, and
becomes the default output a moment later, since audiod picks the one
plugged in last. The kernel's log says what it is:

```
Audio: usb0 is a USB headset at 1-3, USB Audio Class 1: 2 channels of 16 bits, 44100 to 48000 frames a second on the host's clock, volume on the device
```

Its volume is the device's own, in its steps. Unplugging it takes
`usb0` away, and `default` goes back to another output; plugging in the
same headset or another one makes it `usb0` again. A program that was
playing through it when it was unplugged is told so ("No such device or
address") and can start again on the default output. A headset's
microphone is not recorded yet.

## Volume and mute

```
# audio volume 50
50%
# audio volume +10
60%
# audio mute
muted
```

The volume keys on the keyboard do the same, 5 at a time, and the mute
key mutes and unmutes. The volume is every output's, and stays after a
restart. An output with no volume of its own, such as a virtio card, has
its sound scaled by the kernel, by the cube of the volume, so that each
step of a key sounds as loud a change as the last: 50% is an eighth of
the full amplitude.

## Choosing the output

`audio output usb0` plays through `usb0` from then on, and after a
restart. Without a choice, audiod picks the output plugged in last (a
headset), or else the first.

## Playing a file

`audio play song.wav` plays WAV files of any rate and channel count,
converted to what the output plays. One program plays on an output at a
time: while one plays, another is refused with "Device or resource busy"
until it ends. The screen reader is the exception: it takes the output
from any program, so that it is always heard.

## For programs

A program plays by opening an output, setting the rate and channels with
OSS's requests (SNDCTL_DSP_SPEED and SNDCTL_DSP_CHANNELS, which answer
with the nearest the output plays), and writing signed 16-bit samples, as
on FreeBSD. The kernel converts nothing: the `audio` crate in Dukovo's
source converts and resamples for programs that want it.
