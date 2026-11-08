<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0321. The virtio sound card

- Status: Accepted
- Date: 2026-11-08

## Context

Sound needs a device the boot tests can drive and check, before the real
ones of PRs 6b (Intel HDA) and 6c (USB). QEMU has three: the Intel HDA
codecs, a USB audio device, and since 8.2 the virtio sound card
(`virtio-sound-pci`), virtio 1.2's device type 25 (section 5.14). The
virtio card is the simplest of them, rides on the virtqueues the block
and network drivers already use (ADR 0053), and is what cloud and
desktop hypervisors offer next.

## Decision

- **`radix-virtio`'s `snd` drives it**, with no `unsafe` code, against a
  simulated card in its unit tests, as the block and network drivers are.
- **It plays through the first output stream that takes signed 16-bit
  samples**, and uses no jacks, channel maps or input streams; the event
  and receive queues are left unset.
- **A stream is driven as the specification lays it out**: SET_PARAMS,
  PREPARE, periods on the transmit queue, START; STOP when it runs out;
  RELEASE, which hands back every period, when its format changes or the
  player is done. Each period is a slot of DMA memory holding the
  transfer header and the samples side by side, sent as one buffer the
  card reads, and an 8-byte status it writes: two descriptors, so the
  queue holds 32 periods without indirect tables.
- **Control requests are sent one at a time and waited for** in steps of
  a millisecond, for at most a second; a card that does not answer is
  reset and plays nothing more.
- **One MSI-X entry, for the transmit queue**, wakes the output's worker
  (ADR 0320); the control queue raises none.
- **The card has no volume**: the kernel scales the samples.

## Consequences

- The `audio-virtio` boot test plays tones through QEMU's card with the
  `none` audio back end, and checks what QEMU's `wavcapture` recorded.
- Virtual machines under QEMU, and hypervisors that offer the card, have
  sound with nothing more.
- The driver's design (a slot per period, one interrupt, the worker
  calling it alone) is the one the HDA and USB drivers follow.
