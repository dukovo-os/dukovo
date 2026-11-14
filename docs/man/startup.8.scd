startup(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

startup - why starting the computer took as long as it did

# SYNOPSIS

*startup* [*--start* _start_] [*--compare*[=_start_]] [*--json*]

# DESCRIPTION

startup shows how long a start took, step by step and service by service
(ADR 0474). Each start's record (*changes*(1)) keeps its times, which the
health check writes as it finishes the record, once no service that
starts at boot is still on its way to ready, or two minutes after it saw
the start through, whichever is first: how long the firmware and
the boot loader took, where the firmware says (ACPI's FPDT, which the
kernel reads at _/kernel/boot/times_, *radix-sysfs*(5)); when the kernel
started Spark; when each service started and was ready, and which it
started after; when the services settled; and when the first person
signed in at a console. With no option it shows the newest start; while
the health check has not finished it, the times come from Spark as they are
now.

```
$ startup
Start 42 on 2026-11-11 21:04, Dukovo 0.15.2, slot B: good
  firmware     1.8 s   this computer's firmware does not say
  boot loader  0.9 s
  kernel       1.2 s
  services     6.8 s   settled 8.0 s after the kernel started
  sign-in      4.0 s   the first sign-in, 12.0 s after the kernel started
From the computer starting to the services settling: 10.7 s

The chain that set when the services settled, each waiting for the one above:
  logd  started 1.3 s  ready 1.4 s  took 0.1 s
  db    started 1.5 s  ready 5.0 s  took 3.5 s
  web   started 5.6 s  ready 8.0 s  took 2.4 s

The slowest services, from started to ready:
  db    3.5 s  started 1.5 s
  web   2.4 s  started 5.6 s
```

The services settle when the last service that starts at boot is ready.
Not counted are the health check itself, a service started by a
request and one that was started again after it ended, which say nothing
of how the start went. The chain begins at the service that was ready
last and goes back along the services it started after, each time to the
one that was ready last by the time it started, as systemd-analyze
critical-chain does. The firmware's and the boot loader's times are not
known on every computer; where they are not, the line says so.

A start that settles more than 20% and at least two seconds later than the
median of the last five good starts is reported by the health check, in the
log and as a notification, with the step that grew most and the service
that did (BOOT-106). It says nothing before five good starts with times
are kept.

# OPTIONS

*--start* _start_
	Show start _start_, by number (*changes --list* lists them).

*--compare*[=_start_]
	Put the start beside the last good start before it, or beside start
	_start_: each step's time in both and the change, and the services
	whose time from started to ready changed most.

*--json*
	Write one line of JSON, for the web console: the steps, the chain,
	the slowest services and, with *--compare*, the other start's steps.

# EXIT STATUS

0, or 1 when the start has no times: it began before they were kept, or
ended before the health check saw it through, or none is recorded yet.

# FILES

_/system/state/starts/_
	The record of each start, with its times.

_/kernel/boot/times_
	The firmware's and the kernel's times (*radix-sysfs*(5)).

# SEE ALSO

*troubleshoot*(8), *changes*(1), *service*(8), *sysupdate*(8)
