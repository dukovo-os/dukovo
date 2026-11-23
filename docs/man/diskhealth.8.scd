diskhealth(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

diskhealth - what each disk says of its health

# SYNOPSIS

*diskhealth* [*-a* | *--json*] [_disk_...]

*diskhealth --watch*

# DESCRIPTION

diskhealth reads what each disk keeps of its own health, with
*radix-block*(2)'s *GET_HEALTH* (ADR 0157): a SATA disk's SMART
attributes and their thresholds, which Linux's *smartctl -A* shows, an
NVMe controller's SMART / Health Information log, or a SCSI or SAS disk's
log pages and its count of grown defects (ADR 0665). A SATA disk behind a
SCSI controller, as a hypervisor's, is read as a SATA disk. With no
_disk_, it
shows every disk the kernel has (_/kernel/block_, *radix-sysfs*(5)) in
a table: its kind, a verdict, its temperature, its hours powered on, the
share of an NVMe drive's rated endurance it has used, and its model. A
disk that keeps no such record, as QEMU's virtio and SCSI disks and USB
disks, shows dashes.

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
reliability degraded, it went read-only or its backup failed; for a
SCSI disk, its own warning that it will fail (an additional sense code of
5Dh in its informational exceptions page, which says why: hardware, media,
its controller, channel, servo, spindle or firmware). It is *WARNING* when
an attribute that tells of age is at or below its threshold, when sectors
were reallocated, wait to be or could not be read, when SMART is off, or
when an NVMe drive is too hot, has used its rated endurance or has had
media errors; for a SCSI disk, when it could not correct a read, write or
verify, has moved blocks to spares as they grew bad (its grown defects),
has used 100% of a solid state disk's endurance, or warns of its
temperature or a background test that failed (0Bh); and *PASSED*
otherwise. Each reason is given with *-a*.

Named disks, as _sda_ or _/devices/sda_, or *-a*, show each disk's
reasons and then its attributes, by ID, with their type, value, worst
value, threshold and raw value, the fields of its NVMe log, or what a SCSI
disk's log pages say, each on a line, those the disk keeps: its SMART trip
(*none*, or the code and what it means, as *5D/10, the drive predicts its
own failure, of hardware*), its temperature and the limit it gives, hours
powered on (from its background scan page, which counts minutes), start-stop
and load-unload cycles, the reads, writes and verifies it could not correct,
errors that are not the medium's, the share of a solid state disk's
endurance used, its grown defects, and its log of self-tests, the newest
first. A partition answers for its disk. What an attribute's raw value counts is
the disk maker's choice; the names are those most disks share. A name
the kernel does not list is refused (*DSK-101*, with its page in the
support center).

The disks behind a RAID controller (ADR 0664) are listed after the
others, as *raid*_n_*:*_bay_, for the controller's number and the disk's
bay: *raid0:bay4* is the disk in bay 4 of the first controller, and
*raid0:bay4-2* the one in bay 4 of its second backplane (*storage raid*
says the bays, *storage*(8)). A disk with no slot is not listed. A disk
that the controller passes through to the system is a disk of the
system, and is listed by its own name, as *sda*. The record of a member
disk is read through the controller, from
_/kernel/raid/n/health/id_, which holds the same 1032 bytes as
GET_HEALTH, so that its SATA, NVMe or SCSI record is read and judged as a
disk's of the system is; the model, serial number and firmware shown are
those the controller gives. Opening that file fails as a disk's
GET_HEALTH does: a disk in standby is not woken and is reported as
asleep, and a disk that keeps no record, as one behind a controller that
does not pass the request on, shows dashes. A member disk has no
self-test to start and no log of tests of its own to ask for, apart from
what a SCSI disk's log pages hold, and *--watch* keeps its live file and
alerts as for any disk.

# OPTIONS

*-a*, *--all*
	Show every disk's details, not the table.

*--json*
	Print what each disk says as one line of JSON (below), for the web
	console. It does not go with *-a*, as the JSON always has the
	attributes.

*--watch*
	Look at every disk now and every ten minutes, as smartd(8) does on
	Linux, and write a line of each disk at the first look and again
	whenever its verdict or reasons change. Spark runs it as the
	*diskhealth* service, which holds the disks and nothing else of
	_/devices_, *files.disks = "read"* (*service.toml*(5)), and puts the
	lines in the kernel log:

```
diskhealth: nvme0n1: PASSED, 50 °C, 0 hours on
diskhealth: sda: WARNING: 8 sectors were reallocated
```

	After each look the service keeps the disks' alerts (*alerts*(8)):
	*smart:*_serial_ while a disk's verdict is WARNING, as a warning, or
	FAILING, as critical; and the early warning *smart-rising:*_serial_
	for a week after a count rises from one look to the next
	(reallocated sectors, sectors waiting to be reallocated, offline
	uncorrectable sectors, an NVMe drive's media errors, a SCSI disk's
	grown defects and the errors it could not correct), and while an
	NVMe drive or solid state disk has used 90% or more of its rated
	endurance. Disks
	often fail within weeks of these counts starting to rise. A disk
	with no serial number is named by its device name.

	A SATA disk that the kernel says is in standby is not read, so that
	it is not woken: its live file keeps what the last look read and its
	alerts stay as they were, and the file says since when the disk
	sleeps. While a disk is asleep *metricsd*(8) adds no point of its
	temperature, since that was read earlier, and *hardware*(8) shows it
	as asleep. A look at a disk that is awake keeps its temperature and
	the temperatures at which it is too hot (*TEMPERATURES*).

# TEMPERATURES

A disk's warning and critical temperatures are its own where it says
them: an NVMe drive's, from its health log, a SATA disk's critical one,
from the threshold of its airflow temperature attribute (190) where that
counts down from 100 as the disk warms, and a SCSI disk's, from the
reference temperature of its temperature page. A limit the disk does not
give is the usual for its kind: a hard disk is warm at 50 °C and hot at
60 °C, a solid-state disk at 65 °C and 70 °C, and an NVMe drive at 70 °C
and 80 °C. The live file says which: _from_ is *disk* or *kind*.
*metricsd*(8) warns of a disk at its warning temperature, with the
disk's model in the title (*MET-108*), and *hardware sensors* shows
each disk's reading and limits.

# JSON

With *--json*, one line with an array, an object for each disk that keeps
a record of its health, in the order of the disks; a disk that keeps
none, as a virtio disk, is left out, and with it every disk is, so that
the array can be empty. A disk's object has these fields, in this
order:

*disk*
	The disk's name, as it was asked for or as _/kernel/block_ has it.

*model*, *serial*
	What the driver says of the disk, or *null*.

*verdict*
	*passed*, *warning* or *failing* (the table's verdict, in lower
	case).

*reasons*
	The reasons of the verdict, each a sentence; none for *passed*.

*hours*
	Hours powered on, or *null*.

*temperature_c*
	The temperature in degrees Celsius, or *null*.

*moved*, *waiting*, *offline*
	A SATA disk's counts of sectors moved to spares (attribute 5),
	sectors waiting to be moved (197) and sectors that could not be read
	offline (198). *null* for an NVMe drive, for a disk whose SMART is
	off, and for a disk that does not list the attribute. For a SCSI
	disk, *moved* is its grown defects, and the others are *null*.

*media_errors*, *used_percent*
	An NVMe drive's count of media and data integrity errors, and its
	share of its rated endurance used, in percent. *null* for a SATA
	disk. For a SCSI disk, *media_errors* is the reads, writes and
	verifies it could not correct, *null* if it counts none, and
	*used_percent* a solid state disk's share, *null* for a disk that
	has none.

*attributes*
	A SATA disk's SMART attributes, objects of *id*, *name* (the name the
	table gives it), *prefail* (whether it tells of failure, not of age),
	*value*, *worst*, *threshold* and *raw*, as the *-a* listing gives
	them. *null* for an NVMe or SCSI disk and for a disk whose SMART is
	off.

