systemctl(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

systemctl - start, stop and look at services with systemd's verbs

# SYNOPSIS

*systemctl* *start* | *stop* | *restart* | *reload* | *enable* | *disable* _name_ ...

*systemctl status* [*-n* _lines_] _name_ ...

*systemctl is-active* | *is-enabled* | *is-failed* [*-q*] _name_ ...

*systemctl list-units* [*--type=service*|*timer*] [*--all*] [*--failed*] [*--no-legend*]

*systemctl list-timers* [_name_ ...]

*systemctl cat* _name_ ...

*systemctl show* [*-p* _property_[,_property_]] [*--value*] _name_ ...

*systemctl daemon-reload*

# DESCRIPTION

systemctl is *service*(8) with the verbs, the words and the exit
statuses of systemd's systemctl, for the habits and scripts that come
from Linux servers (ADR 0478). A unit is a service: _name_ and
_name_*.service* are the same, and _name_*.timer* is the timer in the
service's file (*service.toml*(5)). Dukovo has no other kind of unit: a
socket, a mount or a target says what to run instead.

The verbs that change a service need root, as *service*(8)'s do, and say
so otherwise: *elevate systemctl restart* _name_ (*elevate*(8)).

# VERBS

*start*, *stop*, *restart*
	As *service* _name_ *start*, *stop* and *restart*: they wait until
	the service has settled and say nothing when it did. *restart*
	starts a service that is stopped, as systemd's does. *--no-block*
	returns at once. A name Spark does not know is *Unit* _name_*.service
	not found.* with status 5.

*reload*
	Asks the running service to read its settings again, as *service*
	_name_ *reload* does (SIGHUP).

*enable*, *disable*
	As *service* _name_ *enable* and *disable*: the service starts now
	and at each boot, or stops now and no longer starts at boot. This is
	what *systemctl enable --now* does elsewhere, which *--now* asks for
	and is taken.

*status*
	As *service* _name_ *status*: the state, what the service holds, its
	processes and its last log lines (*-n* sets how many). Its status is
	0 for a service that runs, 3 for one that does not, and 4 for a name
	Spark does not know. For _name_*.timer*, the times it ran and runs.

*is-active*
	Prints each service's state in systemd's words: *active* (running, or
	done as a setup step does), *activating* (waiting, starting, or to start
	again), *deactivating*, *failed* or *inactive* (stopped, and for a
	name Spark does not know). The status is 0 if any is active, 3 if
	none is. *-q* prints nothing.

*is-enabled*
	Prints *enabled* or *disabled* for each service, as it starts at
	boot or not. The status is 0 if all are enabled and 1 otherwise,
	and a name Spark has no file for is an error.

*is-failed*
	Prints each service's state, as *is-active*. The status is 0 if any
	has failed (Spark gave up on it) and 1 if none has.

*list-units*
	The services that run or are on their way, or have failed, with
	_UNIT_, _LOAD_, _ACTIVE_, _SUB_ and _DESCRIPTION_ as systemd's
	table has them; *--all* adds those that are stopped, *--failed* lists
	those that failed alone, and *--no-legend* leaves out the header and
	the count. *--type=service* lists the services alone and
	*--type=timer* the timers alone. Every service Spark has is loaded.

*list-timers*
	Each timer with when it runs next and when it last ran, and how far
	away those are, soonest first; names choose which. They are timerd(8)'s
	(*service.toml*(5), *[timer]*).

*cat*
	A service's file, under a line that names it; the system's own copy
	when the file has not been changed.

*show*
	The properties systemctl shows of a service, as _Name_=_value_
	lines, and with *--value* the values alone; *-p* chooses them, in a
	list or repeated: _Id_, _Description_, _LoadState_ (*loaded*, or
	*not-found* for a name Spark does not know), _ActiveState_,
	_SubState_, _UnitFileState_ (*enabled* or *disabled*), _MainPID_,
	_ExecMainStatus_ (the exit code of the last end, or the signal that
	ended it), _NRestarts_ (the starts since boot after the first) and
	_MemoryCurrent_ (the memory charged to the service's domain, or
	*[not set]* before Spark has made one). _SubState_ is *running*,
	*exited*, *start*, *start-pre*, *auto-restart*, *stop-sigterm*,
	*failed* or *dead*. A property systemd has and Dukovo does not is left
	out, as systemd leaves out one it does not know.

*daemon-reload*
	Spark takes a changed service file as soon as it is written, so
	there is nothing to reload: this checks every file and says what Spark
	does with each, as *service check* does.

# WHAT DUKOVO HAS INSTEAD

Each of these says in one line what to run, and ends with status 1:

- *mask*, *unmask*: *elevate service* _name_ *disable*.
- *edit*, *set-property*, *revert*, *link*: *elevate service edit* _name_.
- *isolate*, *default*, *rescue*, *emergency*: there are no targets;
  *service list* shows what runs.
- *poweroff*, *reboot*, *halt*, *kexec*: *elevate poweroff*, *reboot* or
  *halt* (*shutdown*(8)). *suspend* and *hibernate* are not there.
- *list-dependencies*: *service* _name_ *status* and *explain*.
- *list-unit-files*: *service list*. *list-sockets*: *ss -l*.
- *try-restart*, *reload-or-restart*, *reenable*: *elevate service*
  _name_ *restart*.
- *kill*, *reset-failed*, *preset*, *list-jobs* and the environment
  verbs: the line says why there is no such verb.
- *--user*, *--global*: Dukovo has no services per person; *crontab*(1)
  runs a person's jobs. *-H*, *-M*: *ssh* _host_ *systemctl* ...

*--no-pager*, *--no-ask-password*, *--full* and *--plain* are taken and
change nothing.

# EXIT STATUS

As systemctl's: 0, 1 for a failure or a verb Dukovo does not have, 3 for
*is-active* of a service that is not active and for *status* of one that
is not running, 4 for *status* of a name Spark does not know, and 5 when
a service to start, stop or reload is not known.

# SEE ALSO

*service*(8), *service.toml*(5), *spark*(8), *journalctl*(1),
*hostnamectl*(1), *timedatectl*(1), *crontab*(1), *elevate*(8)
