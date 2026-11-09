<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0434. Optical reads are held to Linux's in ob bench

- Status: Accepted
- Date: 2026-11-09

## Context

`ob bench` holds Radix's raw disk transfers to Linux's on the same
virtual machine: a virtio disk (ADR 0111) and an NVMe disk (ADR 0115),
read and written through their first 32 MiB, `O_DIRECT` on Linux so that
neither side has a cache between the program and the disk. ADR 0431 and
ADR 0432 brought optical drives on AHCI, IDE and USB, and the install
DVD (ADR 0433) is read whole when Dukovo is installed from it, so a
drive's reads now matter as a disk's do.

An optical drive takes SCSI commands in ATAPI packets. On Radix each
drive has a thread of its own, which takes a read from the disk queue,
sends it, sleeps until the port's interrupt and gives the read back; on
Linux, libata sends the packet for the SCSI CD driver (`sr`) from the
reading thread and completes it from the interrupt. What a read costs
beyond the drive's own time is what the two do differently.

## Decision

- **Both virtual machines get a DVD drive** on q35's AHCI controller,
  QEMU's `ide-cd` on the first port, holding a disc: a 64 MiB file on
  the host of the same bytes as the disks, read-only, made afresh for
  each boot. It is `cd0` on Radix and `sr0` on Linux.
- **Two rows**: the POSIX benchmarks, given `--disc`, read the drive 64
  KiB and 1 MiB at a time, one read after the other through its first
  32 MiB, as the disk rows do (`posix_disc_read_64k`,
  `posix_disc_read_mib`), from `/devices/cd0` raw on Radix and
  `/dev/sr0` with `O_DIRECT` on Linux. A disc is read, not written, and
  in large pieces: there is no 4 KiB row and no write row.
- **Linux loads what the drive needs**: its init loads AHCI's module,
  and the SCSI CD driver's where it is a module, beside Btrfs's (the
  pinned Ubuntu kernel builds `sr_mod` in and AHCI as a module), and
  `linux-bench`, given the drive after `--` on its command line, waits
  up to ten seconds for it, as AHCI probes its ports in the background.
- **The rows are gated** as the disk rows are: Radix may take no longer
  than Linux (ADR 0107).

## Consequences

- A change to the optical drive's thread, the ATAPI path of AHCI or the
  disk queue that makes a disc's reads slower than Linux's fails the
  gate.
- Under TCG, where the numbers measure the emulator, Radix read 64 KiB
  in 0.55 of Linux's time and 1 MiB in 0.60, as with the virtio and NVMe
  disks; the gate holds the numbers CI measures under KVM.
- Radix's drive thread looks at the drive every two seconds (ADR 0430),
  and Linux, with no udev in its initramfs, does not poll: the other
  rows of the POSIX boot carry a few microseconds every two seconds on
  Radix's side.
- The POSIX boot reads 64 MiB more from the host per side, a second or
  two of CI time.

## Alternatives considered

- **Reading files from an ISO 9660 disc**, as a copy from a disc does.
  That measures the filesystem too, which the Btrfs rows already do for
  their filesystem; the raw reads isolate the drive's path.
- **A USB DVD drive.** QEMU's USB mass storage spends most of each read
  in the emulated xHCI, so the row would measure QEMU; USB storage is
  measured by its own work later.
- **Reported only until parity** (ADR 0294). Radix reads the disc as
  fast as Linux does from the start, under TCG at least, so the rows
  are gated from the start, as the NVMe rows were.
