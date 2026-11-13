<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Checking the hardware

`hardware check` looks for hardware that is failing: a disk with sectors
it cannot read, memory that gives back a wrong value, a processor that
computes a wrong answer when it works hard, a fan that stopped, a battery
that is worn out, a network cable that has dropped to half duplex. It
runs while the system does, says what it found as it goes, and keeps the
result. The full reference is hardware(8); each warning and failure has a
page in the support center under its ID, such as HW-113.

Run it when a machine behaves oddly, when a disk or the memory is new or
second-hand, after a fall or a hot day, and before a machine is trusted
with data that matters. It changes nothing on any disk. It needs an
administrator, since only one may read the disks whole and start their
self-tests:

```
elevate hardware check
```

## The quick check

A quick check takes about three minutes, most of it a disk testing itself,
and says how long before it starts. Each check prints a line when it
ends, in the order they end:

```
Checking the hardware: a quick check, about 3 minutes. Ctrl-C stops it.
sensors  hardware monitors      skipped  the kernel shows no temperature or fan on this machine
battery  battery                skipped  this machine has no battery
disks    sda health             passed   Samsung SSD 860: PASSED, 31 °C, 1,204 hours on
memory   memory test            passed   1.0G tested by 4 CPUs; Test the memory in the start menu tests all of it
memory   memory errors          passed   none counted since the start
disks    sda self-test          passed   the short self-test ended without an error, at hour 1204
disks    sda read scan          passed   read 1.0G of the disk's 477G; none unreadable, none slow
cpu      load                   passed   4 CPUs ran 148,203 rounds in 60 seconds, and every answer agreed
cpu      temperature            passed   hottest is Package id 0 at 71.0 °C (high limit 100.0 °C)
cpu      machine checks         passed   none in the kernel's log since the start
network  enp1s0                 passed   link up at 1000 Mb/s, full duplex; no receive errors during the check
9 passed, 0 warnings, 0 FAILED, 2 skipped
```

Each line is one check, and each ends in one of four ways:

| Result | What it means |
| --- | --- |
| `passed` | The check found nothing wrong |
| `warning` | Something is wrong or about to be, and nothing has been lost yet |
| `FAILED` | The hardware is faulty: do not trust it with data |
| `skipped` | The check could not be made, and the line says why: this machine has no battery, a disk keeps no record of its health, a self-test was already running |

A warning or a failure has a second line, under it, with the ID of its
page and one sentence on what to do. The page says more: what the line
means, the steps in order and how to tell that they worked.

```
disks    sda read scan          FAILED   unreadable sectors: 40000-40001; 1.0G read
         HW-113: Back up what is on the disk now, and replace it.
```

The command ends with a status that a script can read: 0 when every check
that ran passed, 1 when one failed, 2 when none failed and one warned,
and 3 when it could not run, for instance because the person was not an
administrator, or because every check was skipped.

### What it looks at

| Area | What the check does |
| --- | --- |
| Memory | Reads the memory controller's count of the errors it found since the start, and tests part of the free memory: a quarter of it, at most 1 GiB |
| Disks | For each disk: what it says of its own health, its short self-test, and a read of 1 GiB of it, in 256 pieces spread over its whole size |
| CPU | Runs every processor at full load for a minute, with the same work on each, and every answer must be the same; reads the temperatures meanwhile, and counts the machine checks in the kernel's log |
| Sensors | The temperatures and fans of the hardware monitors, at rest, against the limits each monitor gives |
| Battery | How much each battery holds when full, against what it held new |
| Network | Each card's link: its speed and duplex, and the receive errors it counted during the check |

To check only some areas, name them: `elevate hardware check memory disks`.
`--disk sdb` checks one disk, `--time 30` runs the processors for 30
seconds instead of a minute.

A disk that fails a read is not given up on at once. The check reads the
failed piece again in halves, down to single sectors, and names the
first ten ranges of sectors that cannot be read. The kernel tries each
failed read several times first, so a disk with bad sectors takes longer.

The check leaves a disk's own test of itself to the disk: it starts the
test, asks how far it is every five seconds, and reads the result from the
disk's own log. If the person presses Ctrl-C, the check stops at once and
aborts the self-tests it started.

## The full check

```
elevate hardware check --full
```

A full check takes hours. The disks run their long self-tests, which take
from one to many hours a disk, and are read from end to end; the processors
run for ten minutes; and three quarters of the free memory is tested.
The machine is usable meanwhile and slower. It says how long it expects to
take, and asks before it starts; `--yes` starts it without asking, for a
script.

