<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: Specifications and papers
- FreeBSD's `axge(4)` driver, `if_axge.c` and `if_axgereg.h`
  (BSD-2-Clause, Copyright Kevin Lo), and OpenBSD's `axen(4)` driver,
  `if_axen.c` and `if_axenreg.h` (ISC, Copyright Yojiro UO), the
  reference for the registers, vendor requests, bring-up, burst and
  header formats and bulk-in settings of ASIX's AX88179, AX88179A and
  AX88178A in `radix_usb::net::asix`
  ([ADR 0362](docs/adr/0362-asix-usb-ethernet-adapters-speak-their-own-protocol.md)).
  The bulk-in settings are FreeBSD's values. No code is taken from them.
