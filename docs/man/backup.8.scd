backup(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

backup - keep encrypted backups of snapshots on a disk, a share, a server or S3 storage

# SYNOPSIS

*backup add* [*--format* _format_] [*--every* _when_] [*--verify* _when_] [*--recipient* _key_]... [*--append-only*] _name_ _to_

*backup remove* _name_

*backup list* [*--json*]

*backup show* [*--json*] _name_

*backup find* [*--json*]

*backup run* [*--progress*] [_name_ ...]

*backup auto* [*--watch*]

*backup verify* [*--progress*] [*--restore*] _name_

*backup restore* [*--progress*] [*--key* _file_|*-*] [*--place* _place_] [*--at* _time_] [*--into* _folder_] [*--unchecked*] _name_|_source_

*backup restore --machine* [*--progress*] [*--key* _file_|*-*] [*--at* _time_] [*--card* _old_*=*_new_|*automatic*]... [*--unchecked*] _name_|_source_

*backup restore --machine --plan --json* [*--key* _file_|*-*] [*--at* _time_] [*--card* _old_*=*_new_|*automatic*]... [*--unchecked*] _name_|_source_

*backup restore --machine --cancel*

*backup key* [*--json*] _name_

*backup ssh-key* [*--json*]

# DESCRIPTION

backup copies snapshots of the places *snap*(1) keeps, */users*,
*/shared* and */system/settings* by default, and of the apps installed
for everyone, */applications*, to a destination: a folder, as a USB
disk's in */volumes* or a mounted share's, a folder on a server reached
over SSH, or a bucket of S3 storage (ADRs 0463, 0464 and 0465). A
destination keeps its backups in one of two formats: as Btrfs's send
streams (*btrfs*, the default), described here, or as a restic
repository (*restic*, which S3 storage always keeps), described under
RESTIC REPOSITORIES. The first backup of a place is its whole snapshot;
each one after holds only what changed since the one before. Every
backup is a Btrfs send stream (*subvolume*(8)), encrypted with age to
the destination's own key and written as files, so a destination can be
a disk of any filesystem, FAT and exFAT included, or any server with SSH
and a POSIX shell, and whoever holds it reads nothing but names, sizes
and times.

A round of backups makes a read-only snapshot of each place, named
*backup-*_time_ in the place's *@snapshots* folder, with the services'
backup hooks run around it (*service.toml*(5)). Each destination then
gets the stream of that snapshot's changes from the snapshot it took
before, or a whole one. A snapshot that no destination needs as the
start of its next backup is removed.

At the destination, the folder *dukovo-backup/*_machine_ holds
*backup.toml*, then a folder for each place (*users*, *shared*,
*settings*, *applications*), a folder for each chain, named by the time
of its whole backup, and in it a file for each backup, named by its
time, with its tag beside it. Each round also writes the machine's
record in the folder *machine*: the places and their paths, and which
apps are installed and what each person allowed them (bundled's
registry and the grants), encrypted and tagged as a backup is. A record
is kept while a backup it describes is.

A record is of a _format_, which it says. Format 2, which a round writes
now, adds what a person moving to new hardware looks at before anything
is received: the machine's hardware (its network cards with their MAC
addresses and models, the disks its root is on with their models and
sizes and whether they mirror, the serial port its console is on, and
its processor), the bytes a restore of each place receives, the names
of the people's accounts, and the texts of *network*(5)'s file and
*firewall*(8)'s, at most 64 KiB each, so that the settings can be
compared without receiving the settings place. It also carries
*authd*(8)'s files of the accounts: the password hashes, the sharing
passwords' hashes, the numbers of removed accounts, whether each
person's jobs run while they are signed out, each person's keyring,
sealed with a key from their password, and their second steps and API
tokens for the web console. The recovery key opens a record as it
opens the backups, so whoever holds it holds these hashes too. A record of format 1,
which earlier rounds wrote, has none of these, and is still read; one
of a format newer than this release knows is refused (BAK-107). A chain
holds *chain* backups at most (30 by default); then a whole backup
starts the next one. The newest *chains* whole chains are kept (2 by
default), and an older one is removed only once a newer one is whole. A
file over 1 GiB is cut into parts, *.age.000*, *.age.001* and on. A
file is written as *.partial* and named once whole, so a backup cut
short is never taken for one; the next round removes what it left. A
round refuses a destination whose folder has no *backup.toml*, which
*backup add* wrote there, as a share's folder before the share is
mounted, rather than keep the backups on the machine itself.

# RESTIC REPOSITORIES

With *format = "restic"* (*backup.toml*(5)), the folder
*dukovo-backup/*_machine_ is a restic repository, written by rustic's
library: files are split into chunks by their content, each chunk kept
once however many backups and places hold it, compressed, and encrypted
and authenticated with the repository's key. Its password is the
destination's recovery key. Each round adds one restic snapshot of each
place, read from the round's snapshot of it, whose path is the place's,
whose host is the machine's name, and whose tags are the round's time
and the place's folder; a file whose size, times and inode have not
changed is taken from the place's previous backup without being read
again. The machine's record is a snapshot of */dukovo-machine*, tagged
*machine*. *backup.toml* beside the repository's files says it is this
machine's.

After each round, the newest backup of each place is kept, with one a
day for *daily* days, one a week for *weekly* weeks and one a month for
*monthly* months (7, 4 and 12 by default); the rest are forgotten, and
the data only they used is removed. A destination with *append_only*
removes nothing, ever, so the key it writes with can be one that is not
allowed to delete: a machine broken into cannot then destroy its
backups. backup writes none of restic's lock files, so run Linux's
*restic forget* or *restic prune* on a repository only while the
machine is not backing up to it.

For S3 storage, *backup add* takes the access key from
*AWS_ACCESS_KEY_ID* and *AWS_SECRET_ACCESS_KEY*, and the region from
*AWS_REGION* or *AWS_DEFAULT_REGION* (*us-east-1* without either), as
Amazon's own tools do, and keeps them in
_/system/settings/backup/name.s3_. Addresses are path-style, which
Amazon's S3, Backblaze B2, Wasabi, Cloudflare R2, MinIO, Garage and
Ceph answer; requests are signed with AWS's Signature Version 4. A
share, SMB or NFS, is mounted with *mount*(8) and named as a folder; a
server over SSH needs a shell, as for streams.

# KEYS

*backup add* makes an age key for the destination, keeps it in
_/system/settings/backup/name.key_, readable by root alone, and shows
it once as the recovery key: a line that starts *AGE-SECRET-KEY-1*.
Keep it somewhere other than the machine. With it, the backups can be
read on any machine; without it and the machine, they cannot. *backup
key* shows it again to an administrator. Each *--recipient* adds an age
public key (*age1*...), as one kept on another machine, that every
backup is encrypted to as well.

Anyone who has a key's public half can encrypt a file to it, so each
backup and record has a tag, _time_*.tag*: an HMAC-SHA256 of its path and
bytes with a key made from the recovery key (ADR 0464). Only the
machine and whoever holds the recovery key can make one. *verify* and
*restore* refuse a file whose tag is missing or does not match, so a
backup that whoever holds the disk or the server put there is never
restored. Restoring with another of the destination's keys, which
cannot check the tags, needs *--unchecked*.

# COMMANDS

*add* [*--format* _format_] [*--every* _when_] [*--verify* _when_] [*--recipient* _key_]... [*--append-only*] _name_ _to_
	Adds the destination _name_, of up to 32 letters, digits, *-* and
	*\_*, at _to_: a folder, written from */*;
	*ssh://*_user_*@*_host_[*:*_port_]*/*_path_, where a _path_ that
	starts with *~/* is in the account's home; or
	*s3://*_host_[*:*_port_]*/*_bucket_[*/*_prefix_], over HTTPS, or
	*s3+http://* over plain HTTP, for storage on the local network. It
	checks that it can write there, marks the folder as this machine's,
	makes the key and shows it. _format_ is *btrfs* (the default) or
	*restic*, which S3 storage always is; a folder that already holds a
	restic repository that backup did not make there is refused.
	*--append-only* makes a restic destination that never removes
	anything. _when_ is *day* (the default), *week* or *hand* (only by
	*backup run*) for *--every*, and *week*, *month* (the default) or
	*never* for *--verify*. For a server, the first contact records its
	host key, which every later one must match; the server must accept
	the machine's key, which *backup ssh-key* shows. Needs an
	administrator.

	```
	$ elevate backup add stick /volumes/BACKUP
	Added stick: /volumes/BACKUP/dukovo-backup/laptop, backed up every day.

	Its recovery key, which reads its backups on any machine:

	    AGE-SECRET-KEY-1...

	Keep it somewhere other than this machine. Without it and this
	machine, the backups cannot be read.
	```

	A NAS's share, and a bucket that is only ever added to:

	```
	$ elevate smb mount -o user=ann //nas/backups /volumes/nas
	$ elevate backup add --format restic nas /volumes/nas
	$ export AWS_ACCESS_KEY_ID=AKIA... AWS_SECRET_ACCESS_KEY=...
	$ elevate backup add --append-only cloud s3://s3.eu-central-1.amazonaws.com/family-backups
	```

*remove* _name_
	Forgets the destination, and its key, and removes the snapshots it
	would have started its next backups from, unless another destination
	needs them. The backups stay where they are, and only its recovery
	key reads them. Needs an administrator.

*list* [*--json*]
	Each destination: where, how often, its last backup and whether it
	went well, and its last check by a restore. Anyone may ask.

	With *--json* it writes one object, *format* 1, with the
	destinations in *places* and the whole-machine restore that is
	staged in *staged*, as *{"from": "atlas", "at":
	"2026-11-11-030012"}* when there is one, otherwise null: anyone may
	see that a restore is staged, and from where. Each destination has its *name*, *to*,
	*kind* (*ssh*, *folder* or *s3*), *format*, *every*, *verify*, *keeps*
	(*chain*, *chains*, *daily*, *weekly* and *monthly*), *places* (those
	it backs up), *append_only*, *recipients* (how many), *plugged_in*
	(for a folder on a volume that is plugged in and out, whether it is
	there; else null), *key_ms* (when its key file last changed, which
	needs no right to read the key), *state*, *overdue_days* (as the
	notices count it, null when it is not overdue) and *running* (a
	round holds the lock). Its *state* is *tried_ms*, *done_ms*,
	*failed*, *verified_ms* and *verify_failed*, each null when it has
	not happened or is empty. Times are milliseconds since 1970.

*show* [*--json*] _name_
	What the destination holds: each place's chains, and the times of
	their backups, oldest first; for a restic repository, each place's
	backups. Needs an administrator.

	With *--json* it writes the *name*, the *place* where it is, the
	*format*, the *places* (each with its *path* here, null when this
	machine no longer backs it up, its *folder* there, and its
	*backups*, oldest first, each with its *time*, *at_ms* and whether
	it is *whole*, a chain's first backup; in a restic repository all
	are) and the machine's *records*, each with its *time* and *at_ms*.

*find* [*--json*]
	Looks in each volume mounted under */volumes*, down to three
	folders, for what a move to new hardware starts from: folders that
	hold a machine's backups (a *backup.toml*, as *add* writes there,
	and a *machine* folder of records), with the machine, its newest
	record and the volume (and whether the volume is a drive that is
	plugged in and out), and key files, which are files whose first line
	is the one *add* writes. Of a key file it reads those two comment
	lines, which say what destination it is for and its public key, and
	never the key. It passes over this machine's own root, which Spark
	mounts under */volumes* too, as its key files are this machine's own.
	Needs an administrator.

	With *--json* it writes *sources*, each with its *source* folder
	(which *restore* takes), *machine*, *newest* (a time, or null),
	*newest_ms*, *volume* and *removable*; and *keys*, each with its
	*path*, the destination it is *for* and its *public* key.

*run* [*--progress*] [_name_ ...]
	Backs up to each _name_, or to every destination, now. Needs an
	administrator. With *--progress* it says *step* _n_ *of* _m_*:* _place_
	before each place's turn, as a task shows its steps (with more than
	one destination, _place_ *to* _name_). *verify --restore* and
	*restore* say it before each place they restore, and
	*restore --machine* before each place of the record.