Run a full check when a quick one finds a warning, before a machine goes
to a new job, and when a disk is old. The self-tests of the disks run side
by side, so a machine with several disks takes about as long as its
slowest one.

## The memory test

A running system cannot test the memory it is using, which is the memory
of the kernel and of every program that runs, nor the memory the firmware
keeps for itself. The check tests a share of what is free. It writes
three patterns to it, each written and read back twice, so that a word that
changes between two reads is found as well: each word's own address,
which finds a fault in an address line; bytes of alternate bits, 0x55 and
then 0xAA, which flip every bit both ways; and random values.

A memory test that finds nothing, in a system that runs, is a good sign
and not a proof. To test all of the memory, choose *Test the memory* in
the start menu. It runs memtest86+ before Dukovo starts, so that every
address can be tested. memtest86+ is a separate program, free software
by its own authors; [memtest.org](https://memtest.org) says what it tests
and how to read its screen.

The check can also say of memory's faults without a test: the memory
controller counts the errors it corrects, and the ones it cannot. A
corrected error is a warning (HW-101), since memory that needs correcting
may be starting to fail; an uncorrected one means the memory gave wrong
data, and is a failure (HW-102). A machine that counts none, as a virtual
machine does not, gets a `skipped` line that says so.

## Seeing the last result

Each check that ends keeps its result in
`/system/state/hardware/last.json`, and writes one line to the log with
the status and the IDs of what it found. A check that Ctrl-C stopped does
neither, so that the last result is always of a whole check.

```
elevate hardware last
elevate log -s hardware
```

`hardware last` shows what the last check showed, with when it ended and
what release ran it. `hardware last --json` gives the file itself, and
`hardware check --json` gives the result in the same form, in place of
the lines as they end, for another program to read.

## What to do with a result

| ID | What it says | What to do |
| --- | --- | --- |
| HW-101 | The memory controller corrected errors | Replace the memory module if the count keeps rising |
| HW-102 | The memory gave wrong data | Back up and replace the module |
| HW-103 | The memory test read back a wrong value | Do not trust the machine's data; confirm with *Test the memory* |
| HW-111 | A disk says it is failing | Back up now, and replace the disk |
| HW-112 | A disk's self-test failed | Back up now, and replace the disk |
| HW-113 | Sectors that cannot be read | Back up now, and replace the disk |
| HW-114 | Reads were very slow, or failed and worked again | Check the cable and the port |
| HW-115 | Counts that come before a failure rose | Make sure it is backed up; plan to replace it |
| HW-121 | The kernel's log holds corrected machine-check errors | Check the cooling and settings |
| HW-122 | A processor gave a wrong answer under load | Undo an overclock; check power and cooling |
| HW-123 | A processor got too hot under load | Check the fans, vents and cooler |
| HW-131 | A temperature at rest is over its limit | Check the fans, vents and cooler |
| HW-132 | A fan stopped or is too slow | Check it is plugged in, and replace it |
| HW-141 | A battery holds under 80% of what it held new | Replace the battery |
| HW-152 | A network link is at half duplex | Check the cable and the switch port |
| HW-153 | A card counted receive errors during the check | Replace the cable; try another port |

Most of these say the same first thing: **back up what is on the disk, now**.
A disk that is failing may be readable today and not tomorrow, and a
check that reads it is no help to the files on it. The `diskhealth`
service watches the disks' own counts all the time and raises an alert
when they rise ("Metrics and early warnings" and "Alerts" say more); the
check is what to run when that alert comes, or to look before it does.

## When it does not work

| What you see | What to do |
| --- | --- |
| `hardware check needs an administrator` | Run it as `elevate hardware check` |
| `no check has run yet` from `hardware last` | Run `elevate hardware check` first |
| A line says `skipped` and why | It is not a fault: the machine has no such part, or the part keeps no record, or a self-test was already running; the line says which |
| Every check says `skipped` and the status is 3 | Nothing could be checked: the lines say why |
| It says `no area named` or `no disk named` | The areas are memory, disks, cpu, sensors, battery and network; `--disk` takes a name from `lsblk` |
| The check ends at once with `not run: a full check was not confirmed` | A full check asks; answer `y`, or give `--yes` |
| A disk's self-test line says it was aborted or interrupted | A reset or another program stopped it: run the check again |
| The disk scan takes long | A disk with bad sectors does, since each failed read is tried again several times; Ctrl-C stops it |
