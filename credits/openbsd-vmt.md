<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: Projects whose ideas we follow
- [OpenBSD's vmt(4)](https://github.com/openbsd/src/blob/master/sys/dev/pv/vmt.c)
  (ISC): the registers of VMware's backdoor calls, the steps of its
  guest RPC, and the commands and answers of the guest's toolbox, which
  the kernel's VMware toolbox follows
  ([ADR 0424](docs/adr/0424-vmware-guests.md)). No code is copied.
