backup.toml(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

backup.toml - a destination of backups, and when it is backed up

# SYNOPSIS

_/system/settings/backup/name.toml_

# DESCRIPTION

Each destination of *backup*(8) has a file of its own, named by the
destination, which *backup add* writes and *backup remove* removes (ADR
0463). The backup service reads them at each round, every hour, so a
change needs no restart. It is TOML:

```
to = "/volumes/BACKUP"
every = "day"
verify = "month"
chain = 30
chains = 2
```

*to*
	Where the backups go: a folder, written from */*, as a USB disk's
	*/volumes/*_label_, or *ssh://*_user_*@*_host_[*:*_port_]*/*_path_
	for a folder on a server, from the account's home when _path_ starts
	with *~/*. The backups are in *dukovo-backup/*_machine_ there.

*every*
	How often the service backs up to it: *day* (the default), *week*,
	or *hand*, only when *backup run* is asked.

*verify*
	How often a backup is followed by a check by a restore of the
	newest backup of each place (*backup verify --restore*): *week*,
	*month* (the default) or *never*.

*chain*
	The most backups in a chain, from its whole backup: 30 by default.
	Restoring the newest backup reads its whole chain, so a shorter one
	restores sooner, and a longer one sends fewer whole backups.

*chains*
	How many whole chains are kept: 2 by default. A chain older than
	those is removed once the newest is whole.

*places*
	The places backed up, each the top folder of a Btrfs subvolume;
	*snap.toml*(5)'s places when it is left out.

*recipients*
	Age public keys (*age1*...) every backup is encrypted to besides the
	destination's own key.

The destination's key is in _name.key_ beside it, which *backup add*
makes.

# EXAMPLES

A server, every week, with the backups also readable with a key kept
elsewhere:

```
to = "ssh://backup@nas.local/srv/backups"
every = "week"
recipients = ["age1lggyhqrw2nlhcxprm67z43rta597azn8gknawjehu9d9dl0jq3yqqvfafg"]
```

# SEE ALSO

*backup*(8), *snap.toml*(5), *service.toml*(5)
