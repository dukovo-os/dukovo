<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0323. USB Audio

- Status: Accepted
- Date: 2026-11-09

## Context

USB headsets, speakers and small DACs are how most people add sound to a
machine, and how a blind user gets a screen reader's voice from a PC
whose own jacks are hard to reach. Nearly all of them follow the USB
Device Class Definition for Audio Devices, release 1.0 (UAC1, full
speed, most headsets) or 2.0 (UAC2, high speed, most DACs of the last
ten years), so one driver plays them all. QEMU offers a UAC1 speaker
(`usb-audio`), so a boot test can play through one, unplug it and plug
it in again.

An audio function is an AudioControl interface, whose class-specific
descriptors describe a graph of terminals and units (and, in UAC2,
clocks), and AudioStreaming interfaces whose alternate settings each
carry one format on an isochronous endpoint. Isochronous transfers are
new to the kernel: the xHCI driver moved only control, bulk and
interrupt transfers until now. The specifications are the USB
Implementers Forum's. Linux's driver is GPL and is not read; FreeBSD's
uaudio(4) (BSD-2-Clause) is, for facts the specifications leave out.

## Decision

- **The xHCI driver queues isochronous TDs** (xHCI 4.11.2.3): an Isoch
  TRB that starts as soon as possible after the TD before, its burst
  counts worked out from the endpoint, and up to two Normal TRBs after
  it for a packet whose bytes are in pieces. A batch is queued, then
  the doorbell rung once.
- **`radix_usb::audio` reads the descriptors**, of UAC1 and UAC2 alike,
  with host tests: the streaming interface whose OUT endpoint feeds a USB
  streaming terminal that a path of units leads from to a speaker,
  headphones, a headset or a line out, among the interfaces the function
  lists; the setting with two channels (or one) of the fewest bytes a
  sample; the feature unit nearest the output; and the UAC2 clock behind
  the terminal, through a clock selector if there is one. A recording's
  path, a microphone's interface, and formats other than PCM of 16 to
  32 bits are passed over.
- **The controller's thread drives the device, the output's worker the
  sound.** The worker writes each period into a ring of samples it
  shares with the thread (widened to 24 or 32 bits if the setting is),
  and the thread queues a packet of the ring each service interval,
  about 16 ms ahead, asking for an event every 2 ms or so and on the
  last packet queued. The events say what has played, and the worker is
  woken to hand the periods back to the sound core. Setting a format up
  is asked of the thread, which answers: the streaming interface goes
  to the setting with no endpoint (dropping whatever was queued), then
  to the playing one, and the rate is set on the endpoint (UAC1) or the
  clock (UAC2).
- **Packets are sized by a pacer**: the rate's frames each interval, the
  fraction carried exactly, so 44.1 kHz at full speed is 44 frames a
  millisecond with a 45 every tenth. An asynchronous endpoint's device
  says through its feedback endpoint how fast it really plays, which
  the pacer follows; devices send either speed's format at either
  speed, so the first report's format is found by its size against the
  nominal rate, as uaudio(4) does. A packet that the samples written do
  not fill is filled with silence when nothing else is queued: devices
  take whole packets only.
- **The volume is the feature unit's**, on the master channel or on each
  channel, at the same attenuation in decibels that the kernel's own
  scaling and HD Audio's amplifiers use (radix-audio's `attenuation`),
  rounded to the unit's steps and never above 0 dB; 0% mutes where the
  unit can. A device with no volume of its own is scaled by the kernel.
- **A USB output comes and goes with its device.** The sound core keeps
  every output's index and name for good, so unplugging takes the file
  out of `/devices/audio`, drops what was queued, fails the player's
  next write with ENXIO, and points `default` elsewhere; the worker lets
  go of the driver and waits. The next USB output plugged in takes the
  place, so `usb0` is the headset plugged in, whatever came before. A
  file opened on the device before is not the new one's.
- **Only rates a packet has room for** are offered: a UAC1 setting's
  list or range, a UAC2 clock's ranges (48 kHz if it will not say),
  each kept only if a service interval's packet holds its frames.

## Consequences

- `usb0` and on appear in `/devices/audio`, named for their output
  terminal (`USB headset`, `USB speaker`), and play as any output does
  (ADR 0320). audiod makes the one plugged in last the default, as it
  did already.
- The `audio-usb` boot test plays tones through QEMU's speaker and checks
  what `wavcapture` recorded at two volumes, then unplugs the speaker
  while it plays and plugs it in again. The lab's `sound/usb` test
  checks the real headsets and speakers plugged into the lab's machines,
  and `sound/tone` now plays through the machine's own jack, whichever
  output is the default.
- Microphones and the recording half of headsets, implicit feedback
  (a recording's rate standing in for a feedback endpoint), more than two
  channels, UAC3, and devices that need a vendor's own requests are left
  for later.
