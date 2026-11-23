megaraid(4) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

megaraid - Broadcom and LSI MegaRAID SAS RAID controllers

# DESCRIPTION

The MegaRAID SAS controllers that Dell sells as PERC, Lenovo and others
under their own names, and Broadcom as its own, and that QEMU emulates as
*megasas* and *megasas-gen2* (ADR 0664). The controller's firmware builds
the RAID volumes and does the RAID; the kernel asks it for them and
sends it their requests. It takes the controller's registers and gives it
one MSI-X or MSI interrupt.

The kernel knows these by their PCI device ID, which says which
interface the controller has. The older parts speak *mfi*:

	- *SAS1078*: PERC 6, and QEMU's *megasas*
	- *SAS2108*: PERC H700 and H800, and QEMU's *megasas-gen2*
	- *SAS2008*, the "skinny" parts: PERC H310

The newer parts speak *fusion*:

	- *SAS2208*
	- *SAS3108* and *SAS3008*: PERC H730 and H330, and the parts of
	  their kind
	- *SAS3508* and its kin: PERC H740P, the card of a Dell PowerEdge
	  R740
	- *SAS3908*: PERC H755

A Broadcom 3908 part whose secure boot failed or is not set up has its
own device IDs, which its firmware refuses a driver on; the kernel does
not take them.

The kernel finds each controller on the PCI bus as it starts and brings
its firmware up. It asks the firmware what the controller is, and for
its volumes and its physical disks, and then asks each volume, and each
disk the controller passes through to the host (JBOD), what it is.
Every one of them becomes a disk with an *sd* name, *sda*, *sdb*..., the
first free, as a SATA disk does: the volumes in the order of their
targets, then the disks in the order of their device IDs. A
disk that is a member of a volume is not one itself; the volume is. A
controller with no volume and no pass-through disk is kept: its disks are
shown in _/kernel/raid_ all the same.

The controller's commands are shared by its disks, which keep them in
flight together (the controller's *commands*, in _/kernel/raid/_*n*_/controller_).
A command that ends busy, or for a reason the firmware may get over, is
tried again. The kernel log names the controller and each disk as they
are found:

```
Disk: sdb is pass-through disk 0 of 64 MiB on the MegaRAID controller at 0000:00:05.0: QEMU QEMU HARDDISK, 512 bytes a block, MSI-X vector 0x31
Disk: MegaRAID SAS2108 controller at 0000:00:05.0: LSI MegaRAID SAS 9260-8i, firmware package 20.10.1-0123, 0 volumes, 1 pass-through disk, MSI-X vector 0x31
```

A pass-through disk is numbered by its device ID, a volume by its target.

A device that is neither a volume nor a disk is logged and not used. A
volume or disk that cannot be used is logged with the reason. A
controller whose firmware does not come up is logged with the state it
stayed in, and left alone.

A volume's write cache is the controller's. A firmware that does not
take SYNCHRONIZE CACHE for a volume keeps its cache with a battery or
flash module, and the volume is said to have no write cache.

# /kernel/raid

What the controllers have is shown in _/kernel/raid/_*n*, *n* being the
controller's number, 0 first, in PCI order: _controller_, _volumes_,
_disks_, _battery_ and _events_, the formats of which *radix-sysfs*(5)
holds. They are read-only, and a read never waits on the controller.
*storage*(8) shows them (*storage raid*), and the console, *metricsd*(8)
and *hardware*(8) read them.

# HEALTH OF DISKS

_/kernel/raid/_*n*_/health/*id*_ has a file for each physical disk, in a
volume or not, where _id_ is the disk's device ID in _disks_. Reading it
asks the disk, through the controller, for its SMART data or log pages,
and gives the record that *diskhealth*(8) reads. Only root may read it.
Opening it fails with *ENOTSUP* when the disk keeps no record, as QEMU's
disks do not.

# EVENTS

Each controller has a thread, *megaraid*_n_, which reads the controller's
log of its last 256 events, and then waits for each next one: a volume or
disk that changes state, a rebuild that moves, a battery that charges.
When the events stop for half a second, and every 30 seconds while a task
(a rebuild, a check) runs, it asks the controller again what it has. An
event of class warning or worse is written to the kernel log. If the
controller refuses to tell of its events, that is logged once and the
thread asks what the controller has every few minutes instead.

# SHUTDOWN

Before the machine is powered off, restarted or halted, once the disks
have flushed, the kernel has each controller write its cache and its
disks' caches out, and shut its firmware down. A step that fails is
logged, and the next is tried.

# LIMITS

The kernel does not yet:

	- use the controller's RAID map to send requests straight to the disks;
	  they go through the firmware's logical disk path
	- use more than one reply queue, as against one for each CPU
	- reset a controller whose firmware has faulted; the controller stays
	  lost until the next boot
	- reach the NVMe disks behind a tri-mode card
	- send read and write frames for volumes on the older *mfi* parts, beyond
	  what QEMU's models test
	- start patrol reads or import foreign configurations; they are shown
	  and not acted on
	- see a volume made, or a disk added, while the machine runs; it is
	  logged, and used from the next boot
	- stop the wait for the controller's next event when the machine shuts
	  down

# SEE ALSO

*storage*(8), *diskhealth*(8), *radix-sysfs*(5), *pvscsi*(4)
