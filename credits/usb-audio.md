<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: Specifications and papers
- The USB Implementers Forum's Device Class Definition for Audio Devices,
  releases 1.0 and 2.0, with their Audio Data Formats and Terminal Types,
  from which `radix_usb::audio` reads USB speakers, headsets and DACs and
  the kernel plays through them; and the eXtensible Host Controller
  Interface specification, from which `radix-xhci` queues isochronous
  transfers ([ADR 0323](docs/adr/0323-usb-audio.md)). How devices report
  their rate in the other speed's format, and how clock selectors lead to
  a clock source, come from FreeBSD's uaudio(4) (`sys/dev/sound/usb`,
  BSD-2-Clause). No code is taken from it, or from Linux's driver.
