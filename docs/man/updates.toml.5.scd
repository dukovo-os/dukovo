updates.toml(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

updates.toml - which channel of updates the system follows, and from where

# SYNOPSIS

_/system/settings/sysupdate/updates.toml_

# DESCRIPTION

*sysupdate check*, *download* and *stage* read the update server's
repository of releases through one of its channels (*sysupdate*(8), ADR
0466). This file says which, and from which server. Without it, the
defaults hold: the stable channel of Dukovo's update server.
*sysupdate channel* _name_ writes it. It is TOML:

```
channel = "beta"
server = "https://mirror.example.org/dukovo/system/"
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

# EXAMPLES

Follow the beta channel:

```
# sysupdate channel beta
sysupdate: this system follows the beta channel from now on; it never goes back to an older version: it waits until the channel passes the one it runs
```

# SEE ALSO

*sysupdate*(8), *system-release*(8)
