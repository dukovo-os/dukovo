serial(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

serial - the serial ports, and the console on them at the next start

# SYNOPSIS

*serial* [*list*]

*serial console* _port_[,_options_]...

*serial console auto*

*serial console off*

*serial log all*

*serial log none*

*serial log* _port_[,_port_]...

# DESCRIPTION

A computer managed from afar has its console on a serial port: a BMC's
Serial-over-LAN, Intel AMT's, a PCI card cabled to a console server.
*serial* shows the ports the kernel found and what each does at this
start, and sets, for the next start, which ports have a console and at
what line, which carry the kernel log, and so which the boot menu
shows on (ADR 0481).

The ports are named as on Linux: COM1 to COM4 are *ttyS0* to *ttyS3*,
and PCI serial cards and Intel AMT's Serial-over-LAN port follow from
*ttyS4* (*uart*(4), *radix-sysfs*(5)).

*list*, or no command, shows the ports (see OUTPUT below), then what
the next start does, as the boot menu's settings say. Anyone may ask.

*console* _port_[,_options_]... gives each port named a console, as
*serial-console*(8) keeps on a port named by *console=*, and shows the
boot menu on each that GRUB can show it on. _options_ are Linux's
console options, as *console=ttyS1,57600n8* has them: the speed, 300 to
115200, then optionally the parity, *n*, *o* or *e*, and the data bits,
*7* or *8*; 115200n8 when none are given, the line the menu and the
kernel log run. The last port is the primary console, which runs the
installer and Rescue, as the last *console=* does
(*dukovo-boot*(7)). The menu shows on the ports in the order given, at
their speeds, up to four. A port GRUB cannot show it on keeps its
console and is left off the menu, and *serial* says why.

*console auto* makes the firmware's console port the console, the
default of a system with no settings: the port the firmware's ACPI
table SPCR names, at the firmware's speed, if it names one
(*dukovo-boot*(7)). The menu shows on that port at that speed, or on
COM1 at 115200 baud when no port is named.

*console off* gives no port a console and no port the menu, as the
kernel flag *spcr=off* does with no *console=*. The kernel log and *kdb*
still go to the ports *log* gives.

*log* chooses the ports that carry the kernel log, *kdb* and a panic
report, as the kernel flag *serial-log=* does (*dukovo-boot*(7)): *all*,
the default; *none*; or a list of ports separated by commas, such as
*ttyS0,ttyS4*. A port left out is sent nothing by the kernel, which a
UPS or a modem on it needs. It does not change which ports have a
console or the menu.

A change takes effect at the next start: the kernel and *serial-console*
read the command line, which a start makes, and GRUB reads the settings
as it shows its menu. It is written for every EFI system partition of
the disks the system started from, so that a mirror's disks agree, and
only root may write it: run *console* and *log* with *elevate*(8). A
system that did not start from a Dukovo root on a disk, such as the
install stick's live system, has no settings to change.

*--esp* _dir_
	Write the settings under _dir_, an EFI system partition mounted
	there, in place of the system's own. The installer uses it for the
	partition it has mounted, and takes the same arguments; it is no use
	elsewhere.

# OUTPUT

*list* prints a table with a row for each port:

*PORT*
	The port's name: *ttyS1*.

*WHAT*
	What it is: *COM2 at I/O port 0x2F8*, or the card and its PCI
	address, *Intel AMT Serial-over-LAN at PCI 0000:00:16.3*.

*LOG*
	*yes* when it carries the kernel log at this start, *no* when
	*serial-log=* left it out.

*CONSOLE NOW*
	The console it has at this start, as *serial-console*(8) chose it:
	its options, as *57600n8*, with *primary, * before the primary
	console's and *firmware, * before the firmware's; *any* on every
	port under *serial-console=any*, as the install stick's serial entry
	starts the live system; or *-* when it has none.

When the firmware names a port, a line says which: *The firmware sends
its console to ttyS1 (ACPI SPCR), at 57600n8.* Last, a line or a block
says what the next start does:

```
At the next start (boot/grub/serial.env):
  Consoles:   ttyS1 at 57600n8 (primary)
  Kernel log: all ports
  Boot menu:  com1 at 57600 baud
```

_Consoles_ is the ports with a console and their options, the primary
console marked, *the firmware's console port*, or *none*. _Kernel log_
is *all ports*, *no port* or the ports. _Boot menu_ is the ports GRUB
shows the menu on, by GRUB's names, *com0* to *com3* for COM1 to COM4
and *pci,00:16.3* for a port of a PCI function, each with its speed.
A console left off the menu gets a line of its own, *ttyS5 has a
console but not the boot menu*, and the reason: *GRUB shows it only on a
PCI card's first port*, *GRUB shows no port on a PCI domain other than
0000*, *GRUB has no name for it* (a kind of port GRUB does not run),
*GRUB shows it only at 8 data bits and no parity*, or *GRUB shows it on
four ports at most*. Before the settings are ever written the line says
so: *as this one, since the boot menu has no serial settings*, and a
start from the stick or with no Dukovo root on a disk says *this system
did not start from a Dukovo root on a disk*. The settings are on the EFI
system partition, whose device only root reads, so for any other user
the line says *only root reads the boot menu's serial settings*.

A change prints the settings it wrote in that form, then *The serial
console's settings take effect at the next start.*

# EXAMPLES

A BMC's Serial-over-LAN on COM2, with no ACPI table to say so, at 57600
baud: the console and the boot menu on it.

```
$ elevate serial console ttyS1,57600
At the next start (boot/grub/serial.env):
  Consoles:   ttyS1 at 57600n8 (primary)
  Kernel log: all ports
  Boot menu:  com1 at 57600 baud
The serial console's settings take effect at the next start.
```

Intel AMT's port, the console and the menu at 115200 baud:

```
$ elevate serial console ttyS4
At the next start (boot/grub/serial.env):
  Consoles:   ttyS4 at 115200n8 (primary)
  Kernel log: all ports
  Boot menu:  pci,00:16.3 at 115200 baud
The serial console's settings take effect at the next start.
```

A UPS cabled to COM2, which is kept from the kernel's text, as its
controller would read the text as commands:

```
$ elevate serial log ttyS0,ttyS4
At the next start (boot/grub/serial.env):
  Consoles:   the firmware's console port
  Kernel log: ttyS0, ttyS4
  Boot menu:  com0 at 115200 baud
The serial console's settings take effect at the next start.
```

The ports, on a server whose firmware redirects its console to COM2 at
57600 baud:

```
$ serial
PORT   WHAT                    LOG  CONSOLE NOW
ttyS0  COM1 at I/O port 0x3F8  yes  -
ttyS1  COM2 at I/O port 0x2F8  yes  firmware, 57600n8

The firmware sends its console to ttyS1 (ACPI SPCR), at 57600n8.

At the next start: as this one, since the boot menu has no serial settings (boot/grub/serial.env).
```

Back to the firmware's console and the log on every port:

```
$ elevate serial console auto
$ elevate serial log all
```

# EXIT STATUS

0 when what was asked was done; 1 when something failed or the command
line was wrong, said on standard error. When some of a mirror's EFI
system partitions could not be written and others could, the settings
are written on those that could, each failure is said, and the status
is 1.

# FILES

_boot/grub/serial.env_
	The settings, on each EFI system partition, as a GRUB environment
	block that the boot menu reads with *load_env*, beside _grub.cfg_.
	It holds *dukovo_console*, the kernel flags the menu puts on each
	kernel line, and the ports and speeds the menu shows on, *serial_0*
	to *serial_3* and *serial_0_speed* to *serial_3_speed*
	(*dukovo-boot*(7)). A system installed before it has none and starts
	with the menu on COM1 at 115200 baud.

_/kernel/class/tty_
	The ports the kernel found (*radix-sysfs*(5)).

_/kernel/cmdline_
	The command line this start has, from which *CONSOLE NOW* comes.

# SEE ALSO

*serial-console*(8), *dukovo-boot*(7), *dukovo-install*(8), *uart*(4),
*radix-sysfs*(5), *elevate*(8)
