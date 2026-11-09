<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0431. Optical drives read a disc whole, from its last session

- Status: Accepted; a disc's reads on an AHCI port go out from the reading
  thread and end in the port's interrupt since [0434](0434-optical-reads-are-held-to-linux-s-in-ob-bench.md)
- Date: 2026-11-09

## Context

Roadmap step 11h brings CD, DVD and Blu-ray drives. Radix already reads
the filesystems discs hold, ISO 9660 with Rock Ridge and Joliet and UDF
(ADR 0245), from disc images on disks and sticks, and ADR 0430 made the
drives whose medium comes and goes: a name kept while empty, a look
every two seconds, a medium swapped under the name, a lock while it is
used, and an eject. What was missing was the drives themselves. An AHCI
port with an optical drive on it, whose signature says it takes ATAPI
commands, was left alone (`an ATAPI device; not used`), and a USB unit
whose INQUIRY says it is a CD/DVD device (peripheral type 5) was not a
disk.

An optical drive differs from a disk in what it is asked and how its
medium is read:

- It takes SCSI commands, the MMC set, whether it is on SATA, where
  they go in ATA PACKET commands (ATAPI), or on USB. It takes one at a
  time, and some take seconds while a disc spins up.
- A disc is not always data. A music CD holds audio tracks, a blank
  disc nothing, and a CD can hold both. A CD-R or DVD+R can be written
  in several sessions, each with its own filesystem descriptors, of
  which the last describes the whole disc.
- Linux's `sr` reads a data disc whole, in blocks of 2,048 bytes,
  read-only, and looks for no partition table on it. It asks the drive
  where the last session starts (READ TOC format 1, the
  CDROMMULTISESSION request), and its ISO 9660 driver reads the volume
  descriptors there. Linux names the drives `sr0` and on, with a
  `/dev/cdrom` link; FreeBSD names them `cd0` and on.

## Decision

**An optical drive is a drive whose medium comes and goes (ADR 0430),
named `cd0` and on. Its driver asks the disc what it is with MMC's
commands, reads a disc of data whole and read-only, and gives the block
device where its last session starts, from which ISO 9660 reads the
volume. A music CD or blank disc leaves the drive empty as a device,
and the drive says what is in it.**

- **Which drives.** An AHCI port whose device answers IDENTIFY PACKET
  DEVICE with a valid identity, and a USB logical unit whose INQUIRY
  says it is a CD/DVD device. The drives on IDE (PATA) channels follow
  in the next step of 11h.
- **Names.** `cd` and the first number free, in the order the drives
  are found: the SATA drives in the PCI scan's order, then the USB ones
  in the order their devices were found, as the `sd` letters go (ADR
  0161). A disc has no partitions, so its device has only its drive's
  name. `cd` is FreeBSD's name and the target of Linux's `/dev/cdrom`
  link, and says what the drive is.
- **ATAPI on AHCI.** A command is an ATA PACKET command whose command
  header has its ATAPI bit set, with the SCSI command in the command
  table's ATAPI area. Data moves by DMA when the drive does DMA and the
  transfer is a whole number of 16-byte units in the queue's buffers,
  telling the drive the direction (DMADIR) when it asks for it, and
  otherwise by PIO through a buffer of 2,048 bytes, at most 63 KiB a
  burst. A command the drive fails gives its sense key in the error
  register's upper bits; the driver then asks REQUEST SENSE, as Linux's
  libata does, so that the caller sees the whole sense data. A port an
  error stopped is restarted; a command unanswered after its time has
  the link reset, and a drive that does not come back is gone.
- **A thread for each SATA drive.** The drive takes one command at a
  time and a disc may take seconds to spin up, so each optical drive on
  an AHCI port has a thread of its own, as each USB device has (ADR
  0161). The thread serves the disc's reads, which come through the
  shared queue one at a time, as READ(10) of 30 seconds each (Linux's
  `sr` timeout), and between them looks at the drive every two seconds
  with TEST UNIT READY (ADR 0430). Each command is waited for asleep:
  the port's interrupt turns the port's interrupts off and wakes the
  thread, and a controller without MSI is looked at after waits that
  double from 10 microseconds to 4 milliseconds.
- **What disc.** When a disc goes in, its drive is asked GET
  CONFIGURATION (its current profile: CD-ROM, DVD+R DL, BD-RE and the
  others of MMC-6), READ DISC INFORMATION (whether it is blank), READ
  TOC (its tracks, data or audio, and the lead-out), READ TOC format 1
  (where its last session starts) and READ CAPACITY(10), falling back to
  the lead-out on old drives that say a disc has one block. An old drive
  that does not know GET CONFIGURATION or READ DISC INFORMATION has its
  disc taken as a CD-ROM of data; a TOC it cannot read too.
