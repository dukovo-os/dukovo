<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0446. CompactFlash cards in IDE adapters move data by PIO

- Status: Accepted
- Date: 2026-11-10

## Context

CompactFlash cards are still used in cameras, industrial machines and
retro computers, where an IDE adapter often stands in for a hard disk. A
CF card reaches a PC in one of four ways:

- **In an IDE adapter**, in True IDE mode (the *CF+ and CompactFlash
  Specification* 6.0, 6.2.1.6), as an ATA device on an IDE channel. Its
  IDENTIFY DEVICE data begin with 848Ah, which ADR 0244's driver took
  for "not an ATA device". Cards before CF 5.0 have no 48-bit
  addresses, and many have no DMA in True IDE mode; a passive adapter
  need not wire the DMA lines even when the card has them, and a DMA
  command on such an adapter hangs the channel until it is reset.
- **In a USB card reader**, as a USB disk, which ADR 0430 drives.
- **CFast cards** are Serial ATA devices, on AHCI ports.
- **CFexpress cards** are NVMe devices on PCIe.

ADR 0244's driver also refused ATA disks without 48-bit addresses, which
ATA-6 brought in 2002, though READ DMA and WRITE DMA with 28-bit
addresses reach their 128 GiB.

QEMU 8.2's `ide-cf` is a CF card of 28-bit addresses with no write
cache, named QEMU MICRODRIVE.

## Decision

- **IDENTIFY DEVICE data that begin with 848Ah are a CF card's**, and are
  read as a disk's. A device without 48-bit addresses gives its capacity
  in words 60 and 61; AHCI still refuses it, as every SATA disk and
  CFast card has them.
- **A CF card's sectors move by PIO**: READ SECTORS and WRITE SECTORS
  (their EXT forms for a card of 48-bit addresses), a sector to each
  DRQ block, through the channel's bounce buffer below 4 GiB. The
  channel moves at most eight sectors each time the queue polls it, as
  the queue holds a lock with interrupts off meanwhile.
- **A device without 48-bit addresses takes 28-bit commands**: READ DMA,
  WRITE DMA and FLUSH CACHE for a disk, READ SECTORS and WRITE SECTORS
  for a card, of at most 256 sectors, the top four bits of the address
  in the device register. A request is at most 32 pages, so it fits one
  command.
- A card is named `sda` and on as the other disks on IDE, ATA and SATA
  are, is solid state unless it says it spins, and its line in the log
  says what it is: `sdb is a CompactFlash card of 32 MiB on the IDE
  primary channel at 0000:00:04.0, slave: QEMU MICRODRIVE, 512 bytes a
  block, solid state, 28-bit addresses, by PIO`.
- CF cards in USB readers, CFast cards and CFexpress cards are the USB,
  SATA and NVMe disks they are, with nothing of their own.

## Consequences

- The IDE storage boot test runs one of its four disks as QEMU's CF
  card, so PIO, 28-bit addresses and the bounce buffer are checked on
  every change that touches them; 28-bit DMA, a card of 48-bit
  addresses, a failed read and the limits of a 28-bit command are
  checked by unit tests against the simulated channel. A CF card in an
  IDE adapter, a CFast card and a CFexpress card in a reader go on the
  lab's list.
- PIO costs a CPU's time for each word, and a card in an adapter runs at
  a few megabytes a second: enough for what such cards hold.
- A card pulled out of an adapter while the machine runs fails its
  requests, as a disk pulled out would; True IDE mode has no hot-plug.

## Alternatives considered

- **DMA for a card that says it has it.** Whether the adapter wires the
  DMA lines cannot be read from the card, and a wrong guess hangs the
  channel. A card found in the lab to run well by DMA can be listed
  later.
- **PCMCIA and CardBus CF adapters** in memory or I/O mode, as old
  laptops have. They need a PC Card driver, and the laptops that have
  their slots predate what Dukovo targets.
