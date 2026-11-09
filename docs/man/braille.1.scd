braille(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

braille - turn braille displays on and off, and name a display's driver,
port and text table

# SYNOPSIS

*braille* [*status*]++
*braille* *on* | *off*++
*braille* *driver* [_code_ | *auto*]++
*braille* *device* [_/devices/name_ | *auto*]++
*braille* *table* [_name_]++
*braille* *displays*

# DESCRIPTION

A braille display shows the consoles through *brltty*(1), which
*brailled*(8) starts when it finds one plugged in by USB (ADR 0326).
braille changes the settings, in *braille.toml*(5), which brailled reads
every two seconds.

*status*, or no command, shows the settings and the display brailled
would use now:
```
Braille: on
Driver:  auto
Port:    auto
Table:   en-nabcc
Display: Baum SuperVario 40 on /devices/ttyUSB0, with BRLTTY's bm driver
```
The last line says *none plugged in* when there is no display it knows,
and says what is missing when the settings name a port but no driver.

*on* turns braille displays on, now and with the system from then on,
and *off* turns them off, which stops BRLTTY. They are on unless turned
off.

*driver* names BRLTTY's braille driver for the display, by its code,
such as *bm* for Baum's or *hw* for HumanWare's, and *device* names its
port, a terminal in _/devices_ such as _/devices/ttyS0_ for a serial
port, or _/devices/ttyUSB0_ for a USB one. A display that brailled knows
by its USB IDs needs neither, as both are *auto*. A display on a serial
port needs both, and so does one whose USB IDs are a chip maker's own,
which other devices share, as FTDI's 0403:6001 is.

*table* names the text table, how each character is shown in dots:
*en-nabcc*, North American Braille Computer Code, unless set; *en-us*,
*de*, *fr* and the others in _/system/resources/brltty/Text_.

*displays* lists the displays brailled knows by their USB IDs, and
BRLTTY's driver codes.

Each command with no value shows the setting. Only root changes the
settings.

# EXIT STATUS

0 when it did as asked; 1 for a driver, port or table that is not one,
or a mistake in the command line.

# FILES

_/system/state/braille/braille.toml_
	The settings (*braille.toml*(5)).

_/system/resources/brltty_
	BRLTTY's tables.

# EXAMPLES

Use a Papenmeier display on the first serial port:

	braille driver pm++
braille device /devices/ttyS0

Show German text:

	braille table de

# SEE ALSO

*brailled*(8), *braille.toml*(5), *brltty*(1), *consoled*(8), *reader*(1),
*systools*(1)