*auto* [*--watch*]
	One round of the service: a backup to each destination that is
	due, by its *every*, and that answers; a disk not plugged in is
	passed over until a round finds it. After a backup, a destination
	whose check by a restore is due gets one. With *--watch*, a round
	every hour until stopped. Needs an administrator.

*verify* [*--progress*] [*--restore*] _name_
	Reads every backup at the destination through, decrypting it and
	reading its stream, and checks each chain's links: each backup's
	parent is the one before it, and its tag, and reads each record.
	A file that was changed, cut short or tagged with another key is
	refused. With *--restore*, the newest backup of each place is
	also restored into the place's *@snapshots* folder and compared,
	file by file, with the snapshot it was made from: names, kinds,
	owners, modes, the times contents changed, links, extended
	attributes and data. The copy is removed after. A restic repository
	is checked as *restic check --read-data* checks one: its indexes,
	snapshots and trees, and every pack read through, each chunk against
	its MAC and hash. Needs an administrator.

*restore* [*--progress*] [*--key* _file_|*-*] [*--place* _place_] [*--at* _time_] [*--into* _folder_] [*--unchecked*] _name_|_source_
	Restores the newest backup of each place, or of _place_ (its path
	here, as */users*, or its folder's name there, as *users*), or
	the one made at _time_ (as *show* lists it, written
	_YYYY-MM-DD-HHMMSS_), as a read-only snapshot *restored-*_time_
	in the place's *@snapshots* folder, or in _folder_, which must be on
	Btrfs. *snap restore --from restored-*_time_ then copies files
	back. From another machine's backups, name the machine's folder
	(_/volumes/BACKUP/dukovo-backup/laptop_, or an *ssh://* address of
	it) as _source_ and give its recovery key in _file_, or on the
	input with *--key -*, as the web console does. A key that does not
	open the backups stops the restore (BAK-103). Needs an
	administrator.

	```
	$ elevate backup restore --place /users stick
	Restored the backup of users at 2026-11-11 03:00:12 as /snapshots/users/restored-2026-11-11-030012
	$ snap restore --from restored-2026-11-11-030012 notes.txt
	```

*restore --machine* [*--progress*] [*--key* _file_|*-*] [*--at* _time_] [*--card* _old_*=*_new_|*automatic*]... [*--unchecked*] _name_|_source_
	Restores the whole machine as its newest record, or the newest
	from no later than _time_, describes it. Each of the root's places
	the record names (*/system/settings*, */applications*, */users*
	and */shared*) is received as of that time into
	*@snapshots/machine-restore/staged*, with the app registry, the
	grants and *authd*(8)'s files of the accounts that the record
	carries; nothing in use changes. At the next start, before any
	service starts, *spark*(8) puts each in the place of the one there,
	which it keeps, read-only, in
	*/snapshots/machine-restore/before-*_time_, empties the grants and
	authd's keyrings and console files, and writes the record's files
	into */system/state*. The machine then has its name, accounts and
	passwords, second steps, keyrings, settings, files and apps again,
	and each app the permissions it was given. A record of format 1
	carries none of authd's files: its accounts keep this machine's
	passwords. A place not on the root,
	as a disk's in */volumes*, is restored as *restored-*_time_ in its
	own *@snapshots* folder, if this machine has it. While a restore is
	staged, another is refused (BAK-104) until the restart carries it
	out or *--cancel* withdraws it. Needs an administrator, and room on
	the root for everything restored.

	The restored *network*(5) settings set a card up by the first
	section that names it, by its name or its MAC address. A section
	that names a card by a name this machine's card has keeps it; one
	that names a MAC address this machine lacks names no card, and a
	card that no section names asks DHCP, so the restored machine never
	takes an address of the old one, which may still be on the network,
	by chance. *--card* _old_*=*_new_ gives the section of the old
	card _old_, by its name or MAC address as *--plan* lists it, to
	the card of this machine whose MAC address is _new_, which then
	has its addresses; *--card* _old_*=automatic* takes the section's
	*address*, *router*, *dns* and *dhcp* lines out, so that its card
	asks DHCP. Every other line of the file stays as it was.

	With *--plan --json*, nothing is received: it prints, as JSON, what
	the restore would bring across and how this machine differs. Its
	keys are *machine*, *at* and *at_ms*, the record's time;
	*record_format*; *staged*, whether a restore is staged already;
	*accounts*, the names of the people's accounts; *apps*, each *id*
	and *version*; *places*, each *path*, *folder* and *bytes*;
	*needed*, the bytes of the root's places, *free*, the bytes free on
	the root, and *fits*; *hardware*, *then* and *now*; *cards*, each
	old card with addresses of its own (*old*, with its *name*, *mac*,
	*addresses* and *router*), the card of this machine it goes to
	(*new*), and its *choice*, *fixed* or *automatic*; and
	*firewall_on_missing*, the *firewall*(8) rules that name a card of
	the old machine that this one lacks. What a record of format 1 does
	not say is *null*. The web console's Move page shows it.

	After a fresh install, from a disk's backups:

	```
	$ elevate backup restore --machine --key /volumes/BACKUP/recovery.key /volumes/BACKUP/dukovo-backup/atlas
	/applications: staged its backup of 2026-11-11 03:00:12
	...
	Staged atlas as it was at 2026-11-11 03:00:12. Restart to finish: ...
	$ elevate reboot
	```

	Onto other hardware, with the old card *net0*'s fixed address given
	to this machine's card:

	```
	$ elevate backup restore --machine --plan --json --key /volumes/BACKUP/recovery.key /volumes/BACKUP/dukovo-backup/atlas
	$ elevate backup restore --machine --card net0=52:54:00:12:34:56 --key /volumes/BACKUP/recovery.key /volumes/BACKUP/dukovo-backup/atlas
	```

	The subvolumes it replaced stay until *subvolume delete* removes
	them.

*key* [*--json*] _name_
	Shows the destination's recovery key. Needs an administrator. With
	*--json* it writes the *name*, the *key* and its *public* half.

*ssh-key* [*--json*]
	Shows the public key the machine signs in to servers with, making
	it the first time, which needs an administrator: add it to the
	backup account's _~/.ssh/authorized_keys_ on the server. With
	*--json* it writes it as *public*.

# RESTORING WITH LINUX

The parts of a backup, in order, decrypted, are what *btrfs send*
writes. With the recovery key in a file, Linux's *age* or *rage* and
*btrfs receive* make each backup of a chain again, from its whole one.
They do not check the tags:

```
$ cd dukovo-backup/laptop/users/2026-11-01-030000
$ for b in $(ls *.age* | sed 's/\.age.*//' | sort -u); do
>     cat $b.age* | age -d -i recovery.key | sudo btrfs receive /mnt/restore
> done
```

A restic repository is read by Linux's *restic*, version 0.14 or later,
or *rustic*, with the recovery key as the password. Restored this way,
a place's top folder takes restic's owner and mode, not the place's:

```
$ export RESTIC_PASSWORD="$(cat recovery.key)"
$ restic -r /mnt/nas/dukovo-backup/laptop snapshots
$ restic -r /mnt/nas/dukovo-backup/laptop restore latest --path /users --target /mnt/restore
$ export AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=...
$ restic -r s3:https://s3.eu-central-1.amazonaws.com/family-backups/dukovo-backup/laptop check
```

# NOTICES

After each round, *run*, *verify --restore* and *remove*, backup
keeps for the next sign-in what the owner should see in
_/system/live/sign-in/backup_, which the sign-in screens show under the
issue text (*consoled*(8), ADR 0504): each destination whose last backup,
or last check by a restore, failed, and each that has gone without a
backup for 3 days when it backs up every day, or 14 days when every
week, counted from when it was added until its first. A destination
backed up by hand is never overdue. When the notice changes to something,
the consoles are told at once (*notifyd*(8)).

After each of its rounds the *backup* service also keeps the alerts of
the destinations (*alerts*(8)): *backup-failed:*_name_ while the last
backup or check failed, and *backup-overdue:*_name_ while the sign-in
says it is overdue, a warning that becomes critical once it is overdue
twice over (6 days for a daily destination, 28 for a weekly one). A
*run* from a shell changes the notice at once and the alerts at the
service's next round.

# EXIT STATUS

0 when it did what was asked; 1 when it did not, after what came before
(a destination of *run* or *auto* that failed is told of and the rest
done), and for a mistake in the command line. A refusal that has an ID
ends its message with it in parentheses, and the support center
explains it: *BAK-101* for a name that no destination has, and
*BAK-102* for a *run* while another round holds the lock.

# FILES

_/system/settings/backup/name.toml_
	A destination's settings (*backup.toml*(5)).

_/system/live/sign-in/backup_
	The notice for the next sign-in, while one is needed.

_/system/settings/backup/name.key_
	A destination's key.

_/system/settings/backup/name.s3_
	An S3 destination's access key.

_/system/settings/backup/ssh_ed25519_, _/system/settings/backup/known_hosts_
	The machine's key for servers, and the servers' host keys.

_/system/state/backup/name.json_
	What a destination last did, and the bytes it holds of each place.

_/system/live/backup.lock_
	Held, with its process number, while a round runs.

_@snapshots/machine-restore_, shown in _/snapshots/machine-restore_
	A whole machine's restore: *staged* until the next start, then
	*before-*_time_, what it replaced.

_/system/settings/spark/services/backup.toml_
	The service (*service.toml*(5)).

# SEE ALSO

*backup.toml*(5), *alerts*(8), *snap*(1), *subvolume*(8), *service.toml*(5),
*spark*(8), *network*(5), *firewall*(8), *ssh*(1), *age*(1), *mount*(8), *smb*(1)
