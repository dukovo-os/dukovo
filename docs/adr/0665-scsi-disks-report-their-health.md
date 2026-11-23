<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0665. SCSI disks report their health, through the controller when there is one

- Status: Accepted
- Date: 2026-11-23

## Context

SATA disks report their health with SMART and NVMe drives with their SMART /
Health Information log, both read through `GET_HEALTH` (ADR 0157) and judged
by diskhealth(8). A server's disks are SAS: they keep no SMART attributes.
SCSI has its own equivalent. A disk keeps counters in log pages, read with
LOG SENSE (SPC-6): its own warning that it will fail (informational
exceptions, 0x2F), its temperature and the limit it gives (0x0D), the errors
it could not correct (0x02, 0x03 and 0x05), a solid state disk's wear (0x11),
its log of self-tests (0x10). The blocks it has moved to spares as they grew
bad are its grown defect list, whose length READ DEFECT DATA gives.

Phase 19 step 5 adds the controllers a server's disks hang off. Three of the
SCSI ones are already drivers: virtio SCSI, VMware's PVSCSI and Hyper-V's
storvsc, and the shared command layer is `radix-scsi`. None keeps a health
record, so `GET_HEALTH` answers `not_supported` for every disk behind them.
A SATA disk behind a SAS host bus adapter or a hypervisor's SCSI controller
shows the same way, though SMART reaches it through ATA PASS-THROUGH(16)
(SAT-5). The RAID controller of step 5, MegaRAID, does not let the host send
a disk a command: it sends the controller a frame that names the disk
behind it, and the controller passes the command on.

QEMU's SCSI disks refuse LOG SENSE; a disk that does so keeps no record, as
a virtio disk does today.

## Decision

- **A record of log pages.** `radix_scsi::health` asks a disk for log page
  0x00, the list of pages it has, and then for each of ten pages it
  supports, most useful first: informational exceptions, temperature, read,
  write, verify and non-medium errors, solid state media, start-stop cycles,
  background scan (whose first parameter counts the minutes powered on) and
  the self-test results last. It asks for the grown defect list's header
  last, in the address format the disk chooses. The answers are packed into
  1,024 bytes: entries of a page code, a subpage, a length (u16, little
  endian) and the page as the disk returned it, cut to what fits, ending at
  an entry of code 0 and length 0. The first entry, code 0xF0, is the count
  of grown defects. A disk with no list of pages keeps no record.
  `GET_HEALTH` gives it as a new kind, `HEALTH_SCSI`; the log pages stay
  the disk's own, and diskhealth(8) reads them, as it reads an NVMe log.
- **`gather` sends nothing.** It is given a function that runs one query
  into a buffer. The kernel's SCSI lanes run it through the disk's queue, as
  new operations (`Op::Query`) that read into the request's one buffer. The
  RAID controller wraps the same two queries in its pass-through frames, and
  reuses `gather` and the record.
- **A SATA disk behind a SCSI layer answers as a SATA disk.** A disk whose
  INQUIRY vendor is "ATA" is read as AHCI reads one: SMART READ DATA and
  READ THRESHOLDS, as ATA PASS-THROUGH(16) (PIO data-in, one sector; the
  queue's `Op::Smart` on a SCSI lane), into the two halves of the record,
  kind `HEALTH_ATA`. Unlike AHCI, the power mode is not asked first: a
  SMART read may spin a sleeping disk up.
- **The verdict follows the ATA and NVMe ones.** A disk's own warning (a code
  of 0x5D, failure prediction) is FAILING; a code of 0x0B, its temperature
  or background tests, is a WARNING. Errors the disk could not correct,
  grown defects, or a solid state disk that has used 100% of its endurance
  are a WARNING; else PASSED. The table, details, `--json`, the live files
  and the early warning of rising counts take the SCSI record in the shapes
  they have for the others.

## Consequences

- SAS disks behind a host bus adapter or a RAID controller, and SATA disks
  behind a hypervisor's SCSI controller, show their health in diskhealth(8),
  and Spark's service sets the same alerts for them. No disk can be
  tested with real hardware yet; the log page layouts are tested against
  records written to SPC-6 and SBC-4, and QEMU's disks, which refuse, are
  unchanged.
- A SAS disk's SMART data is its log pages: there are no attributes and no
  thresholds, so the verdict rests on the disk's own warning and on counts.
- The record is an ABI. A page longer than the room left is cut, and the
  pages after it left out, so the order of the ten pages matters; the
  longest, the self-test log, is last.
- The self-test of a SCSI disk (SEND DIAGNOSTIC, and the log page that shows
  it) is a deferred row: the log is shown, but `SELF_TEST` does not start or
  abort a test on a SCSI disk.
- The RAID controller reaches its member disks with pass-through frames, and
  its health reuses `gather` (ADR 0664).

## Alternatives considered

- **Parse the pages in the kernel and give diskhealth a fixed record, as for
  NVMe.** The kernel would carry every page's layout and a fixed set of
  fields, and each page a vendor adds would need a kernel change. The
  disk's own bytes need no more than a length.
- **Read every log page the disk lists.** A disk lists dozens, most of them
  vendor pages no tool reads, and the record is 1,024 bytes.
- **Read a SATA disk behind a SCSI layer by its log pages.** The layer
  translates few of them, and a SATA disk's attributes and thresholds are
  what its verdict is judged by.
- **Add a request that passes a command through, as Linux's `SG_IO` does.**
  ADR 0157 keeps a disk's commands out of userspace's hands: only those the
  kernel sends are sent.
