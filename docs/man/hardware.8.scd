hardware(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

hardware - check the memory, the disks, the processors, the sensors, the
battery and the network cards for faults

# SYNOPSIS

*hardware check* [_area_...] [*--full*] [*--disk* _disk_] [*--time* _seconds_] [*--yes*] [*--json*]

*hardware last* [*--json*]

# DESCRIPTION

*hardware check* looks for hardware that is failing, while the system
runs, and says for each thing it looked at whether it passed, gave a
warning, failed, or could not be looked at. It needs an administrator:
run it with *elevate*(8), which a person who is not one is told, in one
line, with status 3.

A quick check takes about three minutes, most of it a disk's own
self-test. *--full* takes hours: a disk's long self-test, a read of every
disk from end to end, ten minutes of load and most of the free memory
tested. It says how long it will take and asks before it starts, unless
*--yes* is given. Nothing is written to any disk, and the system stays
usable, slower while the memory is tested and the processors are loaded.

Each check prints a line when it ends: the area, the check, how it ended
and a detail. A warning or a failure has a second line with its ID, which
has a page of its own in the support center, and one sentence on what to
do:

```
$ elevate hardware check
Checking the hardware: a quick check, about 3 minutes. Ctrl-C stops it.
sensors  hardware monitors      skipped  the kernel shows no temperature or fan on this machine
battery  battery                skipped  this machine has no battery
disks    sda health             passed   Samsung SSD 860: PASSED, 31 °C, 1,204 hours on
memory   memory test            passed   1.0G tested by 4 CPUs; Test the memory in the start menu tests all of it
memory   memory errors          passed   none counted since the start
disks    sda self-test          passed   the short self-test ended without an error, at hour 1204
disks    sda read scan          FAILED   unreadable sectors: 40000-40001; 1.0G read
         HW-113: Back up what is on the disk now, and replace it.
cpu      load                   passed   4 CPUs ran 148,203 rounds in 60 seconds, and every answer agreed
cpu      temperature            passed   hottest is Package id 0 at 71.0 °C (high limit 100.0 °C)
cpu      machine checks         passed   none in the kernel's log since the start
network  enp1s0                 passed   link up at 1000 Mb/s, full duplex; no receive errors during the check
8 passed, 0 warnings, 1 FAILED, 2 skipped
```

A check that was skipped did not run, and says why; it is not a fault.
Disks and the processors' load run side by side, so the lines come in the
order the checks end, and the saved result has them by area.

*hardware last* shows the result of the last check that ended, with when
it ended and which release ran it, as the check showed it.

# AREAS

With no _area_, every one is checked. Name one or more to check only
those: *memory*, *disks*, *cpu*, *sensors*, *battery*, *network*.

*memory*
	The memory controller's counts of the errors it found since the
	start (the files _ce_count_ and _ue_count_ of the first memory
	controller in _/kernel/devices/system/edac_, *radix-sysfs*(5)), and
	a test of part of the free memory. The test takes anonymous memory,
	a quarter of what is available and at most 1 GiB in a quick check,
	three quarters in a full one, and has one thread for each CPU test
	a slice of it. It writes three kinds of pattern, each written and
	then read back twice, so that a word that changes between two reads
	is found as well: each word's own address, moving inversions of
	0x55 and 0xAA bytes, and random values from a seed. The first word
	that reads back wrong fails the check, with its address. A running
	system cannot test the memory it uses itself, nor the memory of
	other programs, so the line says that *Test the memory* in the start
	menu tests all of it.

*disks*
	Each whole disk that is not a drive for media that comes and goes,
	such as a CD drive, or the disk *--disk* names. Its health, as
	*diskhealth*(8) judges it, including its early warning; its own
	self-test, the short one, or the extended one in a full check, which
	*radix-block*(2) starts and which the check waits for, asking the
	disk how far it is every 5 seconds and giving up after three times
	what the disk says it takes and five minutes; and a read scan.
	The scan reads the disk through its device in pieces of 4 MiB: 256
	pieces spread evenly over the disk in a quick check, all of it in a
	full one. A piece that cannot be read is read again in halves, down
	to the sectors that cannot be, and the check names the first 10
	ranges of sectors, as _40000-40001_. A read that takes over 2
	seconds, or that failed and worked when tried again, is a warning.
	The disks are checked side by side, and each disk's checks one after
	the other. A disk whose self-test is already running is left alone.

*cpu*
	One thread for each CPU, kept on it, repeats the same work for
	*--time* seconds: a chain of SHA-256 hashes and a product of two
	floating-point matrices, folded into one number. The work gives the
	same number on every CPU, so a CPU that computes wrongly under load
	gives another, and every answer must be the first one given. While
	they run, the temperatures of the processors' hardware monitors
	(*coretemp*, *k10temp*, *zenpower* and *cpu_thermal*) are read every
	2 seconds, and the highest of each is judged by the monitor's own
	limits: a failure at the critical limit, a warning within 5 °C of
	the high limit. The kernel's log is read before and after for the
	corrected machine-check errors it holds.

*sensors*
	Each hardware monitor in _/kernel/class/hwmon_, read at rest, before
	the load of the check begins, as *sensors*(1) reads it. A
	temperature at its critical limit is a failure, and over its high
	limit a warning, when the monitor gives those limits. A fan that
	stopped or turns under its minimum, or has its alarm or fault set,
	fails. A fan that reads 0 RPM and gives no minimum is not judged,
	since the motherboard's headers with no fan on them read so.

*battery*
	How much each battery holds when full, against what it held new
	(_energy_full_ against _energy_full_design_, or the charge pair, in
	_/kernel/class/power_supply_). Under 80% is a warning. A machine
	with no battery skips the check.

*network*
	Each network card that has a wire, as *nic*(8) reads it: whether it
	has a link, at what speed and duplex, and the receive errors it
	counted while the check ran, the counts at its start against the
	counts at its end. Half duplex is a warning, and so are receive
	errors that rose. A card with no link is skipped: *netcheck*(8) looks
	at what is above it.

# OPTIONS

*--full*
	Check thoroughly, which takes hours; see *DESCRIPTION*.

*--disk* _disk_
	Check only this disk, named as _sda_ or _/devices/sda_, any whole
	disk the kernel has. Only for the *disks* area.

*--time* _seconds_
	How long the processors run: 60 seconds, or 600 in a full check.

*-y*, *--yes*
	Start a full check without asking.

*--json*
	Print the result when the check ends, as the JSON that is saved,
	instead of a line for each check as it ends. See *RESULT*.

# RESULT

When a check ends, its result is kept in _/system/state/hardware/last.json_,
which *hardware last* and the web console read. It is replaced whole, by a
new file renamed over the old, so that a reader finds the last result or
the one before, never half of one. A check that Ctrl-C stopped keeps no
result, so the file still has the last whole one.

```
{
  "time": 1794000000,
  "version": "0.16.0",
  "full": false,
  "status": 1,
  "checks": [
    {
      "area": "disks",
      "check": "sda read scan",
      "result": "failed",
      "detail": "unreadable sectors: 40000-40001; 1.0G read",
      "id": "HW-113",
      "advice": "Back up what is on the disk now, and replace it."
    }
  ]
}
```

_time_ is when the check ended, in seconds since 1970, _version_ is the
release that ran it and _status_ the exit status. A check has an _area_
and a _check_ name, a _result_ of *passed*, *warning*, *failed* or
*skipped*, a _detail_, and for a warning or a failure its _id_ and
_advice_.

Each finished check also writes one line to the log, at the level of its
status (error for 1, warning for 2, information otherwise), with how many
checks ended each way and the IDs it found:

```
hardware: quick check ended with status 1: 8 passed, 0 warnings, 1 FAILED, 2 skipped (HW-113)
```

*log -s hardware* shows them (*log*(1)).

# IDS

Each warning and failure has an ID, and a page with that ID in the
support center says what it means and what to do.

[[ *ID*
:- *Area*
:- *Result*
:- *When*
|  HW-101
:  memory
:  warning
:  The memory controller counted corrected errors since the start
|  HW-102
:  memory
:  failure
:  It counted uncorrected errors
|  HW-103
:  memory
:  failure
:  The memory test read back a wrong value
|  HW-111
:  disks
:  failure
:  The disk says it is failing
|  HW-112
:  disks
:  failure
:  The disk's self-test failed
|  HW-113
:  disks
:  failure
:  The read scan found sectors that cannot be read
|  HW-114
:  disks
:  warning
:  Reads were very slow, or failed and worked when tried again
|  HW-115
:  disks
:  warning
:  Counts that come before a failure rose, or the disk warns
|  HW-121
:  cpu
:  warning
:  The kernel's log holds corrected machine-check errors
|  HW-122
:  cpu
:  failure
:  A CPU gave a wrong result under load
|  HW-123
:  cpu
:  failure or warning
:  A processor was at its critical limit, or near its high limit, under load
|  HW-131
:  sensors
:  failure or warning
:  A temperature is at its critical limit, or over its high limit, at rest
|  HW-132
:  sensors
:  failure
:  A fan stopped, or turns under its minimum
|  HW-141
:  battery
:  warning
:  The battery holds under 80% of its design capacity
|  HW-152
:  network
:  warning
:  A link at half duplex
|  HW-153
:  network
:  warning
:  Receive errors were counted during the check

# EXIT STATUS

*hardware check*:

0
	Every check that ran passed, and some did.

1
	A check failed.

2
	No check failed, and one gave a warning.

3
	It could not run: a mistake in the command line, no disk of that
	name, the person was not an administrator, a full check was not
	confirmed, Ctrl-C stopped it before it found anything, or no check
	could be made (every one was skipped).

*hardware last* gives the status the saved check ended with, or 3 when
none is saved.

# SIGNALS

Ctrl-C, or SIGTERM or SIGHUP, stops the check at once. It aborts the
disk self-tests it started, and leaves any that were running before.

# FILES

_/system/state/hardware/last.json_
	The result of the last check, as described under *RESULT*.

_/kernel/devices/system/edac_, _/kernel/class/hwmon_,
_/kernel/class/power_supply_, _/kernel/class/net_, _/kernel/block_
	What the checks read (*radix-sysfs*(5)).

_/devices/kmsg_
	The kernel's log, which the check reads for machine checks and
	writes its line to (*dmesg*(1)).

# EXAMPLES

A quick check:

```
elevate hardware check
```

Only the disks, and only one of them, with the processors left alone:

```
elevate hardware check disks --disk sdb
```

A full check, which asks first, and with its result to read later:

```
elevate hardware check --full
elevate hardware last
```

The result for another program:

```
elevate hardware check --json > result.json
```

# SEE ALSO

*diskhealth*(8), *sensors*(1), *nic*(8), *netcheck*(8), *dmesg*(1),
*elevate*(8), *radix-block*(2), *radix-sysfs*(5), *log*(1), *systools*(1)
