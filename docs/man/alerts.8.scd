alerts(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

alerts - show the alerts, test where they go, and raise them from a script

# SYNOPSIS

*alerts* [*list*] [*--json*]

*alerts history* [*-n* _count_] [*--json*]

*alerts test*

*alerts status* [*--json*]

*alerts raise* _key_ *--title* _text_ [*--detail* _text_] [*--critical*]

*alerts event* _key_ *--title* _text_ [*--detail* _text_] [*--critical*]

*alerts clear* _key_

# DESCRIPTION

*alerts* is the command of *alertd*(8), which keeps the alerts of the
system (ADR 0485): what needs the owner's attention, and what has
happened to it. *alerts* shows them, sends a test alert through every
channel that *alerts.toml*(5) names, and lets a script of root's raise
and clear an alert of its own.

Every command needs root, because alertd serves only root's programs and
the system's services. An administrator types *elevate alerts*; anyone
else is told *only root reads and raises alerts; try elevate alerts*. If
alertd is not running, *alerts* says so and names *service alertd
start*.

# COMMANDS

*list*
	Shows the alerts that are active, the critical ones first and then
	the oldest, which is the default. A line has the severity, since
	when the alert is active in UTC, its source and its title:

	```
	# alerts
	SEVERITY  SINCE (UTC)       SOURCE      TITLE
	critical  2026-11-13 10:02  diskhealth  Disk WDC (sda) is failing
	warning   2026-11-12 22:15  backup      The last backup to Off_Site was 3 days ago
	```

	With nothing active it says *No alerts are active.*

*history* [*-n* _count_]
	Shows what happened to the alerts, the newest last: 20 changes,
	or _count_, up to 1000. A line has the time in UTC, *raised*,
	*cleared*, *event* or *test*, the severity, the source and the title.
	The history is cut to the newest 1000 changes when it passes 2000
	(*alertd*(8)).

*test*
	Sends a test alert, *Test alert from alertd*, through every channel
	at once, and says how each went, a line for each. A channel that
	refused says why, in the words of the server that refused it, so
	that the line says what to change:

	```
	# alerts test
	email to ops@example.com: sent
	ntfy (ntfy.sh): failed: ntfy answered 403 Forbidden: forbidden
	```

	It exits with 1 if any channel failed, so that a script can check
	it. With no channels it says *No channels are set up; notices go
	to the screen. See alerts.toml(5).* and exits with 0. A test is in
	the history and on the screen, and is sent only once, so a channel
	that was down is not sent it later.

*status*
	Shows each channel and how it stands: when it last sent a message,
	when it last failed and why, and how many messages wait to be
	tried again (*alertd*(8) says when):

	```
	# alerts status
	email to ops@example.com
	  last sent   2026-11-13 10:02:41 UTC
	  waiting     0 messages
	ntfy (ntfy.sh)
	  last sent   never
	  last failed 2026-11-13 10:02:41 UTC: ntfy answered 403 Forbidden: forbidden
	  waiting     2 messages
	```

*raise* _key_ *--title* _text_ [*--detail* _text_] [*--critical*]
	Raises an alert, as a service does. It is a warning, or critical
	with *--critical*, for what costs data or is about to. The _key_
	names it for as long as it is active: of the letters *a* to *z*,
	the digits and *.*, *\_*, *:*, */* and *-*, up to 128; the title is
	one line of up to 120 characters; the detail is what to do about
	it, up to 2000. *raise* prints nothing when alertd took it.
	Raising an alert that is active only updates it, unless it was a
	warning and is critical now, so a script may raise what is wrong on
	each run without repeating itself.

*event* _key_ *--title* _text_ [*--detail* _text_] [*--critical*]
	Tells of something that happened, once. It is kept in the history
	and told to the owner and the channels, and it is never active, so
	there is nothing to clear.

*clear* _key_
	Clears the alert of that key, which tells the owner it is over. It
	is not an error when the alert is not active.

*--json*
	With *list*, *history* and *status*, prints what alertd answered,
	a line of JSON each (the objects *alertd*(8) describes), for a
	script.

# SOURCES

alertd names each alert's source by the resource domain of the process
that raised it (*radix-resource-domain*(2)), not by anything the
program says. A service's script has the service's name. A command run
with *elevate*(8) is started by *authd*(8) and has the source *authd*,
whoever runs it, so every alert raised from an administrator's shell
belongs to *authd*, and any shell of an administrator clears it. Give
such an alert a key with the name of its script, such as *backup-late*,
so that it does not meet another's.

*raise*, *event* and *clear* act on the alerts of that source, never on
another's: *alerts clear smart:wd-wcc4e1234567* from a shell does not
clear what *diskhealth*(8) raised.

# EXAMPLES

A script of root's that says when a job fails and when it is mended:

```
#!/bin/sh
if /volumes/data/bin/copy-nightly; then
	alerts clear copy-failed
else
	alerts raise copy-failed --title "The nightly copy failed" \
		--detail "Run /volumes/data/bin/copy-nightly by hand to see why."
fi
```

An event for something that happened and needs no clearing, and a look at
what the owner was told:

```
# alerts event disk-swapped --title "The data disk was replaced"
# alerts history -n 5
```

# EXIT STATUS

0 when the command did what it was asked, and for *test* when every
channel took the test or none is set up. 1 when it could not: not run as
root, alertd not running, alertd refused the request (and says why), or
a channel refused the test. 2 for a mistake in the command line.

# FILES

_/system/live/alerts/socket_
	The socket of *alertd*(8) that *alerts* asks.

_/system/settings/alerts.toml_
	The channels (*alerts.toml*(5)).

# SEE ALSO

*alertd*(8), *alerts.toml*(5), *elevate*(8), *notifyd*(8),
*diskhealth*(8), *backup*(8), *service*(8), *dukovo-privacy*(7)
