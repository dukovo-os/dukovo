<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0436. Floppy disks are formatted a track at a time, and USB floppy drives are floppy drives

- Status: Accepted
- Date: 2026-11-10

## Context

ADR 0435 reads and writes 1.44 MB and 720 KB disks in the PC's floppy
drives, and left two parts of roadmap 11h's floppy step for the next:
formatting a disk, and USB floppy drives.

- A floppy disk is formatted by the drive, a track at a time: the
  82077AA's FORMAT TRACK writes a track's sector IDs, which it takes from
  memory by DMA, four bytes each, and fills the sectors. Linux has the
  FDFMTTRK request for it, and fdformat asks it for each track before
  mkfs.fat makes a filesystem. A formatted disk is the only kind the
  driver reads; a disk bought blank, or one of another format, needs it.
- USB floppy drives, the floppy drives of PCs with no controller, speak
  the UFI command set (USB mass storage subclass 4), over the
  Control/Bulk/Interrupt transport (CBI), whose command blocks go on the
  control pipe and whose statuses come on an interrupt pipe, or over
  Bulk-Only. UFI has no MODE SENSE(6), pads every command to 12 bytes,
  and formats a track with FORMAT UNIT.
- A USB floppy drive says that its disk changed only with a UNIT
  ATTENTION at the next command it is sent. Polling it every two
  seconds, as a card reader is, would work it as polling a PC drive
  would.

## Decision

- **A track is formatted by radix-block(2)'s FORMAT_TRACK,** Dukovo's own
  request, as Linux has FDFMTTRK: the size of disk in sectors (2880 or
  1440), the cylinder and the head. It needs the write right, and is
  refused while the drive is in use. It is handed to the drive's thread,
  as EJECT and a look are (`removable::Drive::format`). The first track
  takes the disk out of the kernel, as what it held is going: the drive
  is empty until a look finds the disk formatted.
- **The PC's controller formats with FORMAT TRACK,** the track's sector
  IDs written to the ISA DMA buffer, with the gap and filler byte Linux's
  floppy driver gives each density (radix-fdc's `Fdc::format_track`). A write-protected disk is refused before the
  command, from SENSE DRIVE STATUS.
- **A USB floppy drive is a floppy drive.** Over CBI or CB
  (`radix_usb::storage::cbi`), or Bulk-Only with its commands padded to 12
  bytes, it is named `fd` with the first number free once the PC's
  drives are named, so that `fd0` is the PC's first drive when it has
  one. It is looked at only when asked, as a PC drive is, and when a
  request finds its disk changed; its disk is read whole, with no
  partition table; whether it is write-protected comes from MODE
  SENSE(10). It formats a track with UFI's FORMAT UNIT, one track on one
  side, the disk's size in its format descriptor (`radix_scsi::ufi`).
- **`/kernel/block/<drive>/formats` lists the sizes a floppy drive
  formats**: `1.44M 720K` for a 3.5-inch high-density drive, PC or USB.
- **floppy(1) lists the drives, looks, and formats.** `floppy format`
  formats every track, opening the drive again for each, as the device
  opened before goes with the disk; has the kernel look; reads the disk
  back; makes a FAT on it with mkfs-fat's floppy geometry; and mounts it
  at its label. 2.88 MB and 5.25-inch disks are not formatted, as they
  are not read.

## Consequences

- The boot test `floppy-format` formats a blank 1.44 MB disk and a blank
  720 KB one in two drives on QEMU's ISA floppy disk controller, makes a
  FAT labelled as asked on each, mounts them, and writes a file that the
  host reads back. It also finds a mounted disk and a write-protected one
  refused. QEMU's controller takes FORMAT TRACK but writes nothing, so
  the test shows the requests and what comes after them; a real drive's
  format is the lab's to check.
- The lab test `storage/floppy-format` formats blank disks of both sizes
  in a real drive.
- QEMU has no USB floppy drive: CBI, UFI's commands and FORMAT UNIT are
  checked by unit tests against simulated drives, and a real USB floppy
  drive is to be checked when the lab has one.
- FORMAT_TRACK is a new request in the ABI, so every port is built again
  once.

## Alternatives considered

- **Formatting in the kernel, the whole disk in one request.** A disk
  takes about a minute to format, which a request would hold the drive
  for with no word of how far it got, and fdformat's way of a track a
  time is what Linux's tools know.
- **Linux's FDFMTBEG, FDFMTTRK and FDFMTEND, with its floppy_struct.**
  They carry Linux's table of formats and its own drive parameters,
  which Dukovo's driver does not keep; one request of three numbers does
  the same.
- **USB floppy drives named `sd`, as Linux names them.** A floppy disk
  in one is then polled every two seconds, which steps its heads, and
  the drive is told apart from a stick only by its size; as `fd`, it is
  found, looked at and formatted as a PC's floppy drive is.
