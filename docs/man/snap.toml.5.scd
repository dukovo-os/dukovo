snap.toml(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

snap.toml - which places get a snapshot a day, and how many are kept

# SYNOPSIS

_/system/settings/snap.toml_

# DESCRIPTION

The *snap* service makes a read-only snapshot of each place this file
names once a day, and prunes the snapshots by its retention (*snap*(1),
ADR 0461). The service reads it at each round, every hour, so a change
needs no restart; *snap list* and *snap prune* read it too. Without it,
the defaults hold. It is TOML:

```
places = ["/users", "/shared", "/system/settings"]
daily = 7
weekly = 4
```

*places*
	The places, each the top folder of a Btrfs subvolume: on Dukovo's
	root, or on another disk in */volumes*, which is skipped while it
	is not there. By default */users*, */shared* and
	*/system/settings*. An empty list makes no snapshots by itself.

	The *snap* service holds only the places of the default list, with
	_/snapshots_ and *@snapshots* at the top of the root (ADR 0473). A
	place that is not in its view, as a data disk's, is skipped and
	logged until the service's file holds it: *service edit snap* and
	add the disk's top, *files.read-write = ["/volumes/sdb1"]* for the
	example below (*service.toml*(5)).

*daily*
	How many of the latest days that have a snapshot keep their newest:
	7 by default.

*weekly*
	How many of the latest weeks, Monday to Sunday, that have a
	snapshot keep their newest: 4 by default.

The newest snapshot of a place is always kept, and so is every snapshot
given a name; with *daily* and *weekly* 0 only the newest is left.

# EXAMPLES

A data disk too, with a month of days kept:

```
places = ["/users", "/shared", "/system/settings", "/volumes/sdb1/@data"]
daily = 31
weekly = 8
```

# SEE ALSO

*snap*(1), *service.toml*(5), *subvolume*(8)
