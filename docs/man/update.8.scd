update(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

update - see, take, hold back and undo every update: the system, its firmware set and the apps

# SYNOPSIS

*update* [*check*]++
*update apply* [*--yes*]++
*update undo* _item_ [*--settings-from-before* | *--keep-settings*]++
*update hold* _item_++
*update unhold* _item_++
*update history*

_item_ is *system*, *firmware* or an app's ID.

# DESCRIPTION

update is the one command for every update (ADR 0502). Underneath, the
system and its firmware set are *sysupdate*(8)'s, in the two slots, and
the apps are *bundle*(1)'s; update asks each and says it in one place.
*updated*(8) runs the same check once a day.

*check*, or no word at all
	Says what waits, under three headings. *System*: the release the
	channel offers (*updates.toml*(5)), or that the system is up to
	date, and whether the release it runs was withdrawn. *Firmware*:
	the firmware set the channel offers for this machine (ADR 0467).
	*Apps*: the updates of the apps installed for everyone, as *bundle
	update --check* lists them. Anyone may check.

*apply* [*--yes*]
	Stages the system update into the spare slot, to start at the
	next restart, which the owner picks; or, when no system update
	waits, the firmware set. The two share the spare slot, so a new
	firmware set waits until a new system has proved itself. Then it
	updates the apps installed for everyone, asking first unless
	*--yes* is given. An administrator's, through *elevate*(8).

*undo* _item_
	Goes back on one thing. *system* makes the previous version the
	default again, as *sysupdate rollback* does, with the firmware set
	it named. *firmware* does the same when the previous version is
	the same system with another firmware set, and says to undo the
	system when it is another system too. *--settings-from-before*
	and *--keep-settings* choose the system's settings, as for
	*sysupdate rollback*. An app's ID goes back to the app's version
	before, as *bundle rollback* does.

*hold* _item_
	Holds one thing back at the version the channel offers now, or the
	one it runs: a newer one is taken as usual, and the held one is
	not. *system* and *firmware* are kept in the *[hold]* table of
	*updates.toml*(5); an app as *bundle hold* keeps it. A held
	version gets no security fixes, so a hold is for a while, not for
	good. *sysupdate stage --version* still stages a held release
	when asked by name.

*unhold* _item_
	Lifts the hold.

*history*
	Each change of the system and the firmware set, by the start that
	first ran it, from the starts' records (ADR 0500), with how that
	start ended; then the apps' history, as *bundle history* gives it.

# EXIT STATUS

0 when it did what was asked, 1 when it could not, with why on standard
error, and 2 for a usage error.

# EXAMPLES

What waits:

```
$ update
System:
sysupdate: the stable channel offers Dukovo 2026.3.1; this system runs Dukovo 2026.2.1

Firmware:
update: firmware 2026.3.1 is offered; this system runs firmware 2026.2.1: `update apply` stages it

Apps:
0 to update, 4 up to date
```

Hold the system back for a week of travel, then lift it:

```
$ elevate update hold system
update: system is held back at 2026.3.1: a newer version is taken as usual; until then the held version gets no security fixes. `update unhold system` lifts the hold
$ elevate update unhold system
update: system is no longer held back
```

What changed, start by start:

```
$ update history
System and firmware, by the start that first ran each:
  start 41 on 2026-11-03 09:12: Dukovo 2026.2.1 to 2026.3.1; good

Apps:
...
```

# FILES

_/system/settings/sysupdate/updates.toml_
	The channel, the server, what *updated*(8) does, and the holds
	(*updates.toml*(5)).

_/system/state/starts_
	The starts' records that *history* reads (*changes*(1)).

# SEE ALSO

*updated*(8), *sysupdate*(8), *bundle*(1), *updates.toml*(5), *changes*(1)
