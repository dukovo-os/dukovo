update(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

update - see, take, hold back and undo every update: the system, its firmware set and the apps

# SYNOPSIS

*update* [*check*] [*--json*]++
*update apply* [*--yes*]++
*update undo* _item_ [*--settings-from-before* | *--keep-settings*]++
*update hold* _item_++
*update unhold* _item_++
*update history* [*--json*]++
*update advisories* [*--json*]

_item_ is *system*, *firmware* or an app's ID.

# DESCRIPTION

update is the one command for every update (ADR 0502). Underneath, the
system and its firmware set are *sysupdate*(8)'s, in the two slots, and
the apps are *bundle*(1)'s; update asks each and says it in one place.
*updated*(8) runs the same check once a day, or each hour on a canary
(*updates.toml*(5)), and keeps the newer signatures of *troubleshoot*(8)
that the update server offers.

*check*, or no word at all
	Says what waits, under four headings. *System*: the release the
	channel offers (*updates.toml*(5)), or that the system is up to
	date, and whether the release it runs was withdrawn. *Firmware*:
	the firmware set the channel offers for this machine (ADR 0467).
	*Apps*: the updates of the apps installed for everyone, as *bundle
	update --check* lists them. *Security*: a line for each security
	advisory that applies to the release the system runs, with how its
	fix stands, and how many flaws the apps have, which *advisories*
	lists (ADR 0534). Anyone may check: a check not run as root reads
	the server from a copy of the metadata the system trusts, in the
	caller's own */tmp*, and keeps nothing, so it never changes what the
	system trusts. With *--json*, one document says it all; see *JSON*.

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
	With *--json*, one document says both; see *JSON*.

*advisories*
	Says which Dukovo Security Advisories, DKSA, apply to this
	machine (ADR 0534). The update server signs each advisory and an
	index of them as targets of the repository, and the command reads
	both through the same client, and the same *updates.toml*(5), as
	*check* does. The advisories are matched here: nothing about the
	machine is sent. An advisory that names the package *dukovo*
	applies when the release the system runs is in one of its ranges,
	in Dukovo's version order, so that 1.4.1 comes before 1.4.1.1,
	which comes before 1.4.2. One that names an app's ID applies when
	the version installed is in a range, for the apps this person can run.
	The flaws that the apps' catalogs list as known (*bundle*(1)) are
	listed with them, and one with the same ID or alias as an advisory
	is listed once.

	For each, it says the ID, the severity (*critical*, *high*,
	*medium* or *low*), what is wrong, the other names it is known by,
	and how the fix stands. For the system the fix is one of: staged
	in the spare slot, so that a restart finishes it; offered by the
	channel, which *apply* stages; offered, and waiting for this
	machine's wave or held back by *hold*; or not offered, with the
	releases that have it, if there are any. The advisory's own words
	on what to do follow. For an app it names the version of the app
	that has the fix when its catalog offers one. With nothing to say it
	says that no advisory applies. Anyone may run it. With *--json*, one
	document says it all; see *JSON*.

	When the update server cannot be reached, or its advisories do not
	check, it says so and still lists the flaws that the catalogs' lists
	give the apps, as bundled last kept them. The system's part is then
	not checked, and the document says so in *unchecked*.

# JSON

*--json* with *check*, or no word at all, and with *history* and
*advisories*, prints one line of JSON, an object whose first member is *format*, 1 (ADR 0541),
which scripts and Ansible's *dukovo.system* collection read in place of
the text. A later field may be added without a new *format*. Names are in
snake_case, times are RFC 3339 in UTC, and a value that is not known is
*null*. An error is told to standard error as without *--json*, with the
same exit status.

*check --json* has:

*offers*
	A list with an object for each part, the system first, then the
	firmware set, then each app of the person's own with a newer
	version in its catalog or its own feed, as *bundle update --check*
	lists them. Each has *item*, *from* and *to*, and *state*, which
	is *current* when nothing newer is offered, *update* when
	*update apply* takes it, *waiting* when it reaches this machine in
	a wave not open yet, *held* when a hold keeps it back, or
	*withdrawn*. The rest depends on the item.

	*system*: *item* is *system*; *from* is the release the system
	runs and *to* the one the channel offers. *wave_day* is the day
	after the release on which this machine's wave opens, and
	*waves_held* why no further wave opens when the rollout holds
	them, both with *waiting*. *withdrawn* is why the release was
	withdrawn. *bridge* is the release *update apply* stages first,
	when *to* updates only from a later release than the system
	runs. *download* is what staging fetches, *files* and *bytes*,
	with *update*. Each is *null* where it does not apply.

	*firmware*: *item* is *firmware*; *from* is the set the running
	slot names and *to* the set the channel offers, each *null* for
	none. *state* is *current*, *update* or *held*.

	An app: *item* is its ID; *from* is the version installed and *to*
	the newest its catalog lists. *state* is *update*, or *held*
	when *bundle hold* holds it at its version. *catalog* is the
	catalog it updates from, or *null* for its own feed.

*running_withdrawn*
	*null*, or, when the release keys withdrew the release this system
	runs, *version*, *reason*, and *advice*, *rollback* to go back to
	the version before it or *stay* to stay on it until the next
	release.

*unchecked*
	A list with an object for each part that could not be checked,
	*item*, *system*, *firmware* or *apps*, and *why*. The other parts
	are still checked, as the text form does, and the exit status is
	0.

*notes*
	Lines for a person that no other member holds: what *bundle
	update --check* says besides, and what the read of the
	troubleshoot signatures says.

*security*
	An object with *system*, the list of advisories that apply to the
	system, each as *advisories --json* gives them, and *app_flaws*,
	how many flaws the apps have. A part that could not be checked is
	named in *unchecked* with the *item* *security*.

*history --json* has:

*changes*
	A list with an object for each change of the system or the
	firmware set, oldest first, by the start that first ran it:
	*start*, its number; *time*, when it began; *item*, *system* or
	*firmware*; *from* and *to*, what ran before and after, *null*
	for no firmware set; *outcome*, *good*, *unfinished* or *failed*;
	and *error*, the error ID that told why a start failed, or *null*.

*starts_kept*
	How many starts are kept, which the changes are among.

*apps*
	A list with an object for each thing bundled did to the person's
	bundles and everyone's, oldest first, as *bundle history* gives
	them: *time*, *id*, *for_everyone*, *what* (*installed*,
	*updated*, *rolled-back*, *removed*, *held*, *released*, *waiting*,
	*failed* or *flawed*), *from* and *to*, *by_user*, the user ID of
	who asked, or *null* when bundled did it by itself, and *note*.

*advisories --json* has:

*system*
	A list with an object for each advisory that applies to the system,
	the worst first: *id*, *DKSA-YYYY-NNNN*; *severity*, *critical*,
	*high*, *medium* or *low*; *summary*; *aliases*, the other IDs it is
	known by; *fixed_in*, the releases that have the fix, the oldest
	first; *action*, what the advisory says to do, or *null*; *fix*,
	*staged*, *offered*, *waiting*, *held* or *none*, as *advisories*
	above says; and *release*, the release that has the fix with the
	first four, or *null* with *none*.

*apps*
	A list with an object for each flaw of an app installed, the worst
	first: *item*, the app's ID; *version*, the version installed; *id*,
	*severity* (with *unknown* for a catalog that does not say),
	*summary*, *aliases* and *action* as above; *source*, *dksa* for an
	advisory or *catalog* for a flaw its catalog lists; *fixed_in*, the
	versions of the app that have the fix for a *dksa*, and of its
	*component* for a *catalog*; *component*, the component of the app
	that has the flaw as *NAME VERSION*, or *null*; and
	*fixed_by_update*, the version its catalog offers when it has the
	fix, or *null*.

*unchecked*
	As for *check*, with the *item* *system* or *apps*.

# EXIT STATUS

0 when it did what was asked, 1 when it could not, with why on standard
error, and 2 for a usage error.

*advisories* ends with 0 when no advisory applies and no flaw is known in
the apps, 1 when one does, and 3 when none was found but a part could not be
checked, such as the system's with no update server to read, which it says
on standard output. 2 is a usage error, as for the others.

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

Which security advisories apply, and what to do:

```
$ update advisories
update: Dukovo: DKSA-2026-0007, high: A local user can read another user's files (CVE-2026-12345)
update:   Dukovo 2026.3.1 has the fix and is offered: `update apply` stages it
update:   Update to Dukovo 2026.3.1 with `update apply`, then restart.
$ echo $?
1
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

_/system/state/sysupdate/lists_
	The files read from the update server, kept as they were signed,
	among them the security advisories and their index that
	*advisories* reads.

# SEE ALSO

*updated*(8), *sysupdate*(8), *bundle*(1), *updates.toml*(5), *changes*(1),
*system-release*(8)
