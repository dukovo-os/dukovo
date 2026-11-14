sysupdate(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

sysupdate - fetch, stage, try, prove and roll back updates of Dukovo's system

# SYNOPSIS

*sysupdate status*

*sysupdate check* [*--version* _version_]

*sysupdate download* [*--version* _version_]

*sysupdate stage* [*--version* _version_] [_repository_ | _file_]

*sysupdate stage --request* _file_

*sysupdate channel* [*stable* | *beta* | *nightly*]

*sysupdate unstage*

*sysupdate verify* [_repository_ | _file_]

*sysupdate rollback* [*--settings-from-before* | *--keep-settings*]

*sysupdate firmware* [*stage* [*--version* _version_] [_repository_] | *remove*]

*sysupdate health*

# DESCRIPTION

An installed Dukovo runs its system, all of _/system_, read-only from one
of two slots, the subvolumes _@system.a_ and _@system.b_ of its Btrfs
root. Each slot holds what GRUB starts, in _/system/startup_: the kernel,
the startup archive, the boot menu and _slot.env_, which says the slot's
version. The other slot, the spare, holds the version before, or an
update (ADR 0451).

An update is built into the spare slot while the system runs, and starts
at the next restart, as a try. GRUB takes one of its three tries before
each start of it, so that a hang, a panic and a power cut each cost one.
The health check, a service, then decides: an update whose essential
services run for two minutes, and whose required checks pass, proves
itself and becomes the default, with the slot it updated as the previous
version. One that fails, or runs out of tries, falls back: the version
before starts again by itself, with the settings it had before the
update (ADRs 0452 to 0454). Nothing installs or restarts by itself: the
update waits for a restart the administrator chooses.

The root's top level, where the slots are, is held only by the services
that need it, each in a view of its own (ADR 0473). The health check, the
service *sysupdate*, holds the two slots, the staging place and the
firmware store by name, the EFI system partitions and its own state, and
no home folder. The stage, the service *sysupdate-stage*, holds all of
that, and the root's *@settings*, *@state* and *@snapshots* besides, for
the snapshots a fallback needs, and the network; it holds no home folder
either. It is not started at boot: *update apply* asks for a system
update or a firmware set by writing the request, one line, *system* or
*firmware* _set_, to _/system/live/sysupdate/request_ and starting the
service, whose program is *sysupdate stage --request* _file_. The
service stages it and writes what it said, a *said* line each, and how
it ended, *ok* or *failed* _why_, to _/system/live/sysupdate/outcome_,
which *update* reads and prints; it then ends, and Spark counts a stage
that failed no failed service. When it does not answer, *log
sysupdate-stage* shows what it printed. A person who runs *sysupdate
stage* from a shell has the whole tree and needs no service. Where the root is, and which
partitions it is on, the updater reads from Spark's record of it,
_/system/live/spark/root_ (*spark*(8)), and the partition type of each
partition from _/kernel/block/_*name*_/parttype_.

Which slot starts is the slot state, two copies on each EFI system
partition, which GRUB reads and writes before each start. A power cut
while one copy is written leaves the other whole.

*status* says which slot runs, what each slot holds, which starts next,
the health check's last result, the last update, and where the slot
state is:

```
# sysupdate status
Running:  slot A, Dukovo 0.15.0
Slot A:   Dukovo 0.15.0, the default
Slot B:   Dukovo 0.15.1, an update being tried: 3 tries left
Next start: slot B
Last health check: passed, 2026-11-11 09:14: slot A (Dukovo 0.15.0)
Last update: 0.15.0 to 0.15.1 in slot B
Slot state: on vda1 (generation 2)
```

Updates come from the update server through a channel: *stable*, the
default; *beta*, two weeks before each stable; or *nightly*, every
change. Each channel offers one release, and the server keeps every
release it offers, signed (ADR 0466). Everything the server says is
checked by Dukovo's TUF client against the root this system trusts and
the metadata it read before, so that no one can offer it an older
release, or older metadata, as new. A system never goes back to an older
version through a channel: it waits until the channel passes the version
it runs. A release names the oldest version that updates to it directly;
a system older than that takes the release it names first, a bridge, and
the newer one at its next update.

*check* reads the channel and says what it offers, whether a bridge comes
first, and how many files this system lacks, and how large they are:

```
# sysupdate check
sysupdate: the stable channel offers Dukovo 0.15.2; this system runs Dukovo 0.15.0
sysupdate: Dukovo 0.15.2 needs 37 files this system lacks, 18.4 MiB; sysupdate stage fetches and stages it
```

*check* also fetches the signed target _troubleshoot/signatures.toml_, as
it fetches a channel's file, when the server has one, and keeps it in
_/system/state/troubleshoot/_ if its version is newer than the shipped
one's and the kept one's (ADR 0474, *troubleshoot-signatures*(5)). A server
with none is no failure. A file that does not read is refused, whole, and
logged once; so is one over 256 KiB. The daily check of *updated*(8) does
the same, and so does staging an update file or a folder that carries one.

A release reaches machines in waves (ADR 0469): the canaries take it on
the day its channel offers it, and the rest from day 1 to day 7, by the
number from 0 to 999 each machine draws the first time it checks and
keeps to itself. The update server holds the waves when a release is
paused or a canary is not green. Until this machine's wave opens, *check*
says when it does, and *stage* stages nothing; *stage --version* takes
the release at once, as its owner asked for it:

```
# sysupdate check
sysupdate: the stable channel offers Dukovo 0.15.2; this system runs Dukovo 0.15.0
sysupdate: it reaches machines in waves; this machine's opens on day 3
sysupdate: the waves are held: paused: disk errors on NVMe
sysupdate: `sysupdate stage --version 0.15.2` takes it now
```

A release the project withdrew is never staged, and *check* says why.

*download* fetches those files, and *stage* with no _repository_ fetches
any that are left and stages the release. Only the files the running
slot lacks are fetched: a file it holds at the same place with the same
contents is kept, and one it holds elsewhere is copied. Each file is
checked against the release's signed list as it lands, and refused if it
is not that file; a fetch cut short goes on from where it stopped at the
next try, up to four tries. Requests carry nothing about the machine or
its owner.

*stage* also takes a repository folder, as *system-release*(8) makes
one, or an update file, a release's repository as one file
(_dukovo-VERSION.update_), for a machine with no network. An update file
is read where it is, and checked as an install stick is: every
signature and file, but not expiry, since a file kept for a while is
still the release it was. *stage* builds the release into the spare
slot, and names it the update to try at the next restart:

. It refuses unless the running slot is the default. An update staged
  earlier is replaced.
. It checks the release's metadata against the root this system trusts,
  _/system/resources/updates/root.json_, and refuses a release that is
  not newer than the running version, or that the running version cannot
  update to directly.
. It snapshots the running slot as _@system.new_, writes only the files
  the release changes, from the files fetched, the repository's _files/_
  or the update file, and removes those it no longer has.
. It reads every file back against the signed list.
. It seals the new slot read-only in the spare's place.
. It snapshots the settings and the state into _@snapshots/update_, for a
  fallback.
. Last, it writes the slot state.

Until that last write the running slot is the one that starts, whatever
happens. A slot a stage left half built, as a power cut can, is removed
at the next start.

```
# sysupdate stage /volumes/UPDATES/dukovo-0.15.1
sysupdate: checking the release against the root this system trusts
sysupdate: building slot B from slot A and the release's changes
sysupdate: wrote 41 changed files; reading the slot back against the signed list
sysupdate: snapshotting the settings and the state, for a fallback
sysupdate: staged Dukovo 0.15.1 in slot B: it starts at the next restart, with 3 tries to prove itself, and Dukovo 0.15.0 starts again if it does not
```

*channel* says which channel this system follows and from which server,
or, with a name, follows that channel from now on.

*unstage* takes a staged update back before it is tried: the running
slot goes on starting, and the update stays in the spare slot until the
next stage.

*rollback* makes the previous version the default from the next restart
on. The settings stay as they are: each release writes only what the
release before it can read. It refuses while an update is staged or
being tried. That rule covers one step, so when the update server
offered a release between the two versions, *rollback* asks first which
settings to take, and says what each choice keeps:

*--settings-from-before*
	Puts the settings from before the update back at the next start,
	as a fallback does: Spark keeps today's in
	_@snapshots/update/settings-failed_ and makes _@settings_ a copy of
	the snapshot the stage took. Only the version the update came from
	has such a snapshot.

*--keep-settings*
	Keeps the settings as they are, whatever the previous version is.

*verify* reads the running slot back against its release's signed list,
as a stage reads the slot it builds: every directory, file and link,
each file's size and SHA-256, and each mode. The list comes from the
_repository_ or update _file_ named, or else from Rescue's copy on the
EFI system partition when it holds the same release, or else from the
update server; each is checked against the trusted root first. It says
what differs, at most ten things, and fails. *updated*(8) runs it once
a week.

*health* is the health check, which Spark starts at every start as the
service _sysupdate_, after every other. It removes a half-built slot,
then watches the essential services that *health.toml*(5) names. An
update being tried proves itself or fails, as above; any other start is
seen through once those services run. Until a start is seen through,
GRUB shows its menu at the next start, so that a machine that stopped on
its way up can be started otherwise; after three such starts in a row,
or with no slot to start, the menu starts Rescue by itself
(*dukovo-rescue*(8), ADR 0460). Each start seen through sets that count
back to 0.

# OPTIONS

*--version* _version_
	The release to check, fetch or stage, in place of the channel's, or
	when a repository holds several. It must be newer than the running
	version.

# FIRMWARE

Closed firmware, such as CPU microcode, stays out of _/system_: it comes
in the optional firmware package, as signed firmware sets that each
slot names. A set's files are kept once each, by SHA-256, in the root's
_@firmware_ subvolume, and GRUB loads the microcode of the set the
starting slot names. A system update keeps the set the running slot
names; only *sysupdate firmware stage* changes it.

*sysupdate firmware*
	Shows the set each slot names, with its files and their licenses,
	and whether this machine has taken the firmware package.

*sysupdate firmware stage* [*--version* _version_] [_repository_]
	Stages the set _version_, or the newest, of the repository folder
	_repository_; with no folder, the set the update server's channel
	offers, or _version_ from the server. The set must be newer than the
	running slot's. The spare slot becomes a snapshot of the running one
	that names the set, staged as an update: its first start is a try,
	and if it does not prove itself, the slot before it starts again,
	with the set before it.

*sysupdate firmware remove*
	Stages a slot that names no set, the same way.

During a firmware set's try, the health check also requires that the
set's microcode reached the kernel and was neither damaged nor refused
(_/kernel/firmware/microcode_, *radix-sysfs*(5)), and that every network
card and root partition the last good start had is still there. At
other starts, what is missing is only logged.

# BOOT MENU

The menu stays hidden unless the last start was not seen through, Shift
is held as the computer starts, or Windows is on the machine. Its first
entry starts what the slot state says, and its title says which: the
version, the version with *(update, first try)*, *(update, second try)*
or *(update, last try)*, or the version before with *(the update to
VERSION failed)*; for a firmware set, *(firmware update, first try)* and
*(the firmware update to VERSION failed)*. While an update is being tried, an entry starts the
version it updates, *(without the update)*, and leaves the tries as they
are. The previous version's entry starts it once, without making it the
default. After the slots' entries, *Rescue (Dukovo* _version_*)* starts
Rescue.

# EXIT STATUS

0
	The command did what it says; *health* always ends with 0, its
	result in the log and the slot state.

1
	It did not. The message says why: the server could not be read, or
	what it sent was refused; nothing to stage, nothing staged to take
	back, or no previous version; a release that is not signed by the
	trusted root, not newer, or made for an update from another version;
	or a write that failed. A stage that fails leaves the running slot and the slot
	state as they were.

2
	The command line was wrong.

# FILES

_/system/settings/sysupdate/health.toml_
	What the health check asks of a new version: *health.toml*(5).

_/system/settings/sysupdate/updates.toml_
	The channel this system follows, and the server: *updates.toml*(5).

_/system/state/starts/_
	The record of each start, which the health check begins and
	finishes, for *changes*(1). The health check and *authd*(8), which
	notes when the first person signed in, each hold the lock _lock_
	there while they change a record.

_/system/live/sysupdate/start_, _/system/live/sysupdate/sign-in_
	The number of this start's record, once the health check has begun
	it, and when the first person signed in on this start, which waits
	there for a record begun later. Both go when the machine stops.

_/system/live/sysupdate/failure_
	What the sign-in screens show of a failed start until the next start
	(ADR 0501): which start failed and why, what Dukovo did, what
	changed since the last good start, and the error ID's support page.
	The health check writes it when a start fails its check and goes on,
	or follows an update that fell back, and shows its first line on the
	screen until a key is pressed. It tells *alertd*(8) of the same, as
	the critical event *update-fell-back* or the warning
	*start-failed*, so that the notice reaches the owner by email or a
	webhook too; and of *update-failed* when an update gives up its tries
	and the machine cannot be restarted into the version before it.
	When the signatures of *troubleshoot*(8) name a likely cause, a
	line says so: "Likely cause: _cause_ (_ID_). Fix: _fix_".

_/system/state/troubleshoot/signatures.toml_
	The newer signatures for *troubleshoot*(8) that a check or an update
	file brought.

_/system/state/sysupdate/metadata_
	The update server's metadata this system has read and trusts.

_/system/state/sysupdate/files_
	The files fetched for the next stage, by SHA-256; a stage removes
	them once the slot holds them.

_/system/state/sysupdate/wave_
	This machine's number from 0 to 999, which places it in a wave.

_/system/state/sysupdate/health_
	The health check's last result.

_/system/state/sysupdate/devices_
	The network cards and root partitions of the last start that
	passed the health check, which a firmware set's or a version's try
	must still have.

_@firmware_ on the root's top level
	The firmware sets' files, by SHA-256, in _files_, and each set's
	signed list in _sets_. Sets no slot names are removed after a stage.

_startup/firmware.env_ in each slot
	The firmware set the slot names, and its microcode's SHA-256.

_/system/live/sysupdate/lock_
	Keeps a stage and the health check from writing the slot state at
	once. It names the process that holds it, which the other finds in
	_/processes_: both services' files name it with *files.read*.

_/system/live/sysupdate/request_, _/system/live/sysupdate/outcome_
	What *update apply* or *updated*(8) asked of the *sysupdate-stage*
	service, and how it went. An asker holds the lock
	_/system/live/sysupdate/asking_ until it has read the outcome, and
	starts the stage only when none runs, so two that ask at once take
	turns.

_/system/live/spark/root_
	Which root this is, and the partitions it is on (*spark*(8)).

_@snapshots/update_ on the root's top level
	The settings and the state from before the last update, what it was,
	and after a fallback the failed attempt's settings,
	_settings-failed_.

_boot/grub/state1_, _boot/grub/state2_ on each EFI system partition
	The two copies of the slot state.

_boot/grub/rescue.env_ on each EFI system partition
	The count of starts in a row that were not seen through.

# DIAGNOSTICS

When an update will not stage, the last line *stage* prints says why:
start there. A line naming the server and ending in "nothing was changed"
means that the server could not be read, or that what it sent was not
signed by keys this system trusts, had expired, was older than what it
read before, or was not the file the signed list names: the system did
not use it.

When an update fell back, the health check's line in the log says what
failed (*log sysupdate*), and the settings the update had are in
_@snapshots/update/settings-failed_.

# SEE ALSO

*changes*(1), *update*(8), *updated*(8), *health.toml*(5), *updates.toml*(5), *spark*(8), *dukovo-boot*(7),
*dukovo-rescue*(8), *system-release*(8), *service*(8), *alerts*(8)
