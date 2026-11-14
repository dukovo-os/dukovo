dukovo-check(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

dukovo-check - look at the computer's security and health, and keep what
it finds

# SYNOPSIS

*dukovo-check* [*--json*]

*dukovo-check --last* [*--json*]

# DESCRIPTION

*dukovo-check* looks at what a person would want to know before it
hurts: whether other machines can reach more than they should, whether
SSH takes passwords from anyone, whether an account has no password,
whether an update is waiting, whether the clock was set, whether a
service gave up, whether an early warning is still active, and whether
the people's files have snapshots. It says for each thing whether it
passed, gave a warning, failed, or could not be looked at. It needs an
administrator, since it reads the password hashes and the firewall: run
it with *elevate*(8), which a person who is not one is told, in one
line, with status 3.

A default install has no warnings, so a warning is real. Each one has an
ID, which has a page of its own in the support center, and one sentence
on what to do. *dukovo-check* changes nothing but the result it keeps: it
makes no fix by itself, and each line says the command that fixes what
it found.

Each check prints a line when it ends: the area, the check, how it ended
and a detail. A warning or a failure has a second line with its ID and
what to do:

```
$ elevate dukovo-check
Checking this computer's security and health. Ctrl-C stops it.
network  reachable             passed   from other machines: tcp 22 (sshd), tcp 443 (nginx)
network  firewall rules        passed   2 rules; none lets every port in from everywhere
network  firewall on           passed   the firewall service loaded its rules
ssh      passwords             warning  passwords sign in over SSH (sshd_config: PasswordAuthentication yes)
         SEC-110: Add your SSH key, then set PasswordAuthentication no in /system/settings/ssh/sshd_config.
ssh      who may sign in       passed   only group admins may sign in
ssh      blocking              passed   on: 5 wrong tries in 10 minutes block an address for 10 minutes, doubling up to 1 day
accounts empty passwords       passed   every person's account has a password
accounts administrators        passed   1 administrator: alice
updates  waiting               passed   none: the last check, today at 06:12, found Dukovo 1.0.0 current
updates  last check            passed   today at 06:12
time     clock                 passed   synchronized 4 min ago
services failed                passed   none gave up
health   early warnings        passed   none active
health   hardware              skipped  hardware check has not run on this computer
backups  snapshots             passed   a snapshot of /users and /shared is made each day
13 passed, 1 warning, 1 skipped
```

A check that was skipped did not run, and says why; it is not a fault. A
check that looks at something that happens now and then, such as the
update check or the time server, is skipped in the first hour after the
computer started, since its service has not had its time yet; after that,
a check that never happened is a warning.

A result is *passed*, *warning*, *FAILED* or *skipped*. *FAILED* appears
only when it repeats a failure that another tool found: a *hardware
check* that failed, or an early warning that is critical. Such a line
keeps the ID of the tool that found it, such as *HW-113* or *MET-101*, so
that one fault has one page.

The service *dukovo-check* runs it each morning, at five and up to an
hour after, and keeps the result, so that the web console and
*dukovo-check --last* have one when nobody ran it. A run by hand keeps
its result in the same way.

# CHECKS

[[ *Area*
:- *Check*
:- *Warns when*
|  network
:  reachable
:  Never: it lists the ports other machines reach, from the firewall's list of what listens, so that the report says what is open
|  network
:  firewall rules
:  A rule of _/system/settings/firewall_ allows every port from every address, with no interface and no program named, and no *deny* of everything comes before it (*SEC-101*)
|  network
:  firewall on
:  The *firewall* service is off, failed or is not there, so that the rules are not loaded (*SEC-102*)
|  ssh
:  passwords
:  The *sshd* service is on and _sshd_config_ has *PasswordAuthentication yes* (*SEC-110*)
|  ssh
:  who may sign in
:  *sshd* is on, passwords sign in, and neither *AllowUsers* nor *AllowGroups* is set (*SEC-111*)
|  ssh
:  blocking
:  Blocking of addresses that guess passwords is turned off, *on = false* in _/system/settings/blocking.toml_ (*SEC-112*). The file is read as *authd*(8) reads it, so that the line says the settings authd holds; a file authd cannot read skips the check, and says why
|  accounts
:  empty passwords
:  A person's account that is not disabled has an empty password (*SEC-120*)
|  accounts
:  administrators
:  No person's account that is not disabled is in _admins_, so that nothing can be elevated, and Rescue asks for nobody (*SEC-121*)
|  updates
:  waiting
:  The last look at the update server found a newer release that this system takes now, and it is not installed (*SEC-130*)
|  updates
:  last check
:  The update server was last looked at over 7 days ago, or never (*SEC-131*)
|  time
:  clock
:  *timed* last set the clock over a day ago, or never (*SEC-140*)
|  services
:  failed
:  A service gave up after failed starts in a row, named in the line (*SEC-150*)
|  health
:  early warnings
:  Any early warning is active (*metricsd*(8), *alertd*(8)): one line for each, a warning for a warning and *FAILED* for a critical one, under its own ID
|  health
:  hardware
:  The last *hardware check* warned or failed: one line for each of its warnings and failures, under its own HW ID. Skipped when none has run: *dukovo-check* does not run one, which takes minutes
|  backups
:  snapshots
:  The *snap* service is off, or _snap.toml_ leaves out _/users_ or _/shared_ (*SEC-160*)

The checks of SSH look at *sshd* only while its service is on: with SSH
off, nothing signs in over it, whatever its file says, and they pass with
"SSH is off". The check *waiting* looks at the release this system runs
when the look was made: a look from before an update says nothing, and
the check is skipped until the next one.

# OPTIONS

*--json*
	Print the result when the checks end, as the JSON that is kept,
	instead of a line for each check as it ends. See *RESULT*.

*--last*
	Print the kept result of the last run, as the run showed it and with
	when it ended, without checking again. It needs no elevation: any
	account in _admins_ may read it, which the file's mode allows. With
	*--json*, it prints the file.

# RESULT

Each run under *elevate*, and each run of the service, keeps its result
in _/system/state/dukovo-check/last.json_, which *dukovo-check --last*
and the web console read. The file is mode 0640, owned by root and the
group _admins_. It is replaced whole, by a new file renamed over the old,
so that a reader finds the last result or the one before, never half of
one. A run that Ctrl-C stopped keeps no result.

*dukovo-check --json* prints the same.

```
{
  "version": 1,
  "checked": "2026-11-20T05:17:03Z",
  "release": "1.0.0",
  "results": [
    {
      "area": "ssh",
      "check": "passwords",
      "result": "warning",
      "detail": "passwords sign in over SSH (sshd_config: PasswordAuthentication yes)",
      "id": "SEC-110",
      "fix": "Add your SSH key, then set PasswordAuthentication no in /system/settings/ssh/sshd_config."
    },
    {
      "area": "health",
      "check": "hardware",
      "result": "skipped",
      "detail": "hardware check has not run on this computer"
    }
  ],
  "counts": { "passed": 13, "warning": 1, "failed": 0, "skipped": 1 }
}
```

_version_ is the version of this format, 1. A program that reads it must
not read a version above the one it knows, since a later version may mean
something else by what it has in common. Fields are added, not changed or
taken away, within a version, so a program should ignore fields it does
not know.

_checked_ is when the run ended, in UTC, and _release_ the release of the
system that ran it.

Each of _results_, in the order of the report, has an _area_, one of
*network*, *ssh*, *accounts*, *updates*, *time*, *services*, *health* and
*backups*; the name of the _check_ as the report gives it; a _result_ of
*passed*, *warning*, *failed* or *skipped* (in lower case here, where the
report writes *FAILED*); and a _detail_. A warning or a failure also has
an _id_ and a _fix_, a sentence on what to do; they are left out of a
line that has none, as an early warning has none when its alert carries
no ID. A check that finds several things, such as the early warnings,
gives a line for each, with the same _check_.

_counts_ says how many lines ended each way. The file says nothing of a
score: what a warning weighs is for the reader to decide.

Each run also writes one line to the log, at the level of its status
(error for 1, warning for 2, information otherwise), with how many
checks ended each way and the IDs it found:

```
dukovo-check: check ended with status 2: 13 passed, 1 warning, 1 skipped (SEC-110)
```

*log -s dukovo-check* shows them (*log*(1)).

# IDS

Each warning has an ID, and a page with that ID in the support center
says what it means, why it matters and what to do.

[[ *ID*
:- *Area*
:- *When*
|  SEC-101
:  network
:  A rule allows every port in from everywhere
|  SEC-102
:  network
:  The firewall service is off, or failed
|  SEC-110
:  ssh
:  Passwords sign in over SSH
|  SEC-111
:  ssh
:  Passwords sign in over SSH and no list says who may
|  SEC-112
:  ssh
:  Blocking of addresses that guess passwords is off
|  SEC-120
:  accounts
:  A person's account has an empty password
|  SEC-121
:  accounts
:  No account is an administrator
|  SEC-130
:  updates
:  An update was found, and is not installed
|  SEC-131
:  updates
:  The update server has not been looked at for 7 days, or ever
|  SEC-140
:  time
:  The clock was not set by a time server in the last day, or ever
|  SEC-150
:  services
:  A service gave up after failed starts in a row
|  SEC-160
:  backups
:  No snapshot is made of _/users_ and _/shared_

A line of *health* that repeats another tool's finding has that tool's
ID: *MET-101* to *MET-104* for the early warnings of *metricsd*(8), and
*HW-101* to *HW-153* for those of *hardware*(8).

# EXIT STATUS

0
	Every check that ran passed, and some did.

1
	A check failed: a failure that another tool found, which a line
	repeats.

2
	No check failed, and one gave a warning.

3
	It could not run: a mistake in the command line, the person was not
	an administrator, no kept result could be read by *--last*, or no
	check could be made (every one was skipped).

*dukovo-check --last* gives the status of the result that is kept, or 3
when none is.

# FILES

_/system/state/dukovo-check/last.json_
	The result of the last run, as described under *RESULT*.

_/system/settings/spark/services/dukovo-check.toml_
	The service that runs it each morning, with its *[timer]*
	(*service.toml*(5)). *service dukovo-check disable* stops the daily
	run.

_/system/settings/firewall_, _/system/settings/ssh/sshd_config_,
_/system/settings/blocking.toml_, _/system/settings/snap.toml_
	The settings the checks read (*firewall*(8), *sshd_config*(5),
	*blocking.toml*(5), *snap.toml*(5)).

_/system/settings/passwd_, _/system/settings/group_,
_/system/state/authd/passwords_
	The accounts, which *authd*(8) keeps. The passwords are root's alone.

_/system/state/timed/last_, _/system/state/sysupdate/checked_,
_/system/state/hardware/last.json_
	When *timed*(8) last set the clock, when the update server was last
	looked at (*updated*(8), *update*(8)), and the result of the last
	*hardware check*.

_/devices/kmsg_
	The log, which the run writes its line to.

# EXAMPLES

Check, and read the report:

```
elevate dukovo-check
```

See the result of the last run, which the service made this morning,
without checking again:

```
dukovo-check --last
```

The result for another program, or to count its warnings:

```
dukovo-check --last --json | jq '.counts.warning'
```

After a fix, check again, and see the warning gone:

```
elevate dukovo-check
```

# SEE ALSO

*hardware*(8), *firewall*(8), *sshd*(8), *sshd_config*(5), *blocking.toml*(5), *authd*(8),
*update*(8), *updated*(8), *timed*(8), *service*(8), *snap*(1),
*metricsd*(8), *alertd*(8), *elevate*(8), *log*(1), *systools*(1)
