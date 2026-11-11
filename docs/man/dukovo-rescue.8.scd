dukovo-rescue(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

dukovo-rescue - repair an installed Dukovo whose system will not start

# SYNOPSIS

*dukovo-rescue*

# DESCRIPTION

Rescue is a whole Dukovo kept on each EFI system partition of an
installed system, in _boot/rescue_: the live system of the release the
system was installed from, which runs from memory and needs nothing from
the root to start (ADRs 0459 and 0460). The boot menu offers it always,
as *Rescue (Dukovo* _version_*)*, and starts it by itself, after showing
for 10 seconds, when three starts in a row were not seen through by the
health check (*sysupdate*(8)), or when neither slot's menu can be read.

The menu starts Rescue with *rescue=*_uuid_, the root it repairs. Spark
then starts only the log, the firewall, notices, the screen reader and
the consoles; nothing listens on the network, and networking starts only
when it is turned on here. Every console, and the serial console where
the system keeps one, runs *dukovo-rescue* in place of a sign-in, and
again whenever it ends.

*dukovo-rescue* mounts the root, says which it is and where, and asks for
one of the computer's administrators, a member of _admins_, checked
against the root's own accounts. A root whose accounts cannot be read, or
that has no administrator, asks for nobody. Three wrong answers end it,
and the console starts it again after 10 seconds.

```
Dukovo Rescue 0.15.0
Rescue repairs this computer's Dukovo. Files, apps and settings stay as they are
unless you choose otherwise. Networking is off until you turn it on.

The Dukovo root is 5f0c2d1e-..., on sda4.
Rescue needs one of this computer's administrators.
Name: alice
Password:
Rescue: alice is an administrator of this computer.

What would you like to do?
  1  Reinstall Dukovo, keeping settings, apps and files
  2  Put back the settings from before the last update
  3  Check the root's disk
  4  Copy the users' and shared files to another disk
  5  Turn networking on
  6  Open a shell (type exit to come back)
  7  Start Dukovo normally
  8  Restart
  9  Power off
Choose 1 to 9:
```

A choice that needs the root, or a last update to undo, says *(not here)*
when there is none.

# CHOICES

*1 Reinstall Dukovo*
	Builds the system into the default slot, or slot A when no slot is
	left, from Rescue's own copy of the release, or from an update folder
	on a disk (*sysupdate stage* reads the same), signed by the root that
	Rescue trusts. It starts from a snapshot of the slot, so that only
	what differs is written, and afresh when that fails, reads every
	file back against the signed list, and seals the slot read-only. The
	slot state then names the slot the default, with no previous version
	and no update, and the count of starts not seen through goes back to
	0. The settings, the state, every user's files and apps, and shared
	files are not written. Rescue reinstalls its own version; the system
	then updates as usual.

*2 Put back the settings*
	Runs the fallback's steps (ADR 0454) on the last update's snapshot,
	_@snapshots/update_: the settings the system has now are kept in
	_settings-failed_, and those from before the update take their place.

*3 Check the disk*
	Unmounts the root and runs *fsck-btrfs*(8) on it, read-only, then
	mounts it again. When it finds problems, copy the files off before
	anything else.

*4 Copy files off*
	Copies _@users_ and _@shared_ to a new folder,
	_dukovo-rescue-_*time*, on another disk or partition, which Rescue
	mounts and unmounts.

*5 Turn networking on*
	Starts the network services the system has, behind the firewall
	already running: for a reinstall from an update folder on a network
	share, or to copy files off over the network from the shell.

*6 Open a shell*
	Runs root's shell, with the root mounted at _/volumes/_*disk*. Exit
	the shell to come back.

*7 Start Dukovo normally*
	Sets the count of starts not seen through back to 0 and restarts, so
	that the menu starts the default slot again.

*8 Restart*, *9 Power off*
	End Rescue.

# EXIT STATUS

0
	The console's input ended.

1
	Three wrong answers to the administrator's sign-in; or it was not
	started by the boot menu's Rescue entry, with *rescue=* on the
	kernel's command line; or the command line was wrong.

# FILES

_boot/rescue_ on each EFI system partition
	Rescue: _startup/_ with the kernel, _system_, the boot archive of
	the rest of the release, and _release/_, its signed metadata and
	list. _boot/rescue.new_ and _boot/rescue.old_ are there only while a
	new copy is switched in.

_boot/grub/rescue.cfg_, _boot/grub/rescue.env_ on each EFI system partition
	Rescue's menu entry, and the count of starts in a row that were not
	seen through, *rescue_starts*.

# SEE ALSO

*sysupdate*(8), *system-release*(8), *spark*(8), *consoled*(8),
*dukovo-install*(8), *dukovo-boot*(7), *fsck-btrfs*(8)
