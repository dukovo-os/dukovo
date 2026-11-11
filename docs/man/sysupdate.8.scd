sysupdate(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

sysupdate - stage, try, prove and roll back updates of Dukovo's system

# SYNOPSIS

*sysupdate status*

*sysupdate stage* [*--version* _version_] _repository_

*sysupdate unstage*

*sysupdate rollback*

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

*stage* builds the release in _repository_ into the spare slot, and names
it the update to try at the next restart:

. It refuses unless the running slot is the default. An update staged
  earlier is replaced.
. It checks the release's metadata against the root this system trusts,
  _/system/resources/updates/root.json_, and refuses a release that is
  not newer than the running version, or that the running version cannot
  update to directly.
. It snapshots the running slot as _@system.new_, writes only the files
  the release changes, from the repository's _files/_, and removes those
  it no longer has.
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

*unstage* takes a staged update back before it is tried: the running
slot goes on starting, and the update stays in the spare slot until the
next stage.

*rollback* makes the previous version the default from the next restart
on. The settings stay as they are: each release writes only what the
release before it can read. It refuses while an update is staged or
being tried.

*health* is the health check, which Spark starts at every start as the
service _sysupdate_, after every other. It removes a half-built slot,
then watches the essential services that *health.toml*(5) names. An
update being tried proves itself or fails, as above; any other start is
seen through once those services run. Until a start is seen through,
GRUB shows its menu at the next start, so that a machine that stopped on
its way up can be started otherwise.

# OPTIONS

*--version* _version_
	The release to stage, when the repository holds several.

# BOOT MENU

The menu stays hidden unless the last start was not seen through, Shift
is held as the computer starts, or Windows is on the machine. Its first
entry starts what the slot state says, and its title says which: the
version, the version with *(update, first try)*, *(update, second try)*
or *(update, last try)*, or the version before with *(the update to
VERSION failed)*. While an update is being tried, an entry starts the
version it updates, *(without the update)*, and leaves the tries as they
are. The previous version's entry starts it once, without making it the
default.

# EXIT STATUS

0
	The command did what it says; *health* always ends with 0, its
	result in the log and the slot state.

1
	It did not. The message says why: nothing staged to take back, no
	previous version, a release that is not signed by the trusted root,
	not newer, or made for an update from another version, or a write
	that failed. A stage that fails leaves the running slot and the slot
	state as they were.

2
	The command line was wrong.

# FILES

_/system/settings/sysupdate/health.toml_
	What the health check asks of a new version: *health.toml*(5).

_/system/state/sysupdate/health_
	The health check's last result.

_/system/live/sysupdate.lock_
	Keeps a stage and the health check from writing the slot state at
	once.

_@snapshots/update_ on the root's top level
	The settings and the state from before the last update, what it was,
	and after a fallback the failed attempt's settings,
	_settings-failed_.

_boot/grub/state1_, _boot/grub/state2_ on each EFI system partition
	The two copies of the slot state.

# DIAGNOSTICS

When an update will not stage, the last line *stage* prints says why:
start there.

When an update fell back, the health check's line in the log says what
failed (*log sysupdate*), and the settings the update had are in
_@snapshots/update/settings-failed_.

# SEE ALSO

*health.toml*(5), *spark*(8), *dukovo-boot*(7), *system-release*(8),
*service*(8)
