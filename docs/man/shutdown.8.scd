shutdown(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

shutdown - stop the system, now or later

# SYNOPSIS

*shutdown* [*-H* | *-P* | *-r* | *-h*] [*-k*] [*--no-wall*] [_time_] [_message_ ...]

*shutdown -c* [*--no-wall*]

*shutdown --show* [*--json*]

# DESCRIPTION

shutdown asks Spark, over its control socket _/system/live/spark/control_
(*spark*(8)), to stop the system at _time_: Spark stops the services,
the most recently started group first, and then powers the machine
off, restarts it or halts it. The Radix kernel writes back every
filesystem's changes before the machine goes (*radix-power*(2)).

_time_ is *now*, *+*_minutes_ from now, or _hh_:_mm_, the next time the
clock says so: a time that has passed today means tomorrow. Without
one, the system stops in a minute. A later request replaces a pending
one, and *shutdown -c* cancels it; with none pending it does nothing,
and that is no failure, so a script can cancel twice. *shutdown --show*
says whether one is pending, and when, from Spark's read socket, which
answers anyone.

The options and the time are systemd's. Users come in Phase 13 of the
roadmap; until then there is nobody to send a wall message to, so the
_message_ is printed on standard error.

# OPTIONS

*-P*, *--poweroff*, *-h*
	Power the machine off; the default.

*-r*, *--reboot*
	Restart the machine.

*-H*, *--halt*
	Halt the machine: the processors stop, and it is safe to turn it
	off by hand.

*-k*
	Only print the message: nothing stops.

*--no-wall*
	Print no message.

*-c*
	Cancel the pending shutdown, if there is one.

*--show*
	Say whether a shutdown is pending: what it ends in, the time it is
	set for and the seconds left.

*--json*
	With *--show*, print it as one line of JSON: *{"format": 1,
	"shutdown": null}* when none is pending, else *"shutdown": {"end",
	"in_seconds", "at_ms"}*, where _end_ is *reboot*, *poweroff* or
	*halt* and _at_ms_ is the time it is set for, in milliseconds since
	1970, to the second. The web console reads this (*admind*(8)).

*--help*
	Show the options.

# EXIT STATUS

0 when Spark took the request, and for *-c* with nothing pending; 1
when Spark refused it (a shutdown already under way) or could not be
reached; and 2 for a time or option it does not know.

# EXAMPLES

Power off in ten minutes, and change your mind:

	shutdown +10 "Moving the server to the other rack"
	shutdown --show
	shutdown -c

Restart at once:

	shutdown -r now

# FILES

_/system/live/spark/control_
	Spark's control socket.

_/system/live/spark/read_
	Spark's read socket, which *--show* asks.

# SEE ALSO

*reboot*(8), *service*(8), *spark*(8), *radix-power*(2), *systools*(1)
