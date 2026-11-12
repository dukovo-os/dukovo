<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: Build, test and documentation tools
| [dnsmasq](https://thekelleys.org.uk/dnsmasq/doc.html), by Simon Kelley and contributors | DHCP and TFTP on the Dukovo Lab's network, which real lab machines boot the disk writer from ([ADR 0429](docs/adr/0429-real-lab-machines-are-written-fresh-by-a-writer-booted-from-the-lab-network.md)) | GPL-2.0-only OR GPL-3.0-only |
| [iPXE](https://ipxe.org/)'s network boot ROMs, as QEMU ships them | The network card's UEFI driver in `ob test lab-writer`, which net boots the disk writer in QEMU | GPL-2.0-or-later (parts GPL-2.0-or-later WITH UBDL-exception) |
