hardware(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

hardware - check the memory, the disks, the processors, the sensors, the
battery and the network cards for faults, and say what the machine is and
has

# SYNOPSIS

*hardware check* [_area_...] [*--full*] [*--disk* _disk_] [*--time* _seconds_] [*--yes*] [*--progress*] [*--json*]

*hardware last* [*--json*]

*hardware history* [*--json*]

*hardware show* [*--json*]

*hardware sensors* [*--json*]

*hardware devices* [*--json*]

*hardware memtest* *--next-start*

*hardware memtest* *--cancel*

*hardware memtest* *--status* [*--json*]

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
it ended and which release ran it, as the check showed it. With no result
saved it says so on standard error and ends with status 3; with *--json*
it also prints *null* on standard output, so that a program reads "none"
as JSON.

*hardware history* lists the checks that were kept, the oldest first and
at most the newest 200: when each ended, whether it was a quick or a full
check, how many checks ended each way, and the IDs of what it found. With
*--json* it prints them as a list of the objects described under
*HISTORY*, which is the empty list when none was kept. It needs no
rights.

*hardware show* says what the machine is: its maker, model and the kind
of case, its processor with the cores and threads and the clock it is
rated at, the memory the kernel counts, and the firmware's vendor,
version and date. It needs no rights, and no serial number or ID of the
machine is read, so that what it prints can be shown or put in a report
as it is. What only SMBIOS's tables say of the memory, whether it
corrects errors, how many slots the board has and each module in them,
is told when the program holds the platform capability that reads those
tables whole (*hwinfo*(8)), as a command run with *elevate*(8) does; for
anyone else those three are _null_. See *WHAT THE MACHINE IS*.

*hardware sensors* says what the machine's temperatures and fans read:
each sensor of the hardware monitors, as *sensors*(1) reads it, each
thermal zone of the firmware, and each disk, as *diskhealth*(8) last saw
it. Each has the name of the series that *metricsd*(8) keeps its history
under, so that *metrics show* gives the points. A disk is never asked, so
that it is not woken: a disk that sleeps says since when, and is shown as
asleep. On a machine with no sensor and no zone the lists are empty, and
the text says so. It needs no rights. See *SENSORS*.

*hardware devices* lists every PCI function and USB device the kernel
found, as the Hardware page shows them: what it is called, its vendor and
device numbers, what it is to the system (a network card's name, a
disk's), the driver that took it where the system can tell, how it is
doing, and where it is. Bridges and hubs, which work and need nothing,
are marked hidden. No serial number is read. It needs no rights. See
*DEVICES*.

*hardware memtest* asks the kernel to test the free memory at the next
start only, before it starts the first program. *--next-start* asks, and
says about how long the test takes and how to restart; *--cancel* takes
the ask back; *--status* says whether a test is asked, and what the last
tests found. The first two need an administrator: run them with
*elevate*(8). The test takes about 4 minutes for each 16 GiB of free
memory, and stops after an hour. It tests what *hardware check memory*
cannot, all the memory that is free, and takes each part that reads back
wrong out of use until the next restart. It is asked for once: the next
start after it is the usual one. See *THE MEMORY TEST AT THE START*.

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
	that reads back wrong fails the check, with what it read. A running
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

*--progress*
	Print a line as each check ends that says how far the run is:
	_step N of M: area check result_, as _step 4 of 11: disks sda
	self-test passed_. _M_ is the number of checks the run counts on,
	which a run that gives more says as the number it is at. The lines
	come on standard output, with the lines of the checks; with
	*--json* they come on standard error, which leaves standard output
	to the JSON.

*--json*
	With *check*, print the result when the check ends, as the JSON
	that is saved, instead of a line for each check as it ends. See
	*RESULT*. With *last*, *history* and *show*, print JSON instead of
	text. See *HISTORY* and *WHAT THE MACHINE IS*.

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

# HISTORY

When a check ends and its result is kept, a line is added to
_/system/state/hardware/history_, which *hardware history* and the web
console read. The file holds the newest 200 lines, and is replaced whole,
as the result is. A line is a JSON object, and *hardware history --json*
prints them as a list:

```
{"at_ms":1794000000000,"full":false,"status":2,"passed":8,"warnings":1,"failed":0,"skipped":2,"ids":["HW-115"]}
```

_at_ms_ is when the check ended, in milliseconds since 1970, _full_
whether it was a full check and _status_ its exit status. _passed_,
_warnings_, _failed_ and _skipped_ are how many checks ended each way,
and _ids_ the IDs of what it found, each once.

# WHAT THE MACHINE IS

*hardware show --json* prints one JSON document. A member the machine
does not say is _null_.

```
{
  "format": 1,
  "maker": "Example Computers",
  "model": "Tower 200",
  "chassis": "Mini Tower",
  "processor": {
    "model": "Intel Core i5-6500",
    "cores": 4,
    "threads": 4,
    "base_mhz": 3200,
    "max_mhz": 3600
  },
  "memory": {
    "bytes": 16623468544,
    "ecc": null,
    "slots": null,
    "modules": null
  },
  "firmware": {
    "vendor": "Example BIOS Ltd.",
    "version": "1.20",
    "date": "07/12/2023"
  }
}
```

_format_ is the version of the document, 1. _memory.bytes_ is what the
kernel counts. For a program that holds the platform capability, _ecc_ is
whether the memory corrects errors, _slots_ how many slots the board has,
empty ones too, and _modules_ the modules that are in them, each with its
_slot_ as the board names it, its size in _bytes_, its _kind_, the _speed_
it runs at in MT/s, and its _maker_ and _part_ number when the firmware
says them:

```
"modules": [
  {"slot": "DIMM A1", "bytes": 8589934592, "kind": "DDR4", "speed": 2133,
   "maker": "Example Memory", "part": "EM8G2133"}
]
```

# SENSORS

*hardware sensors --json* prints one line of JSON. Where a member is not
told it is _null_.

```
{"format":1,"sensors":[{"chip":"k10temp-pci-00c3","label":"Tctl","kind":"temp","series":"temp{k10temp-pci-00c3/Tctl}","value":48.5,"high":null,"crit":95.0,"min":null,"alarm":false,"fault":false}],"zones":[{"zone":"acpitz","series":"temp{zone/acpitz}","value":27.8}],"disks":[{"disk":"sda","model":"Samsung SSD 860","kind":"ssd","temperature_c":31,"checked_ms":1794000000000,"asleep_ms":null,"warning_c":65,"critical_c":70,"limits_from":"kind","series":"temp{disk/sda}"}]}
```

_format_ is the version of the document, 1. A member of _sensors_ is a
temperature or a fan (_kind_ *temp* or *fan*) of the chip _chip_, with its
_label_, its _value_ in degrees Celsius or revolutions a minute, the
limits the sensor gives (_high_ and _crit_ for a temperature, _min_ for a
fan), whether the monitor's _alarm_ is set and whether it says the sensor
is faulty. _series_ is the name *metricsd*(8) keeps a temperature's
history under, and _null_ for a fan. A member of _zones_ is a thermal zone
of the firmware, with its _value_ in degrees Celsius.

A member of _disks_ is what the *diskhealth* service wrote of the disk
at its last look, _checked_ms_ milliseconds since 1970: its _kind_
(*hdd*, *ssd* or *nvme*), its _temperature_c_, and the temperatures at
which it is too hot, _warning_c_ and _critical_c_, with where they come
from, _limits_from_: *disk*, the disk's own, or *kind*, the usual for its
kind. A disk that sleeps has _asleep_ms_, the time since when, in
milliseconds since 1970, and what it said at its last look before it;
it is shown as asleep and not read. A disk that gives no temperature has
_null_ for it and for the limits.

The text has a line for each sensor, zone and disk, the name and what it
reads, its limits, and *ALARM* when the monitor's alarm is set:

```
k10temp-pci-00c3/Tctl  48.5 °C (critical 95.0 °C)
zone/acpitz            27.8 °C
sda                    31 °C  warning 65 °C, critical 70 °C (for a solid-state disk)
sdb                    asleep since 08:15, not read
```

# DEVICES

*hardware devices --json* prints one line of JSON, with the version of
the document, _format_ 1, and the list _devices_, the PCI functions first
and then the USB devices. Where the system does not say a member it is
_null_.

```
{"format":1,"devices":[{"group":"network","name":"Intel Corporation I211 Gigabit Network Connection","ids":"8086:1539","class":"Ethernet controller","system":["net0"],"driver":"radix-igb","driver_told":true,"state":"working","where":"PCI 0000:03:00.0","hidden":false}]}
```

*group*
	The part of the page: *network*, *storage*, *usb*, *display* or
	*other*.

*name*, *ids*, *class*
	What it is called, from the vendor's and the device's names (their
	numbers where the databases of *lspci*(8) and *lsusb*(8) do not know
	them), its vendor and device numbers, as _8086:1539_, which may be
	carried in a report and are never a serial number, and what kind of
	thing it is, as _Ethernet controller_, or _null_.

*system*
	What it is to the system: a network card's name, or the names of
	the disks of a storage controller. A disk is named on its controller
	only where the kernel's names leave no doubt.

*driver*, *driver_told*
	The driver that took it. The kernel does not list which driver uses a
	PCI function; what can be told is told: a network card says its
	driver, a SATA or NVMe controller's is the AHCI or the NVMe driver,
	and a USB interface has a link to its driver or none. *driver_told*
	is false where nothing is told, which is not the same as having
	none: then _driver_ is _null_ and the text says *driver not told*.
	With *driver_told* true and _driver_ _null_, none took it.

*state*
	*working* when its driver took it and it is in use, *no-link* for a
	network card whose link is down, *no-driver* for a USB interface that
	no driver took, or _null_ where nothing is told.

*where*
	Where it is: _PCI 0000:03:00.0_ or _USB 1-3.2_.

*hidden*
	True for bridges and hubs, which work and need nothing, so that the
	page folds them away. The text does not list them, and says how many
	there are.

The text lists the devices that are not hidden, under *Network*,
*Storage*, *USB*, *Display* and *Other*, a line for each: its name and
numbers, what it is to the system, its driver and state, and where it is.

# THE MEMORY TEST AT THE START

*hardware memtest --next-start* writes the file _/system/state/hardware/memtest-asked.json_
and makes the start menu take the next start as a test, the way it takes
a try of *changes bisect* (*changes*(1)): the start's own boot options
and *memtest=once*, as the start numbered 99 that *changes --list* names
a memory test at the start. It changes no slot and no update, and a test
that hangs or loses the power leaves the next start the usual one. The
menu has one such start at a time, so the command refuses while an update
waits to be tried, or while a search of *changes bisect* waits on a try
(*HW-105*), and *changes bisect* refuses while a test is asked for.
Asked twice, it is asked once. *--cancel* takes the file and the menu's
choice away while the menu has not taken it; with nothing asked it says
so and changes nothing.

The kernel tests the free memory in blocks, leaving 64 MiB for itself,
before the first program, says in the log how far it has come at each
tenth and how it ended, takes each page that has an error out of use
until the next restart, and keeps its result in _/kernel/boot/memtest_
(*radix-sysfs*(5)), which is there only when the test ran. Once the start
has been seen through (*sysupdate*(8) checks every start), *hardware*
keeps the result, clears what was asked, and tells *alertd*(8): errors
raise the critical alert *memory-test* (*HW-103*), a pass clears it, and
a test the menu took that never told its end, because the power went or
the machine stopped, is said once as a warning (*HW-104*). Both are of
the kind *hardware* (*alerts*(8)). A test that
ran, or did not finish, also has a line in *hardware history*. A test
that was asked for and not taken, because an update was tried first, is
kept as *not-taken*, and nothing is said.

*hardware memtest --status --json* prints one line of JSON:

```
{"format":1,"asked":{"at_ms":1794000000000,"by":1000,"by_name":"alice"},"last":{"at_ms":1794000600000,"start":12,"result":"passed","complete":true,"tested_bytes":16106127360,"total_bytes":16623468544,"errors":0,"bad":[],"retired":0,"seconds":241},"history":[]}
```

_asked_ is _null_ when no test is asked, and otherwise when it was asked,
in milliseconds since 1970, and by whom, _by_ the number of the user
*elevate* ran it for and _by_name_ the name, or the number again where
there is none. _last_ is _null_ when no test has run at a start, and
otherwise the last one, which _history_ begins with: the newest first,
at most the last ten. A test has _at_ms_, when its result was kept, the
number of the _start_ that kept it, its _result_ (*passed*, *errors*,
*unfinished*, or *not-taken*), whether it was _complete_, which is false
when it stopped after an hour, the _tested_bytes_ of the _total_bytes_ it
could take, the number of words that read back wrong, _errors_, the
ranges they were in, _bad_, as objects of _from_ and _to_, the first and
the last byte in physical addresses of hexadecimal, and _errors_ for the
range, the pages taken out of use, _retired_, and the _seconds_ it took.

The text says whether a test is asked, and where the last found errors:

```
$ hardware memtest --status
asked for the next start (2026-11-16 10:12)
last: 2026-11-15, start 12: passed, 15.2 of 16 GiB tested in 4 minutes
```

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
:  The memory test read back a wrong value, in a check or at the start
|  HW-104
:  memory
:  warning
:  The memory test at the start did not finish
|  HW-105
:  memory
:  refusal
:  The next start is taken already, so the memory test cannot be asked for, or a search cannot start
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
none is saved. *hardware history*, *hardware show*, *hardware sensors*
and *hardware devices* give 0, or 3 when the command line is wrong or
what they read cannot be read.

*hardware memtest*: 0 when it did what was asked, or when there was
nothing to take back; 1 when it was refused, because the person is not an
administrator or the next start is taken (*HW-105*); 3 for a mistake in
the command line.

# SIGNALS

Ctrl-C, or SIGTERM or SIGHUP, stops the check at once. It aborts the
disk self-tests it started, and leaves any that were running before.

# FILES

_/system/state/hardware/last.json_
	The result of the last check, as described under *RESULT*.

_/system/state/hardware/history_
	A line for each check that was kept, as described under *HISTORY*.

_/system/state/hardware/memtest-asked.json_
	That a memory test is asked for the next start, with when and by
	whom. It is there from *--next-start* until the start that is
	told of it, or *--cancel*, and anyone may read it.

_/system/state/hardware/memtest.json_
	The last memory test at a start and the ten before it, as
	*hardware memtest --status --json* prints them.

_/kernel/boot/memtest_, _/kernel/cmdline_
	The kernel's result of the test of this start, which is there only
	when it ran, and the start's boot options, which have *memtest=once*
	for it (*radix-sysfs*(5)).

_/system/live/diskhealth/_
	What *hardware sensors* reads of the disks (*diskhealth*(8)).

_/kernel/class/dmi/id_, _/kernel/cpus_
	What *hardware show* reads of the firmware and the processors.

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

A check that says how far it is on standard error, with its result alone
on standard output:

```
elevate hardware check memory cpu --json --progress > result.json
```

The checks that were kept, and what the machine is:

```
hardware history
hardware show
```

The temperatures, and what the machine's cards and disks are:

```
hardware sensors
hardware devices
```

The memory tested at the next start, and the restart that takes it:

```
elevate hardware memtest --next-start
elevate shutdown -r now
hardware memtest --status
```

# SEE ALSO

*diskhealth*(8), *sensors*(1), *nic*(8), *netcheck*(8), *dmesg*(1),
*elevate*(8), *hwinfo*(8), *lscpu*(1), *lspci*(8), *lsusb*(8),
*radix-block*(2), *radix-sysfs*(5), *log*(1), *metrics*(1), *metricsd*(8),
*changes*(1), *alertd*(8), *shutdown*(8), *systools*(1)
