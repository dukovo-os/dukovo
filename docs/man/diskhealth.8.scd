diskhealth(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

diskhealth - what each disk says of its health

# SYNOPSIS

*diskhealth* [*-a*] [_disk_...]

*diskhealth --watch*

# DESCRIPTION

diskhealth reads what each disk keeps of its own health, with
*radix-block*(2)'s *GET_HEALTH* (ADR 0157): a SATA disk's SMART
attributes and their thresholds, which Linux's *smartctl -A* shows, or
an NVMe controller's SMART / Health Information log. With no _disk_, it
shows every disk the kernel has (_/kernel/block_, *radix-sysfs*(5)) in
a table: its kind, a verdict, its temperature, its hours powered on, the
share of an NVMe drive's rated endurance it has used, and its model. A
virtio disk keeps no such record, and shows dashes.

```
$ diskhealth
DISK       TYPE  HEALTH     TEMP    HOURS  USED  MODEL
nvme0n1    NVMe  PASSED    50 °C        0    0%  QEMU NVMe Ctrl
sda        SATA  PASSED    31 °C        1     -  QEMU HARDDISK
vda        -     -             -        -     -
```

The verdict is *FAILING* when the disk says it is about to fail: for a
SATA disk, an attribute that warns of failure (pre-fail) at or below
its threshold, as the disk's own SMART RETURN STATUS would say; for an
NVMe drive, a critical warning that its spare capacity is short, its
reliability degraded, it went read-only or its backup failed. It is
*WARNING* when an attribute that tells of age is at or below its
threshold, when sectors were reallocated, wait to be or could not be
read, when SMART is off, or when an NVMe drive is too hot, has used its
rated endurance or has had media errors; and *PASSED* otherwise. Each
reason is given with *-a*.

Named disks, as _sda_ or _/devices/sda_, or *-a*, show each disk's
reasons and then its attributes, by ID, with their type, value, worst
value, threshold and raw value, or the fields of its NVMe log. A
partition answers for its disk. What an attribute's raw value counts is
the disk maker's choice; the names are those most disks share.

# OPTIONS

*-a*, *--all*
	Show every disk's details, not the table.

*--watch*
	Look at every disk now and every half hour, as smartd(8) does on
	Linux, and write a line of each disk at the first look and again
	whenever its verdict or reasons change. Spark runs it as the
	*diskhealth* service, which puts the lines in the kernel log:

```
diskhealth: nvme0n1: PASSED, 50 °C, 0 hours on
diskhealth: sda: WARNING: 8 sectors were reallocated
```

	After each look the service keeps the disks' alerts (*alerts*(8)):
	*smart:*_serial_ while a disk's verdict is WARNING, as a warning, or
	FAILING, as critical; and the early warning *smart-rising:*_serial_
	for a week after a count rises from one look to the next
	(reallocated sectors, sectors waiting to be reallocated, offline
	uncorrectable sectors, an NVMe drive's media errors), and while an
	NVMe drive has used 90% or more of its rated endurance. Disks
	often fail within weeks of these counts starting to rise. A disk
	with no serial number is named by its device name.

# EXIT STATUS

The bits of smartctl's that apply: 0 if every disk passed or keeps no
record; 1 for a mistake in the command line; with 2 if a disk could not
be read, 4 if one warns and 8 if one is failing.

# FILES

_/system/state/diskhealth/serial_
	What a disk counted at the service's last look, and its last rise,
	so that a rise across a restart is seen too.

# SEE ALSO

*spark*(8), *radix-sysfs*(5), *systools*(1), *scrub*(8), *alerts*(8)
