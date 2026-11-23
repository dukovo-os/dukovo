<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0664. MegaRAID RAID controllers are driven

- Status: Accepted
- Date: 2026-11-23

## Context

Most rack servers keep their disks behind a RAID controller, and most of
those are Broadcom's (LSI's) MegaRAID: Dell sells them as PERC (the
PowerEdge R740's PERC H740P among them), Lenovo as ThinkSystem RAID, and
Broadcom under its own name. Without a driver for one, a server's disks are
not there at all.

The controller's firmware runs the RAID. The host sees each volume the
firmware builds as a SCSI target, and each disk the firmware passes through
to it (JBOD) as another. It manages the controller with DCMD frames: an
opcode, a 12-byte mailbox and a buffer for the answer, to list volumes and
disks, read their details, read the event log or wait on the next event,
flush the cache and shut the firmware down. Two interfaces carry the
frames. MFI, of the SAS1078 and SAS2108 parts (PERC 6, H700, H800), takes a
frame's address at a register and writes its context to a reply queue in
host memory. Fusion, of every part since the SAS2208 (PERC H330, H730,
H740P, H755), takes MPI2 request descriptors that name 256-byte request
frames, and answers with reply descriptors on a ring: a DCMD rides inside a
pass-through request, and reads and writes of a volume go as LD I/O
requests.

The kernel has a SCSI layer, `radix-scsi`, that virtio SCSI, VMware's
PVSCSI and Hyper-V's storvsc share: each disk is a block disk named `sdX`,
probed with INQUIRY and READ CAPACITY, and its commands share the adapter's
queue (ADR 0424). Disk health is `GET_HEALTH` (ADR 0157), which ADR 0665
extends to SCSI disks with log pages, for disks behind a controller too.

QEMU models two MFI parts, `megasas` (SAS1078) and `megasas-gen2`
(SAS2108), with a JBOD mode. It models no Fusion part. The layouts of the
frames, the opcodes and the structures they answer with are documented in
FreeBSD's `mrsas` and `mfi` drivers, which LSI and Avago wrote.

## Decision

- **One crate for the controller.** `radix-megaraid` (no_std, no unsafe,
  no allocation) holds both transports, the MFI frames they share, the
  DCMDs and the parsers of their answers, and the text of the files the
  kernel serves. Its tests run both transports against a simulated
  firmware. The kernel's part, `fs/disk/megaraid.rs`, binds the PCI
  function, gives it one interrupt (MSI-X entry 0, or MSI) and one reply
  queue, and adds the disks. Up to four controllers are driven, with up to
  32 volumes and pass-through disks each.
- **Volumes and pass-through disks are SCSI disks.** Each takes the first
  free `sd` name, is probed by `radix_scsi::probe`, and its reads and
  writes go to the firmware as LD I/O (Fusion) or SCSI I/O frames (MFI),
  sharing the controller's commands as PVSCSI's disks share the
  adapter's. Two commands are kept from the disks: one for a management
  command, one for the wait on the next event.
- **The firmware's cache is the volume's.** A firmware that refuses
  SYNCHRONIZE CACHE for a volume keeps its cache with a battery or flash,
  and the volume is said to have no write cache, as Linux answers the
  command itself. At shutdown the controller flushes its cache and the
  disks', then shuts its firmware down.
- **Management commands run one at a time.** Their answers land in the
  controller's own 64 KiB of DMA memory below 4 GiB. Each waits in 10 ms
  steps, and each step takes the replies the interrupt has not, so they
  work before the interrupt is on and at shutdown. A firmware that faults
  is reported as a fault, with its code.
- **A thread for each controller.** `megaraid<n>` reads the event log, the
  last 256, then waits on each next event. Once the events stop for half a
  second, every 30 seconds while a task (a rebuild, an initialisation, a
  check, a patrol read) runs, and every 5 minutes otherwise, it asks the
  controller again what it has. Events of warning class or worse are
  logged. A volume or a pass-through disk added while the machine runs is
  logged, and is a disk from the next boot.
- **`/kernel/raid/<n>` shows each controller (radix-sysfs(5)).**
  `controller`, `volumes`, `disks`, `battery` and `events` are the text
  of the last gathering, so reading them never waits on the controller:
  key lines and TAB-separated records whose unknown values are `-`.
  `health/<device id>` is read anew on each open: the driver asks the
  physical disk, in a volume or not, through the firmware's pass-through
  for its SMART data (a SATA disk) or its log pages (a SAS disk, ADR
  0665), in `GET_HEALTH`'s layout. Only root may read it, as it sends the
  disk commands.
- **Userspace reads the files.** `storage raid` shows the controllers,
  their volumes, their disks by bay and their battery; diskhealth(8) reads
  member disks as `raid<n>:bay<m>` alongside the system's own; `hardware
  check` has a `raid` area; metricsd(8) warns of a volume that degrades, a
  disk that fails or is predicted to, a battery that fails and a
  controller in fault; admind serves `storage.raid`, which the web
  console's Storage page and the Hardware page's check read.

## Consequences

- A server's disks behind a PERC or MegaRAID card are disks, whether the
  card builds volumes of them or passes them through, and their health is
  watched as a directly attached disk's is.
- QEMU tests the MFI transport, volumes and JBOD, and the files: the
  `megaraid` boot test runs a SAS1078 with a volume and a SAS2108 passing a
  disk through, and `storage-megaraid` runs the storage suite on four
  volumes. Fusion runs only against the crate's simulated firmware until a
  card is in the lab: the R740's H740P, with `lab/tests/storage/raid.toml`.
- The fast path that sends a volume's reads straight to its disks (the
  RAID map), a reply queue for each CPU, resetting a controller whose
  firmware faulted, NVMe disks behind tri-mode cards, and taking a volume
  or disk added at run time are deferred rows (docs/deferred.md).
- Changing the configuration (making and deleting volumes, spares,
  starting a patrol read or a check, importing a foreign configuration) is
  not here: the files show it, and later steps of Phase 19 add it.

## Alternatives considered

- **Pass the firmware's management interface to userspace, as Linux's
  ioctl does for Broadcom's storcli.** That tool is Linux's and closed,
  and a frame from userspace reaches every disk: ADR 0157 keeps a disk's
  commands in the kernel's hands.
- **A driver for each interface.** The frames, the DCMDs and their answers
  are the same in both; only how a frame reaches the firmware differs, so
  one crate holds both transports behind one interface.
- **Drive only pass-through disks, as a host bus adapter's.** Most servers
  come with their disks in volumes, and the volume is what the firmware
  protects.
- **Ask the controller on every read of its files.** A read would wait on
  the firmware, and the console polls; the thread's gathering after
  events gives the same answers without that.
