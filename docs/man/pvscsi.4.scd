pvscsi(4) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

pvscsi - VMware's paravirtual SCSI disk controller

# DESCRIPTION

PVSCSI, which VMware's hypervisors give a machine when asked for their
fastest disk controller, and QEMU emulates as its *pvscsi*. Its requests
and their completions go through rings in memory the kernel gives it,
and it raises one MSI-X or MSI interrupt when commands end and when the
host adds or removes a disk (ADR 0424).

The kernel finds the adapter on the PCI bus as it starts, resets it,
asks each of its targets which logical units it has and what each is,
and gives each disk an *sd* name, *sda*, *sdb*..., after the SATA,
USB and virtio SCSI disks found before it. Each disk keeps up to 256
commands in flight, each a READ, WRITE or SYNCHRONIZE CACHE of up to 32
runs of memory. The kernel log names each disk as it is found:

```
Disk: sda is a SCSI disk of 16 GiB on the VMware PVSCSI adapter at 0000:03:00.0, target 0 LUN 0: VMware Virtual disk, 512 bytes a block, write cache, MSI-X vector 0x50
```

A CD or DVD drive on the adapter is logged and not used. A disk the host
adds or removes while the machine runs is logged; one added is used from
the next boot.

# SEE ALSO

*vmware*(4), *vmxnet3*(4), *blkid*(8)
