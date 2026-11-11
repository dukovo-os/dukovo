subvolume(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

subvolume - make, snapshot, remove and show Btrfs subvolumes

# SYNOPSIS

*subvolume create* _path_ ...

*subvolume snapshot* [*-r*] _source_ _dest_

*subvolume delete* _path_ ...

*subvolume show* _path_

# DESCRIPTION

subvolume makes, removes and shows Btrfs subvolumes, as Linux's *btrfs
subvolume* does with the same four commands and messages. A
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

# OPTIONS

*-r*
	Make the snapshot read-only: every change in it fails with
	_Read-only file system_ (*EROFS*), though the filesystem is mounted
	to write.

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

# EXIT STATUS

0 when every subvolume was made, snapshotted, removed or shown; 1 when
one was not, after the ones before it, and for a mistake in the command
line, as Linux's *btrfs* exits.

# SEE ALSO

*snap*(1), *radix-directory*(2), *radix-files*(7), *systools*(1), *df*(1)
