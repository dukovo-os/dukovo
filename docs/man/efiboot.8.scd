efiboot(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

efiboot, efibootmgr - list and change the firmware's boot entries

# SYNOPSIS

*efiboot* [*-v*]

*efiboot* *-c* | *-C* [*-d* _disk_] [*-p* _part_] [*-l* _loader_] [*-L* _label_] [*-b* _XXXX_]

*efiboot* *-b* _XXXX_ *-B* | *-a* | *-A*

*efiboot* [*-o* _XXXX_,_YYYY_,... | *-O*] [*-n* _XXXX_ | *-N*] [*-t* _seconds_ | *-T*] [*-q*]

# DESCRIPTION

efiboot lists the entries of a UEFI firmware's boot manager, the systems
and programs it starts, and makes, deletes and orders them, with the
options of Linux's *efibootmgr*, by whose name it runs too (ADR 0313).
The installer makes Dukovo's entry the same way when it installs Dukovo
next to Windows.

The entries are the firmware's variables: *Boot*_XXXX_, each a
description and the path of what to start, numbered in hexadecimal;
*BootOrder*, the numbers in the order the firmware tries them;
*BootNext*, an entry to start at the next start alone, before the order;
*BootCurrent*, the one the firmware started this time; and *Timeout*,
the seconds its menu waits. The kernel reads and writes them through the
firmware's runtime services, for a program with the platform capability
(*radix-platform*(2)), which an administrator's programs have: run
efiboot with *sudo*(8) to change anything.

With no options it lists them, as efibootmgr does:

```
BootCurrent: 0003
Timeout: 3 seconds
BootOrder: 0003,0000,0001
Boot0000* Windows Boot Manager
Boot0001* UEFI: USB stick
Boot0003* Dukovo
```

An asterisk marks an active entry, which the firmware may start. After a
change it lists them again, unless *-q*.

A new entry names its partition by its number, its first sector and
length, and its GPT GUID (or, on a disk with an MBR, the disk's
signature), then the file on it to start. The firmware finds the
partition by those wherever the disk is attached, as it does for the
entries Windows and Linux make.

The kernel writes only the boot manager's variables: *Boot*_XXXX_,
*BootOrder*, *BootNext*, *Timeout*, *Driver*_XXXX_, *DriverOrder*,
*SysPrep*_XXXX_, *SysPrepOrder* and *OsIndications*. It refuses a change
that would leave the firmware under 5 KiB of room for its variables, as
Linux does: some firmware has failed to start with its store full.

# OPTIONS

*-c*, *--create*
	Make an entry and put it first in the order.

*-C*, *--create-only*
	Make an entry and leave the order as it is: *-n* starts it once.

*-d*, *--disk* _disk_
	The disk of the new entry's partition, as named in _/devices_:
	*sda*, *nvme0n1*. *sda* if not given.

*-p*, *--part* _part_
	The partition's number on it: 1 if not given.

*-l*, *--loader* _loader_
	The file the entry starts, from the partition's root, with
	backslashes or slashes: *\\EFI\\BOOT\\BOOTX64.EFI* if not given, where
	*rgrub-setup*(8) puts GRUB.

*-L*, *--label* _label_
	The description the firmware's menu shows: *Dukovo* if not given.

*-b*, *--bootnum* _XXXX_
	The entry the other options are about, by its hexadecimal number.
	With *-c* or *-C*, the number to make it with; otherwise the first
	free one.

*-B*, *--delete-bootnum*
	Delete entry *-b*, and take it out of the order.

*-a*, *--active*; *-A*, *--inactive*
	Let the firmware start entry *-b*, or not.

*-o*, *--bootorder* _XXXX_,_YYYY_,...
	Set the order. Every entry in it must exist.

*-O*, *--delete-bootorder*
	Delete the order.

*-n*, *--bootnext* _XXXX_
	Start entry _XXXX_ at the next start alone; the firmware forgets it
	once it has.

*-N*, *--delete-bootnext*
	Forget *-n*.

*-t*, *--timeout* _seconds_
	Set the seconds the firmware's menu waits.

*-T*, *--delete-timeout*
	Delete the time, which leaves it to the firmware.

*-v*, *--verbose*
	Show each entry's path, in UEFI's text form:
	*HD(1,GPT,*_guid_*,0x800,0x100000)/File(\\EFI\\BOOT\\BOOTX64.EFI)*.

*-q*, *--quiet*
	List nothing after a change.

*-V*, *--version*; *-h*, *--help*
	Say the version, or how to use it.

# DIFFERENCES FROM LINUX

- It lists an entry's path only with *-v*, as efibootmgr before
  version 18 did.
- *-e*, *-g*, *-u*, *-w*, *-@* and the *Driver*_XXXX_ and *SysPrep*_XXXX_
  options are not taken.
- *-o* refuses entries that do not exist, rather than warning.
- The kernel writes only the boot manager's variables, where Linux lets
  root write any it unlocks first.

# EXIT STATUS

0
	The entries are as asked, or were listed.

1
	A change was refused, the firmware failed, the machine started
	without UEFI, or a mistake in the command line.

# SEE ALSO

*rgrub-setup*(8), *rdisk*(8), *dukovo-install*(8), *radix-platform*(2)
