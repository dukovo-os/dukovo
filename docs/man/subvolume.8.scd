subvolume(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

subvolume - make, snapshot, remove, show, send and receive Btrfs
subvolumes

# SYNOPSIS

*subvolume create* _path_ ...

*subvolume snapshot* [*-r*] _source_ _dest_

*subvolume delete* _path_ ...

*subvolume show* _path_

*subvolume send* [*-q*] [*-p* _parent_] [*-f* _file_] _subvolume_ ...

*subvolume receive* [*-q*] [*-e*] [*-f* _file_] _folder_

*subvolume receive* *--dump* [*-f* _file_]

# DESCRIPTION

subvolume makes, removes and shows Btrfs subvolumes, as Linux's *btrfs
subvolume* does with the same four commands and messages, and sends and
receives them as Linux's *btrfs send* and *btrfs receive* do. A
subvolume is a directory whose files are a tree of their own on the
disk; a snapshot of one is a new subvolume that holds what it held, at
once, in blocks the two share until either changes them (ADR 0138).

*create* makes each _path_ a new, empty subvolume.

*snapshot* makes _dest_ a snapshot of the subvolume that holds
_source_; if _dest_ is a directory, the snapshot goes in it, with
_source_'s name. What was written to _source_'s files but not yet
synced is in it. A subvolume inside _source_ is an empty directory in
the snapshot, as on Linux.

*delete* removes each _path_, a subvolume, with everything in it, and
frees what no snapshot shares. *rmdir*(1) and *rm*(1) remove an empty
subvolume too.

*show* tells what Btrfs keeps of the subvolume that holds _path_, as
anyone who can open _path_ may ask (ADR 0461): its name, its UUID, the
UUID of the subvolume it is a snapshot of (*Parent UUID*, *-* if it is
none) and the one it was received from, when it was made, its ID and
its parent's, the transaction that last wrote it and the one that made
it, and *readonly* when nothing in it may change:

```
$ subvolume show /snapshots/users/2026-11-11-0300
/snapshots/users/2026-11-11-0300
	Name:            2026-11-11-0300
	UUID:            0f6c1f3e-2b4d-4c39-9a51-7d0e8b2c4a61
	Parent UUID:     5d2e9a40-7c13-4b8e-a2f6-31c9d0e7b852
	Received UUID:   -
	Creation time:   2026-11-11 03:00:04 -0700
	Subvolume ID:    271
	Generation:      1843
	Gen at creation: 1843
	Parent ID:       270
	Flags:           readonly
```

*send* writes each _subvolume_, a read-only snapshot, as a send stream:
Linux's format, version 1, which Linux's *btrfs receive* reads as well as
*receive* does (ADR 0462). With *-p*, the stream holds only the changes
from _parent_, an older read-only snapshot of the same subvolume that the
receiving side has already received; without it, everything. The kernel
compares the two snapshots' trees and skips every block they share, so
an incremental send reads only what changed. A subvolume that can change
is refused: snapshot it with *-r* first. Sending reads every file
whatever its owner and mode, so it needs an administrator (*elevate*(8)),
as Linux's needs root. The stream goes to standard output, never to a
terminal, or to _file_ with *-f*; each _subvolume_ is a stream of its
own, one after the other, and each is named by its last name.

*receive* reads streams from standard input, or from _file_ with *-f*,
and makes each subvolume they hold in _folder_, on Btrfs: a new one for
a whole stream, or a snapshot of its parent for an incremental one,
then changed into the subvolume sent. The parent is the subvolume in
_folder_ received as that parent, or that is it, at the same
transaction. Each subvolume made ends read-only and marked received from
the subvolume sent: *show* gives its *Received UUID*, and a later stream
from a snapshot of it finds it as a parent. Every change goes through
the subvolume being made, by paths it checks, and never follows a
symbolic link that is a path's last name, so a stream changes nothing
outside it. A stream that fails partway leaves nothing: the subvolume it
was making is removed. It needs an administrator.

*receive --dump* writes the commands of the streams as text, one a line,
as *btrfs receive --dump* does, and changes nothing. Anyone may dump a
stream.

```
$ elevate sh -c 'subvolume send /snapshots/users/2026-11-10-0300 | subvolume receive /volumes/backup'
At subvol /snapshots/users/2026-11-10-0300
At subvol 2026-11-10-0300
$ elevate subvolume send -p /snapshots/users/2026-11-10-0300 -f users.stream /snapshots/users/2026-11-11-0300
At subvol /snapshots/users/2026-11-11-0300
$ elevate subvolume receive -f users.stream /volumes/backup
At snapshot 2026-11-11-0300
```

# OPTIONS

*-r*
	Make the snapshot read-only: every change in it fails with
	_Read-only file system_ (*EROFS*), though the filesystem is mounted
	to write.

*-p* _parent_
	Send only the changes from _parent_, a read-only snapshot.

*-f* _file_
	Write the stream to _file_, made or emptied, readable only by its
	owner (*send*); read it from _file_ (*receive*).

*-e*
	Stop after the first end of a stream, as when more follows on the
	same input.

*--dump*
	Write the streams' commands as text instead of receiving them.

*-q*, *--quiet*
	Leave out the *At subvol* and *At snapshot* lines.

# DIFFERENCES FROM LINUX

- A subvolume with a file or directory in it open, or one that is the
  filesystem's default subvolume, fails to go with _Device or resource
  busy_; Linux lets its open files live on until they close, and
  refuses the default with _Operation not permitted_.
- A subvolume that holds another fails to go with _Directory not
  empty_, as on Linux; delete the inner one first. Linux's *-R* (all
  below too) is not there yet.
- *list*, *get-default* and *set-default*, and the other commands of
  *btrfs subvolume*, are not there yet: *snap*(1) lists and restores
  snapshots.
- *show* leaves out Linux's *Top level ID*, the times of a send and a
  receive, and the snapshots of the subvolume, which Linux finds by
  searching every subvolume as root.
- Removing a subvolume frees its blocks in one transaction, at once,
  rather than in the background as Linux's cleaner does.
- *send* writes version 1 streams only, without Linux's *-c* (clone
  sources), *--proto*, *--compressed-data* and *--no-data*; it writes a
  file's data rather than a clone of another file's. *receive* reads
  version 1 streams, clones among them; a version 2 stream is refused.
- *receive* finds a parent in _folder_ alone, where Linux searches every
  subvolume of the filesystem; and it has no *-C* (chroot), *-m* or
  *--max-errors*, since its changes stay inside the subvolume being made
  without them.

# EXIT STATUS

0 when every subvolume was made, snapshotted, removed, shown, sent or
received; 1 when one was not, after the ones before it, and for a mistake
in the command line, as Linux's *btrfs* exits.

# SEE ALSO

*snap*(1), *elevate*(8), *radix-directory*(2), *radix-files*(7),
*systools*(1), *df*(1)
