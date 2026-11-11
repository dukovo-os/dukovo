health.toml(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

health.toml - what a new version of the system must do to prove itself

# SYNOPSIS

_/system/settings/sysupdate/health.toml_

# DESCRIPTION

After an update, the new version of the system starts as a try, and the
health check, *sysupdate health*, decides whether it proves itself and
becomes the default, or falls back to the version before (*sysupdate*(8),
ADR 0453). This file says what it asks. The health check reads it at
every start; without it, or with one it cannot read, the defaults hold,
and the log says why. It is TOML:

```
essential = ["logd", "authd", "consoled", "netd", "sshd"]
stable = 120
deadline = 600

[[check]]
name = "web site"
command = ["curl", "-fsS", "http://localhost/"]
required = true
timeout = 30
```

*essential*
	The services the machine needs, by name. Each that is turned on must
	be running; one that is turned off, or ended with status 0 and is not
	meant to restart, does not count. By default: logging, sign-in, the
	consoles, the network and SSH.

*stable*
	Seconds the essential services must run, none of them restarting,
	before an update counts as proved: 120 by default.

*deadline*
	The most seconds an update has to start them all: 600 by default.

*[[check]]*
	A check of your own: a command that passes by exiting with 0, run
	once the essential services are stable. Its keys:

	*name*
		What the log calls it.

	*command*
		The program and its arguments, as a list; no shell.

	*required*
		*true* if an update fails when the check fails. A wanted
		check, *false* by default, is only logged.

	*timeout*
		Seconds it may take before it fails: 60 by default.

An update being tried fails at once when an essential service restarts
three times, or stops after a failure; when the essential services are
not all running by the deadline; or when a required check fails. It
then gives up its remaining tries, and the machine restarts into the
version before it, with the settings from before the update. On any
start that is not a try, the checks only report, and the start is seen
through once the essential services run.

# EXAMPLES

Hold updates to a web site's answer as well, and log whether the backups
mount:

```
[[check]]
name = "web site"
command = ["curl", "-fsS", "http://localhost/"]
required = true

[[check]]
name = "backup disk"
command = ["findmnt", "/volumes/BACKUP"]
```

# SEE ALSO

*sysupdate*(8), *service.toml*(5), *spark*(8)
