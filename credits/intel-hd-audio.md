<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: Specifications and papers
- Intel's High Definition Audio Specification, revision 1.0a, from which
  `radix-hda` drives HD Audio controllers and their codecs
  ([ADR 0322](docs/adr/0322-intel-hd-audio.md)). The PCI settings that
  make other makers' controllers snoop, and the coefficient the Realtek
  ALC255 and ALC256 need, come from FreeBSD's hdac(4) and hdaa(4)
  (`sys/dev/sound/pci/hda`, BSD-2-Clause). No code is taken from them, or
  from Linux's driver.
