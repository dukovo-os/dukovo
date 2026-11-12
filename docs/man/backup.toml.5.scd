backup.toml(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

backup.toml - a destination of backups, and when it is backed up

# SYNOPSIS

_/system/settings/backup/name.toml_

# DESCRIPTION

Each destination of *backup*(8) has a file of its own, named by the
destination, which *backup add* writes and *backup remove* removes (ADRs
0463 and 0465). The backup service reads them at each round, every hour, so a
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
	*/volumes/*_label_ or a mounted share; *ssh://*_user_*@*_host_[*:*_port_]*/*_path_
	for a folder on a server, from the account's home when _path_ starts
	with *~/*; or *s3://*_host_[*:*_port_]*/*_bucket_[*/*_prefix_] for
	S3 storage over HTTPS, *s3+http://* for one on the local network
	over plain HTTP. The backups are in *dukovo-backup/*_machine_ there.

*format*
	How the backups are kept: *btrfs*, as encrypted send streams in
	chains (the default), or *restic*, as a restic repository, whose
	password is the recovery key. S3 storage keeps only *restic*, which
	is its default.

*every*
	How often the service backs up to it: *day* (the default), *week*,
	or *hand*, only when *backup run* is asked.

*verify*
	How often a backup is followed by a check by a restore of the
	newest backup of each place (*backup verify --restore*): *week*,
	*month* (the default) or *never*.

*chain*
	For *btrfs*: the most backups in a chain, from its whole backup: 30
	by default. Restoring the newest backup reads its whole chain, so a
	shorter one restores sooner, and a longer one sends fewer whole
	backups.

*chains*
	For *btrfs*: how many whole chains are kept: 2 by default. A chain
	older than those is removed once the newest is whole.

*daily*, *weekly*, *monthly*
	For *restic*: besides the newest backup of each place, one a day is
	kept for *daily* days (7 by default), one a week for *weekly* weeks
	(4) and one a month for *monthly* months (12), at most 1000 each;
	all three 0 keep the newest alone. As restic's *forget* does, the
	oldest backup of each place also counts for its day, week and month
	until newer days fill them. After each round the others are
	forgotten, and the data only they used is removed.

*append_only*
	For *restic*: *true* to never remove anything from the repository,
	which then needs a key that can write but not delete; *false* by
	default.

*places*
	The places backed up, each the top folder of a Btrfs subvolume;
	*snap.toml*(5)'s places when it is left out.

*recipients*
	For *btrfs*: age public keys (*age1*...) every backup is encrypted
	to besides the destination's own key.

The destination's key is in _name.key_ beside it, which *backup add*
makes, and an S3 destination's access key in _name.s3_, readable by
root alone, which *backup add* writes from the environment
(*backup*(8)), a line each:

```
id = AKIAIOSFODNN7EXAMPLE
secret = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY
region = eu-central-1
```

# EXAMPLES

A server, every week, with the backups also readable with a key kept
elsewhere:

```
to = "ssh://backup@nas.local/srv/backups"
every = "week"
recipients = ["age1lggyhqrw2nlhcxprm67z43rta597azn8gknawjehu9d9dl0jq3yqqvfafg"]
```

A bucket of S3 storage, written to and never cleaned up by the machine,
keeping two weeks of daily backups:

```
to = "s3://s3.eu-central-1.amazonaws.com/family-backups"
format = "restic"
append_only = true
daily = 14
```

# SEE ALSO

*backup*(8), *snap.toml*(5), *service.toml*(5)
