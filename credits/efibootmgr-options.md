<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: Projects whose ideas we follow
- [efibootmgr](https://github.com/rhboot/efibootmgr): the options efiboot
  takes, by whose name it runs too, and the way it lists boot entries.
  Linux's `EFI_MIN_RESERVE`: the 5 KiB of room the kernel keeps in the
  firmware's variable store. We use none of their code
  ([ADR 0313](docs/adr/0313-the-firmwares-boot-entries-through-its-runtime-services.md)).
