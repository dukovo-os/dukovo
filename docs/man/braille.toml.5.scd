braille.toml(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

braille.toml - whether braille displays are used, and which

# SYNOPSIS

_/system/state/braille/braille.toml_

# DESCRIPTION

The braille displays' settings, which *braille*(1) changes and
*brailled*(8) reads every two seconds (ADR 0326). Until the first
change there is no file, and the values are the defaults. They are
lines of TOML:
```
on = true
driver = "auto"
device = "auto"
table = "en-nabcc"
```

*on*
	*true* to start BRLTTY for a display that is plugged in, now and
	with the system.

*driver*
	BRLTTY's braille driver, by its two-letter code: *bm* for Baum's,
	*hw* for HumanWare's, *pm* for Papenmeier's and so on, as *braille
	displays* lists them. *auto* takes the one the display's USB IDs
	say.

*device*
	The display's port, a terminal in _/devices_: _/devices/ttyS0_ for
	the first serial port, _/devices/ttyUSB0_ for the first USB one.
	*auto* takes the port of the first display brailled knows by its USB
	IDs. A port named needs its driver named too.

*table*
	BRLTTY's text table, a file's name in
	_/system/resources/brltty/Text_ without *.ttb*: *en-nabcc*, North
	American Braille Computer Code, *en-us*, *de* and so on. Letters,
	digits, *-* and *\_* alone.

A value that cannot be read is the default. Comments and other keys are
kept when braille changes the file.

# SEE ALSO

*braille*(1), *brailled*(8), *brltty*(1)
