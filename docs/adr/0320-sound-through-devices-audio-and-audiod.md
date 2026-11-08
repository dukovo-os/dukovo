<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0320. Sound through /devices/audio, OSS's requests, and audiod

- Status: Accepted
- Date: 2026-11-08

## Context

Phase 14b brings sound: the screen reader of PR 7 must speak at the
installer's first screen, and the release needs speakers, headphones and
USB headsets to work on the machines of the lab. Programs need a way to
play samples, the system a way to choose the output and its volume, and
the screen reader a way to be heard whatever else plays.

Linux's way is ALSA's devices and library, with a sound server
(PulseAudio or PipeWire) on top that mixes and resamples. Both are large,
and ALSA's library and its plug-ins are LGPL code written against
Linux's ioctls. FreeBSD's and Linux's older interface, OSS, is a device
file per output that a program opens, sets up with a few requests and
writes samples to; FreeBSD's _sys/soundcard.h_ documents it under the
BSD-2-Clause licence, and the programs Dukovo will port first (eSpeak NG,
SDL's and many games' OSS back ends) speak it.

## Decision

- **Each output is a file of _/devices/audio_**: `virtio0` and on for a
  virtio sound card (ADR 0321); `hda0` and `usb0` follow in PRs 6b and 6c.
  `default` is a link to the output in use, and `speech` plays on it for
  the screen reader. The files are root's, mode 0600: a service is given
  the outputs it needs by its service file.
- **The requests are OSS's, with the numbers Linux's _<sys/soundcard.h>_
  gives them**, so a program written for OSS plays unchanged through
  mlibc: SPEED, CHANNELS, SETFMT, SETFRAGMENT, GETBLKSIZE, GETOSPACE,
  GETODELAY, SYNC, POST, RESET/HALT, NONBLOCK, GETCAPS, GETFMTS and
  GET/SETPLAYVOL. Two are Dukovo's own: GET_INFO, what an output is, and
  MAKE_DEFAULT. They are the ABI's `audio` module (radix-audio(2)).
- **The kernel converts nothing.** Samples are signed 16-bit
  little-endian, at a rate and channel count the output plays: SPEED and
  CHANNELS take the nearest the device has and write it back, as OSS
  does, and the program converts. The `audio` crate in user space does
  that (WAV files, sample formats, channel maps and a resampler), for
  audio(1) and for any program that wants it.
- **One player at a time.** The first write or format request makes a
  description the output's player; others are refused with busy until it
  closes, and what it wrote plays out before the next starts. There is
  no mixing in the kernel: a sound server can come later, in user space,
  as the one player.
- **Speech is never kept from being heard.** `speech`'s first write takes
  the output from whoever plays, dropping what they queued; their next
  write fails with busy. Only a service given `speech` (the screen
  reader's) can do it, since the file is root's.
- **The kernel scales the samples for outputs with no volume of their
  own**, as it copies them to the device, by the cube of the volume's
  fraction, so that the steps of a volume key sound even.
- **A worker thread per output drives the device**: it sets the device up
  for the player's format, moves whole fragments (about 10 ms each, 8 of
  them, unless the program asks otherwise) to it, starts it once two wait
  or the program asks for what it wrote to play, and counts underruns. The
  device's interrupt only wakes the worker.
- **audiod(8) keeps the settings**, _/system/state/audio/audio.toml_
  (audio.toml(5)): it gives every output the volume (none while muted),
  points `default` at the output named or else the one plugged in last,
  looks again every two seconds, and turns the keyboard's volume and mute
  keys into changes of the file. audio(1) lists the outputs, changes the
  settings, plays a test tone and plays WAV files.
- **The file is state, not one of _/system/settings_**, as ALSA's saved
  mixer is in _/var/lib/alsa_ on Linux: every service reads the settings
  and none may change them (ADR 0217), and audiod must save what the
  volume keys do. Only root changes it.

## Consequences

- A program plays with open, three requests and write, as on FreeBSD, and
  needs no library; eSpeak NG's output is piped to `speech` in PR 7.
- Two programs cannot play at once. That is enough for the installer and
  a console system, and the desktop's sound server (Phase 16) will be the
  one player that mixes the rest.
- Linux's ALSA programs do not play until a compatibility layer comes;
  OSS programs do.
- The `audio-virtio` boot test records what the card plays, through
  QEMU's wavcapture, and checks each tone's channel, length and loudness
  on the host; the lab checks the real outputs through a cable to the
  Mint PC (PR 6b).
