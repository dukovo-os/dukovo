<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: Projects whose ideas we follow
- FreeBSD's [mrsas](https://github.com/freebsd/freebsd-src/tree/main/sys/dev/mrsas)
  and [mfi](https://github.com/freebsd/freebsd-src/tree/main/sys/dev/mfi)
  drivers (BSD-2-Clause): the MFI frames, the DCMD opcodes and their
  structures, and the Fusion message frames of Broadcom's MegaRAID
  controllers, which the kernel's MegaRAID driver follows. QEMU's
  [`hw/scsi/megasas.c`](https://gitlab.com/qemu-project/qemu/-/blob/master/hw/scsi/megasas.c)
  model (GPL-2.0-or-later) is what the boot tests run against
  ([ADR 0664](docs/adr/0664-megaraid-raid-controllers-are-driven.md)). No
  code is copied.
