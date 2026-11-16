updated(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

updated - check for updates once a day, and tell or take them as the owner chose

# SYNOPSIS

*updated* [*--once*]

# DESCRIPTION

updated is the updater's service (ADR 0502). Spark starts it after the
network; ten minutes later, and then once a day, or each hour on a
canary, it checks for a newer system, firmware set and apps, as
*update*(8) does, and does what *updates.toml*(5) says:

- By default (*check = "notify"*, *apps = "notify"*) it tells whoever is
  signed in, on each console, that an update is ready and that
  *elevate update apply* stages it. It changes nothing.
- *check = "download"* fetches the system update first, so that staging
  it later needs no network.
- *apps = "auto"* updates the apps installed for everyone by itself.
- *unattended = true*, for a machine nobody watches, stages the system
  or firmware update by itself, through the service *sysupdate-stage*,
  as *update apply* does, which holds the system's subvolumes and no
  home folder (*sysupdate*(8), ADR 0473), and goes back to the version before
  when the release it runs is withdrawn with that advice (ADR 0469).
  With a *restart* time as well, it restarts the machine into the
  update at that time of day; without one it waits for a restart.
  Just before that restart it asks the update server once more: while
  the release's waves are held it waits, and a withdrawn one is
  unstaged instead.
- *check = "off"* turns the checks off.
- While the connection is metered (*netd*(8) says so in
  _/system/live/netd/metered_), it still fetches the channel's small
  signed files, as *check = "notify"* does, but downloads and stages
  nothing, whatever the other settings say: it tells that an update is
  ready to download, that the connection is metered and that it waits
  for another one, or for *elevate update apply*. The next check on a
  connection that is not metered goes on as usual. *metered =
  "download"* in *updates.toml*(5) turns this off.

A canary (*canary = true* in *updates.toml*(5), made by *sysupdate canary
enrol*) takes each release on the day its channel offers it, which is why
it checks each hour, and tells the update server how it does (ADR 0533).
After its first check since the start it starts the Spark job
*dukovo-canary-report*, which runs *sysupdate canary report* and which
Spark runs each hour as well, so that a restart into a new release is
reported soon (*sysupdate*(8)). It does not wait for the job: what the
job says is in its own log. A machine with no *canary_name* starts
nothing.

Once a week it reads the running slot back against its release's signed
list, as *sysupdate verify* does, and tells the consoles, as critical,
when the slot is not the release.

It tells *alertd*(8) of the critical event *update-not-verified* when
the slot is not the release, and of the warning *update-not-staged*
when an unattended machine's update could not be staged.

Whatever the settings, a staged release that the release keys withdraw
is unstaged at the next check, and the console says so: it is never
tried (ADR 0469).

Each check ends by keeping what it found, as *update check --json*
says it, in _/system/state/sysupdate/last-check.json_, which *update
last* reads: the web console shows it when it opens, so that a page does
not ask the update server. The record is replaced whole, so that a
reader finds the last one or the one before. A check that cannot be
kept says so in the log and goes on.

It never restarts the machine otherwise. What it does goes to the log
under *updated*, and what it tells goes to *notifyd*(8).

*--once* checks once, now, and ends, as a test or a script would want.

# PRIVACY

Each check reads the update channel the machine follows and the app
catalogs it trusts; the requests carry nothing about the machine
(*dukovo-privacy*(7)). It also fetches the signatures of
*troubleshoot*(8), a signed file of the update server's, and keeps them
when they are newer than those in use: they are
no update of the system, and only data (*troubleshoot-signatures*(5)). A
canary's report, which the job *dukovo-canary-report* sends and updated
does not, is described in *dukovo-privacy*(7).

# FILES

_/system/settings/sysupdate/updates.toml_
	What it does (*updates.toml*(5)).

_/system/state/sysupdate/last-check.json_
	What the last check found, one line, written after each check
	(*update*(8), *update last*).

_/system/live/netd/metered_
	Whether the connection is metered, which *netd*(8) writes; updated
	takes the connection as not metered where there is no such file.

_/system/settings/spark/services/updated.toml_
	Its service file; *enabled = false* there turns it off
	(*service*(8)).

_/system/settings/spark/services/dukovo-canary-report.toml_
	The service file of the job that reports a canary's state, which
	updated starts after its first check (*sysupdate*(8)).

# SEE ALSO

*update*(8), *sysupdate*(8), *bundled*(8), *updates.toml*(5), *notifyd*(8),
*netd*(8), *alerts*(8)
