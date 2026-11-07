dukovo-install(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

dukovo-install - install Dukovo onto a disk from the install stick

# SYNOPSIS

*dukovo-install* [*--release* _dir_]

*dukovo-install --answers* _file_ [*--release* _dir_]

*dukovo-install --hash-password* [*--test-cost*] < _file_

# DESCRIPTION

dukovo-install installs Dukovo onto a disk from the signed release on
the install stick: from the live system the stick starts, or from any
Dukovo system that has the stick plugged in. With no arguments it shows
its screens, which ask what to install and where (ADR 0310); with
*--answers*, it installs as an answers file says (*install-answers*(5)),
with no questions asked (ADR 0309). The same answers install the same
system every time, which is how a fleet of servers is installed, and
how the boot tests install one.

# SCREENS

The stick's boot menu starts the screens on tty1 with its first entry,
*Install Dukovo*, and *Live console* on their first page leaves them
for a shell, where *dukovo-install* starts them again. They first show
a startup screen, whose bar fills as the release on the stick is found
and its signatures checked, the computer's disks and network cards are
read, and the stick's setup files are listed; when the installer cannot
start, as on a computer not started from the stick, it says why there
and waits for Enter, which leaves it for a shell. The screens are drawn in
the approved design's sixteen colours, which they set as the terminal's
palette while they show and give back when they end, a slate background
edge to edge and the keys' hints on a steel bar along the bottom (ADR
0402). A bar at the top shows the five steps:

. *Welcome*: the language, the keyboard layout, which the keys type in
  at once, a box to try it, and whether to answer step by step or
  install from a setup file.
. *Network and name*: the network cards found, DHCP or a fixed address
  for the card chosen, the computer's name and its time zone.
. *Disk*: the disk to install on and how: the guided layout, which
  erases it; its free space, which keeps its partitions; or Manual,
  which opens the rdisk screen. The guided and free-space layouts take
  the swap's size. The stick and a disk too small cannot be chosen, a
  disk that holds Windows says so in red, and the free space says why
  when it cannot be used.
. *You*: the administrator, whose user name follows the name typed
  until it is typed itself, a password typed twice, whether the saved
  answers keep the passwords' hashes, and other people.
. *Review*: everything the install will do. It is the only step that
  changes the computer: Install asks for the disk's name, typed, first.

The rdisk screen lists each disk's partitions as they are and as they
will be, by tabs, with a bar of the disk: N makes a partition in the
free space, for a use (the Dukovo root, the EFI system partition, swap,
BIOS boot, data on Btrfs, exFAT or FAT32, or empty), D takes one out or
keeps it again, F formats one for a use, G lays out the guided
partitions on the disk, U undoes the last change, and Tab shows the
next disk. Each change is worked out as rdisk will write it, and
nothing is written before the review's Install. Enter checks that the
changes leave a Dukovo root and an EFI system partition on one GPT
disk, which is the target.

Tab and Shift+Tab, or Up and Down, move between the fields and the
buttons; Space chooses; Enter goes on; Esc goes back; F1 shows the
page's help. Left and Right change a list, the time zone or the layout,
and letters typed on it find the first name that starts with them,
*america/den* for America/Denver. Each page's answers are checked as an
answers file's are when Next leaves it, and what is wrong is shown above
the buttons.

The install then shows its steps, a bar of its progress and the log's
last line; L shows the whole log, during the install and after, and the
shell on Alt+F2 keeps working throughout. The Done page restarts the
computer, or saves the answers as a setup file on the stick for the
next computer, without the passwords' hashes unless they were kept.

A setup file is an answers file called _install.json_ at the top of the
stick's EFI system partition, which any computer can write. *From a
setup file* on the first page shows the one the stick holds and what is
wrong with it, if anything; Load goes to the review.

Every message the screens show is in a Fluent file built into the
program, one a language, so that a translation is a copy of one file.

# ANSWERS FILES

With *--answers*, it checks the whole answers file before it touches a disk, and names
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
  its layout must fit: the root big enough for the system and a GiB
  more.
. *Partitioning* the disk, and each other disk the manual layout
  changes, as *rdisk --script* applies a layout file
  (*rdisk-layout*(5)) the installer writes and logs. The guided layout
  erases the disk and makes a 1 MiB BIOS boot partition, a 512 MiB EFI
  system partition, the swap the answers ask for, and the Dukovo root
  in the rest; the free-space layout makes the same four in the disk's
  largest free space and keeps the rest, numbering the EFI system
  partition 2, where the BIOS's GRUB finds its files, or leaving out the
  BIOS boot partition if the disk has a partition 2; the manual layout makes the
  answers' *changes*.
. *Making the filesystems*: FAT32 on the EFI system partition, a swap
  area, the root's Btrfs, and the data partitions the manual layout
  formats, with *mkfs-fat*(8), *mkswap*(8), *mkfs-btrfs*(8) and
  *mkfs-exfat*(8).
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
  A disk the manual layout gives no BIOS boot partition gets GRUB for
  UEFI alone.
. *Saving the answers and the install log* in the new system.

It ends with the line to act on:

```
dukovo-install: Dukovo 0.14.0 is installed on vda. Remove the install stick, then restart.
```

# OPTIONS

*--answers* _file_
	Install as the answers file _file_ says, with no screens.

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

_install.json_
	A setup file, at the top of the install stick's EFI system
	partition: the screens offer it, and their Done page writes it.

# EXIT STATUS

0 when the install finished, 1 when the answers file has problems or
the install failed, and 2 for a usage error.

# SEE ALSO

*install-answers*(5), *system-release*(8), *rdisk*(8), *rgrub-setup*(8),
*spark*(8), *consoled*(8), *dukovo-boot*(7), *keymap*(1)
