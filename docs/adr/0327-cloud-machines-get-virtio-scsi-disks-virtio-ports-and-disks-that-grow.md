<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0327. Cloud machines get virtio SCSI disks, virtio ports and disks that grow

- Status: Accepted
- Date: 2026-11-09

## Context

Dukovo is to run as a cloud machine, from an image its build makes (Phase
14d). Three things such a machine has that the kernel did not handle:

- **Disks on a virtio SCSI adapter.** Google Cloud attaches every disk
  this way, as do Proxmox and OpenStack when asked, since one adapter
  holds many disks and SCSI commands carry what block requests cannot.
  The virtio specification (version 1.2, section 5.6) gives the adapter a
  control queue, an event queue and request queues; a request names a
  target and logical unit, and carries a SCSI command and its data.
- **Ports of a virtio console.** QEMU's guest agent and the agents of
  Proxmox and OpenStack talk to the machine over a named port of a virtio
  console (section 5.3): `org.qemu.guest_agent.0`. Linux shows each port
  as `/dev/vportNpM` and links its name from `/dev/virtio-ports`; the
  agents open it by that name.
- **A disk larger than the image.** An image is a few GiB; the disk it is
  copied to is as large as the person chose. Linux distributions grow
  their root partition to the disk's end on the first boot (cloud-utils'
  `growpart`) and then the filesystem on it (`btrfs filesystem resize
  max`), while both are in use. A GPT on such a disk still has its backup
  where the image's end was.

## Decision

- **A virtio SCSI driver.** `radix-virtio`'s `scsi` module drives the
  adapter's first request queue; the control and event queues are not set
  up, so tasks are not aborted, and disks added while the machine runs
  are not found until it starts again. The kernel's glue scans every
  target up to the adapter's last, asking each with REPORT LUNS (or unit
  0 if it does not answer), and probes each unit with the SCSI crate the
  USB disks use (ADR 0162). A disk takes the next `sd` name, as SATA ones
  do; its model and revision come from INQUIRY, and its serial number
  from the unit serial number page (SPC-6 7.7.18), for
  `/kernel/block/sdX/device`. Reads and writes are READ and WRITE (10),
  or (16) past 2 TiB; a flush is SYNCHRONIZE CACHE. The disks of one
  adapter share its queue, and its MSI-X interrupt wakes each. CD and DVD
  drives on the adapter are logged and not used yet.
- **A virtio console driver, for its ports.** `radix-virtio`'s `console`
  module needs the multiport feature, sets up the control queues and each
  port's two queues, and tells the device when a port is ready and when a
  program has it open. The kernel makes each port a terminal,
  `/devices/vportNpM`, whose line carries bytes as they are, with no
  echo, line editing or carriage returns added; a named port is linked
  from `/devices/virtio-ports/NAME`, as on Linux. While no program has a
  port open, what the host sends is dropped, as Linux drops it. A port
  the host takes away goes, with its link. The first console of a QEMU
  machine (`-device virtconsole`) is a port like the others; the kernel
  does not write its log there.
- **The kernel grows a partition in place.** When it reads a disk's
  table again (ADR 0301), a partition with the same number and first
  sector that is now larger grows where it is, even while it is mounted,
  as Linux's BLKPG_RESIZE_PARTITION does; one that moved or shrank is
  still refused while in use. `rdisk --grow DISK [N]` grows partition N,
  or the one that ends last, to the free space after it, and writes the
  table with a GPT's backup at the disk's end. A partition already as
  large as it can be is left, and that is no failure, so a first boot may
  run it on every boot.
- **Btrfs grows over a grown disk.** A new directory method,
  `resize_disk` (radix-directory(2)), grows a disk of a mounted Btrfs to
  a size or to all of its device: the disk's device item in the chunk
  tree and each superblock's total say the new size, and new chunks may
  go in the room. `device resize [DEVID:]max|SIZE PATH`, beside `device
  add` and `device remove` (ADR 0159), calls it, as `btrfs filesystem
  resize` does. A disk is not shrunk: that moves chunks off its end.

## Consequences

- The `storage-scsi` boot test runs the storage test on four disks at
  targets 0, 1, 2 and 5 of QEMU's virtio SCSI adapter, one with a GPT,
  and checks what the first boot wrote in the second.
- The `virtio-ports` boot test gives the machine a port named as QEMU's
  guest agent's, held by the test through a socket: bytes cross it as
  they are both ways, and the port goes, with its link, when QEMU takes
  it away. The guest agent (Phase 14d) opens that port.
- The `disk-grow` boot test makes a Btrfs partition on a disk, grows the
  disk between boots, grows the partition and the Btrfs in use, writes
  more than the partition held before, and has `fsck-btrfs` and the
  host's `sgdisk` check the result.
- The first-boot setup of cloud images runs `rdisk --grow` on the root
  disk and `device resize max /` after it.
- The adapter's event queue, for disks added and taken away while the
  machine runs, and task management, for aborting a command the disk does
  not answer, are left for later; a disk that stops answering has the
  adapter given up, as an unanswered NVMe controller is.
