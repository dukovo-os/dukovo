snap(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

snap - make, list, restore and remove snapshots of Btrfs subvolumes

# SYNOPSIS

*snap create* [*--name* _name_] _place_

*snap list* [*--json*] [_place_]

*snap browse* [*--json*] _place_ _snapshot_ [_folder_]

*snap restore* [*--from* _snapshot_] [*--to* _path_] _path_

*snap restore* *--from* _snapshot_ *--beside* _label_ _path_ ...

*snap delete* _place_ _snapshot_ ...

*snap prune* [_place_ ...]

*snap auto* [*--watch*]

# DESCRIPTION

snap keeps read-only snapshots of Btrfs subvolumes (ADR 0461). A
snapshot holds what its subvolume held when it was made, and shares the
disk's blocks with it until either changes them, so it takes room only
for what changes after it. A _place_ is the top folder of a subvolume:
on Dukovo's root, */users*, */shared*, */system/settings*,
*/system/state* or */applications*; or a subvolume of another Btrfs
disk in */volumes*.

The snapshots of a place are in its filesystem's *@snapshots*
subvolume, in a folder named after the place's subvolume without its
*@*: *users*, *shared*, *settings*; *top* for the top of a disk. On
Dukovo's root, *@snapshots* is mounted read-only at */snapshots*, so
*/snapshots/users/2026-11-11-0300* holds */users* as it was at 03:00 on
11 November 2026. On another disk, snap makes *@snapshots* at its top
the first time it needs it.

Every snapshot is read-only: a write into one fails with _Read-only
file system_, whoever makes it. A snapshot keeps the owners and modes
of its files, so a person reads in one what they could read when it was
made, and a removed account's files are no one else's. Apps never see
a snapshot: an app's view has no */snapshots* and no */volumes*.

A snapshot is named by the local time it was made, *YYYY-MM-DD-HHMM*,
with *.2* and on for another in the same minute; or by a name given with
*--name*.

# THE SNAP SERVICE

The *snap* service runs *snap auto --watch*: when it starts and then
every hour, each place *snap.toml*(5) names that has no snapshot from
today gets one, and then each is pruned. By default the places are
*/users*, */shared* and */system/settings*. A place that is not there,
as a disk not plugged in, is skipped, and the log says so once.

The service holds only what its file gives it (*service.toml*(5), ADR
0473): the places of the default *snap.toml*, which it reads and
snapshots, _/snapshots_, and *@snapshots* alone at the top of the root,
where it makes and removes the snapshots, and no one's other files. A
place of *snap.toml* that the service's view lacks, such as a data
disk's, is skipped, and the log says so once, in these words:

```
snap: /volumes/sdb1/@data is not in the snap service's view: add it with `service edit snap`; skipped
```

and *snap list* tells of each place the service lacks, as the service
would. *service edit snap* adds it to the service's file: a place on
another disk needs that disk's top in the file, as
*files.read-write = ["/volumes/sdb1"]*, as its snapshots go to that
disk's own *@snapshots* (*snap.toml*(5)). Run from a shell, *snap* has
the whole tree and needs nothing of the kind.

# RETENTION

Of the snapshots of a place named by their time, a prune keeps the
newest of each of the latest *daily* days that have one (7 by default),
the newest of each of the latest *weekly* weeks, Monday to Sunday, that
have one (4 by default), and the newest of all, as restic's
*--keep-daily* and *--keep-weekly* count them. Days and weeks are
local, by when each snapshot was made. The rest are removed.

A snapshot given a name is kept until it is deleted. The snapshot an
update keeps of the settings, in *@snapshots/update*, and the two system
slots are never removed by snap: the slot not running is the previous
version, which *sysupdate rollback* and the boot menu start.

# COMMANDS

*create* [*--name* _name_] _place_
	Makes a read-only snapshot of _place_, and writes its path. A
	_name_ is a file name that does not start with a dot and is not a
	time, which snap gives its own. Needs an administrator: *elevate snap
	create /users*.

*list* [*--json*] [_place_]
	With no _place_, each place *snap.toml* names: how many snapshots
	it has, its newest, and where they are. With one, its snapshots,
	oldest first: the name, when it was made, and why it is kept:
	*newest*, *daily*, *weekly*, or *named* for one given a name; one
	the next prune removes says so. *--json* prints the same as one
	line of JSON (JSON).

	```
	$ snap list /users
	NAME             MADE              KEPT
	2026-10-25-0300  2026-10-25 03:00  weekly
	2026-11-01-0300  2026-11-01 03:00  weekly
	before-update    2026-11-03 18:12  named
	2026-11-05-0300  2026-11-05 03:00  daily
	...
	2026-11-11-0300  2026-11-11 03:00  newest, daily, weekly
	```

*browse* [*--json*] _place_ _snapshot_ [_folder_]
	Shows what _folder_ held in _snapshot_ of _place_, each entry
	compared with the same path now, to see what a restore would bring
	back. _folder_ is a path of the place, which it must be or be
	within, and is the place's top without one. The folders come first,
	then the rest, each by name: at most 2000 entries, and a last line
	says when there are more. Each entry has its kind (*file*,
	*folder*, *link* or *other*), its size, when it was last changed,
	and how it is now: *deleted* if it is not there, *changed* if its
	kind, size or time differs (a folder's size is not compared), *same*
	if it is as it was. It runs as whoever asks, with their rights, so
	a folder is shown to a person who could read it when the snapshot
	was made; one that cannot be read fails with the system's words,
	and a _snapshot_ that the place does not have with *BAK-105*.

	```
	$ snap browse /users 2026-11-13-0302 /users/alice/photos
	NAME           SIZE  MODIFIED          NOW
	2025/          -     2026-09-01 10:14  same
	IMG_4410.heic  3.0M  2026-11-12 18:40  deleted
	notes.txt      812   2026-11-10 09:02  changed
	```

	With *--json* it writes one object, whose times are milliseconds
	since 1970 and whose _size_ is null for a folder:

	```
	{"place": "/users", "snapshot": "2026-11-13-0302",
	 "folder": "/users/alice/photos",
	 "entries": [{"name": "IMG_4410.heic", "kind": "file",
	  "size": 3100000, "modified_ms": 1763000000000,
	  "then": "deleted"}], "more": false}
	```

	The snapshot is named after *--*, as a name that starts with *-*
	can be.

*restore* [*--from* _snapshot_] [*--to* _path_] _path_
	Copies _path_, a file or a folder of a place, back from the newest
	snapshot that holds it, or from _snapshot_, to where it was or to
	_path_ after *--to*. It never writes over a file that is there:
	restore beside it with *--to*. Folders that are missing on the way
	are made. The copy keeps the modes and times, and the owners when an
	administrator restores, and shares the snapshot's blocks until
	either changes. It runs as whoever asks, so a person restores their
	own files without an administrator:

	```
	$ rm notes.txt
	$ snap restore notes.txt
	Restored /users/alice/notes.txt from /snapshots/users/2026-11-11-0300/alice/notes.txt
	```

*restore* *--from* _snapshot_ *--beside* _label_ _path_ ...
	Copies each _path_ back from _snapshot_ beside the one there now, as
	_stem_ (_label_)._ext_ for a file with an extension and _name_
	(_label_) for a folder or a name with none, so that both can be
	compared; this is how the web console restores. _label_ is 1 to 40
	characters with no */* and no control character, and is not *.* or
	*..*. A name that is taken stops the restore with BAK-106, after the
	paths before it, which it has said; nothing is ever written over.

	Each folder on the way, in the snapshot and in the place, is opened
	beneath the one before it, never through a symbolic link: a path with
	a link on the way is refused, and a link in the snapshot is copied as
	a link. The copy is made in a folder of its own that only snap may
	enter, then moved to its name, which snap has taken first, in one
	rename. So an administrator who restores into a folder another
	account may write in never writes where that account points. The
	copy keeps the modes and times, and the owners when an administrator
	restores. An access list is not copied: a copy of a file or folder
	that had one gets the group permissions the list gave its group,
	never more, and snap says so; give the list again with *setfacl*(1).

	```
	$ snap restore --from 2026-11-11-0300 --beside "Nov 11" notes.txt photos
	Restored /users/alice/notes (Nov 11).txt from snapshot 2026-11-11-0300
	Restored /users/alice/photos (Nov 11) from snapshot 2026-11-11-0300
	```

*delete* _place_ _snapshot_ ...
	Removes each _snapshot_ of _place_, freeing what no other snapshot
	shares. Needs an administrator.

*prune* [_place_ ...]
	Removes the snapshots of each _place_, or of each place *snap.toml*
	names, that retention does not keep, and writes their paths. Needs
	an administrator.

*auto* [*--watch*]
	One round of the service: a snapshot of each place *snap.toml*
	names that has none from today, then each pruned. With *--watch*,
	a round every hour until stopped, reading *snap.toml* again each
	time. Needs an administrator.

# JSON

*list --json* prints one line of JSON, an object whose first member is
*format*, 1 (ADR 0541), which scripts and Ansible's *dukovo.system*
collection read in place of the text. A later field may be added without
a new *format*. Names are in snake_case, times are RFC 3339 in UTC, and a
value that is not known is *null*. An error is told to standard error as
without *--json*, with the same exit status.

With no _place_, it has *places*, a list with an object for each place
*snap.toml* names that could be read:

*place*
	The place, as *snap.toml* names it.

*snapshots*
	How many snapshots it has.

*newest*
	The name of the newest, or *null* for none.

*newest_made*
	When the newest was made, or *null* for none or when the
	filesystem does not say.

*folder*
	Where its snapshots are read.

*in_service*
	Whether the snap service's view holds the place. Its daily round
	skips one that it does not, which *service edit snap* adds.

It also has *settings*, what *snap.toml* says with the defaults for what
it leaves out (*places*, *daily* and *weekly*); and *update*, a list with
an object for each snapshot in _/snapshots/update_, oldest first: the
settings and the state from before the last update, and the settings of
an attempt that fell back (*sysupdate*(8)), each with its *name* and
when it was *made*. The list is empty when no update has made any.

With a _place_, it has *place*, the top folder of its subvolume;
*folder*, where its snapshots are read; and *snapshots*, a list with an
object for each, oldest first:

*name*
	The snapshot's name.

*made*
	When it was made, or *null* when the filesystem does not say.

*named*
	Whether its name was given with *--name*, so that no prune
	removes it.

*kept*
	Why a snapshot named by its time is kept, a list of *newest*,
	*daily* and *weekly*; empty for a named one and for one the next
	prune removes.

*prunable*
	Whether the next prune removes it.

*restored*
	Whether *backup restore* put it in the place, named *restored-* and
	the backup's time (*backup*(8)). No prune removes it.

# SPACE

A file deleted from a place keeps its room until the last snapshot that
holds it is removed: up to four weeks with the defaults. To free the
room sooner, delete the snapshots that hold it with *snap delete*, or
lower *daily* and *weekly* in *snap.toml* and run *snap prune*. *df*(1)
counts the room snapshots take with the rest.

# EXIT STATUS

0 when it did what was asked; 1 when it did not, after what came before
(a place of *auto* or *list* that failed is told of and the rest done),
and for a mistake in the command line. A _snapshot_ of *browse* that the
place does not have ends the message with its ID, *BAK-105*, which the
support center explains.

# FILES

_/system/settings/snap.toml_
	The places and the retention (*snap.toml*(5)).

_/snapshots_
	The root's *@snapshots*, read-only: a folder of snapshots for each
	place, and the update's in *update*.

_/system/settings/spark/services/snap.toml_
	The service (*service.toml*(5)).

# SEE ALSO

*snap.toml*(5), *subvolume*(8), *sysupdate*(8), *ls*(1), *spark*(8),
*cp*(1)
