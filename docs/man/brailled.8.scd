brailled(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

brailled - start BRLTTY when a braille display is plugged in

# SYNOPSIS

*brailled*

# DESCRIPTION

brailled starts *brltty*(1) for a braille display (ADR 0326). Spark runs
it as the *brailled* service, which ends at once, doing nothing, while
*braille.toml*(5) says *on = false*.

Every two seconds it reads braille.toml(5) and looks in
_/kernel/bus/usb/devices_ (*radix-sysfs*(5)) for a display it knows by
its USB IDs, whose serial port the kernel shows as a terminal: an FTDI
chip under its maker's IDs, *ttyUSB0*, or a CDC-ACM device,
*ttyACM0*. *braille displays*(1) lists them. Where braille.toml names a
port, brailled uses that and the driver it names instead.

For the display it runs BRLTTY in the foreground, with the display's
braille driver and port, the screen driver *dk*, which reads the
consoles from *consoled*(8), and the text table:
```
brltty -n -e -b bm -d serial:/devices/ttyUSB0 -x dk -t en-nabcc
```
BRLTTY reads its own configuration file,
_/system/settings/braille/brltty.conf_, if there is one; while there is
not, brailled gives it *-f /devices/null*, since BRLTTY counts the file
missing as an error.

When the display is unplugged, another is used in its place, or braille
is turned off, brailled stops BRLTTY, with SIGTERM and after five
seconds SIGKILL, and starts it again as things now are. When BRLTTY
ends by itself, brailled starts it again for the same display after ten
seconds. Its log says each of these:
```
brailled: looking for a braille display
brailled: found Baum SuperVario 40 on /devices/ttyUSB0: starting BRLTTY with its bm driver
brailled: Baum SuperVario 40 was unplugged: stopping BRLTTY
```
and BRLTTY's own lines follow, in the same log.

Stopped with SIGTERM, it stops BRLTTY and ends.

# FILES

_/system/state/braille/braille.toml_
	The settings (*braille.toml*(5)).

_/kernel/bus/usb/devices_
	The USB devices, and the terminals of their serial ports.

_/system/commands/brltty_
	BRLTTY.

_/system/settings/braille/brltty.conf_
	BRLTTY's configuration file, if there is one.

_/system/settings/spark/services/brailled.toml_
	The service.

# SEE ALSO

*braille*(1), *braille.toml*(5), *brltty*(1), *consoled*(8),
*radix-sysfs*(5), *service.toml*(5), *spark*(8)
