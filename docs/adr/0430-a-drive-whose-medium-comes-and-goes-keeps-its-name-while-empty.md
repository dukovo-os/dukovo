<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0430. A drive whose medium comes and goes keeps its name while empty

- Status: Accepted
- Date: 2026-11-08

## Context

Roadmap step 11h brings floppy, CD, DVD and Blu-ray drives, and SD and
CompactFlash card readers. Each is a drive whose medium comes and goes
while the drive stays: a disc goes in a tray, a card in a slot. Until
now Radix had only disks that come and go whole. A USB stick is found
when it is plugged in and goes when it is pulled out (ADR 0161), volumed
mounts what is plugged in and unmounts what is pulled out (ADR 0163),
and a USB card reader with no card in it was skipped, because READ
CAPACITY fails on it. A card put in later was never seen, and one taken
out left a disk whose every request failed.

Linux keeps such a drive as a block device of size 0 while it is empty
(`sd` for a card reader's slot, `sr` for an optical drive, `fd` for a
floppy drive). With systemd's udev rules the block layer asks the drive
every two seconds whether its medium changed (`events_dfl_poll_msecs` of
2000), with TEST UNIT READY on a SCSI drive. A medium put in is read as
a new disk under the same name; udisks mounts it. While a disk is open,
`sd` sends PREVENT ALLOW MEDIUM REMOVAL so that a drive that can lock
its medium in does, and `eject` (the CDROMEJECT request) lets go of the
lock and sends START STOP UNIT with LoEj set, which opens a tray.

Every later 11h driver needs the same pieces: a name kept while empty,
a medium found when it goes in, its partitions read, what was mounted
from it lost when it comes out, a lock while it is used, and an eject.
This ADR makes them once, on the drives Radix already has: USB card
readers, and USB sticks and disks that say their medium is removable.

## Decision

**A drive whose medium comes and goes is a disk that keeps its name
while it is empty. Its driver looks at it every two seconds, swaps the
device under its name when a medium goes in or comes out, locks the
medium in while it is used, and ejects it when asked.**

- **Which drives.** A USB logical unit whose INQUIRY says its medium is
  removable (the RMB bit), or one that answers with no medium when it is
  found, as a card reader's empty slot does. Each such unit gets a
  `Drive` (`fs::disk::removable`), kept by its name while the unit is
  plugged in.
- **Empty.** With no medium in it, the drive's device is
  `radix_block::Empty`, of no sectors, whose reads and writes fail with
  io. It is in `/devices` and `/kernel/block` under the drive's name,
  with a size of 0. Spark leaves a device of size 0 alone at boot.
- **Looking.** The thread that serves the USB device (ADR 0161) sends
  TEST UNIT READY every two seconds, as the others' requests allow:
  between requests on a Bulk-Only stick, and when nothing is in flight
  on a UAS one. It clears the unit attentions a new medium raises (up
  to four tries). A unit attention of a changed medium, or a NOT READY
  of no medium, on any request marks the medium as maybe changed: the
  request fails, and the next look settles it at once.
- **A medium in.** The drive's thread reads the medium's capacity, makes
  its disk and request queues as for a stick plugged in, and reads its
  partition table. `fs::disk::replace` then puts the new device in the
  drive's entry, takes out the old partitions and adds the new, and
  counts the change, all under one hold of the disk table's lock; the
  device nodes are replaced in place after it. The name never leaves.
- **A medium out.** The old medium's queues are ended first, so that
  any request on it fails at once with io. What was mounted from it or
  its partitions is lost, as for a disk pulled out (`Vfs::lose`), and
  the drive's device is `Empty` again.
- **The count.** `/kernel/block/<drive>/removable` is 1 for a drive and
  0 for any other disk, as on Linux. `/kernel/block/<drive>/media_changes`
  counts each medium that went in or came out. volumed reads the counts
  before and after it lists `/kernel/block`, and lists again until they
  agree, so that a swap while it looked is not missed. A drive whose
  count moved has its old names taken as gone and its new names as
  plugged in, though a new partition may have an old one's name, so a
  card put in is mounted at its label, and is never taken as a disk
  partitioned again (ADR 0301).
