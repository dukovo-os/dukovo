<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: Specifications and papers
- SCSI Multi-Media Commands 6 (MMC-6, INCITS 468), from which burn(1)
  writes CDs, DVDs and Blu-ray discs and `ob disc-drive` records them,
  with IEC 60908 (the Red Book) for a music CD's pregaps and its tracks
  of at least four seconds;
  ECMA-119 4th edition with Microsoft's Joliet specification and Rock
  Ridge 1.10 (IEEE P1281 and P1282), and ECMA-167 3rd edition with
  OSTA's UDF 2.01, from which `disk::disc` makes the images that burn and
  mkiso(1) write
  ([ADR 0449](docs/adr/0449-discs-are-burned-by-a-program-through-mmc-commands-the-kernel-passes.md)).
  We use no code of cdrecord, wodim, growisofs, xorriso, libisofs or
  libburn.
