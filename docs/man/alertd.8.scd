alertd(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

alertd - keep the alerts and tell the owner of them

# SYNOPSIS

*alertd*

# DESCRIPTION

alertd keeps the alerts of the system (ADR 0485): the things that need
the owner's attention, such as a disk that is filling, a backup that is
late or a service that keeps failing. Services and root's scripts raise
an alert on alertd's socket, alertd keeps which are active and what has
happened to them, and it tells the owner of each change once: on the
screen at once, at the next sign-in, and by email and webhooks as
*alerts.toml*(5) says. Spark runs it as the *alertd* service, after
*notifyd*(8) and before *logforward*(8).

*alerts*(8) is its command: it lists what is active and what happened,
sends a test through every channel, and raises and clears an alert from a
script.

## What an alert is

An alert has

_key_
	The source's own name for it, of the letters *a* to *z*, the
	digits and the characters *.*, *\_*, *:*, */* and *-*, up to 128
	of them: *fs-full:/data*, *smart:wd-wcc4e1234567*.

_severity_
	*warning* for something to see to soon, before it costs data, and
	*critical* for something that costs data or is about to: a disk
	that is failing, a backup that has not run for twice as long as
	it should.

_title_
	What is wrong, in one line of up to 120 characters.

_detail_
	What to do about it, up to 2000 characters, line breaks too. It
	may be empty.

An alert's *source* is the resource domain of the process that raised
it, which the kernel gives and the request cannot change
(*radix-resource-domain*(2)): a service's own name, such as *diskhealth*
or *backup*, and *authd* for a command run with *elevate*(8), which
*authd*(8) starts. An alert is told apart by its source and its key, so
two sources may use the same key, and a source clears and sets only its
own alerts.

## What is told, and when

alertd tells the owner when there is something new to hear, and only
then:

- an alert that was not active is *raised*;
- an alert that is active as a warning and is raised again as a
  *critical* one is *raised* again, so that the owner hears that it
  got worse;
- an alert that is cleared is *cleared*, with the alert as it was last
  raised, so that its title says what was cleared;
- an *event* is told once and is not kept active: something that
  happened and is not a condition, such as a restart after a kernel
  panic.

Raising an active alert again with the same or a lower severity only
updates the title and detail that *alerts list* shows, and tells nobody,
so a producer that raises what is wrong every minute never repeats
itself. Clearing an alert that is not active is not a change either.

Every change goes in the history, a line of JSON each in
_/system/state/alerts/history_. When the history passes 2000 changes it
is cut to the newest 1000. *alerts history* shows it.

alertd keeps at most 100 active alerts for each source and 1000 in all,
so that a program in a loop cannot fill the memory; a raise past either
is refused, and says so.

## The owner's screen

Each change is a notification, shown by *notifyd*(8) on the consoles
under the name *alerts*: the alert's title, or *Cleared:* and the title,
and the source and the detail below it. A critical one stays until a key
is pressed. Three changes at once are three notifications; more are one,
*N alerts changed*, and notifyd's own limit of ten in ten seconds is
kept to. A notification is not kept anywhere but notifyd's memory.
An event whose sender has shown it on the consoles in its own words, as
*sysupdate*(8) does a fallback and *updated*(8) a failed staging, is
not shown again: a newer notification would take the sender's place on
the bottom line.

alertd also keeps the notice that the next sign-in shows,
_/system/live/sign-in/alerts_, with a line for each active alert,
critical first, and the line says since when:

```
Alert: /data is 97% full (since 2026-11-13 10:02, alerts list)
```

At most eight lines are written, and the rest are counted, so that a
sign-in does not scroll away. The notice goes when no alert is active.

## Email and webhooks

For each channel that *alerts.toml*(5) names, alertd gathers the changes
the channel is to hear for 30 seconds after the first, and sends them as
one message: a disk that fills and a backup that is late arrive
together. A message holds at most 50 changes; more start the next.

An email has the subject *[host] Warning: title*, *[host] CRITICAL:
title* or *[host] Cleared: title*, with the machine's name in brackets.
Several changes make *[host] 3 alerts: first title, and 2 more*. The
body is plain text, a section for each change:

```
Raised, warning: /data is 97% full
  The disk is almost full. Run "df /data" to see what uses it.
  From metricsd at 2026-11-13 10:02:11 UTC.
```

A message to a webhook says the same in the form the service takes
(*alerts.toml*(5)), and the *json* kind is the changes as data:

```
{"version":1,"host":"server","changes":[{"state":"raised","source":
"metricsd","key":"fs-full:/data","severity":"warning","title":
"/data is 97% full","detail":"...","at":1794996131}]}
```

(on one line). *state* is *raised*, *cleared*, *event* or *test*, and
*at* is seconds since 1970.

Each channel has its own queue, in order, one message at a time, so a
server that does not answer holds up nothing but its own messages. A
message that is not delivered stays in the outbox and is tried again
after 1, 5, 15 and 30 minutes, and every hour after that. It is dropped
when it is 24 hours old, and the log says so, with the channel and the
last error. A message keeps its name through every try, so that Matrix, which drops
a name it has seen, shows it once.
At most 100 messages wait for a channel; past that the oldest is
dropped, and the log says so. When a channel is taken out of
*alerts.toml*, what waits for it is dropped at the next reload.

The outbox is kept on the disk, so a restart or a reboot loses no
message, and one that went is not sent again.

alertd sends nothing over a connection that is not encrypted, and checks
that each server's certificate is valid for its name (*alerts.toml*(5)).
It never writes a password, a token or a webhook address in the log or in
an error: a channel is known by its kind and host, such as *email to
ops@example.com* and *ntfy (ntfy.sh)*. When a server refuses, the error
quotes the server's own words, so that *alerts test* shows what to fix.

## Services

alertd asks Spark what its services are doing every 10 seconds, and
raises *service:*_name_ for each that has not been as asked for 60
seconds in a row, with *spark* as its source. A service is as asked when
it runs and passes its health check, is done, is disabled, or was
stopped by a request; it is not when it failed, should run and does not,
or fails its health check (*service*(8)). The
alert is a warning until Spark has given up on the service, and
critical after. It clears as soon as the service is as asked. A service
that fails and starts again is counted from when it was first seen
wrong, so a crash loop is not missed between its runs.

## Who raises alerts

The system's services and root's scripts raise alerts. These raise them:

*fs-full:*_mount_, *fs-filling:*_mount_
	A file system that is nearly full, and one that the last hours
	say will be before long, from *metricsd*(8), which also raises
	*btrfs-missing:*_volume_, *btrfs-behind:*_volume_, *memory:low*,
	*temp:*_sensor_ and *certificate:*_file_, the early warnings
	of its own.

*smart:*_serial_, *smart-rising:*_serial_
	A disk that *diskhealth*(8) finds failing, and the early warning
	that the counts that come before a failure are rising.

*backup-failed:*_name_, *backup-overdue:*_name_
	A backup destination whose last run failed, and one that is late
	(*backup*(8)).

*service:*_name_
	A service that is not running as asked, as above.

The events *update-fell-back*, *start-failed*, *update-failed*,
*update-not-staged*, *update-not-verified* and *kernel-panic* come from
*sysupdate*(8), *updated*(8) and *dumps*(8). The support center has a
page for each title.

## The socket

alertd answers on _/system/live/alerts/socket_. A program sends one
request, a line of JSON, and reads the answer, a line of JSON for each
item, after which alertd closes the connection. Every request has an
*ask*:

*{"ask":"raise","alert":{...}}*
	Raises the alert, which has a *key*, a *severity* of *warning* or
	*critical*, a *title* and, if it likes, a *detail*. It answers
	*{"ok":true}*.

*{"ask":"event","alert":{...}}*
	Tells of the alert once. It answers *{"ok":true}*. With
	*"shown":true*, the sender has shown it on the consoles itself: it
	goes to the channels and the history, with no notification.

*{"ask":"clear","key":"..."}*
	Clears the sender's alert of that key, which need not be active.
	It answers *{"ok":true}*.

*{"ask":"set","alerts":[...]}*
	Makes the list the whole of the sender's alerts: each is raised,
	and each of the sender's alerts that is not in the list is cleared.
	A service that works out what is wrong in each round sends this, so
	that what has mended clears by itself. It answers *{"ok":true}*, and
	changes nothing if the list would pass a limit.

*{"ask":"list"}*
	The active alerts, critical first and then the oldest, one a line:
	the *source*, the *alert*, *since* when it was raised and when it
	last *changed*, both in seconds since 1970, and then
	*{"end":true}*.

*{"ask":"history","count":20}*
	The newest *count* changes, 20 without it and at most 1000, oldest
	first: *at* in seconds since 1970, the *source*, the *state* and
	the *alert*, and then *{"end":true}*.

*{"ask":"test"}*
	Sends a test alert to every channel at once and answers a line for
	each: *channel*, *ok*, and *error* when it failed, in the words of
	the server that refused it, and then *{"end":true}*. It waits up to
	30 seconds for each channel. The test is in the history and on the
	screen, and is not sent again.

*{"ask":"status"}*
	A line for each channel: its *channel* name, *last_ok*, when it
	last sent, *last_error*, when it last failed and why, and
	*waiting*, how many messages wait, and then *{"end":true}*.

A request alertd does not take is answered with *{"error":"..."}* and
the reason. A request has 5 seconds in all to arrive and may be 16 KiB
at most, and alertd serves 16 connections at a time; one more is told
that alertd is busy.

Only root's processes and the system's services are served. The kernel
says who is at the other end of the socket (*radix-names*(2)), whatever
the request says, and anyone else is refused: *only root and the
system's services raise and read alerts; try elevate*. The socket is open
to every program so that they can be told so. The alerts hold the
system's secrets, such as where a disk fails, and tell nothing to
those who may not act on them.

A service reaches the socket through _/system/live/alerts_, which every
service's view holds read-only (*service.toml*(5)): connecting to a
socket there needs no write. Alerts from a bundle's services are not
taken yet.

## Settings

alertd reads _/system/settings/alerts.toml_ when it starts and when it
is sent SIGHUP, which *service alertd reload* does. A mistake in it
leaves out the channel it is in and nothing else, and the log says what
and where (*log alertd*). Without the file, or with one that names no
channel, alerts go to the screen only; *alerts test* says so.

# FILES

_/system/live/alerts/socket_
	The socket alertd answers on.

_/system/settings/alerts.toml_
	The channels (*alerts.toml*(5)).

_/system/state/alerts/active.json_
	The active alerts, so that they are the same after a restart.

_/system/state/alerts/history_
	What happened to the alerts, a change a line.

_/system/state/alerts/outbox.json_
	The messages that wait to be sent. They hold the alerts' words and
	no secret.

_/system/live/sign-in/alerts_
	The notice the next sign-in shows.

_/system/settings/spark/services/alertd.toml_
	The service.

# EXIT STATUS

As a service, alertd does not exit but when it is stopped with SIGTERM,
with 0; it exits with 1 if it cannot start, as when it cannot make its
socket.

# SEE ALSO

*alerts*(8), *alerts.toml*(5), *notifyd*(8), *diskhealth*(8),
*backup*(8), *sysupdate*(8), *updated*(8), *dumps*(8), *service*(8),
*spark*(8), *log*(1), *dukovo-privacy*(7)
