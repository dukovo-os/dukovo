serial-console(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

serial-console - the installer, a shell or the sign-in on serial ports

# SYNOPSIS

*serial-console*

# DESCRIPTION

serial-console keeps a console on serial ports, which Radix makes the
terminals _/devices/ttyS0_ and on (*radix-files*(7)), for a computer
with no screen or one managed from afar, as *agetty*(8) keeps one on
each of a Linux system's serial consoles (ADRs 0318, 0480 and 0481). A
terminal program at the other end of the cable, such as *screen*(1),
*picocom*(1), *minicom*(1) or PuTTY, or a server's Serial-over-LAN, is
then all a person needs to install Dukovo and to work on it.

The ports are named as on Linux: COM1 to COM4 are *ttyS0* to *ttyS3*,
and the ports of PCI serial cards and Intel AMT's Serial-over-LAN port
follow from *ttyS4*, in the order they are found. A server's BMC, such
as a Dell iDRAC, an HPE iLO or a Supermicro IPMI controller, most often
puts its Serial-over-LAN on COM2, *ttyS1*; Intel AMT's is usually
*ttyS4*. _/kernel/class/tty_ shows each port the kernel found
(*radix-sysfs*(5)).

Spark starts serial-console at every boot as the *serial-console*
service (_/system/settings/spark/services/serial-console.toml_,
*spark*(8)). It chooses the ports to keep a console on, in this order
(*dukovo-boot*(7)):

- With *serial-console=any* on the kernel's command line, as the install
  stick's *Install Dukovo (serial console)* entry gives it, every serial
  port. Rescue's entry passes it over.
- Otherwise each port a *console=ttyS*_n_ flag names, as *console=ttyS1,57600
  console=ttyS4*, as a system installed over a serial port, or one set
  with *serial*(8), has. The last *console=* is the primary console, as
  on Linux.
- With no *console=* flag at all and no *spcr=off*, the port the firmware
  sends its own console to, which its ACPI table SPCR names, at the
  firmware's speed, as Linux takes it on Arm servers. It is never the
  primary console, so the installer and Rescue stay on tty1. *serial*
  shows it, and the kernel's log names it, and _/kernel/class/tty_
  holds its line in *firmware_console* (*radix-sysfs*(5)).

With none of these it logs *no console= flag and no firmware console
port: no serial console*, exits with 0 and stays stopped. Each console
is in a process of its own that serial-console starts again should it
end. A port the computer does not have is passed over with *no
/devices/ttyS1: this computer has no serial port ttyS1*.

On each port it runs one of these, and again each time it ends:

- With *install* on the kernel's command line too, on the primary
  console, *dukovo-install*(8)'s screens, once, as root, and it logs
  *install: the installer's screens on /devices/ttyS0*. *consoled*(8)
  then leaves them to the line and runs the shell on tty1, if there is
  a screen at all. When the screens end, by Live console or at the end
  of the install, root's shell follows. The other ports get their
  sign-in or shell meanwhile.
- With *install* and *serial-console=any*, on every port, first *Press
  Enter to install Dukovo on this port (ttyS1).* The first port a
  person presses Enter on runs the installer's screens, as above, once,
  and logs *install: the installer runs on ttyS1*; the others say *The
  installer is running on ttyS1.* and go on to root's shell or the
  sign-in. The port that runs it is claimed by creating
  _/system/live/serial-install_ with its name, which only one port
  can, and which lives in memory, so that a restart frees it.
  *consoled*(8) leaves the screens to the line, as it does for the
  primary console. The installed system keeps that port as its console,
  at the line it ran at (*dukovo-install*(8)).
- With *rescue=*_uuid_, as Rescue's boot menu entry gives it, on the
  primary console alone, *dukovo-rescue*(8)'s screens, as root, and it
  logs *rescue: Rescue's screens on /devices/ttyS0* (ADR 0460).
- With *console-sign-in=off*, root's login shell from
  _/system/settings/passwd_, with no sign-in, as *consoled*(8) runs it
  on the screen's consoles.
- Otherwise _/system/settings/issue_'s text, in which *\\l* is the
  port's name, such as *ttyS1*, and the sign-in, *login*(1), as on the
  screen's consoles. *authd*(8) records sign-ins there under that
  name. Under the issue text, until the
  next start, shows what the health check said of a failed start,
  _/system/live/sysupdate/failure_ (*sysupdate*(8), ADR 0501), and the
  notices kept for the next sign-in in _/system/live/sign-in_
  (*consoled*(8), ADR 0504).

One that ends within 2 seconds five times in a row waits 10 seconds
before the next. While a session's programs have the line open, the
kernel's log stays off it, as it would draw over them; *dmesg*(1) and
*log*(1) show it meanwhile, and a panic report and *kdb* still go out
on the line. Between sessions the log goes out on it again. The kernel
flag *serial-log=* keeps the log, *kdb* and panic reports off a port
altogether, as *serial-log=ttyS0* sends them to COM1 alone.

# THE LINE

Each session finds the line as a new terminal: canonical input with
echo, *Ctrl+C* and the other keys of *stty*(1)'s defaults, at the speed
and character format the port's *console=* gives, whatever the last
session left. As on Linux, the flag may give them after a comma: the
speed in baud, 300 to 115200, then optionally the parity, *n*, *o* or
*e*, and the data bits, *7* or *8*, as in *console=ttyS1,57600n8*; a
final *r*, for RTS/CTS flow control, is passed over, as the port does
none. The kernel runs the port at that line from boot, so the log and
the console agree. With no options the line runs at 115200n8, as GRUB's
menu and the kernel's log do. The firmware's console port runs at the
speed the firmware's table gives, 8N1, and under *serial-console=any*
every other port at 115200n8. Options it cannot read are logged, and
the line runs at 115200n8. CLOCAL is set, so the line works with no modem signals, and
DTR stays raised.

The sessions get *TERM=vt220*, as Linux's serial consoles do: what
nearly every terminal and terminal program understands. One that
understands more, such as xterm's, can be named in the shell, as with
*export TERM=xterm-256color*.

The terminal at the other end is asked its size as xterm's *resize*(1)
asks: serial-console moves the cursor as far down and right as it goes
and asks where it is, *ESC [ 6 n*, then puts it back. The answer,
*ESC [* _rows_ *;* _columns_ *R*, sets the line's size (*TIOCSWINSZ*), which
the installer's screens and the shell's line editing follow. With no
answer within half a second, as from a program that only logs the
line, the line is 80 columns by 24 rows. A terminal resized later is
not seen: *stty rows* _n_ *cols* _n_ sets the size by hand.

# FILES

_/devices/ttyS0_, _/devices/ttyS1_, ...
	The serial ports, as terminals.

_/kernel/class/tty_
	The serial ports the kernel found, with their I/O ports and
	interrupts, and the port the firmware's console is on
	(*radix-sysfs*(5)).

_/kernel/cmdline_
	The kernel flags that choose the ports.

_/system/live/serial-install_
	The port that runs the installer, under *serial-console=any*.

_/system/settings/issue_
	What shows before the sign-in and root's shell. Its default is in
	_/system/resources/default-settings/issue_.

_/system/settings/spark/services/serial-console.toml_
	The service.

# SEE ALSO

*consoled*(8), *dukovo-boot*(7), *dukovo-install*(8), *login*(1),
*radix-debug*(7), *radix-files*(7), *radix-sysfs*(5), *serial*(8),
*spark*(8), *stty*(1)
