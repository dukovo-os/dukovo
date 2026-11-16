<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: In every image
| [OpenBSD](https://www.openbsd.org/)'s `rge(4)` driver, `if_rge.c` 1.49 and `if_rgereg.h`, by Kevin Lo | The setup of Realtek's RTL8125, RTL8126 and RTL8127 chips and their PHYs, translated into Rust in `kernel/drivers/rge/src/setup.rs` with its notice kept, and OpenBSD's code run on a model of the chips in `tools/rge-openbsd-check` to check the translation; Realtek's firmware for the chips, which OpenBSD loads from files, is not included | ISC |
