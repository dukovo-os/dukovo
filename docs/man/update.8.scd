update(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

update - see, take, hold back and undo every update: the system, its firmware set and the apps

# SYNOPSIS

*update* [*check*] [*--json*]++
*update apply* [*--only* _part_] [*--yes*]++
*update undo* _item_ [*--settings-from-before* | *--keep-settings*]++
*update hold* _item_ [*--why* _text_]++
*update unhold* _item_++
*update history* [*--json*]++
*update advisories* [*--json*]++
*update last* [*--json*]++
*update settings* [*--json*]++
*update set* [*--check* *notify*|*download*|*off*] [*--apps* *notify*|*auto*]
[*--unattended* *on*|*off*] [*--restart* _HH:MM_ | *--no-restart*]
[*--metered* *wait*|*download*]++
*update checks* [*--json*]++
*update checks add* _name_ (*--url* _url_ [*--insecure*] | *--service* _name_)
[*--required*] [*--timeout* _seconds_]++
*update checks remove* _name_++
*update checks essential* _service_ *on*|*off*

_item_ is *system*, *firmware* or an app's ID. _part_ is *system*,
*firmware*, *apps* or an app's ID.

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

*apply* [*--only* _part_] [*--yes*]
	Stages the system update into the spare slot, to start at the
	next restart, which the owner picks; or, when no system update
	waits, the firmware set. The two share the spare slot, so a new
	firmware set waits until a new system has proved itself. Then it
	updates the apps installed for everyone, asking first unless
	*--yes* is given. An administrator's, through *elevate*(8).

	With *--only*, it does that part alone and nothing else. *system*
	stages the system update, and says that none waits when none
	does, without taking the firmware set in its place. *firmware*
	stages the firmware set, even while a system update waits. *apps*
	updates every app installed for everyone, and an app's ID updates
	that app, as *bundle update* does; what goes wrong with an app is
	then the command's answer, with the exit status *bundle* gives,
	where without *--only* it is told and the command ends with 0.

*undo* _item_
	Goes back on one thing. *system* makes the previous version the
	default again, as *sysupdate rollback* does, with the firmware set
	it named. *firmware* does the same when the previous version is
	the same system with another firmware set, and says to undo the
	system when it is another system too. *--settings-from-before*
	and *--keep-settings* choose the system's settings, as for
	*sysupdate rollback*. An app's ID goes back to the app's version
	before, as *bundle rollback* does.

*hold* _item_ [*--why* _text_]
	Holds one thing back at the version the channel offers now, or the
	one it runs: a newer one is taken as usual, and the held one is
	not. *system* and *firmware* are kept in the *[hold]* table of
	*updates.toml*(5); an app as *bundle hold* keeps it, for everyone
	when root holds it. A held version gets no security fixes, so a
	hold is for a while, not for good. *sysupdate stage --version* still stages a held release
	when asked by name.

	*--why* keeps a reason with the hold, so that whoever sees the hold
	sees why: one line of 1 to 200 characters, with no control character
	(else the status is 2). Everyone who can sign in sees it, as
	*update settings* and *update check* say it, so it is not for what
	should stay private. For *system* and *firmware* it is kept in
	_/system/state/sysupdate/hold-reasons.json_ with the version it was
	given for, who gave it (the user who ran *elevate*, not root) and
	when; it counts only while the hold names that version, and the
	next *hold* or *unhold* drops one that does not. For an app it is
	the note of the hold's event in bundled's history (*bundle*(1)).
	Holding again at the same version without *--why* keeps the reason
	it has.

*unhold* _item_
	Lifts the hold, and the reason kept with it.

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

*last*
	Says the last check that ran as root, which is the daily check of
	*updated*(8), or *elevate update check --json*: when it ran, who
	made it, and what each part offered then. Anyone may read it. It
	is what the web console shows when it opens, without asking the
	update server. A check by a person who is not root keeps nothing,
	as it cannot write where the record is kept. A check by *update
	check* without *--json* keeps nothing either. With nothing kept,
	it says so and ends with status 3. With *--json*, one document
	says it; see *JSON*.

*settings*
	Says how this system updates: the channel, what *updated*(8)
	does when it finds an update, what it does with apps and on a
	metered connection, whether the machine is unattended and when it
	restarts, and the versions held back (*updates.toml*(5)), with
	the defaults filled in where the file says nothing. It says too
	whether *bundled*(8) updates the apps of everyone by themselves,
	and when that disagrees with *apps*. With *--json*, one document
	says it; see *JSON*. Anyone may read it.

*set* [*--check* _word_] [*--apps* _word_] [*--unattended* _word_] [*--restart* _HH:MM_ | *--no-restart*] [*--metered* _word_]
	Changes the settings given in *updates.toml*(5), as *settings*
	says them, and nothing else: the comments and the order of the file
	stay, and a key that already has the value asked, in the file or by
	default, is not written. It says *update: settings changed: KEY =
	VALUE* for each key it changed, or *update: nothing to change*, and
	ends with status 0. *--check* is *notify*, *download* or *off*;
	*--apps* is *notify* or *auto*; *--unattended* is *on* or *off*;
	*--restart* is a time of day, _HH:MM_ as *03:30*; *--no-restart*
	takes the time away; *--metered* is *wait* or *download*. A restart
	time is for a machine that is unattended: it is refused, with status
	1, without *--unattended on* in the file or given with it, and
	*--unattended off* takes the time away with it. A time that is not
	_HH:MM_ is refused, with status 1. With no setting at all it is a
	usage error. *--apps* also has *bundled*(8) update the apps of
	everyone by themselves, or not, as *bundle update --automatic*
	does, so that the two agree. An administrator's, through
	*elevate*(8).

*checks*
	Says what a new version of the system must do to prove itself, as
	the health check reads *health.toml*(5): the services the machine
	needs, the seconds they must run and the seconds an update has to
	start them, and the checks of your own. With *--json*, one
	document says it; see *JSON*. Anyone may read it. When the file
	cannot be read, it says why and ends with status 1: the health
	check goes by the defaults then.

*checks add* _name_ (*--url* _url_ [*--insecure*] | *--service* _name_) [*--required*] [*--timeout* _seconds_]
	Adds a check to *health.toml*(5). *--url* makes a check that
	passes when the web address answers: the command *curl -fsS -g
	--max-time* _seconds_ _url_, with *-k* after *-g* for *--insecure*,
	which accepts a certificate that is not trusted. *--service* makes
	a check that passes when the service runs: *systemctl is-active*
	_name_. Give one of the two. _name_ is what the log calls the check,
	1 to 64 letters, digits, spaces, *.*, *_* and *-*, starting with a
	letter or a digit. *--required* makes an update fail when the check
	fails; without it the check is only logged. *--timeout* is the
	seconds it may take, 1 to 600, 30 by default. A name in use is
	refused, with status 1. An administrator's, through *elevate*(8).

*checks remove* _name_
	Takes the check called _name_ out of *health.toml*(5), with the
	comments above it. If none is called that, it says *update: no
	check is called* _name_ and ends with status 1.

*checks essential* _service_ *on*|*off*
	Adds the service to the *essential* list of *health.toml*(5), or
	takes it out. Where the file has no list, the one the health check
	goes by without it is written, with the change. A service that is
	listed already, or not listed, is told so, and the command ends with
	status 0.

# JSON

*--json* with *check*, or no word at all, and with *history*,
*advisories*, *last*, *settings* and *checks*, prints one line of JSON, an object whose first member is *format*, 1 (ADR 0541),
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

	*system* and *firmware* also have *held_because* and *held_at*,
	each *null* unless *state* is *held*. *held_because* is
	*fell-back* when the health check held the release it fell back
	from, which it records in */system/state/sysupdate/held.json*, and
	that record names the version the hold holds; else *asked*. *held_at*
	is when it was held, for *fell-back*, and *null* for *asked*.
	*held_reason* is the text *update hold --why* kept with the hold,
	or *null*: with no reason, unless *state* is *held*.

	An app: *item* is its ID; *from* is the version installed and *to*
	the newest its catalog lists. *state* is *update*, or *held*
	when *bundle hold* holds it at its version. *catalog* is the
	catalog it updates from, or *null* for its own feed. *held_reason*
	is why it is held, as for the system, or *null*.

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

*last --json* has:

*checked_ms*
	When the check ran, in milliseconds since 1970.

*by*
	*updated* for the daily check, or *update check* for *elevate
	update check --json*.

*updates*
	The document *check --json* printed, as it printed it, with its
	own *format*.

With no check kept, it prints *null*, and ends with status 3.

*settings --json* has:

*channel*, *check*, *apps*, *metered*
	As *updates.toml*(5) has them: *check* is *notify*, *download* or
	*off*; *apps* is *notify* or *auto*; *metered* is *wait* or
	*download*.

*unattended*, *restart*
	Whether the machine stages updates itself, and the time of day,
	_HH:MM_, it restarts into one, or *null*.

*hold*
	An object with *system* and *firmware*, each the version held back
	or *null*.

*held*
	*null*, or, when the health check held the release it fell back
	from and that hold is still there, an object with *version*, *why*
	(*fell-back*), *at_ms*, when, in milliseconds since 1970, and
	*start*, the number of the start that fell back.

*hold_reasons*
	An object with *system* and *firmware*, each *null* or, while the
	hold at that version stands, the reason *update hold --why* kept
	with it: an object with *version*, the version held, *text*, *by*,
	the user ID of who held it, and *at_ms*, when. The words of
	*settings* say it as *hold system reason = text*, where there is one.

*apps_automatic*
	Whether *bundled*(8) updates the apps of everyone by themselves.

*apps_disagree*
	Whether *apps* and *apps_automatic* say different things.
	*update set --apps* makes them agree.

*server*, *canary*
	The update server's address and whether this machine is a canary.

*checks --json* has:

*essential*, *stable*, *deadline*
	As *health.toml*(5) has them, with the defaults filled in.

*checks*
	A list with an object for each check of your own: *name*,
	*command*, its words as a list, *required*, *timeout*, in seconds,
	and *kind* and *target*. *kind* is *url* with *target* the web
	address, for a command that *checks add --url* makes; *service*
	with *target* the service's name, for one that *--service* makes;
	and *command* for any other, with *target* its words on one line.

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

*last* ends with 3 when no check is kept.

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
	(*updates.toml*(5)), which *settings* reads and *set* changes.

_/system/settings/sysupdate/health.toml_
	What a new version of the system must do to prove itself
	(*health.toml*(5)), which *checks* reads and changes.

_/system/state/sysupdate/last-check.json_
	The last check that ran as root, one line, which *last* reads.

_/system/state/sysupdate/held.json_
	The release the health check held after it fell back from it,
	which the health check writes, *check* and *settings* read, and
	*unhold system* takes away with the hold.

_/system/state/sysupdate/hold-reasons.json_
	The reasons kept with the holds of *system* and *firmware*, one
	line of JSON, mode 0644, which *hold --why* writes, *check* and
	*settings* read, and *unhold* takes from.

_/system/state/starts_
	The starts' records that *history* reads (*changes*(1)).

_/system/state/sysupdate/lists_
	The files read from the update server, kept as they were signed,
	among them the security advisories and their index that
	*advisories* reads.

# SEE ALSO

*updated*(8), *sysupdate*(8), *bundle*(1), *updates.toml*(5), *health.toml*(5),
*changes*(1),
*system-release*(8)
