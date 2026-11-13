updates.toml(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

updates.toml - which channel of updates the system follows, from where, and what the updater does

# SYNOPSIS

_/system/settings/sysupdate/updates.toml_

# DESCRIPTION

*sysupdate check*, *download* and *stage* read the update server's
repository of releases through one of its channels (*sysupdate*(8), ADR
0466). This file says which, and from which server, and what
*updated*(8) does with what it finds (ADR 0502). Without it, the
defaults hold: the stable channel of Dukovo's update server, checked
once a day, with whoever is signed in told of each update. *sysupdate
channel* _name_ and *update hold* write it. It is TOML:

```
channel = "beta"
server = "https://mirror.example.org/dukovo/system/"
apps = "auto"

[hold]
system = "2026.3.1"
```

# SETTINGS

*channel*
	*stable*, the default, the releases most people run; *beta*, each
	release two weeks before it is stable; or *nightly*, a build of
	every change, for testing. A system never goes back to an older
	version through a channel: after a move from beta to stable it
	waits until stable passes the version it runs.

*server*
	The repository's address, an _https://_ one, or an _http://_ one
	on a network the owner trusts, by default
	_https://updates.dukovo.org/system/_. A mirror serves the same
	files: everything a server sends is checked against the root this
	system trusts, so a mirror can withhold an update for a while but
	cannot change one, or offer an older one as new.

*canary*
	*true* for a machine that takes each release on the day its
	channel offers it, before the waves reach the rest (ADR 0469);
	*false*, the default. The project's own canaries say *true*. Every
	other machine takes a release when its wave opens, from day 1 to
	day 7, by the number it keeps in
	_/system/state/sysupdate/wave_.

*check*
	What *updated*(8) does once a day: *notify*, the default, tells
	whoever is signed in that an update is ready, and changes nothing;
	*download* fetches the system update as well, so that staging it
	needs no network; *off* checks nothing.

*apps*
	*notify*, the default, tells of the apps' updates; *auto* updates
	the apps installed for everyone by itself.

*metered*
	What *updated*(8) does while the connection is metered, which
	*netd*(8) says (a card's *metered* setting in *network*(5), or the
	DHCP server's): *wait*, the default, still fetches the channel's
	few small signed files to find out what is on offer and tells
	whoever is signed in, but downloads and stages nothing, of the
	system, its firmware or the apps, until a check finds the
	connection not metered; *download* takes a metered connection as
	any other. *elevate update apply* and *update apply* go ahead
	whatever the connection.

*unattended*
	*true* for a machine nobody watches: *updated*(8) stages each
	system or firmware update by itself, and goes back to the version
	before when the one it runs is withdrawn with that advice.
	*false*, the default.

*restart*
	With *unattended*, the time of day, _HH:MM_ in local time, at
	which *updated*(8) restarts the machine into an update it staged.
	Without it, the update waits for a restart. Only an unattended
	machine takes one.

*[hold]*
	The versions held back, as *update hold* writes them: *system*,
	a Dukovo version, and *firmware*, a firmware set. A release or set
	no newer than the one held is not taken, nor told of as ready; a
	newer one is taken as usual. A held version gets no security
	fixes. *update unhold* removes the line.

# EXAMPLES

Follow the beta channel:

```
# sysupdate channel beta
sysupdate: this system follows the beta channel from now on; it never goes back to an older version: it waits until the channel passes the one it runs
```

A laptop that takes its updates only when it is not on a phone's
hotspot is as the defaults have it. A machine whose metered uplink
is the only one it has and that must stay current:

```
metered = "download"
```

A server that restarts itself into each update at 04:00:

```
unattended = true
restart = "04:00"
```

# SEE ALSO

*sysupdate*(8), *update*(8), *updated*(8), *system-release*(8),
*network*(5), *netd*(8)