- **The lock.** At each look, the thread asks whether the medium, a
  partition of it or a BitLocker volume unlocked on it is in use
  (mounted, swap or unlocked, as GET_IN_USE has it), and sends PREVENT
  MEDIUM REMOVAL when it starts being used and ALLOW when it stops. A
  drive that refuses the lock is noted once in the log and works as
  before.
- **Eject.** radix-block(2)'s EJECT (0x5309, Linux's CDROMEJECT) on the
  drive fails with in_use while the drive or a partition of it is in
  use, and otherwise hands the eject to the drive's thread, since that
  thread is the one that talks to the drive, and waits for its answer
  for up to 40 seconds. The thread lets go of the lock, sends START STOP
  UNIT with LoEj, takes the medium out, and answers. A drive that cannot
  eject, as most card readers cannot, has its medium let go all the
  same: it is empty until a medium is taken out and put in. The request
  needs no write right, as on Linux. eject(1) unmounts every volume on a
  drive's medium first, then ejects it.
- **No waiting on the mount table.** An unmount holds the mount table's
  lock while its filesystem writes out what it holds, and those writes
  may need the drive's thread. So the thread never waits for that lock:
  it asks what is in use and loses what was mounted only if the lock is
  free at that moment, and otherwise ends the medium's queues (so that
  the writes fail at once and the unmount ends), and tries again 100
  milliseconds later.

## Consequences

- A card put in a USB card reader is mounted at `/volumes/<label>`, and
  `eject` unmounts and ejects it, with the reader left for the next. The
  `card-reader` boot test checks it on QEMU's `usb-storage` with a
  removable empty drive: the empty drive, a card put in and mounted, the
  lock that makes QEMU refuse to eject it, eject(1), a card put back,
  and one pulled out by force while mounted.
- Most USB sticks set RMB, and are drives now, as on Linux: each is
  looked at every two seconds, which costs one command on its own
  thread, and is locked while mounted. A stick pulled out is still
  unplugged whole. QEMU's boot sticks set it too (`removable=on`), so
  every USB boot test has one.
- A drive that says its medium changed (a UNIT ATTENTION of sense code
  28h or 3Ah) has it taken out and put in again, and loses what was
  mounted from it, since a new medium may look the same as the old. A
  reset's UNIT ATTENTION (29h) does not count, so that the transport's
  own recovery after a slow command does not take a mounted stick away.
- The floppy, optical and SD drivers of 11h make a `Drive` each and use
  the same `replace`, count, lock and eject. A card reader on the PCI
  bus (SDHCI, ADR 0247) still needs its card in at boot until 11h's SD
  work brings it to this.
- A raw open of the drive, by `dd` say, does not lock the medium, as
  Linux's open does: Radix opens devices by capability, and counts
  mounts, swap and unlocked volumes as use.
- `lsblk -o RM` shows the drives. RM is not among the default columns,
  so that what reads them by position, such as the lab's tests, is not
  moved.
- The lab gains `card = "insert"`, `"remove"` and `"reinsert"` steps, a
  card reader on QEMU machines for the tests that use them, and the
  `storage/card-reader` test.

## Alternatives considered

- **A new name for each medium**, a disk plugged in as `sdc` and taken
  out as a disk pulled out. It needs nothing new in the kernel, but the
  drive itself would be invisible while empty, eject would have nothing
  to act on, a new card in the same slot would take the next letter, and
  a desktop could not show an empty drive. Linux keeps the name.
- **Asking the drive to say when its medium changes** (GET EVENT STATUS
  NOTIFICATION, SATA's asynchronous notification). USB card readers do
  not support it; optical drives on SATA do, and 11h's optical driver
  may use it to look less often. Polling is what Linux does for USB.
- **Looking from volumed.** The kernel must know a medium is gone to
  fail its requests and lose its mounts, and only the drive's thread
  may send it commands, so the looking is the kernel's. volumed only
  reads the count.
- **Locking from open, as Linux's `sd` does.** Radix hands devices out
  as capabilities and does not count opens; a mount, swap or unlocked
  volume is what loses data when a medium is pulled, and is what it
  counts.
