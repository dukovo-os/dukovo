brltty(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

brltty - show the consoles on a refreshable braille display

# SYNOPSIS

*brltty* [_option_ ...]

# DESCRIPTION

BRLTTY drives refreshable braille displays: rows of cells of eight pins
each, with keys to move along the screen, routing keys over the cells
that move the cursor there, and often a braille keyboard. On Dukovo it
is built with all its braille drivers and one screen driver, *dk*, which
reads the consoles from *consoled*(8) and types the display's keys
through it (ADR 0326). *brailled*(8) runs it for a display that is
plugged in, so it is seldom run by hand.

It reads the screen from consoled, the shown console's rows and
consoled's own line under them, and shows the window of them the
display has room for, with the cursor in dots 7 and 8. The display's
keys move the window along the line and up and down the screen, and
route the cursor; what each key does depends on the display, and
BRLTTY's help for its driver lists them.

# OPTIONS

*-b* _driver_
	The braille driver, by its two-letter code, as *braille
	displays*(1) lists them.

*-d* _device_
	The display's port: *serial:*_/devices/ttyUSB0_.

*-x* _driver_
	The screen driver, *dk*.

*-t* _table_
	The text table, a name from _/system/resources/brltty/Text_, such as
	*en-nabcc* or *de*.

*-n*
	Stay in the foreground.

*-e*
	Log to standard error.

*-l* _level_
	How much to log: *error*, *warning*, *notice*, *information* or
	*debug*.

*-q*
	Show no messages on the display as it starts.

*-f* _file_
	Read the configuration from _file_.

*-h*
	Show the options.

*-V*
	Log the versions of BRLTTY and its drivers, on standard error
	with *-e*, and end.

# FILES

_/system/resources/brltty_
	The text, contraction, attribute and key tables.

_/system/settings/braille/brltty.conf_
	The configuration file, if there is one.

_/system/state/brltty_
	BRLTTY's preferences, as it keeps them.

_/system/live/consoled/braille_
	consoled's socket, which the *dk* screen driver reads the consoles
	from.

# EXIT STATUS

0 when it was stopped; otherwise not 0, when the display or consoled
could not be reached or an option was wrong.

# SEE ALSO

*brailled*(8), *braille*(1), *braille.toml*(5), *consoled*(8)

BRLTTY's documentation: https://brltty.app/doc/

# COPYRIGHT

BRLTTY is by Dave Mielke and The BRLTTY Developers, under the GNU LGPL
2.1 or later, its tables too. The screen driver for Dukovo is the
project's own, under Apache-2.0 OR MIT. This page is Dukovo's.
