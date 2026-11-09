<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0322. Intel HD Audio

- Status: Accepted
- Date: 2026-11-09

## Context

Nearly every PC since 2004 plays its built-in sound through an Intel
High Definition Audio controller (PCI class 04/03) in the chipset, and a
codec on the controller's link, most often Realtek's, wired to the
speakers, the headphone jack and the line out. The machines the lab
tests on (an OptiPlex, a Dell laptop) are such PCs. QEMU offers the same
controller twice (`intel-hda`, the ICH6's, and `ich9-intel-hda`) with
its own codecs, so the boot tests can drive it as they drive the virtio
card (ADR 0321).

The specification (revision 1.0a) lays out the controller's registers,
its command rings and its streams, and how a codec describes itself: a
tree of widgets (converters, mixers, selectors, pins) found by asking
each its parameters. What it leaves to the firmware is which pins are
connected and to what (each pin's configuration default), and what a
few codecs need beyond it. Linux's driver is GPL and is not read.
FreeBSD's hdac(4) and hdaa(4) (BSD-2-Clause) are, for facts only: the
snoop settings of other makers' controllers and the codecs that need a
coefficient set.

## Decision

- **`radix-hda` drives the controller and the codec**, with no `unsafe`
  code, against a simulated controller and codecs in its unit tests,
  as the virtio drivers are: QEMU's output, a laptop's Realtek codec
  with speakers, headphones and a line out, and a display's codec with
  HDMI pins alone.
- **The PCI function is readied first**: an Intel controller's DMA moves
  to traffic class 0, an AMD, ATI or NVIDIA controller is told to snoop,
  and none may mark a request No Snoop, since the kernel's DMA memory is
  cached.
- **Commands go through the CORB and RIRB rings**, one at a time, and a
  codec's answer is waited for in steps of 10 microseconds for at most
  100 ms; three missed answers in a row leave the output failed.
- **The first codec with an analog output plays**, through one stream
  and the converter the most of its output pins reach. Its pins are the
  ones the firmware says are connected (speakers, headphones, line
  outs), each routed by the shortest way through mixers and selectors to
  a converter; HDMI and DisplayPort pins are left out. A controller with
  no codec of that kind, a display's, is logged and left alone, for a
  later display audio driver.
- **The volume is the codec's own amplifiers'** when every way to a pin
  has one, set in steps as near as the amplifier allows to the kernel's
  curve; otherwise the kernel scales the samples.
- **Headphones take the sound from the speakers**: the pins that sense
  a plug are read every quarter of a second, and a pin is turned on or
  off to match, which the kernel logs. The codec's unsolicited
  responses are not used.
- **The controller raises no interrupts.** The stream plays a ring of at
  least half a second, laid out in slots of the kernel's period size;
  the output's worker reads the engine's position at each of its looks,
  every 20 ms while it plays, and frees the periods it has passed. The
  slots after the last period written are kept silent, so a late period
  plays as silence rather than old sound, and a stream that does not
  move for three seconds leaves the output failed. QEMU's controller
  also stops moving a stream while its completion interrupt status is
  set, which a polled driver never needs to clear.
- **The output plays signed 16-bit samples**, in two channels (one if a
  converter is mono), at each rate every chosen converter takes.
- **Codec quirks come from hdaa(4)'s facts**, in our own code: the
  ALC255 and ALC256 have coefficient 0x46's bits 13 and 12 set, as Dell
  laptops' ALC3246 needs.

## Consequences

- `hda0` and on appear in `/devices/audio` beside `virtio0`, with the
  model their codec's maker names, and play as any output does (ADR
  0320).
- The `audio-hda` and `audio-hda-ich9` boot tests play tones through
  QEMU's two controllers and check what `wavcapture` recorded at two
  volumes, so the codec's amplifier steps are checked too.
- The lab's `sound/hda` test checks each real machine's controllers, and
  `sound/tone` records a machine's jack through the lab PC's line in and
  checks the left and right tones' pitch.
- Microphones, line in, HDMI and DisplayPort audio, more than two
  channels and S/PDIF are left for later; so are interrupts, which
  would let the ring be shorter.
