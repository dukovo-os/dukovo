changes(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

changes - what changed between two starts of the computer

# SYNOPSIS

*changes* [*--failed* | *--between* _start_ _start_ | *--list*]++
*changes* (*--list* | *--failed*) *--json*++
*changes bisect* [*--confirm*] [*--yes*] [*--later*]++
*changes bisect* (*--status* [*--json*] | *--stop*)

# DESCRIPTION

changes says what changed since the last good start (ADR 0500): the
version of Dukovo, the kernel, the firmware set and its CPU microcode,
the boot options, the network cards and the root's partitions, and the
apps installed for everyone. Each start of the computer has a record,
which the health check begins as it runs and finishes with how the
start ended: *good*, when it was seen through or an update proved
itself; *failed*, with what failed and its error ID; or *unfinished*,
when the health check never finished.

Each change says when what is new was installed, where that is known,
and when a start first used it. A change installed days before it was
first used points at the restart that brought it in:

```
$ changes --failed
Start 42 on 2026-11-11 21:04, Dukovo 0.15.2, slot B: failed: the CPU microcode of firmware 2026.11.4 has data that failed its checks, so none of it was used (FW-101)
From the last good start, 41 on 2026-11-11 08:02, Dukovo 0.15.2, to start 42:
  firmware set: 2026.10.1 to 2026.11.4; installed 2026-11-04 10:12; first used 2026-11-11 21:04 (start 42)
  CPU microcode: 3f2a9c1d0b7e6a55 to 81c0e2d4f9a3b716; first used 2026-11-11 21:04 (start 42)
```

With no option, changes compares this start with the last good start
before it. The error ID leads to a page of the support center that
says what it means and what to do.

When several changes came in between, *changes bisect* finds which of
them broke the start (below). The starts it makes for that, its tries,
are recorded like any start but are never the last good one, and
*--list* marks each.

# OPTIONS

*--failed*
	Compare the newest start that failed with the last good start
	before it.

*--between* _start_ _start_
	Compare two starts, by number.

*--list*
	List the starts kept, newest first, each with how it ended. A try
	of *changes bisect* says so: "slot A, try 2 of changes bisect:
	good".

*--json*
	With *--list*, *--failed* or *bisect --status*, print JSON for a
	program to read, as described under JSON below. Alone, or with
	*--between*, it is refused with the usage line.

# JSON

*--json* writes one line, and exits 0 as the text does, whoever asks:
the records are readable by everyone, as the text is. A field that the
records do not hold is *null*, and none is made up. Times are Unix
milliseconds.

*--list --json* is an array with an object for each start kept, newest
first, and an empty array when none is kept:

```
[{"start": 41, "at_ms": 1763100000000, "release": "0.16.0-dev.412",
  "slot": "a", "result": "good", "check": null, "ready_ms": 31200,
  "changes": [{"what": "org.nginx.nginx", "version": "1.30.6",
               "at_ms": 1763000000000}]}]
```

_start_ is the start's number, _at_ms_ when the health check began it,
_release_ the version of Dukovo it ran, and _slot_ its slot, *a* or *b*.
_result_ is:

*good*
	The start was seen through, or an update proved itself.

*failed*
	The health check failed it, and _check_ is the error ID.

*fell-back*
	It failed, and the next start that is no try ran the other slot: an
	update that failed its check and the machine went back. The records
	do not say that the machine fell back; this is how it shows in them,
	so a start in the other slot that a person chose after a failure
	reads the same.

*bisect-try*
	A try of *changes bisect*, whatever its end. Its number is not in
	the record.

_result_ is *null* for a start that has not finished: the one that runs,
until its health check is done, or one that never got that far. _check_ is *null* unless the start failed. _ready_ms_ is when the
services had settled, in milliseconds after the kernel started, as
*startup*(8) says it, and *null* when no times are kept. _changes_ is
what the start ran that the start before it, in the list, did not, newest
first, and an empty array for the oldest start kept. In it, _what_ is the name of
the thing (an app's ID, or *Dukovo*, *Radix*, *firmware set*, *CPU
microcode*, *boot options*), _version_ what the start ran of it (*null*
for one it no longer had) and _at_ms_ when that was installed, where
known.

*--failed --json* is an object:

```
{"failed": {"start": 42, "...": "as above"} | null,
 "since_good": [{"what": "org.nginx.nginx", "version": "1.30.6",
                 "at_ms": 1763000000000}],
 "bisect": {"running": true, "try": 2, "of": 3}}
```

_failed_ is the newest start that failed and is no try, in the form
above, or *null* when no start kept has failed (the exit status is 0).
This differs from the text, which also takes a failed try. _since_good_
is what changed from the last good start before it to it, as in the text,
and an empty array when none is kept before it or nothing changed. _bisect_ is the
search's state: _running_ is whether a search waits on a try; _try_ the
number of the try the next start is, and _of_ the most tries the search
takes, counting those made, which falls as tries narrow the changes. Both
are *null* when none runs, and with no search at all _running_ is
*false*.

*bisect --status --json* is that object with the rest of the search,
for the web console:

```
{"running": true, "try": 2, "of": 2, "failed_start": 42,
 "levers": [{"what": "org.nginx.nginx", "kind": "app", "version": "1.30.6",
   "at_ms": 1794500580000}],
 "cannot": [{"what": "enp1s0", "kind": "card",
   "why": "the machine's hardware, which no restart changes"}],
 "tries": [{"try": 1, "left_out": ["system", "org.nginx.nginx"],
   "confirm": false, "verdict": "good", "start": 43,
   "at_ms": 1794501000000}],
 "next_ms": 1794501120000, "result": null}
```

_failed_start_ is the start the search is about. _levers_ are the changes
it can leave out, numbered from 1 in that order as *--status* numbers
them: _what_ is the app's ID, *system* or *boot options*, _kind_ *slot*,
*options* or *app*, _version_ what the failed start ran, and _at_ms_ when
that was installed, where known. _cannot_ are the changes it cannot try
apart, and why. Each try has the changes it left out, whether it confirms
a change named by elimination, its _verdict_ once judged (*good*,
*failed*, *unfinished*, *otherwise* or *not-started*; *null* while it is
waited on), and the start that was it, and when. _next_ms_ is when Spark
restarts the machine for the next try, while it is set: *--later*'s
restart; the health check's own, a minute after a try's result, is not
set ahead, and is *null* then. _result_ is *null* until the search ends,
then _end_ (*found*, *not-found*, *otherwise*, *not-started*, *broken* or
*stopped*), for *found* the change's _what_ and _version_, whether a try
_confirmed_ it, and _undo_, the lines that say how to undo it; and
_words_, the lines of the result as *--status* prints them. With no search
at all every member is empty, *null* or *false*.

# FINDING THE CHANGE

*changes bisect* finds which change broke a start, when more than one
came in between. It compares the newest start that failed (F) with the
last good start before it (G), and finds each change a start can be made
without, which it calls a lever:

- the system, when F ran one slot and the other slot still holds what G
  ran: a try starts that slot;
- the boot options, when they differ: a try starts with G's;
- each app installed for everyone that is new or changed in F and has
  services: a try leaves its services out, as if they were off, and
  changes neither the app nor its data.

It lists them in that order, with the changes it cannot try apart and
why: the network cards and the root's partitions, an app F no longer has,
an app with no services, a slot built again since G, boot options longer
than a try can hold (400 bytes). It says the most restarts it needs, at
most the base 2 logarithm of the levers rounded up, and asks to go on.
*elevate changes bisect* is the command to run; the failure notice and
the menu of dukovo-rescue(8) name it.

The search halves. Each try undoes the first half of the levers still
in question. A try that starts well puts the cause in the half it
undid; one that fails as F did, by the same error ID and the same
service or check, puts it in the half it kept. Four levers take two
tries and eight take three. The cause is named *confirmed* when a good
try undid it alone, and *by elimination* when the others were cleared
instead. A try that fails in another way ends the search, and both
failures are shown, since its halves cannot be judged. A try that never
reached its health check is tried once more, and counts as failed if it
does not reach it again.

A try is one start. The boot menu starts it once, in place of the start
the slot state names, and changes nothing in the slot state: a power
cut, a hang or a panic during it leaves the next start the usual one,
and the machine is never left on a try's settings. The health check
judges each try as it judges any start, and 60 seconds after its verdict
the machine restarts into the next, saying so on the console and in a
notification. *--stop* stops the search before that.

When the search ends it says what it found and how to undo it, and
changes nothing. The result is kept for the next sign-in (ADR 0504) and
*--status* shows it again. The machine is left running the last try, and
the next usual start is F's again until the fix is taken.

*--yes*
	Do not ask before the first restart. Without it, an answer of no
	writes nothing and ends with status 1.

*--later*
	Restart for the first try in a minute, through Spark, instead of at
	once, so that whoever asked is answered first: the web console asks
	so. *shutdown --show* shows the restart, and *shutdown -c* cancels
	it; the try is then the next start, whenever that is.

*--status*
	Show the search: the failed start, the levers, each try and what
	it found, and the result once it has ended. With *--json*, only
	the search's state, as under JSON above.

*--stop*
	Stop the search. The next start is the usual one.

*--confirm*
	When the result was named by elimination, make one more restart
	that undoes only that change, to make sure. A good start confirms
	it. With *--yes* it does not ask.

It refuses when no start has failed since the last good one, when no
good start before it is kept, when a good start came after the failed
one, when an update is waiting to be tried (the boot menu starts that in
place of a try), when a search is already waiting on a try, when the
slot F ran is neither the default nor the previous version, and when
nothing that changed can be tried apart.

```
$ elevate changes bisect
Start 42 failed its health check: sshd has restarted 3 times
From the last good start, 40, these changes can each be left out for one start:
  1. the system: Dukovo 0.15.2 to Dukovo 0.15.3
  2. the boot options: `quiet` to `quiet nowatchdog`
  3. app org.nginx.nginx: new, 1.30.6; installed 2026-11-12 14:03
  4. app org.python.python: 3.14.0 to 3.14.1; installed 2026-11-12 14:05
Finding which one takes up to 2 restarts. The machine restarts by itself for each try, a start with some of the changes left out for that start alone: nothing is changed for good, and a try that hangs costs only that try. `elevate changes bisect --stop` stops it at any point, and `changes bisect --status` follows it.
Restart for the first try now? [y/N] y
changes bisect: restarting for try 1
```

The machine restarts. Try 1 leaves out the system and the boot options,
which are the first half, and fails as start 42 did, so the cause is one
of the apps. Try 2 leaves out nginx, starts well, and ends the search:

```
changes bisect: The start fails with org.nginx.nginx 1.30.6, installed 2026-11-12 14:03, and starts without it.
changes bisect: To remove it: `elevate bundle remove org.nginx.nginx --for-everyone`.
changes bisect: Nothing was changed: this start was the last try, and the next usual start is start 42's again until a fix is taken.
```

# FILES

_/system/state/starts/_
	The record of each start, the last 100, by number.

_/system/state/bisect/state.json_
	The search under way or the last one, written whole or not at all.

_/system/state/bisect/try-_N_
	The services try _N_ leaves out, one to a line. Spark reads it
	when the kernel's command line says *trial=*_N_.

_/system/live/sign-in/bisect_
	The result, for the next sign-in.

# SEE ALSO

*sysupdate*(8), *health.toml*(5), *dukovo-rescue*(8), *dukovo-boot*(7), *spark*(8), *update*(8), *log*(1)
