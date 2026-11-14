snap(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

snap - make, list, restore and remove snapshots of Btrfs subvolumes

# SYNOPSIS

*snap create* [*--name* _name_] _place_

*snap list* [_place_]

*snap restore* [*--from* _snapshot_] [*--to* _path_] _path_

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

*list* [_place_]
	With no _place_, each place *snap.toml* names: how many snapshots
	it has, its newest, and where they are. With one, its snapshots,
	oldest first: the name, when it was made, and why it is kept:
	*newest*, *daily*, *weekly*, or *named* for one given a name; one
	the next prune removes says so.

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

# SPACE

A file deleted from a place keeps its room until the last snapshot that
holds it is removed: up to four weeks with the defaults. To free the
room sooner, delete the snapshots that hold it with *snap delete*, or
lower *daily* and *weekly* in *snap.toml* and run *snap prune*. *df*(1)
counts the room snapshots take with the rest.

# EXIT STATUS

0 when it did what was asked; 1 when it did not, after what came before
(a place of *auto* or *list* that failed is told of and the rest done),
and for a mistake in the command line.

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
