coreutils(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

coreutils - the core utilities, in one program

# SYNOPSIS

_utility_ [_argument_ ...]

*coreutils* _utility_ [_argument_ ...]

*coreutils* *--list* | *--version* | *--help*

# DESCRIPTION

coreutils is the uutils project's core utilities, written in Rust,
built for Radix as one program: *ls*(1), *cp*(1), *cat*(1), *sort*(1),
*date*(1), *test*(1) and the rest of the utilities POSIX names and GNU's
core utilities have, with GNU's options. Each is a link to
_/system/commands/coreutils_ by its name, and coreutils runs the utility its
name says; *coreutils* _utility_ runs one by name. Each utility has its
own manual page.

On Dukovo, *ls*(1) also leaves out of a directory's listing the names
its _.hidden_ file lists, one a line, unless *-a* or *-A* is given: the
root's lists the links with Unix's names, such as _/dev_ and _/etc_
(*radix-files*(7)), so that *ls /* shows the folders by their plain
names.

Dukovo's *ls* also takes these options, none of which changes its output
unless it is given (ADR 0440):

*--header*
	With *-l*, a line of column titles over the listing.

*--octal*
	With *-l*, each mode as four octal digits, as *0755*, before its
	letters.

*--field-colors*[=_when_]
	With *-l*, the permission letters, sizes and times in colours that
	say what they are: *r* yellow, *w* red, *x* green; small sizes faint
	and big ones yellow or red; times of the last day bold and of over
	a week ago faint. _when_ is *always*, *never*, or *auto*, the
	default: when the names are coloured and standard output is a
	terminal, so that *--color=always* into a pipe is as GNU's. The
	colours come from *LS_FIELD_COLORS*, as *read=33:write=31*, with the
	keys *read*, *write*, *run*, *special*, *none*, *directory*, *link*,
	*device*, *octal*, *small*, *big*, *huge*, *today*, *old* and
	*header*; set empty, it turns them off. Nukesh makes it from
	*colors.toml*(5).

*--icons*[=_when_]
	A Nerd Font icon before each name, by its type, name or ending.
	_when_ is *always*, *auto* (on a terminal, and what *--icons* alone
	means) or *never*, the default. The terminal must use a Nerd Font.

# OPTIONS

*--list*
	List the utilities, one per line.

*-V*, *--version*
	Print coreutils' version.

*-h*, *--help*
	Print how to run it, and the utilities.

# EXIT STATUS

The utility's, or 1 when there is no utility by that name.

# FILES

_/system/commands/coreutils_, and a link by each utility's name.

# SEE ALSO

*systools*(1), for the administration tools, *colors.toml*(5), and
*radix-posix*(7).

uutils coreutils, by the uutils contributors, under the MIT licence:
https://github.com/uutils/coreutils
