sysupdate(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

sysupdate - fetch, stage, try, prove and roll back updates of Dukovo's system

# SYNOPSIS

*sysupdate status* [*--json*]

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

*sysupdate canary enrol* [_name_]

*sysupdate canary report* [*--dry-run*]

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
service, whose program is *sysupdate stage --request* _file_; the health
check asks for *rescue*, below, the same way, and does not wait. The
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
the health check's last result, the last update, where the slot state
is, and which release Rescue was last written from:

```
# sysupdate status
Running:  slot A, Dukovo 0.15.0
Slot A:   Dukovo 0.15.0, the default
Slot B:   Dukovo 0.15.1, an update being tried: 3 tries left
Next start: slot B
Last health check: passed, 2026-11-11 09:14: slot A (Dukovo 0.15.0)
Last update: 0.15.0 to 0.15.1 in slot B
Slot state: on vda1 (generation 2)
Rescue: Dukovo 0.15.0
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
paused or a canary is not green, which the canaries tell it each hour
(*canary report* below, ADR 0533). Until this machine's wave opens, *check*
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
. It adds up the sizes of the files the release changes, an eighth more
  for Btrfs's metadata and 16 MiB, and refuses if the Dukovo root has
  less than that free, before it writes anything: "the update needs N MiB
  on the Dukovo root, and M MiB are free".
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
seen through once those services have run for 10 seconds (or *stable*,
if less) with none restarting, so that one that ends at once and is
started again fails it. Until a start is seen through,
GRUB shows its menu at the next start, so that a machine that stopped on
its way up can be started otherwise; after three such starts in a row,
or with no slot to start, the menu starts Rescue by itself
(*dukovo-rescue*(8), ADR 0460). Each start seen through sets that count
back to 0.

Rescue follows the release that runs (ADR 0543). Once a start is seen
through while Rescue holds an older release, the first being the start
that proves an update, the health check asks the stage to write Rescue
again on each EFI system partition of the root's disks, and goes on
without waiting. The stage writes it at idle disk priority from the
running slot's files and the release's signed metadata, which it kept
when it staged the release, each file checked against the signed list
as it is written, and the copy read back before it replaces the one
there; so a reinstall from Rescue puts back the release that runs, not
the one the machine was installed with. A firmware set staged alone
changes no release and writes nothing, and a rollback leaves Rescue on
the newer release it holds. A partition without room for the new copy
beside the old, as the default 512 MiB one, gets it in the old one's
place, once the copy in the slot has read back whole. One without room
even for that, or whose write fails, keeps the copy it has, and the
stage's log says why with UPD-102; the next start tries again. While
Rescue is being written, *rollback*, *stage* and the other commands
that change the slot state wait for the write to finish, and say so.

A start that fails its check and goes on leaves a notice on the sign-in
screens (ADR 0501), which, when the changes since the last good start
can be tried apart, names *elevate changes bisect* and the most restarts
it takes. A search of that command (*changes*(1), ADR 0475) makes
starts of its own, its tries, each a start of one slot that the boot
menu takes once, with some changes left out. The health check judges a
try as any other start, records it with its try's number, and never
counts it as the last good start. Then it takes the search's next step:
60 seconds after its verdict it writes the next try and restarts the
machine, saying so on the console and in a notification, which gives
whoever is there time to stop it with *changes bisect --stop*; or it
ends the search, keeping what it found for the next sign-in (ADR 0504).
It never changes the slot state: the default, the previous version and
an update's tries are as they were. The boot menu takes no try while an
update is being tried, and *changes bisect* does not begin one then.

# OPTIONS

*--version* _version_
	The release to check, fetch or stage, in place of the channel's, or
	when a repository holds several. It must be newer than the running
	version.

*--json*
	With *status*, print the document of *JSON* below in place of the
	text.

*--dry-run*
	With *canary report*, print the report and the exact bytes that a
	report sends, and send nothing.

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
	offers, once its wave reaches this machine, or _version_ from the
	server at once. A firmware set goes out in waves of its own, as a
	release does, which a pause of the release does not hold (ADR 0544).
	The set must be newer than the running slot's. The spare slot becomes a snapshot of the running one
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

# CANARIES

A canary is a machine that takes each release on the day its channel
offers it, before the waves reach the rest, and tells the update server
each hour how it does. The server holds the waves of a release while a
canary is not green: its last health check failed, an update fell back,
the checks of *dukovo-check*(8) found failures or could not run, a
service failed, it has not taken the newest release in six hours, or it
has not reported in two hours (ADR 0533). The project's canaries are its own; anyone who
serves updates may run one against their own update server. Both
commands need an administrator, and the Spark job that reports does
nothing on any machine that is not an enrolled canary.

*sysupdate canary enrol* [_name_]
	Makes this machine a canary. It makes an Ed25519 key, unless the
	machine has one, in _/system/settings/sysupdate/canary.key_, for
	root alone, and writes *canary = true* and *canary_name* in
	*updates.toml*(5), _name_ or the host name; _name_ is 1 to 32 of
	*a* to *z*, *0* to *9* and *-*, the first not *-*, and the command
	says so when the host name is not one. Then it prints the line to
	run on the maintainer's machine, which lists the canary on the
	update server with the public half of its key:

```
# sysupdate canary enrol ref-box
sysupdate: this machine is the canary ref-box: it takes every release first and reports to the update server each hour; to list it there, run this on the maintainer's machine:
./ob release canary add ref-box 3b6a27bcceb6a42d62a3a8d02a6f0d73653215771de243a63ac048a18b59da29
```

	Run again on an enrolled machine, it prints the same two lines and
	changes nothing. To rename a canary, set *canary_name* in
	*updates.toml*(5) and run it again; the update server lists the
	new name with *./ob release canary add*.

*sysupdate canary report* [*--dry-run*]
	Builds a report of this machine, signs it with the key, and posts
	it over HTTPS to the update server's host, at _/canary/v1/report_,
	as *check* reaches the server. It ends with 0 when the server took
	the report, and with 1 and a message when it did not. The report
	holds:

	- *running*, the release the machine runs, *staged*, the release
	  waiting for the next start, if there is one, and *channel*;
	- *health*, the health check's last result: whether it passed, when
	  it ran, and what it said;
	- *fell_back*, when the last update was tried, never proved itself
	  and the machine went back: the release *tried* and the one it
	  went *back_to*;
	- *check*, what the checks of *dukovo-check*(8) find when run now:
	  how many *failures* and *warnings*, and up to 32 *failed* checks
	  as "area: check: detail", or no *check* at all when they could
	  not run;
	- *services_failed*, up to 32 services that Spark gave up on or
	  finds not running as asked, as *service list --failed* says them.

	Each text is on one line of at most 512 characters, and the whole
	report is at most 16 KiB. The report is signed as it is sent, and the
	update server takes it only from a listed key, for the canary's own
	name, newer than the one it keeps. It never holds a key; it only
	checks the signature. With *--dry-run* it prints the report laid
	out to read and the exact bytes of the request body, and sends
	nothing. On a machine that is not a canary, where *canary* is not
	*true* or *canary_name* or the key is missing, it prints one line
	saying so and ends with 0.

The Spark job *dukovo-canary-report* runs *canary report* each hour, a
little after the hour begins so that canaries do not report at the same
moment, and *updated*(8) starts it once after its first check, so that a
restart into a new release is reported soon. A canary's *updated* checks
for updates each hour in place of once a day. *service
dukovo-canary-report status* and *log dukovo-canary-report* (*service*(8),
*log*(1)) show the job and what it said.

# BOOT MENU

The menu stays hidden unless the last start was not seen through, Shift
is held as the computer starts, or Windows is on the machine. Its first
entry starts what the slot state says, and its title says which: the
version, the version with *(update, first try)*, *(update, second try)*
or *(update, last try)*, or the version before with *(the update to
VERSION failed)*; for a firmware set, *(firmware update, first try)* and
*(the firmware update to VERSION failed)*; or the slot of a try of
*changes bisect* with *(changes bisect, try* _N_*)*, which the menu
starts once in place of the usual start and then forgets. While an
update is being tried, an entry starts the
version it updates, *(without the update)*, and leaves the tries as they
are. The previous version's entry starts it once, without making it the
default. After the slots' entries, *Rescue (Dukovo* _version_*)* starts
Rescue.

# JSON

*status --json* prints one line of JSON, an object whose first member is
*format*, 1 (ADR 0541), which scripts and Ansible's *dukovo.system*
collection read in place of the text. A later field may be added without
a new *format*. Names are in snake_case, times are RFC 3339 in UTC, and a
value that is not known is *null*. An error is told to standard error as
without *--json*, with the same exit status. The members:

*running*, *default*
	The slot that runs and the slot that starts unless an update is
	being tried, each *a* or *b*.

*previous*
	The slot that holds the version before the default, which *rollback*
	goes back to, or *null*.

*staged*
	An update that has not proved itself, or *null*: *slot*, and
	*tries_left*, the starts it has left to prove itself in. With none,
	the next start runs the default.

*next*
	The slot the next start runs.

*unfinished_start*
	The slot whose start was not seen through, or *null*; the boot menu
	shows at the next start while it is set.

*slots*
	A list with an object for each slot, *a* then *b*: *slot*,
	*version*, the release it says it is, and *firmware*, the firmware
	set it names. Each is *null* for a slot that is empty or says
	nothing, or names no set. The firmware set the system runs is the
	running slot's.

*channel*, *canary*
	The channel this system follows, and whether it is one of the
	project's canaries, which take each release on its first day.

*wave*
	This machine's number from 0 to 999, which places it in a wave of
	a release, or *null* before the machine has drawn it. Asking for the
	status does not draw it.

*held*
	The versions *update hold* holds back: *system* and *firmware*, each
	a version or *null*.

*last_health_check*
	The health check's last result, or *null*: *time*, *passed*, and
	*detail*, what it found.

*last_update*
	The last update, or *null*: *from*, *to*, *slot*, and
	*settings_restored*, whether a fallback put the settings from before
	it back.

*rescue*
	The release Rescue holds, or *null* before it is written.

*slot_state*
	A list with an object for each partition that holds the slot state:
	*partition*, and *generation*, that of its newest whole copy, or
	*null* when it has none.

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
	state as they were. A canary's report that the update server did not
	take also ends with 1; on a machine that is not a canary, *canary
	report* ends with 0.

2
	The command line was wrong.

# FILES

_/system/settings/sysupdate/health.toml_
	What the health check asks of a new version: *health.toml*(5).

_/system/settings/sysupdate/updates.toml_
	The channel this system follows, and the server: *updates.toml*(5).

_/system/settings/sysupdate/canary.key_
	A canary's signing key, an Ed25519 key in PKCS #8, which *canary
	enrol* makes, with mode 0600 for root alone. Without it, or without
	*canary_name* in *updates.toml*(5), a machine reports nothing.

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

_/system/state/sysupdate/roots_
	Each root of the update server this system walked to since it was
	installed, by version, so that Rescue's copy of the metadata reads
	from the root the release trusts.

_/system/state/sysupdate/release_
	The signed metadata of the release staged last, as a repository,
	from which Rescue is written once it proves itself.

_/system/state/sysupdate/rescue_
	The release Rescue was last written from, or found to hold.

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
	What *update apply*, *updated*(8) or the health check asked of the
	*sysupdate-stage* service, and how it went. An asker holds the lock
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

_/system/live/sysupdate-esp/_
	Where the stage mounts each EFI system partition, for the while, to
	write Rescue.

_boot/rescue_, _boot/rescue.new_ on each EFI system partition
	Rescue, and a new copy of it while one is written.

_boot/grub/rescue.env_ on each EFI system partition
	The count of starts in a row that were not seen through.

_boot/grub/trial_ on each EFI system partition
	The try of *changes bisect* that waits, its slot and boot options,
	and whether the menu has taken it. The slot state is never changed
	by it.

_/system/state/bisect/_
	The search of *changes bisect*, _state.json_, and for each try
	_try-_N, the services Spark leaves out of it (*spark*(8)).

# DIAGNOSTICS

When an update will not stage, the last line *stage* prints says why:
start there. A line naming the server and ending in "nothing was changed"
means that the server could not be read, or that what it sent was not
signed by keys this system trusts, had expired, was older than what it
read before, or was not the file the signed list names: the system did
not use it.

A server's metadata is good for a limited time (a day, for its newest
timestamp), judged by the computer's clock. When it has expired, the line
says when it expired and what the computer's clock says, and to set the
clock (*timedatectl*(1) *set-time*) if it is wrong: a clock far ahead
refuses the server's current metadata, which is not at fault. A clock
behind the real time is not refused, since the metadata cannot be too new
for it, but a timestamp older than the one read before still is.

When an update fell back, the health check's line in the log says what
failed (*log sysupdate*), and the settings the update had are in
_@snapshots/update/settings-failed_.

# SEE ALSO

*changes*(1), *update*(8), *updated*(8), *health.toml*(5), *updates.toml*(5), *spark*(8), *dukovo-boot*(7),
*dukovo-rescue*(8), *dukovo-check*(8), *system-release*(8), *service*(8), *alerts*(8)
