storage(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

storage - show the disks and the filesystems on them and the RAID controllers, and grow a disk's last partition and its filesystem

# SYNOPSIS

*storage* [*show*] [*--json*]

*storage raid* [*events*] [*--json*]

*storage grow* _disk_

# DESCRIPTION

storage puts what the machine knows of its disks side by side (ADR 0561):
each disk with its partitions and what it says of its health, each Btrfs
volume with the disks it is made of and how it keeps its copies, and the
other filesystems mounted. It joins what *lsblk*(8), *blkid*(8),
*df*(1), *diskhealth*(8) and *scrub*(8) give, which the web console's
Storage page reads as JSON.

*show* runs as anyone. It reads what everyone may: the kernel's disks and
mounts, the space of each mount, the Btrfs volumes in _/kernel/fs/btrfs_
(*radix-sysfs*(5)), with the disks each is on, the live files of the
*diskhealth* service, and the kept results of *scrub*; and what *blkid*
finds on each disk and partition it may open, which root may and a
person may not. For one it may not open, what a mount from it says and
the Btrfs volume the kernel says it is on stand in: a partition of a
volume has the volume's filesystem, ID and label, and its GPT name, and
whatever is on a partition neither mounted nor in a volume, are null.
A disk the service has not looked at has no health, and a volume that
was never scrubbed has no scrub result: they are left null, as is
everything else the machine does not say, and do not stop the command.

*raid* runs as anyone too. It shows the RAID controllers of the machine
(Broadcom MegaRAID, Dell PERC; ADR 0664) from what the driver keeps in
_/kernel/raid_ (*radix-sysfs*(5)), which it refreshes from the controller
and never makes a reader wait for: each controller, its volumes, the disks
behind it by bay, and its battery. A machine with no controller is told
so, and the exit status is 0.

*grow* needs root, and gives a disk's last partition the free space after
it, and the Btrfs filesystem on that partition the room of the partition.

*storage --help* prints a summary. *--json* goes with *show* and *raid*,
and may come before the words or after them.

# COMMANDS

With no command, *storage* does *show*.

*show* [*--json*]
	The disks in the order of their names, each with its partitions, then
	the Btrfs volumes, then, in JSON only, the other filesystems. Without
	*--json* it prints a short table of the disks and one of the volumes.

*raid* [*--json*]
	Each controller, in the order of its number, with its model, its
	firmware, its cache and its temperature, and whether it works or is in
	fault; then its volumes, each with its level, size, the name the
	system has for it, and its state in words ("Working normally",
	"Degraded: one disk is missing", "Partly degraded", "Offline") and,
	under it, what runs on it: a check or an initialization with how far
	it is, and a disk that is rebuilding, as "Rebuilding Bay 3, 42%,
	about 1 h 10 min left". The time left is a guess from the percent
	done and the time the task has run, and is left out until the task has
	run long enough to guess from. A volume that writes straight to disk
	though it is set to write back, because of the battery, says so. Then
	the disks, a line each: bay, state, interface and media, size, model,
	temperature, what it is used for (a volume, a spare, or its name for
	the system when it is passed through) and a note when it is expected
	to fail or has medium errors; and last the battery: its state, its
	charge and its temperature. With *--json*, the document described
	under *JSON, RAID*.

*raid events*
	The last 50 events of each controller's log, the newest last: the
	time, in local time, its class (*info*, *warning*, *critical* and so
	on) and the controller's own description. An event the controller
	stamped with the time since it started, as it does before the machine
	has told it the time, has that time: "start +00:01:35". *--json* is
	refused: the events are in the document of *raid --json*.

*grow* _disk_
	Gives _disk_'s last partition, the one that ends last on the disk,
	the free space after it, and the Btrfs filesystem on it the room of
	the partition. It does this:

	- The command must run as root (*elevate storage grow* _disk_).
	- _disk_ must be a disk of _/kernel/block_, named as *storage show*
	  names it, not a partition and not a path (*DSK-101*, with its page in
	  the support center).
	- The last partition must hold a Btrfs filesystem. A swap area, another
	  filesystem or no filesystem is refused before anything is grown: *the
	  last partition of sda is swap: storage grows a Btrfs partition only*.
	- It runs *rdisk --grow* _disk_ (*rdisk*(8)): the partition keeps its
	  start, type, GUID and name, takes the free space after it, and the
	  kernel grows it in place, even in use.
	- Then, for a filesystem that is mounted, it runs *device resize*
	  _id_*:max* _mount_ (*device*(8)), with the device ID its disk has in
	  the filesystem, so that the filesystem uses the whole partition. It
	  does this when the partition had no room too, so that a grow that
	  stopped between its two steps is finished by running it again. A
	  filesystem that is not mounted is not grown: *storage grow* says so,
	  and does it when run again with the filesystem mounted.
	- It prints what each step did. A disk with no room after its last
	  partition is no failure: *sda has no room after its last partition*,
	  exit status 0.

	A filesystem is grown and never shrunk, so a grow loses nothing,
	cannot cut anyone off, and has no undo. It is not a trial.

# OUTPUT

```
$ storage show
DISK SIZE MODEL    HEALTH  ROOM
sda  1.8T WD20EFZX warning -
sdb  1.8T WD20EFZX passed  -
vda  256M -        -       64M

VOLUME MOUNT         SIZE USED FREE DEVICES   PROFILE STATE SCRUB
data   /volumes/data 1.8T 1.1T 540G sda1,sdb1 raid1   ok    ok
```

The first table has a line for each disk: its size, what the driver says
its model is, the verdict of the *diskhealth* service (*passed*,
*warning* or *failing*), and the room after its last partition, which
*storage grow* would use. A dash is a value the machine does not give.
The second table has a line for each Btrfs volume: its label, or its ID
for a volume that has none; where it is mounted; its size, the space
used and the space free; the devices that hold it; how its data is kept
(*raid1*, or *single/raid1* while a balance converts it); *ok*, or how
many of its disks are *missing* or *behind* (*radix-sysfs*(5)); and how
its last scrub ended. The partitions, with their types and filesystems,
are in the JSON, and in *lsblk*(8)'s tree.

# JSON

With *--json*, one line with an object of these fields, in this order:

*format*
	1. A reader that finds another number should stop.

*disks*
	An array with an object for each disk, in the order of the disks'
	names.

*volumes*
	An array with an object for each Btrfs volume mounted, in the order
	of their IDs.

*filesystems*
	An array with an object for each filesystem mounted from a disk or
	a partition that is not Btrfs, in the order the mounts were made.

A disk has these fields, in this order:

*name*
	The disk's name, as in _/devices/_*name*.

*model*, *serial*
	What the driver says, as *lsblk*(8) shows it, or *null* when it says
	nothing.

*size*
	Its size in bytes: 0 for a drive with no medium in it.

*removable*, *rotational*
	Whether its medium comes and goes, and whether it spins, as the
	kernel says (*radix-sysfs*(5)), or *null* when it does not say.

*bus*
	How it is connected, where its name says: *nvme*, *virtio*,
	*sd-card*, *floppy* or *optical*. A disk named *sd* may be on a SATA
	or a USB port, which the kernel does not say, so for it *bus* is
	*null*.

*partitions*
	An array with an object for each partition, in the order of their
	numbers (below).

*free_after*
	The bytes from the end of the last partition to the end of the disk,
	less the 33 sectors a GPT keeps at its end. It is an estimate from
	the kernel's sizes, which *grow* checks exactly. It is *null* for a
	disk with no partition.

*health*
	What the *diskhealth* service said at its last look, an object, or
	*null* when it has not looked at the disk, or the disk keeps no
	record (a virtio disk). Its fields are *verdict* (*passed*,
	*warning* or *failing*), *checked_ms* (when the service looked, in
	milliseconds since 1970), *hours* (powered on) and *temperature_c*,
	then *moved*, *waiting* and *offline* for a SATA disk (the sectors
	moved to spares, waiting to be moved, and not readable offline),
	*media_errors* and *used_percent* for an NVMe drive, each *null*
	where the disk does not say, and *rose*, the early warning that
	stands, *{"at_ms": N, "title": T}*, or *null* when none does
	(*diskhealth*(8)).

A partition has these fields, in this order:

*name*, *number*
	Its name, and its number in the disk's table.

*start*, *size*
	Where it begins on the disk, and its size, in bytes.

*type*
	Its type as *rdisk*(8) names it (*efi*, *swap*, *linux*), or, for a
	type *rdisk* has no name for, as the kernel gives it: a GUID, or
	an MBR type byte (*0xda*). *null* when the kernel gives none.

*gpt_name*
	Its name in a GPT, or *null* where the table has none or the disk may
	not be opened.

*filesystem*, *label*, *uuid*
	What *blkid*(8) finds on it. When the partition may not be opened,
	*filesystem* and *label* are those its mount gives, and for a
	partition of a Btrfs volume the kernel names, the volume's
	filesystem, label and ID; else *null*. *null* for a partition that
	holds none.

*mounts*
	Where it is mounted, in the order the mounts were made. A Btrfs
	filesystem of several disks is mounted from one of them, so the
	others have none: its volume has the mounts.

A volume has these fields, in this order:

*uuid*, *label*
	Its ID, as in _/kernel/fs/btrfs_, and its label, or *null*.

*mount*
	Where it is mounted: the first mount under _/volumes_, else the first
	mount there is, of the devices that hold it. *null* when none is.

*size*, *used*, *free*
	In bytes, from *statvfs*(3) of *mount*: *used* is the size less the
	blocks free, and *free* the blocks a person may use. *null* with
	no mount.

*devices*
	The disks and partitions it is on, as the kernel says them, or, from
	a kernel that does not, those whose filesystem has this ID as *blkid*
	finds it: a Btrfs filesystem has one for each of its disks.

*disks*, *missing*, *behind*
	How many disks the volume has, how many of them are missing, and how
	many missed changes since a scrub (*radix-sysfs*(5)), or *null* when
	the kernel's file cannot be read.

*profiles*
	How its block groups keep their copies, an object with a list for
	each of *data*, *metadata* and *system*, whose words are *single*,
	*dup*, *raid0*, *raid1*, *raid1c3*, *raid1c4*, *raid10*, *raid5* or
	*raid6*. A list has two words while a balance converts the kind from
	one to the other. *null* when the kernel's *profiles* file cannot be
	read.

*holds*
	Which of _/users_, _/shared_ and _/applications_, in that order, are
	mounted from one of its devices.

*scrub*
	What its last scrub kept (*scrub*(8)): *{"at_ms": N, "seconds": N,
	"bytes": N, "errors": N, "fixed": N, "unfixable": N, "result": R}*,
	where *result* is *ok*, *errors* or *stopped*, or *null* when none
	is kept.

A filesystem has these fields, in this order: *mount*, *type* (the
*filesystem* of its device), *device* (the disk or partition it is on),
and *size*, *used* and *free* in bytes, as for a volume, or *null*.

# JSON, RAID

*storage raid --json* is one line with an object whose first field is
*format*, 1, and then *controllers*, an array with an object for each
controller, in the order of their numbers; an empty array on a machine
with none. The web console's *storage.raid* route gives the same. Each
controller has these fields, in this order:

*n*
	The controller's number, 0 first, in PCI order.

*controller*
	An object: *model*, *serial*, *package* (the firmware package),
	*firmware*, *interface* (*mfi* or *fusion*), *chip*, *pci* (its PCI
	address), *numa*, *cache* (bytes), *mode* (*raid*, *hba* or *mixed*),
	*battery* (*none*, *battery* or *cachevault*), *temperature* (degrees
	Celsius), *state* (*operational* or *fault*), *fault* (the
	firmware's code when it is in fault), *volumes*, *degraded*,
	*offline*, *disks*, *critical* and *failed* (counts) and *commands*.

*volumes*
	An array with an object for each volume: *target*, *name*, *level*
	(*raid0*, *raid1*, *raid5*, *raid6*, *raid10* and the others the
	controller makes), *size* and *stripe* (bytes), *state* (*optimal*,
	*partly-degraded*, *degraded* or *offline*), *write* and
	*default_write* (*back* or *through*: the policy now, and the one set
	for it), *read*, *disk_cache*, *disks* (the device IDs of its member
	disks, in the order of its spans), *os* (its name as a disk of the
	system), *task* (*none*, *check*, *init*, *background-init* or
	*reconstruct*), *percent* and *elapsed* (seconds) of the task.

*disks*
	An array with an object for each physical disk, in the order of
	their device IDs: *id*, *enclosure* (its device ID), *index*, *slot*,
	*bay*, *state* (*online*, *failed*, *rebuilding*, *hot-spare*,
	*jbod* and the others), *interface*, *media*, *vendor*, *model*,
	*serial*, *revision*, *size* and *raw_size* (bytes), *media_errors*,
	*other_errors*, *predictive*, *smart_alert* (a boolean), *temperature*,
	*volume* (the target of the volume it is in), *spare* (*global* or
	*dedicated*), *os* (its name as a disk of the system, when it is
	passed through), *task* (*none*, *rebuild*, *patrol*, *clear* or
	*copyback*), *percent* and *elapsed*. *bay* is "Bay 4" from the slot
	when the controller's disks are in one enclosure, which is the
	machine's backplane; "Bay 4, backplane 2" when they are in more; and
	"Slot 4" for a disk attached directly.

*battery*
	An object, or *null* when the controller has none: *type*, *state*
	(*optimal*, *charging*, *discharging*, *learning*, *degraded*,
	*failed* or *missing*), *charge* (percent), *temperature*, *voltage*
	(millivolts), *current* (milliamps, negative while it discharges),
	*learn* (*none*, *active*, *requested*, *failed* or *timed-out*, the
	test of its capacity), *write_through* (a boolean: a volume set to
	write back writes through now for the battery's sake) and *flags*
	(an array of the status bits that are set).

*events*
	The newest 50 events, the oldest first: *seq*, *time* (RFC 3339 in
	UTC, or *null*), *boot_relative* (the seconds since the controller
	started, for an event that has no *time*), *class*, *locale* (an
	array), *code*, *volume*, *disk*, *enclosure*, *slot*, *bay*,
	*percent* and *text*, the controller's description.

A value the controller does not give is *null*; sizes are bytes.

# FILES

_/kernel/raid/n/_
	What the driver keeps of each controller: _controller_, _volumes_,
	_disks_, _battery_ and _events_ (*radix-sysfs*(5)).

_/kernel/block_, _/processes/mounts_, _/kernel/fs/btrfs_
	The disks and partitions, the mounts, and the Btrfs volumes
	(*radix-sysfs*(5), *radix-proc*(5)).

_/system/live/diskhealth/_*disk*_.json_
	The *diskhealth* service's summary of each disk.

_/system/state/scrub/_*uuid*_.json_
	The result of each filesystem's last scrub.

# EXIT STATUS

0 when the machine was shown, even with no disk or no RAID controller, and when a disk was
grown or has no room to grow. 1 for a disk the machine does not have
(*DSK-101*), for a last partition that is not Btrfs, for a disk with no
partition, when *grow* does not run as root, and when a step of *grow*
fails, with what *rdisk* or *device* says; 1 for a mistake in the command
line, which also prints its usage.

# SEE ALSO

*lsblk*(8), *blkid*(8), *df*(1), *rdisk*(8), *device*(8), *diskhealth*(8),
*scrub*(8), *logtrim*(8), *hardware*(8), *radix-sysfs*(5), *elevate*(8), *systools*(1)
