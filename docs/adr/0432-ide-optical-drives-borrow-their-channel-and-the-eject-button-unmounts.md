<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0432. IDE optical drives borrow their channel, and the eject button unmounts

- Status: Accepted
- Date: 2026-11-09

## Context

ADR 0431 brought CD, DVD and Blu-ray drives on AHCI ports and on USB,
and named two of 11h's later steps: drives on IDE channels, and the
eject button.

Drives on IDE channels are on the machines Radix runs on whose SATA
controller the firmware sets to IDE mode, on older PCs with a parallel
ATA drive, and on QEMU's `pc` machine, whose PIIX controller has its
CD-ROM on the secondary channel. The IDE driver of ADR 0244 found such
a drive by its signature and left it alone (`an ATAPI device`).

An IDE channel differs from an AHCI port in what the drive shares:

- A channel has two ends, master and slave, and takes one command at a
  time for both. A disk and an optical drive on the same channel, as a
  PC's primary channel often has, take turns.
- The channel is polled: the kernel routes neither the legacy IRQs 14
  and 15 nor INTx (ADR 0244), so a command is done when its waiter
  sees the device's status say so.
- An optical drive takes SCSI commands in ATA PACKET commands, as on
  AHCI, but the host moves the packet and the data through the data
  register itself (ATA8-APT): after PACKET, the device asks for the 12
  bytes of the packet, and then moves the data in pieces of the size it
  says, by PIO, or by the bus master if the host asked for DMA.

The eject button is the other half. While a disc is mounted, PREVENT
ALLOW MEDIUM REMOVAL keeps the tray shut (ADR 0430), and the drive does
not open it when its button is pressed. An MMC drive then reports an
eject request among its media events (GET EVENT STATUS NOTIFICATION,
media class, event 1). Linux's `sr` asks for these events before each
look and passes an eject request to user space as
`DISK_EJECT_REQUEST`, on which udev's rules eject the disc. Without
that, the button of a mounted disc does nothing, which looks broken.
QEMU's drives do the same: `eject` of a locked drive without `-f` is
refused, and the drive reports an eject request.

## Decision

- **A drive on an IDE channel is an optical drive as on AHCI.** It has
  a thread of its own (ADR 0431), named `cd` and a number in the order
  drives are found, the AHCI and IDE drives in the PCI scan's order,
  then the USB drives. The thread runs the same code; only the carrier
  of its commands differs.
- **radix-ide carries PACKET commands.** IDENTIFY PACKET DEVICE says
  what the drive is, read by `radix_ahci::ata` as for AHCI. A command
  is started by `start_packet` and driven by `poll_packet`: the packet
  is written when the device asks for it (DRQ with CoD), each PIO piece
  is moved as the byte count says, extra bytes are dropped, and a
  write's missing bytes are zeros. A command of whole 16-byte units
  goes by DMA when the drive takes it, straight to the buffers below
  4 GiB or through a bounce buffer, as the disks' commands do; others
  (INQUIRY, REQUEST SENSE) go by PIO. A device that breaks the protocol
  (data with no packet sent, the packet asked twice, a piece of zero
  bytes) ends the command as a protocol error.
- **The drive borrows the channel from the disks' queue.** A disk's
  commands are a queue's lane (ADR 0244). The drive's thread asks for
  the channel; the queue starts no new disk command while it is asked,
  and lends it as soon as none is in flight. The thread gives it back
  after each command, with the sense of one that failed, and wakes the
  queue. A channel with no disk is the drive's alone. So neither end
  waits for more than one command of the other.
- **A command is polled**, with a wait that doubles from 10 µs to 4 ms,
  for at most the command's time limit; one that runs over has the
  channel reset (SRST), and a drive that does not come back is gone.
- **The eject button is asked for before each look.** An optical drive,
  on AHCI, IDE or USB, is asked GET EVENT STATUS NOTIFICATION of the
  media class, polled, until it has nothing to say, up to four times. A
  drive that refuses the command, or has no media class, is not asked
  again. An eject request is counted in
  `/kernel/block/<drive>/eject_requests` and logged (`its eject button
  was pressed`). A UNIT ATTENTION the drive answers instead, though MMC
  has the command pass them by, is kept for the look's TEST UNIT READY.
- **volumed unmounts and ejects.** When a drive's count moves, volumed
  unmounts the disc's volumes and ejects it through radix-block(2)'s
  EJECT, as eject(1) does. A volume that cannot be unmounted, being in
  use, keeps the disc in, and volumed says why.

## Consequences

- The boot test `optical-ide` puts a FAT disk and a DVD drive on the
  primary channel of QEMU's PIIX3 and another DVD drive alone on the
  secondary. It copies a file from the disc to the disk on its channel
  while the other drive reads, compares them, presses the eject button,
  puts the disc back and pulls it out by force, and writes to the disk
  after.
- The boot test `optical` presses the eject button of the AHCI drive and
  of a USB drive: QEMU's refused `eject` of a locked drive now has the
  disc unmounted and ejected, where before it only showed the lock.
- radix-scsi has `mmc::events`, radix-ide the PACKET protocol, each
  with unit tests against simulated drives.
- A drive's button with nothing mounted opens the tray as before: the
  drive is not locked, and opens it itself.
- Real IDE drives, and the button of real SATA and USB drives, are to be
  checked when the lab has them.
- The live system from a DVD, reading speed against Linux's, music CDs
  and writing discs remain 11h's later steps.

## Alternatives considered

- **A queue of its own for the drive.** The channel takes one command
  at a time; two queues on it would need the same lending, and a disk's
  command could not start while the drive's ran either way.
- **The disks' queue for the drive's commands.** A disk's request is a
  read or write of sectors; a drive's commands are SCSI commands, some
  needing a REQUEST SENSE after, and the drive's thread already runs
  them for AHCI.
- **Ejecting on a press at once**, in the kernel or as udev's rule does
  on Linux. The disc would go from under its mounts, which then fail
  until volumed notices; volumed, which mounted it, unmounts it first,
  and a disc in use stays in rather than going from under a program.
- **Ignoring the button,** as before. A drive whose button does nothing
  while a disc is mounted looks broken, and the user has to know to run
  eject(1).
