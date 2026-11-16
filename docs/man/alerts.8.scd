alerts(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

alerts - show the alerts, set up and test where they go, and raise them from a script

# SYNOPSIS

*alerts* [*list*] [*--json*]

*alerts history* [*-n* _count_] [*--json*]

*alerts test* [*--json*]

*alerts status* [*--json*]

*alerts raise* _key_ *--title* _text_ [*--detail* _text_] [*--critical*] [*--about* _word_]

*alerts event* _key_ *--title* _text_ [*--detail* _text_] [*--critical*] [*--about* _word_]

*alerts clear* _key_

*alerts email set* *--to* _address_ *--server* _host_ [*--port* _587_|_465_] [*--from* _address_] [*--user* _name_ (*--password -* | *--keep-password*)] [*--severity* _severity_] [*--cleared* _yes_|_no_] [*--about* _words_]

*alerts email remove*

*alerts webhook add* _kind_ [*--room* _id_] [*--severity* _severity_] [*--cleared* _yes_|_no_] [*--about* _words_]

*alerts webhook remove* _number_

*alerts push key* [*--json*]

*alerts push add* *--name* _name_ [*--severity* _severity_] [*--cleared* _yes_|_no_] [*--about* _words_]

*alerts push remove* _id_

*alerts channel* _channel_ *--about* _words_

# DESCRIPTION

*alerts* is the command of *alertd*(8), which keeps the alerts of the
system (ADR 0485): what needs the owner's attention, and what has
happened to it. *alerts* shows them, sends a test alert through every
channel that *alerts.toml*(5) names, sets up those channels and the
browsers that get alerts (ADR 0566), says which kinds of alert each is
told of, and lets a script of root's raise and clear an alert of its
own. The web console's Alerts page runs these commands.

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
	when it last failed and why, a test alert counting as any other,
	and how many messages wait to be tried again (*alertd*(8) says
	when). A channel that is told of only some kinds of alert has
	*, told of:* and those kinds after its name, as in *email to
	ops@example.com, told of: data at risk, hardware*:

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

*raise* _key_ *--title* _text_ [*--detail* _text_] [*--critical*] [*--about* _word_]
	Raises an alert, as a service does. It is a warning, or critical
	with *--critical*, for what costs data or is about to. *--about*
	says what the alert is about, one of *data*, *stopped* and
	*hardware* (*Kinds of alert* below); without it the alert is about
	nothing in particular and goes to every channel. Another word is a
	mistake of the command line, status 2, so that a script's typo is
	caught where it is made. The _key_
	names it for as long as it is active: of the letters *a* to *z*,
	the digits and *.*, *\_*, *:*, */* and *-*, up to 128; the title is
	one line of up to 120 characters; the detail is what to do about
	it, up to 2000. *raise* prints nothing when alertd took it.
	Raising an alert that is active only updates it, unless it was a
	warning and is critical now, so a script may raise what is wrong on
	each run without repeating itself.

*event* _key_ *--title* _text_ [*--detail* _text_] [*--critical*] [*--about* _word_]
	Tells of something that happened, once. It is kept in the history
	and told to the owner and the channels, and it is never active, so
	there is nothing to clear. *--about* is as for *raise*.

*clear* _key_
	Clears the alert of that key, which tells the owner it is over. It
	is not an error when the alert is not active.

*email set* *--to* _address_ *--server* _host_ [...]
	Sets up the email channel: writes the *[email]* table of
	*alerts.toml*(5) and has alertd read it again. *--to* names an
	address the alerts go to, and may be given more than once, or with
	commas between the addresses. *--server* is the provider's
	submission server, a host name, and *--port* is *587*, the default,
	which starts TLS with STARTTLS, or *465*, which has TLS from the
	first byte. *--user* is the account to sign in with, and needs its
	password: *--password -* takes it from the first line of the
	standard input, never from an argument, and from the terminal with
	the echo off when the standard input is one; *--keep-password*
	keeps the one the file has. *--from*, *--severity* (*warning* or
	*critical*), *--cleared* (*yes* or *no*) and *--about* are what
	*alerts.toml*(5) calls *from*, *severity*, *cleared* and *about*.
	A setting that is not given is left out of the table, so that it is
	its default, whatever it was; the *ca* setting, which this command
	has no option for, is kept. It prints *alerts: email set*. Run
	again, it makes the table as the options say.

*email remove*
	Takes the *[email]* table out of the file. It prints *alerts:
	email removed*, and exits with 1 when there is no email channel.

*webhook add* _kind_ [*--room* _id_] [...]
	Adds a *[[webhook]]* table after the others. _kind_ is *ntfy*,
	*matrix*, *slack*, *discord* or *json*. The address is the first
	line of the standard input and the token the second, which is left
	out for *slack* and *discord*, whose address holds the secret, and
	may be empty for the others that need none; on a terminal they are
	asked for with the echo off. *--room* is a Matrix room's ID, which
	*matrix* needs. *--severity*, *--cleared* and *--about* are as for
	*email set*. It prints *alerts: webhook* _number_ *added* and the
	channel's name, where _number_ is the webhook's place among the
	webhooks of the file, from 1, as *status --json* gives it.

*webhook remove* _number_
	Takes out the webhook in that place. The ones after it are
	numbered one less, so look at *status* again before the next.

*push key* [*--json*]
	Prints the machine's public key for Web Push, 65 bytes in
	base64url, which a browser subscribes with (RFC 8292). With
	*--json* it is *{"format":1,"public\_key":"..."}*.

*push add* *--name* _name_ [*--severity* _severity_] [...]
	Keeps the subscription of a browser, which is the first thing on
	the standard input: the JSON that the browser's push manager gives,
	with its *endpoint* and its *keys*. _name_ is what to call the
	browser, 1 to 64 characters. It prints *{"format":1,"id":"..."}*,
	the browser's ID, which *push remove* and *channel* take. A
	subscription is a secret, since whoever holds it can send to the
	browser, so it is never an argument and never printed again. The
	same endpoint again replaces what was kept for it. alertd refuses
	an endpoint that is not *https*, and one on a private network
	that *[push] private* in *alerts.toml*(5) does not list. Unlike
	the others, this changes no file: alertd keeps the browsers in a
	file of its own (*alertd*(8)).

*push remove* _id_
	Stops alerts in that browser. It prints *alerts: browser* _id_
	*removed*.

*channel* _channel_ *--about* _words_
	Says which kinds of alert a channel is told of. _channel_ is *email*,
	*webhook:*_number_ or *push:*_id_, as *status --json* gives them,
	and _words_ is *all*, which is the default and writes nothing, or
	some of *data*, *stopped* and *hardware* with commas between them.
	For the email channel and a webhook it writes the table's *about*
	in the file, or takes it out for *all* and for all three, and has
	alertd read the file again; for a browser it asks alertd, which
	keeps it. It prints the channel and what it is told of, as in
	*alerts: webhook 1 (ntfy (ntfy.sh)) is told of: data at risk*, or
	*every kind*. It exits with 1 when the channel is not there, and
	says so, and with 2 for words that are not kinds.

*--json*
	With *list*, *history*, *status* and *push key*, prints what alertd
	answered, a line of JSON each (the objects *alertd*(8) describes),
	for a script; each channel of *status* has its *kind*, *host*,
	*severity*, *cleared*, *about* (null for every kind), *number* for a
	webhook, *id* and *name* for a browser, *error\_id* with the failure,
	and *last\_ok*, *last\_error* and *waiting*. With *test*, prints one
	object: *{"format":1,"deliveries":[...]}*, with a delivery for each
	channel: its *channel*, *ok*, the *error* in the words of the server
	that refused it, and *error\_id*, the ID of the page about that
	failure, or null.

# SETTING UP CHANNELS

*email*, *webhook* and *channel* change _/system/settings/alerts.toml_
with the writer the web console uses: the file's comments and the order
of its keys stay, and it is written whole or not at all. It holds
passwords and tokens, so the command makes it root's and mode 600 before
it writes anything. What would be written is read first as alertd reads
it, and a table that alertd would leave out is refused with alertd's own
words, so that the file is not changed for the worse. Then Spark is asked
to have alertd read the file again; if alertd is not running, the command
says so, and alertd reads the file when it starts. *alerts test* says how
each channel does.

A password, a token, an address that is a secret and a subscription are
never an argument, so that none is in a process list, the shell's history
or the system's journal.

# KINDS OF ALERT

An alert is about at most one of three kinds, which decide the channels it
goes to (*alertd*(8) says which alert is which):

*data*
	Data at risk: a disk failing or wearing out, a file system nearly
	full, a mirror missing a disk, a backup failed or late.

*stopped*
	Something stopped: a service, an update or a start that failed, a
	certificate running out, the kernel stopping, memory running low.

*hardware*
	Hardware: a sensor running hot.

A channel told of *data* and *hardware* hears alerts of those kinds and
no others, and an alert that is about none of them, with every test
alert, goes to every channel whose severity lets it through. A channel is
told of every kind unless it says otherwise. The file's word for this is
*about*, since a webhook's *kind* is what it speaks.

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

Email through a provider, the password from the keyboard, then a test;
and a phone with the ntfy app that is told of data at risk and nothing
else:

```
# alerts email set --to ops@example.com --server smtp.example.com \
	--user alerts@example.com --password -
Password:
alerts: email set
# alerts test
email to ops@example.com: sent
# printf '%s\n' https://ntfy.sh/server-alerts-7f3a9c | alerts webhook add ntfy --about data
alerts: webhook 1 added (ntfy (ntfy.sh))
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
root, alertd not running, alertd refused the request (and says why), a
channel refused the test, the channel to change is not there, or alertd
would leave out what was to be written. 2 for a mistake in the command
line, as an option that is not one, a missing *--to* or *--server*, and
a word that is no kind of alert.

# FILES

_/system/live/alerts/socket_
	The socket of *alertd*(8) that *alerts* asks.

_/system/settings/alerts.toml_
	The channels (*alerts.toml*(5)), which *email*, *webhook* and
	*channel* change.

_/system/state/alerts/push.json_
	The browsers, which *push* asks alertd to change; only root may
	read it.

# SEE ALSO

*alertd*(8), *alerts.toml*(5), *elevate*(8), *notifyd*(8),
*diskhealth*(8), *backup*(8), *service*(8), *dukovo-check*(8),
*dukovo-privacy*(7)
