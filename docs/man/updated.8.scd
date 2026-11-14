updated(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

updated - check for updates once a day, and tell or take them as the owner chose

# SYNOPSIS

*updated* [*--once*]

# DESCRIPTION

updated is the updater's service (ADR 0502). Spark starts it after the
network; ten minutes later, and then once a day, it checks for a newer
system, firmware set and apps, as *update*(8) does, and does what
*updates.toml*(5) says:

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

Once a week it reads the running slot back against its release's signed
list, as *sysupdate verify* does, and tells the consoles, as critical,
when the slot is not the release.

It tells *alertd*(8) of the critical event *update-not-verified* when
the slot is not the release, and of the warning *update-not-staged*
when an unattended machine's update could not be staged.

Whatever the settings, a staged release that the release keys withdraw
is unstaged at the next check, and the console says so: it is never
tried (ADR 0469).

It never restarts the machine otherwise. What it does goes to the log
under *updated*, and what it tells goes to *notifyd*(8).

*--once* checks once, now, and ends, as a test or a script would want.

# PRIVACY

Each check reads the update channel the machine follows and the app
catalogs it trusts; the requests carry nothing about the machine
(*dukovo-privacy*(7)). It also fetches the signatures of
*troubleshoot*(8), a signed file of the update server's, and keeps them
when they are newer than those in use: they are
no update of the system, and only data (*troubleshoot-signatures*(5)).

# FILES

_/system/settings/sysupdate/updates.toml_
	What it does (*updates.toml*(5)).

_/system/live/netd/metered_
	Whether the connection is metered, which *netd*(8) writes; updated
	takes the connection as not metered where there is no such file.

_/system/settings/spark/services/updated.toml_
	Its service file; *enabled = false* there turns it off
	(*service*(8)).

# SEE ALSO

*update*(8), *sysupdate*(8), *bundled*(8), *updates.toml*(5), *notifyd*(8),
*netd*(8), *alerts*(8)
