dukofetch(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

dukofetch - the system at a glance, beside Duke

# SYNOPSIS

*dukofetch* [*--no-logo*] [*--json*]

# DESCRIPTION

dukofetch shows what the system is and how it is doing, as neofetch
does on Linux, beside Duke, Dukovo's mascot: the person and the
machine's name, then the release, the kernel, how long the system has
been up, the bundles installed, the shell, the terminal, the processor,
each display controller, the memory in use, the room on the root
filesystem, and each network interface's IPv4 addresses, then a row of
the brand's colours.

```
$ dukofetch --no-logo
chase@dell
OS: Dukovo Server 0.14.0 (x86-64)
Kernel: Radix 0.14.0
Uptime: 3 days, 4 hours, 12 mins
Apps: 14 bundles
Shell: Nukesh 0.14.0
Terminal: consoled (pts/0)
CPU: Intel Core i7-8550U (8) @ 4.00 GHz
GPU: Intel UHD Graphics 620
Memory: 2.1 GiB / 15.5 GiB (14%)
Disk (/): 38 GiB / 238 GiB (16%) btrfs
Address: 192.168.1.42/24 (net0)
```

Each line comes from Radix's own sources: the release and kernel from
_/kernel/version_ (*radix-sysfs*(5)), the uptime and memory from
_/processes_ (*radix-proc*(5)), the bundles from *bundled*(8), the
processor from CPUID and _/kernel/cpus_, the display controllers from
the PCI functions named by the PCI ID database, as *lspci*(8) names
them, the disk from *statvfs*(3) and the mounts, and the addresses as
*ip*(8) shows them. A line whose source is missing is left out: there is
no *Apps* line when bundled cannot be asked, no *GPU* line on a machine
without a display controller, and no *Disk* line when the root
filesystem is kept in memory, as when the system runs from its boot
archive. A display controller the PCI ID database does not name is shown
by its IDs, as *lspci*(8) shows it: *Device 1234:1111*.

The memory in use leaves out the page cache, as *free*(1)'s *used*
does. The disk's percentage is the share in use of what a person can
use, rounded up, as *df*(1) gives it. The processor's clock is its
highest, where CPUID gives one, and otherwise the one its name ends
with. The shell is SHELL's, with its version where it is Nukesh. The
terminal is the one on the output, with the program on its other side
where that can be told: *sshd* when SSH_CONNECTION is set, the emulator
TERM_PROGRAM names, or *consoled* when TERM is *linux*.

## Duke

On a terminal 80 columns wide or more, Duke is 16 rows high beside every
line; on a narrower one, he is 10 rows high, beside the release, kernel,
uptime, bundles, shell, processor, memory and disk. He is drawn with
half blocks, two pixels to a character cell, in 24-bit colour when
COLORTERM is *truecolor* or *24bit*, and in the nearest of xterm's 256
colours otherwise. A line too long for the terminal is cut short with
*…*, and the row of colours is left out where it does not fit.

The consoles draw the half blocks themselves when their font lacks them,
as Spleen's 8x16 does.

When NO_COLOR is set and not empty (_https://no-color.org_), TERM is
*dumb*, or the output is not a terminal, dukofetch writes the lines as
plain text, without Duke, as *--no-logo* shows above.

# OPTIONS

*--no-logo*
	Leave Duke and the row of colours out, and write each line as
	_label_*:* _value_. The labels keep their colour on a terminal.

*--json*
	Write the facts as a JSON object, one key a line, for scripts:
	*user* and *host*; *os*, with its *name*, *version* and *machine*;
	*kernel*, with its *name* and *version*; *uptime_seconds*;
	*bundles*; *shell*, with its *name*, *version* for Nukesh, and
	*path*; *terminal*, with its *program* where it is known and its
	*device*; *cpu*, with its *name*, *cpus* and *mhz*; *gpus*, a list
	of names; *memory*, with *used* and *total* in bytes; *disk*, with
	its *mount*, *used* and *total* in bytes and its *type*; and
	*addresses*, a list of objects with an *interface*, *address* and
	*prefix*. A key whose source is missing is left out; the lists are
	always there, empty where there is nothing.

	```
	$ dukofetch --json | jq .kernel
	{
	  "name": "Radix",
	  "version": "0.14.0"
	}
	```

# ENVIRONMENT

*COLORTERM*
	*truecolor* or *24bit* draws Duke in 24-bit colour.

*NO_COLOR*
	When set and not empty, the output is plain text.

*SHELL*
	The shell shown.

*TERM*
	*dumb* makes the output plain text; *linux* names the terminal's
	program consoled.

# EXIT STATUS

0, or 1 for an argument it does not take.

# SEE ALSO

*bundle*(1), *df*(1), *free*(1), *ip*(8), *lscpu*(1), *lspci*(8),
*radix-proc*(5), *radix-sysfs*(5), *systools*(1)
