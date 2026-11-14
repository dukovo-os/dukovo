hostnamectl(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

hostnamectl - the machine's name and what it runs on, and set the name

# SYNOPSIS

*hostnamectl* [*status*]

*hostnamectl hostname* [_name_]

*hostnamectl set-hostname* _name_

# DESCRIPTION

hostnamectl shows the machine's name and what it is, and sets the name,
with the commands of systemd's hostnamectl (ADR 0478). The name is the
first line of _/system/settings/hostname_ (*hostname*(5)).

*status*, or no command, shows what the machine says of itself:

```
$ hostnamectl
  Static hostname: web01
          Boot ID: 00112233445566778899aabbccddeeff
 Operating System: Dukovo Server 0.15.0
           Kernel: Radix 0.15.0
     Architecture: x86-64
  Hardware Vendor: Dell Inc.
   Hardware Model: OptiPlex 7040
 Firmware Version: 1.12.2
    Firmware Date: 07/12/2023
```

The release and the kernel are the kernel's own (_/kernel/version_),
the hardware's vendor and model and the firmware's version and date
are the firmware's, as *hwinfo*(8) shows them, and the boot is the one
*logd*(8) names (*journalctl --list-boots*). A line is left out when the
machine does not say it: a machine without the firmware's tables has no
hardware lines, and without logd there is no boot. Dukovo keeps no
machine ID, so no line shows one, and no icon or chassis.

*hostname*, with no name, prints the machine's name; with one it is
*set-hostname*.

*set-hostname* _name_ gives the machine that name, by *sethostname*(2):
the name is the machine's at once for every program that asks, and for
the local networks after *service discoverd restart* (*hostname*(5)).
Only an administrator may: *elevate hostnamectl set-hostname* _name_
(*elevate*(8)). A name is words of letters, digits and *-*, none starting
or ending with *-*, separated by dots, of 64 bytes at most and 63 in each
word; another is refused.

# WHAT DUKOVO HAS INSTEAD

Dukovo has one name for the machine, the static one.

*--static*
	Is taken, and the name is the static one.

*--transient*, *--pretty*
	Say so and end with status 1: there is no transient or pretty host
	name apart from the one *set-hostname* sets.

*set-icon-name*, *set-chassis*, *set-deployment*, *set-location*
	Say that Dukovo keeps only the host name, and end with status 1.

# EXIT STATUS

0, or 1 if the command line is wrong, the name is refused or the
machine's name cannot be set.

# SEE ALSO

*hostname*(5), *hwinfo*(8), *timedatectl*(1), *elevate*(8)
