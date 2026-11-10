<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0435. Floppy drives are looked at only when asked, and read a cylinder at a time

- Status: Accepted
- Date: 2026-11-10

## Context

Roadmap 11h lists floppy drives on the PC's 82077AA controller among
the drives whose medium comes and goes (ADR 0430). They are on older
PCs and on servers whose Super I/O chip still has the controller, and on
QEMU, where `-device isa-fdc` and `-device floppy` give one. A floppy
disk is still how some firmware updates, old data and virtual machines'
driver disks arrive.

The controller differs from every other drive in the kernel:

- It is an ISA device at fixed ports (0x3f0 to 0x3f7), described in the
  ACPI namespace as `PNP0700`. Its `_FDE` says which of four drives are
  there and each drive's `_FDI` what kind it is. It has no PCI function
  to scan.
- Its data moves by ISA DMA, channel 2 of the 8237 DMA controller,
  which reaches only the first 16 MiB of memory and cannot cross a
  64 KiB line. Linux keeps a buffer for it with `GFP_DMA`; the kernel
  has no such zone.
- It takes one command at a time for all its drives, and its interrupt
  (ISA IRQ 6) is not routed: the kernel routes neither the legacy IRQs
  nor INTx (ADR 0244), as with IDE channels.
- A drive cannot say that a disk went in. Its disk change line goes up
  when a disk comes out, and only a step of its heads with a disk in it
  brings the line down. Linux's `floppy` driver finds a new disk only
  when something opens the drive, and does not poll: polling would step
  the heads every two seconds, which a real drive makes heard and wears.
- A drive's tracks are read at the data rate they were written at, 500
  kbit/s for a 1.44 MB disk and 250 kbit/s for a 720 KB one, and a read
  at the wrong rate finds no sectors at all.

## Decision

- **A driver crate of its own, radix-fdc**, drives the 82077AA and the
  Super I/O chips that copy it, with channel 2 of the DMA controller
  (`dma`). It has no `unsafe` code, and its tests run it against a
  simulated controller, DMA controller and drives. It is polled: it
  reads the main status register for each command and result byte, and
  after a seek or recalibration asks SENSE INTERRUPT STATUS until the
  heads arrive.
- **The kernel finds the controller in the ACPI namespace.** Its ports
  and DMA channel come from `_CRS` (a controller on another channel is
  not used), the drives attached from `_FDE` (the first alone when there
  is none), and each drive's kind from the `_FDI` of the child whose
  `_ADR` is its number (a 3.5-inch 1.44 MB drive when there is none). The
  drives are named `fd0` and `fd1` after their numbers on the
  controller, as on Linux. Drives 2 and 3, which no PC cable has, and
  5.25-inch drives are not read; a 2.88 MB drive reads 1.44 MB and
  720 KB disks.
- **The ISA DMA buffer is kept at boot, below 1 MiB.** The first MiB is
  already reserved whole; the kernel keeps the first 32 KiB from 64 KiB,
  on a 32 KiB boundary, that holds nothing else reserved
  (`Devices::isa_dma_buffer`). That costs no memory and is enough for a
  cylinder of a 1.44 MB disk, 18 KiB. Behind an IOMMU it is mapped for
  the ISA bridge for good, as Linux maps the first 16 MiB.
- **One thread runs the controller and its drives**, as the controller
  takes one command at a time. It carries out each request a cylinder at
  a time, both heads in one command, through the DMA buffer, seeking to
  the cylinder itself first. A transfer is tried three times, the heads
  recalibrated between tries, or the controller reset when it did not
  answer. The
  thread turns a drive's motor off three seconds after its last use.
- **A disk is read whole, with no partition table**, as Linux reads a
  floppy disk. The thread tells disks apart by READ ID at each data rate
  the drive has, then checks the last sector of the first track and what
  the FAT boot sector says. 1.44 MB and 720 KB disks are read and
  written; an unformatted disk, or one of another format, is said in the
  log and left alone. A write-protected disk is a read-only device
  (SENSE DRIVE STATUS), and volumed mounts it read-only.
- **A boot sector's geometry counts only when it is a floppy's**, of one
  or two heads, such as a DMF disk's 21 sectors a track. Many tools
  write 63 sectors and 255 heads on every volume, which says nothing of
  a floppy's tracks. `mkfs-fat` was one: on a whole floppy disk it now
  writes the disk's own geometry and media byte, as `mkfs.fat` and DOS's
  `format` do, which DOS and boot code need to find a sector.
- **A drive is never polled for a disk.** It is looked at once at boot,
  and again whenever radix-block(2)'s REREAD_PARTITIONS asks
  (`rdisk --reread /devices/fd0`): a floppy disk has no partition table
  to read again, so the request has the drive's thread look instead,
  and a disk found is in `/devices` when the request returns. Like
  EJECT, it needs no write right. Before each request, the thread reads
  the drive's change line; a disk taken out fails the request and
  leaves the drive empty, and volumed unmounts what was mounted from it.
- **EJECT lets the disk go.** A PC floppy drive has no motor to push the
  disk out: the disk's requests fail from then on, and the drive is
  empty, and stays empty at every look until its change line says the
  disk came out.
- **IDE optical drives and floppy drives share one request queue,
  `handed`.** Both have a thread that runs a controller for several
  drives and takes a disk's requests when it is free. The queue holds a
  medium's requests until the thread takes one and finishes it. AHCI
  optical drives keep their own path, which sends a read from the
  reader's thread (ADR 0434).

## Consequences

- The boot test `floppy` puts a 1.44 MB disk and a 720 KB one in two
  drives on QEMU's ISA floppy disk controller. It mounts both, writes
  and reads back files across many cylinders, ejects a disk, puts it
  back and finds it at the next look, and pulls one out while mounted.
  It also swaps the second for itself write-protected and finds it
  mounted read-only. The host reads back the images.
- The lab test `storage/floppy` puts disks in and takes them out of a
  drive that stays, with the lab's `floppy` steps.
- A disk put in a drive is not found until something asks:
  `rdisk --reread`, or the `floppy` command of 11h's next step, which
  looks and formats. A desktop that wants a disk found when it goes in
  asks when the user opens the drive, as on Linux.
- `/kernel/block/fd0/removable` is 1 and `media_changes` counts disks in
  and out, as for every drive whose medium comes and goes.
- A floppy disk's slow writes showed a race in the FAT, exFAT, NTFS and
  ext4 drivers: a sync took the changed files from the writeback
  thread's sync while that one was still writing them, found nothing to
  do and returned, so an unmount could end, and the drive let its medium
  go, before the files were on it. Their syncs now take turns, as
  Btrfs's already did.
- 2.88 MB and 5.25-inch disks, formatting and USB floppy drives (UFI)
  are 11h's next step. Real drives are to be checked when the lab has
  one.

## Alternatives considered

- **Polling the drive every two seconds, as a card reader is.** The
  change line comes down only when the heads step, so each look would
  step them: heard on a real drive, wearing on it, and the motor would
  rarely be off.
- **The controller's interrupt.** The kernel routes no legacy IRQs (ADR
  0244), and a command waits on the controller in any case; polling the
  status register while the thread sleeps between looks costs nothing
  a person would notice.
- **A DMA zone in the page allocator, as Linux's `GFP_DMA`.** One 32 KiB
  buffer in the first MiB, which the kernel already reserves, does the
  job with no change to the allocator.
- **Reading a sector at a time.** Each command would wait for the disk
  to come round again: a cylinder at a time reads a 1.44 MB disk in
  about 160 turns rather than 2880.
