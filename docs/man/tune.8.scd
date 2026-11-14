tune(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

tune - performance profiles: the kernel's few settings for speed, power
and waiting, chosen together

# SYNOPSIS

*tune*

*tune list*

*tune show* _name_

*elevate tune set* _name_

*tune recommend*

# DESCRIPTION

The kernel has a few settings that trade one good thing for another, such
as how long a thread runs before another gets its turn, or how far ahead a
disk is read (*radix-kernel-settings*(2), ADR 0476). A _profile_ is a
choice of values for the settings that suit one kind of machine, with the
reason for each. *tune* shows the profiles and the one in use, and
*elevate tune set* chooses one.

A profile sets only the settings it lists. Every other setting is put back
to the value the kernel started with, so a profile changes nothing it does
not name, and changing from one profile to another leaves nothing of the
first. The default profile, *balanced*, lists none: the machine runs as it
starts. A machine nobody chose a profile for runs it.

The choice is kept in _/system/settings/tune.toml_ and given to the kernel
by the *tune* service (*service*(8)), the only service that may set the
kernel's settings (*admin.tune* in *service.toml*(5)). It runs at each
boot, because the kernel keeps nothing when the machine stops, and again
whenever a profile is chosen. A setting this machine cannot have, such as
an energy preference on CPUs whose speed the firmware keeps, is said in one
line and the rest of the profile is set.

On a virtual machine, the first start after an install leaves one notice
for the sign-in screens, that *elevate tune set vm-guest* suits the
machine. It is given once; choosing any profile, *balanced* among them,
ends it.

# COMMANDS

With no command, *tune* shows the profile in use and each setting's value.
Everyone may run it and the commands that only read.

*tune*
	The profile in use, and whether an administrator chose it or it is
	the default, with what it is for; then each of the kernel's
	settings and its value. A *\** before a setting marks one the
	profile set. A setting the profile asks for that is not in effect,
	because this machine cannot have it, says so.

*list*
	Each profile with what it is for, the one in use marked *(in use)*.

*show* _name_
	What the profile _name_ is for, each setting it sets with its value
	and the reason it gives that value, and that every other setting is
	at its start value.

*set* _name_
	Chooses the profile _name_ and has the *tune* service give it to the
	kernel; the command returns when that has been done. Choosing a
	profile needs an administrator: run it with *elevate*(8). It says
	*the profile is* _name_, and then a line for each setting the
	machine could not have. A name that no profile has is refused and
	the choice stays. A profile whose value the kernel refuses, or that
	names a setting this kernel lacks, ends the service with a failure,
	and *set* fails with it.

*recommend*
	The profile that fits this machine and why: *vm-guest* in a virtual
	machine, *power-saver* on a battery that is discharging, otherwise
	*balanced*; and the command that chooses it, or that it is the one in
	use.

*apply*
	What the *tune* service runs, and not a command for people: it gives
	the kernel the settings of the profile in use through the capability
	the service holds, and prints what it set.

# PROFILES

Each profile is a file in _/system/resources/tune_, a description and the
settings it sets, each with a comment above it that *tune show* gives as
the reason. The profiles Dukovo has:

*balanced*
	Sets nothing. The default.

*throughput*
	Most work done a second, for servers and builds: the CPUs lean to
	speed, the slice is 6000 microseconds, and reads go 1024 KiB ahead.

*low-latency*
	Quick answers, for a machine people work at: the CPUs lean to speed
	and the slice is 1000 microseconds, so a waiting thread gets its
	turn sooner.

*power-saver*
	Less power, for a laptop on its battery: the CPUs lean to saving
	power and the slice is 4000 microseconds.

*vm-host*
	A machine that runs virtual machines: the CPUs lean to speed, the
	slice is 6000 microseconds and the read-ahead is 512 KiB, which
	serves many guests reading at once.

*vm-guest*
	A virtual machine, whose host sets the CPUs' speed: the slice is
	4000 microseconds and the read-ahead 512 KiB. It leaves the energy
	preference alone, as a virtual CPU has none to set.

# THE SETTINGS

*cpu.energy_preference*
	How the CPUs trade speed against power: *firmware* (the start
	value), *performance*, *balance_performance*, *balance_power* or
	*power*. Only CPUs with a hardware power control take any but
	*firmware*; QEMU's usually have none.

*sched.slice_us*
	The base slice of the fair scheduling class, 500 to 20000
	microseconds, 3000 at start.

*disk.read_ahead_kib*
	How far ahead the page cache reads a file read in order, 0 to 1024
	KiB in steps of 4, 128 at start.

*net.congestion*
	The congestion control of new TCP sockets: *cubic* (the start
	value) or *reno*. No profile sets it.

The kernel's lines are in _/kernel/settings_, a file each, readable by
everyone (*radix-sysfs*(5)).

# FILES

_/system/resources/tune/_name_.toml_
	The profiles.

_/system/settings/tune.toml_
	The profile an administrator chose, as *profile = "name"*. Without
	it, the default applies.

_/system/settings/spark/services/tune.toml_
	The service that gives the kernel the profile, a copy of the one in
	_/system/resources/default-settings/spark/services_.

_/kernel/settings/_
	The kernel's settings and their values.

_/system/state/tune/vm-notice_
	Marks that the notice about virtual machines was given.

# EXIT STATUS

0 when it did what was asked; 1 for a mistake in the command line, a name
that is no profile, a refusal because it was not run as an administrator,
a profile the kernel did not take whole, and an error reading or writing a
file. A setting this machine cannot have is not a failure.

# EXAMPLES

The profile in use and the settings:

```
tune
```

What a profile does, and the one that fits this machine:

```
tune list
tune show throughput
tune recommend
```

Choose the profile for a virtual machine, then go back to the default:

```
elevate tune set vm-guest
elevate tune set balanced
```

# SEE ALSO

*radix-kernel-settings*(2), *radix-sysfs*(5), *service.toml*(5),
*service*(8), *elevate*(8), *spark*(8), *lscpu*(1)