*self_tests*
	The last tests in the disk's own log (*hardware*(8) starts them), at
	most five, the newest first, one that runs first of all: objects of
	*kind* (*short*, *extended*, *conveyance* or *other*), *result*
	(*passed*, *failed*, *aborted* or *running*) and *at_hours*, the
	disk's hours powered on when it ran, *null* while it runs. A test
	cut short by a reset is *aborted*. A test that ended with a status
	this has no word for is left out. The array is *null* when the disk
	keeps no log or the log cannot be read, which is said on standard
	error. A SCSI disk's log is the one in its record, and is *null* for
	a disk with no self-test results page.

# THE LIVE FILES

Each time the service looks at a disk, it writes the disk's summary to
_/system/live/diskhealth/_*disk*_.json_, which anyone may read, so that
*storage*(8) can tell where a disk stands without opening the disk. The
file is one line with the object above without *attributes* and
*self_tests*, and with five fields more at its end:

*checked_ms*
	When the service looked, in milliseconds since 1970.

*rose*
	The early warning that stands, *{"at_ms": N, "title": T}*, with the
	time the rise was seen and the title of its alert, or *null*: a rise
	is kept for a week, as the alert is.

*kind*
	*hdd* for a SATA or SCSI disk that spins, *ssd* for one of flash
	(a SCSI disk that counts a solid state disk's wear is), or *nvme*.
	Missing from a file an older service wrote.

*asleep_ms*
	Since when the disk sleeps, in milliseconds since 1970, or *null*
	while it does not. Missing from a file an older service wrote.

*limits*
	*{"warning_c": N, "critical_c": N, "from": "disk"}*: the temperatures
	at which the disk is too hot, and whether they are the disk's own
	(*disk*) or the usual for its kind (*kind*); *null* when the disk
	gives no temperature. Missing from a file an older service wrote.

The file of a disk that goes, or that stops keeping a record, is
removed. A disk that cannot be read at a look keeps the file of the look
before.

# EXIT STATUS

The bits of smartctl's that apply: 0 if every disk passed or keeps no
record; 1 for a mistake in the command line and for a name the kernel
does not list (*DSK-101*); with 2 if a disk could not be read, 4 if one
warns and 8 if one is failing.

# FILES

_/kernel/raid/n/health/id_
	The health of a disk behind a RAID controller (above).

_/system/state/diskhealth/serial_
	What a disk counted at the service's last look, and its last rise,
	so that a rise across a restart is seen too.

_/system/live/diskhealth/_*disk*_.json_
	The service's summary of each disk (above).

# SEE ALSO

*spark*(8), *radix-sysfs*(5), *systools*(1), *scrub*(8), *alerts*(8),
*storage*(8), *hardware*(8), *metricsd*(8)