- **A disc of data** is read whole, read-only, in blocks of 2,048
  bytes, and no partition table is looked for on it, as Linux's `sr`
  does. A CD of data and music is read through its data track, which
  comes first.
- **Its last session.** The block device says where the disc's last
  session starts (`BlockDevice::last_session`, 0 for any other device).
  ISO 9660 looks for its volume descriptors 16 blocks after it, and at
  the start of the disc if none are there, as Linux's isofs does. As
  Linux's `sr` does, a session start on a disc whose first track is the
  only one counts as none. UDF finds its anchors at block 256 from the
  start and the end of the disc, so a UDF disc written in one session
  reads; one appended to in several, with a virtual allocation table,
  does not yet.
- **No data.** A music CD or a blank disc is not a medium the kernel
  reads: the drive stays empty as a device, of size 0, and the change is
  counted, so that volumed says what went in. Playing a music CD is 11h's
  audio step.
- **What the drive says.** `/kernel/block/<drive>/disc` says what is in
  an optical drive: `none`, the disc's kind and what it holds
  (`dvd-rom data`, `cd-rom audio`, `cd-r mixed`, `dvd+r blank`), or
  `unreadable`. Spark and volumed say it in words (a music CD went in
  cd0), and lsblk's TYPE is `rom` for an optical drive, as Linux's.
- **Lock and eject** are ADR 0430's: PREVENT MEDIUM REMOVAL while the
  disc is mounted, and eject(1)'s EJECT lets go of the lock and sends
  START STOP UNIT with LoEj, which opens the tray.
- **At boot** the kernel waits for each SATA drive's first look, for at
  most 15 seconds, before the first program, so that a disc in a drive
  at boot is there for Spark to mount (USB drives are found within ADR
  0161's wait for USB disks). The PCI summary line counts the optical
  drives.

## Consequences

- A disc in a SATA or USB DVD or Blu-ray drive is mounted at boot at
  `/volumes/cd0`, and one put in later at `/volumes/<label>`, read-only.
  The `optical` boot test checks it on QEMU: an `ide-cd` on AHCI, and a
  `scsi-cd` behind USB over Bulk-Only and over UAS, each with a disc at
  boot, read, its drive's `disc` file and lsblk's `rom`, the lock that
  makes QEMU refuse to eject a mounted disc, eject(1), a disc put back
  and mounted at its label, and one pulled out by force while mounted.
- QEMU's drives always say a disc has one session, and they have no
  music CDs or blank discs. Those, multisession discs and the old drives'
  fallbacks are checked by unit tests against simulated drives
  (`radix-scsi`'s MMC tests, `radix-ahci`'s ATAPI tests) and against
  discs xorriso writes in two sessions (`radix-iso9660`), and on real
  drives by the lab, where a person puts the discs in.
- The boot tests that start from a CD-ROM now find that CD as `cd0`,
  holding the boot image, and Spark mounts it read-only at
  `/volumes/cd0`. Booting the live system from a DVD is the next step.
- The lab gains `disc = "insert"`, `"eject"` and `"reinsert"` steps, an
  empty DVD drive on QEMU machines for the tests that use them, and the
  `storage/optical` test.
- The drives on IDE channels, the eject button (GET EVENT STATUS
  NOTIFICATION), reading speed against Linux's, music CDs and writing
  discs are 11h's later steps.

## Alternatives considered

- **Linux's names, `sr0` and on.** Radix names its disks as Linux does
  where the name says what the device is (`sda`, `nvme0n1`,
  `mmcblk0`). `sr` says "SCSI ROM", which a SATA drive is not, and
  Linux itself points `/dev/cdrom` at it; `cd` is FreeBSD's and plain.
- **Reading a disc's partition table.** A hybrid ISO image, as Dukovo's
  own, has an MBR whose partitions count in 512-byte sectors for a
  stick, and point elsewhere on a disc of 2,048-byte blocks. Linux's
  `sr` reads no table, and neither does Radix.
- **The SATA disks' queue for ATAPI.** A SATA disk's port runs many
  commands at once, matched to the queue's slots, and the interrupt
  handler completes them. An optical drive takes one command at a time,
  some of them long and some answered with a sense that needs another
  command, which a thread of its own does more simply, as the USB
  devices' threads do.
- **GET EVENT STATUS NOTIFICATION to look for discs**, which Linux's
  `sr` asks before TEST UNIT READY. QEMU's and real drives answer TEST
  UNIT READY alike, and ADR 0430's look is one command; GET EVENT
  STATUS NOTIFICATION comes with the eject button, which only it
  reports.
