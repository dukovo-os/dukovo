serial-console(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

serial-console - the installer, a shell or the sign-in on the serial port

# SYNOPSIS

*serial-console*

# DESCRIPTION

serial-console keeps a console on the PC's first serial port, COM1,
which Radix makes the terminal _/devices/ttyS0_ (*radix-files*(7)), for
a computer with no screen, as *agetty*(8) keeps one on a Linux serial
console (ADR 0318). A terminal program at the other end of the cable,
such as *screen*(1), *picocom*(1), *minicom*(1) or PuTTY, or a server's
serial-over-LAN, is then all a person needs to install Dukovo and to
work on it.

Spark starts serial-console at every boot as the *serial-console*
service (_/system/settings/spark/services/serial-console.toml_,
*spark*(8)). It does something only when the kernel was started with
*console=ttyS0* (*dukovo-boot*(7)), as the install stick's *Install
Dukovo (serial console)* entry and a system installed from it are;
otherwise it logs *the kernel was not started with console=ttyS0: no
serial console*, exits with 0 and stays stopped. A computer without
COM1 has no _/devices/ttyS0_, and serial-console says so and stops the
same way.

Otherwise it runs, on the line, one of these, and again each time it
ends:

- With *install* on the kernel's command line too, *dukovo-install*(8)'s
  screens, once, as root, and it logs *install: the installer's screens
  on /devices/ttyS0*. *consoled*(8) then leaves them to the line and
  runs the shell on tty1, if there is a screen at all. When the
  screens end, by Live console or at the end of the install, root's
  shell follows.
- With *rescue=*_uuid_, as Rescue's boot menu entry gives it,
  *dukovo-rescue*(8)'s screens, as root, and it logs *rescue: Rescue's
  screens on /devices/ttyS0* (ADR 0460).
- With *console-sign-in=off*, root's login shell from
  _/system/settings/passwd_, with no sign-in, as *consoled*(8) runs it
  on the screen's consoles.
- Otherwise _/system/settings/issue_'s text, in which *\\l* is *ttyS0*,
  and the sign-in, *login*(1), as on the screen's consoles. *authd*(8)
  records sign-ins there as on *ttyS0*.

One that ends within 2 seconds five times in a row waits 10 seconds
before the next. While a session's programs have the line open, the
kernel's log stays off it, as it would draw over them; *dmesg*(1) and
*log*(1) show it meanwhile, and a panic report and *kdb* still go out
on the line. Between sessions the log goes out on it again.

# THE LINE

Each session finds the line as a new terminal: canonical input with
echo, *Ctrl+C* and the other keys of *stty*(1)'s defaults, at the speed
and character format *console=ttyS0* gives, whatever the last session
left. As on Linux, the flag may give them after a comma: the speed in
baud, 300 to 115200, then optionally the parity, *n*, *o* or *e*, and
the data bits, *7* or *8*, as in *console=ttyS0,115200n8*; a final *r*,
for RTS/CTS flow control, is passed over, as the port does none. With
no options the line runs at 115200n8, as GRUB's menu and the kernel's
log do. Options it cannot read are logged, and the line runs at
115200n8. CLOCAL is set, so the line works with no modem signals, and
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

_/devices/ttyS0_
	The first serial port, as a terminal.

_/system/settings/issue_
	What shows before the sign-in and root's shell. Its default is in
	_/system/resources/default-settings/issue_.

_/system/settings/spark/services/serial-console.toml_
	The service.

# SEE ALSO

*consoled*(8), *dukovo-boot*(7), *dukovo-install*(8), *login*(1),
*radix-debug*(7), *radix-files*(7), *spark*(8), *stty*(1)
