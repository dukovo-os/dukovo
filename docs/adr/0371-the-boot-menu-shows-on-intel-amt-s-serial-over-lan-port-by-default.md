<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0371. The boot menu shows on Intel AMT's Serial-over-LAN port by default

- Status: Accepted
- Date: 2026-11-21

## Context

ADR 0481 put the boot menu's ports in `boot/grub/serial.env`. Without
that block, the installed system's menu shows on COM1 at 115200 baud;
`serial console auto`, which the installer writes for an install done
on a screen, gives the menu on the firmware's SPCR port, or on COM1 when
the firmware names none. The install stick's menu shows on every port a
computer is managed from, COM1 to COM4 and Intel AMT's Serial-over-LAN
function at PCI 00:16.3.

Intel AMT's port (`ttyS4`, kind `amt-sol` in `/kernel/class/tty`)
carries the kernel's log and a console, and the stick's menu shows
there. A machine managed over it that has no block, a system installed
before the block or a disk image written straight to the disk, has no
boot menu on it, and no Rescue entry: the port the machine is managed
from shows everything but the menu. The same holds after `serial
console auto`. The Dukovo Lab can read a real machine's console over
AMT's Serial-over-LAN, and its tests wait on the console for the menu's
lines, such as the entry being booted.

## Decision

- The menu of an installed system with no block shows on COM1 and on
  Intel AMT's Serial-over-LAN port, GRUB's `pci,00:16.3`, each at 115200
  baud, where the machine has them. A port the machine lacks is passed
  over.
- `serial console auto` gives the menu on the firmware's port at the
  firmware's speed, or on COM1 at 115200 baud, and then on AMT's
  Serial-over-LAN port at 115200 baud when the machine has one. When the
  firmware's port is AMT's own, it is listed once, at the firmware's
  speed.
- The kernel's flags, consoles and log ports stay as ADR 0481 gives
  them. A block that names ports is followed as written.

## Consequences

- The menu shows on up to two ports by default. A machine without
  AMT's port sees no change.
- QEMU's output and the boot tests are unchanged: QEMU has no such port.
- An install on a screen of an AMT machine writes a block with both
  ports.
- The memory test's console stays COM1's, as it is set only for COM1 to
  COM4.

## Alternatives considered

- **A block written into the lab's image only.** The lab would test a
  menu no one else gets, and `serial console auto` would drop the port
  again.
- **Every PCI serial port.** GRUB cannot know another card's address
  before the kernel names it, which is why the stick shows on AMT's
  address alone.
