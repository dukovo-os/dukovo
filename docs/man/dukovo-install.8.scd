dukovo-install(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

dukovo-install - install Dukovo onto a disk from the install stick

# SYNOPSIS

*dukovo-install --answers* _file_ [*--release* _dir_]

*dukovo-install --hash-password* [*--test-cost*] < _file_

# DESCRIPTION

dukovo-install installs Dukovo onto a disk from the signed release on
the install stick, as an answers file says (*install-answers*(5)), with
no questions asked: from the live system the stick starts, or from any
Dukovo system that has the stick plugged in (ADR 0309). The same answers
install the same system every time, which is how a fleet of servers is
installed, and how the boot tests install one. The installer's screens,
which ask the same questions, come next and write the same file.

It checks the whole answers file before it touches a disk, and names
each problem by its key:

```
# dukovo-install --answers web01.json
dukovo-install: web01.json: time_zone: "America/Denvr" is not a time zone's name, such as America/Denver
```

An install then runs these steps, each printed as it starts:

. *Checking the signatures of the system release*: the release in the
  stick's DUKOVO_LIVE partition, against the root the running system
  trusts, _/system/resources/updates/root.json_, as *system-release
  check* does (*system-release*(8)). A release that fails is not
  installed.
. *Checking* the disk: it must be a whole disk, not the stick, and
  big enough for the layout, the system and a GiB more.
. *Partitioning* the disk: it is erased and gets the guided layout, as
  *rdisk --script* applies it (*rdisk-layout*(5)): a 1 MiB BIOS boot
  partition, a 512 MiB EFI system partition, the swap the answers ask
  for, and the Dukovo root in the rest.
. *Making the filesystems*: FAT32 on the EFI system partition, a swap
  area, and the root's Btrfs, with *mkfs-fat*(8), *mkswap*(8) and
  *mkfs-btrfs*(8).
. *Making the subvolumes* of the root, as an installed system has them
  (*spark*(8)).
. *Copying Dukovo* _version_ *into its slot, checking every file*:
  every file of the release into the slot
  _@system.a_, each checked against the signed list's size and SHA-256
  as it is written, so that a damaged stick stops the install instead
  of installing a damaged system. The slot is then made read-only.
. *Writing the settings and accounts*: the host name, keyboard layout, time zone and
  network cards in _/system/settings_, and the accounts, the
  administrator's and the others', with their passwords' hashes, where
  only *authd*(8) reads them.
. *Installing GRUB for UEFI and BIOS*: GRUB for a BIOS in the first sector and
  the BIOS boot partition, and for UEFI in the EFI system partition, with
  *rgrub-setup*(8), so that the disk starts under either firmware, and
  the menu that starts the slot, naming the root by its UUID (ADR 0307).
. *Saving the answers and the install log* in the new system.

It ends with the line to act on:

```
dukovo-install: Dukovo 0.14.0 is installed on vda. Remove the install stick, then restart.
```

# OPTIONS

*--answers* _file_
	Install as the answers file _file_ says.

*--release* _dir_
	Install the release in _dir_, a release's repository as
	*system-release*(8) describes it, instead of the one on the
	install stick.

*--hash-password*
	Print the Argon2id hash of the password on the first line of
	standard input, for an answers file's *password_hash*: an
	answers file never holds a password itself. With a shell's
	*read -s*, the password is not shown or kept:

	```
	$ read -s pw; echo "$pw" | dukovo-install --hash-password; unset pw
	$argon2id$v=19$m=65536,t=3,p=4$...
	```

*--test-cost*
	With *--hash-password*, hash at the boot tests' low cost, which
	is quick and easy to guess: for tests only.

# FILES

_/tmp/install.log_
	The install log, as the install runs: each step, each command it
	ran with what that printed, and each problem, stamped with the
	seconds since the install started. When an install fails, read it
	from its end, from the shell on another console (Alt+F2).

_/system/state/log/install.log_
	The install log, in the installed system.

_/system/settings/install/answers.json_
	The answers the system was installed with, without the passwords'
	hashes unless the answers file keeps them, so that the install can
	be repeated.

# EXIT STATUS

0 when the install finished, 1 when the answers file has problems or
the install failed, and 2 for a usage error.

# SEE ALSO

*install-answers*(5), *system-release*(8), *rdisk*(8), *rgrub-setup*(8),
*spark*(8)
