timerd(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

timerd - start services and run people's jobs at their times

# SYNOPSIS

*timerd*

# DESCRIPTION

timerd keeps time for the system (ADR 0478). It starts a service at the
times its file's *[timer]* says (*service.toml*(5)), and runs the lines
of people's crontabs (*crontab*(5)) and the jobs they queued with *at*(1).
It is a service of its own, _/system/commands/timerd_, which Spark
starts; *service timerd status* says whether it runs, and *log timerd*
shows what it did.

A service with a timer is started by timerd with the same request as
*service start --by timer*: Spark starts it as it would any other start,
checks its needs, and runs it as its file says. A timer does not turn a
service on. A service whose file has a timer, and that is enabled, is
off between its times, and a time that comes while it runs is skipped,
with a line in the log. *service status* shows when it last started and
when its timer starts it next, and *service list --timers* lists every
timer. timerd reads the service files when it starts, once a minute
after, and again when it is asked for the timers, which *service
enable* and *disable* ask for once they have changed a file that has a
timer: the change counts from then, not from the next minute.

A job of a person is run for them by *authd*(8), as their account, with
_/bin/sh_ and the environment their crontab or *at* gave it. Whether it
runs while they are signed out is their own setting, *crontab --keep-running*
(*crontab*(1)), which an administrator can bar with
*user set --jobs-signed-out never* (*user*(8)).

# TIMES

Times are those of the system's time zone, which *timedatectl*(1)
shows and sets, or the zone of a timer's *time_zone* or a crontab's
*CRON_TZ*. A calendar names minutes, hours, days of the month, months and
days of the week, as a crontab line's five fields do. A time that does
not exist, because a change to summer time skipped it, runs when the
clock passes it; a time that comes twice, because a change back repeated
it, runs once.

timerd sleeps until the next time, and for a minute at most. On each wake
it compares the clock with the monotonic clock. When the clock has moved
forward, each timer whose time it passed runs once, whatever the number
of times it passed. When it has moved back, no time that already ran runs
again, and each timer's next time is worked out from its last run. Either
way, a time counted from the machine's start (*every* for a timer that
has not run yet, and *after_boot*) moves with the clock, so it comes as
long after the start as the timer says.

*spread* puts a time off by up to as long as the setting says. How long
is worked out from the timer's name and a random number timerd keeps in
_/system/state/timerd/seed_, so that it is the same for a timer each
time and differs between machines.

# AFTER THE MACHINE WAS OFF

When timerd starts, a timer with *catch_up* (which is on unless the file
says otherwise) whose time came while the machine was off runs once, 20
seconds after timerd starts, so that the start of the machine is over
first. A crontab line does the same, as *anacron* does, and *\@reboot*
lines run once at each start, 20 seconds after it. An *at* job whose time
passed runs then too. A job that cannot run for a person, because they
are signed out and have not asked for it, is counted; their next sign-in
says how many did not run.

# LIMITS

One job runs at a time for each timer. A crontab holds 500 lines and 64
KiB of text; a person queues 200 *at* jobs, and the system 1000. A job's
output is kept to its last 64 KiB, and mailed to the crontab's *MAILTO*
when a mail server is set (*sendmail*(8)).

# FILES

_/system/live/timerd/socket_
	Where *crontab*(1), *at*(1) and *service*(8) ask. Anyone may
	ask for their own crontab and jobs; an administrator asks for those
	of others, and for the list of timers.

_/system/state/timerd_
	What timerd keeps: _seed_, _timers.json_ (each timer's last run),
	_crontabs/USER_, _at/ID.json_, _runs/_ (the last run of each
	line and job) and _reboot_. Do not edit them: use
	*crontab*(1) and *at*(1).

# EXIT STATUS

timerd runs until it is stopped. It ends with 1 when it cannot make
its socket or read its state.

# SEE ALSO

*crontab*(1), *crontab*(5), *at*(1), *logturn*(8), *service*(8),
*service.toml*(5), *spark*(8), *authd*(8), *systools*(1)
