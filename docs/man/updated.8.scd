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
  or firmware update by itself, and goes back to the version before
  when the release it runs is withdrawn with that advice (ADR 0469).
  With a *restart* time as well, it restarts the machine into the
  update at that time of day; without one it waits for a restart.
- *check = "off"* turns the checks off.

It never restarts the machine otherwise. What it does goes to the log
under *updated*, and what it tells goes to *notifyd*(8).

*--once* checks once, now, and ends, as a test or a script would want.

# PRIVACY

Each check reads the update channel the machine follows and the app
catalogs it trusts; the requests carry nothing about the machine
(*dukovo-privacy*(7)).

# FILES

_/system/settings/sysupdate/updates.toml_
	What it does (*updates.toml*(5)).

_/system/settings/spark/services/updated.toml_
	Its service file; *enabled = false* there turns it off
	(*service*(8)).

# SEE ALSO

*update*(8), *sysupdate*(8), *bundled*(8), *updates.toml*(5), *notifyd*(8)
