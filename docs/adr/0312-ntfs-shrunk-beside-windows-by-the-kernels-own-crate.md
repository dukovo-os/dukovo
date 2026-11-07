<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0312. NTFS shrunk beside Windows by the kernel's own crate

- Status: Accepted
- Date: 2026-11-06

## Context

Phase 14a's fourth part installs Dukovo next to Windows. The approved
design's Disk step offers *Install next to Windows*: it shrinks Windows'
volume, makes Dukovo's partitions in the space freed, and a menu at
startup picks either system. This record covers the shrink, the first of
three steps; the boot entries and the installer's layout follow.

Linux's installers shrink Windows with ntfs-3g's `ntfsresize`. It is
GPL-2.0 C, and Dukovo already has an NTFS of its own that reads and
writes volumes Windows' `chkdsk` finds sound (ADRs 0236 and 0238).
Windows' own *Shrink Volume* stops at the first file it will not move,
often halfway, and needs Windows started and the person to do it there.

## Decision

- **The kernel's NTFS crate shrinks volumes.** Its `check` and `shrink`
  modules (feature `shrink`, on the host's memory) do the work; the disk
  engine's `ntfs` feature gives them a device, as for Btrfs (ADR 0304);
  `resize-ntfs(8)` takes ntfsresize's options and runs by its name too;
  the installer calls the engine.
- **A check first, and after.** The walk reads every record of the
  master file table and keeps a bit for each cluster: 32 MiB for a
  terabyte in 4 KiB clusters. It finds clusters two files hold, clusters
  a file holds that `$Bitmap` calls free, records in use that cannot be
  read or are marked free, and runs that are damaged or pass the end. A
  volume with any of those is refused: moving such a cluster would harm
  a file. Clusters marked in use that no file holds are lost, which a
  power cut leaves and `chkdsk` gives back; they are counted, not
  refused. The same walk finds what lies past the new end.
- **What it refuses, changing nothing:** a volume BitLocker encrypts; a
  volume Radix would not write (Windows hibernated, Fast Startup, a
  journal to replay, marked to be checked); a volume the check finds
  unsound; a size under the smallest; and a file whose runs, once moved,
  would not fit in its records. Every move is worked out, and every
  record's new runs packed into a copy of it, before anything is
  written.
- **The smallest size** is the clusters files hold, bad ones aside, with
  a margin of a hundredth of the volume (a MiB at least) for Windows to
  work in, and never less than the end of `$Boot` and the master file
  table's first sixteen records, which the boot sector points at and so
  never move. It is rounded up to a whole MiB, as partitions are.
- **Everything else moves**, Windows' own files too: the master file
  table's records past the sixteenth and its bitmap, `$MFTMirr`, the
  journal, `$Bitmap`, `$Secure`, `pagefile.sys` and the rest. Bad
  clusters past the end are left out of `$BadClus`.
- **Spans, moved whole.** The runs of one attribute past the end that
  follow one another in the file are one span. The master file table's
  spans are placed first, then the longest: each into the shortest free
  run below the end it fits in, or, when none is long enough, in pieces
  into the longest. So a file comes out in fewer pieces, and its run
  list shorter, where moving each run as it lies split files across
  small gaps until their runs no longer fit in their records.
- **An order a power cut leaves readable.** Nothing a record points at
  is overwritten while it points there:
  1. The volume is marked dirty and its journal emptied.
  2. The clusters the copies go to are marked in use.
  3. The files' clusters are copied, and the device flushed.
  4. Each record is rewritten to say where its copies are (record 1,
     the mirror, with its own new place), and the device flushed.
  5. The master file table's own clusters past the end are copied, and
     its first record, which never moves, says where.
  6. The clusters copied from are marked free.
  7. `$Bitmap`'s clusters past the end are copied below it, now that no
     other bit changes, and its record says where.
  8. The boot sector says the new size.
  9. `$Bitmap` and `$BadClus` are cut to it.
  10. The copy of the boot sector is written in the new last sector.

  A cut before step 4 leaves clusters marked but no file's, which
  `chkdsk` gives back; a cut after it leaves each record pointing at
  whole data, old or new.
- **Marked to be checked at the end**, as `ntfsresize` leaves a volume,
  so that Windows' `chkdsk` looks at it at its next start, unless asked
  not to (`--no-windows-check`). Until then Radix mounts it to read only.
- **The volume alone.** The shrink leaves the partition as it is: the
  volume uses less of it. The partition table is shrunk after, by the
  partitioner, to the size the shrink returns.

## Consequences

- The installer can offer to install next to any Windows that shut down
  fully with BitLocker off, without starting Windows first, and the
  space it frees is all but what the files hold.
- A file in very many pieces past the end can still be refused, with
  words that say to let Windows optimize (defragment) the drive. The
  tests find none on volumes filled and thinned as Windows' are.
- Windows' restore points (volume shadow copies) record blocks by their
  place on the volume, so Windows may drop them after a shrink, as after
  `ntfsresize`; the files are kept.
- A shrink reads the whole master file table twice and copies what lies
  past the end once, page by page, device to device.
- The tests shrink volumes of 512-byte to 64 KiB clusters and of 4 KiB
  sectors, with `$Bitmap` and the master file table moved near the end,
  cut power at forty points and in each of the last dozen requests, and
  check each volume with radix-ntfs, ntfs-3g (`ntfsresize --info`,
  `ntfsfix`, `ntfscat`) and The Sleuth Kit; the CI's Windows job runs
  `chkdsk` on the volumes they keep. The `ntfs-resize` boot test shrinks
  a disk the host made with `mkntfs` and `ntfscp`, at the console with
  `resize-ntfs` and `ntfsresize`, and ntfs-3g checks it on the host.

## Alternatives considered

- **Porting `ntfsresize`.** GPL-2.0 C beside an NTFS of our own that
  already writes what `chkdsk` accepts; two NTFS implementations would
  disagree in ways the tests would have to chase.
- **Asking the person to shrink Windows in Windows first.** *Shrink
  Volume* frees only what lies past the first file it will not move,
  and the installer's free-space layout already uses space freed that
  way.
- **Moving each run as it lies.** The first version did, and refused
  ordinary volumes: runs moved into small gaps split, and the run lists
  outgrew their records.
- **Leaving the volume clean.** Nothing would then look at it from
  Windows' side; ntfsresize's choice costs one check at the next start.
