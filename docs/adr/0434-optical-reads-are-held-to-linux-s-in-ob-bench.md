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

An optical drive takes SCSI commands in ATAPI packets, one at a time.
ADR 0431 gave each drive a thread of its own, which took a read from the
disk queue, sent it, slept until the port's interrupt and gave the read
back. On Linux, libata sends the packet for the SCSI CD driver (`sr`)
from the reading thread and completes it from the interrupt. What a read
costs beyond the drive's own time is what the two do differently: under
KVM in CI, with every read going through the drive's thread, Radix took
1.07 times Linux's time to read 64 KiB and 1.20 times to read 1 MiB,
whose eight commands of 128 KiB each woke the reader and the drive's
thread in turn.

Under KVM, each read or write of a controller's register is a trap to
QEMU. For each read, libata writes the port's command issue register,
and its interrupt handler reads and clears the controller's and the
port's interrupt status and reads the command issue register again: six
traps. Sent from the reader, with the port's interrupt turned on and its
status cleared for each read, and the reader looking at the port once
before it slept, Radix took ten, and 1.06 times Linux's time for 64 KiB.

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
- **A disc's reads on an AHCI port go as Linux's do.** The thread that
  reads sends its READ(10) to the port itself
  (`radix_ahci::Port::send_packet`), and the port's interrupt hands the
  read back to the disc's queue and sends the next one waiting, so a
  large read's commands follow one another with no thread woken between
  them (`kernel/radix/src/fs/disk/optical_port.rs`). The drive's thread
  keeps all that may sleep: for its own commands it takes the port once
  the read on it is over, and the reads that come meanwhile wait their
  turn; a read the drive fails, or that goes unanswered for 30 s, is the
  thread's to restart the port for, ask the drive why (REQUEST SENSE) and
  hand back judged as before. A drive on an IDE channel, which borrows
  the channel from a disk for each command (ADR 0432), keeps its reads in
  its thread.
- **A read traps as often as Linux's.** The port remembers the
  interrupts it last turned on (`Port::quiet`, `Port::forget_interrupts`
  for a port the controller's handler quieted while the drive's thread
  had it), so a read sent while they are on writes the command issue
  register alone. The reader then sleeps at once: the queue polls only a
  controller that can turn its interrupt off (`Controller::POLLED`,
  virtio's alone, ADR 0365), and a look at an AHCI port before sleeping
  costs two traps to find what the interrupt brings anyway. An interrupt
  that finds the read already handed back, and the port's status clear,
  leaves the port and the drive's thread alone.
- **The rows are gated** as the disk rows are: Radix may take no longer
  than Linux (ADR 0107).

## Consequences

- A change to the optical drive's thread, the ATAPI path of AHCI or the
  disk queue that makes a disc's reads slower than Linux's fails the
  gate.
- A disc's read on an AHCI port costs what a SATA disk's read costs the
  kernel: sent where it is asked for, ended in the interrupt, six traps.
- A thread waiting for an NVMe, AHCI, IDE, SD or USB disk no longer looks
  at its controller once before it sleeps in a virtual machine; the
  interrupt it sleeps for brings the same completion. The drive's
  look every two seconds waits for the read on the port, and the reads
  that come during the look wait for it, a few hundred microseconds.
- Three parties share the port (the reader, the interrupt and the
  drive's thread) under one lock, and the drive's thread lets go of a
  disc's queue only once no interrupt handler holds it, so that its last
  reference never goes in an interrupt.
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

- **Reads kept in the drive's thread, which chains the next read without
  waking the reader.** It takes away the reader's wake between a large
  read's commands, but each read still crosses to the thread and back,
  the cost that put the 64 KiB row over Linux's.
- **Reading files from an ISO 9660 disc**, as a copy from a disc does.
  That measures the filesystem too, which the Btrfs rows already do for
  their filesystem; the raw reads isolate the drive's path.
- **A USB DVD drive.** QEMU's USB mass storage spends most of each read
  in the emulated xHCI, so the row would measure QEMU; USB storage is
  measured by its own work later.
- **Reported only until parity** (ADR 0294). The gate holds a disc's
  reads from the start, as it held the NVMe rows from theirs.
