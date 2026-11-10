<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: Projects whose ideas we follow
- [FreeBSD's pvscsi driver](https://github.com/freebsd/freebsd-src/tree/main/sys/dev/vmware/pvscsi),
  which VMware wrote (BSD-2-Clause, offered also under GPL-2.0, taken
  under BSD-2-Clause): the registers, commands, rings and descriptors of
  VMware's PVSCSI adapter, which the kernel's PVSCSI driver follows
  ([ADR 0424](docs/adr/0424-vmware-guests.md)). No code is copied.
