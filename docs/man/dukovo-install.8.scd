dukovo-install(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

dukovo-install - install Dukovo onto a disk from the install stick or DVD

# SYNOPSIS

*dukovo-install* [*--release* _dir_]

*dukovo-install --answers* _file_|_address_ [*--sha256* _hex_] [*--release* _dir_]

*dukovo-install --hash-password* [*--test-cost*] < _file_

# DESCRIPTION

dukovo-install installs Dukovo onto a disk from the signed release on
the install stick or DVD: from the live system the stick or DVD starts,
or from any Dukovo system that has the stick plugged in. The DVD, as
*./ob image --iso* makes it, carries the release in its _release_
directory and is found by its volume's label, *DUKOVO_LIVE* (ADR 0433);
what this page says of the stick holds for it, except that a disc holds
no setup file: a setup file comes from the web, and the Done page saves
none. With no arguments it shows its screens, which ask what to install
and where (ADR 0310); with *--answers*, it installs as an answers file
says (*install-answers*(5)), with no questions asked (ADR 0309). The same answers install the same
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
0402). A bar at the top shows the six steps:

. *Welcome*: the language, the keyboard layout, which the keys type in
  at once, a box to try it, whether to answer step by step or install
  from a setup file, and the accessibility: the screen reader, the text
  size, sticky keys and slow keys, which change the live system at once
  and are kept on the installed system (ADR 0422).
. *Network and name*: the network cards found, DHCP or a fixed address
  for the card chosen, the computer's name and its time zone.
. *Disk*: the disk to install on and how: the guided layout, which
  erases it; next to Windows, which shrinks Windows and takes the space
  typed, between the least and the most the page shows; a mirror of
  two disks, which erases both, with the second disk chosen with Left
  and Right; its free space, which keeps its partitions; or Manual,
  which opens the rdisk screen. Every layout but Manual takes the
  swap's size. The stick and a disk too small cannot be chosen, a disk
  that holds Windows says so in red, and the layouts that cannot be
  used say why: next to Windows, what to do in Windows first.
. *Software*: what the machine is for, a profile: Minimal, Server or
  Cloud and VM; the release's features as boxes to tick, each with the
  services it turns on, and those that install bundles dimmed, to add
  after the first boot with *feature add*; and whether to turn on the
  web console, which the profile chooses until it is changed and which
  release 0.16 brings (ADR 0317).
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
the buttons. The field with the focus is marked with › in the margin as
well as by its colours.

With the screen reader on, from the stick's *Install Dukovo with speech*
entry or the Welcome page, the screens tell it what they show with
*consoled*(8)'s focus sequence (ADR 0325, ADR 0422): each page as it
comes, its step and its text; each field as the focus reaches it, its
label, its kind, its place among the choices beside it, its value and
its note; the character typed or taken away, or the new value; each
problem as it shows; and each step of the install as it starts. What a
password field holds is never said: a character typed into it is
"star". The reader's own keys, held with Caps Lock or Insert, still
review the screen (*reader*(1)).

The install then shows its steps, a bar of its progress and the log's
last line; L shows the whole log, during the install and after, and the
shell on Alt+F2 keeps working throughout. The Done page restarts the
computer, or saves the answers as a setup file on the stick for the
next computer, without the passwords' hashes unless they were kept.

A setup file is an answers file called _install.json_ at the top of the
stick's EFI system partition, which any computer can write, or on a web
server. *From a setup file* on the first page shows the one the stick
holds and what is wrong with it, if anything, then *Download it from a
web address*, which is chosen when the stick holds none: an _https://_
address, and the SHA-256 the file must have, if one is given, as
*sha256sum*(1) prints it. Load downloads the file over HTTPS and
refuses it if its SHA-256 differs, as *--answers* does (see ANSWERS
FILES, ADR 0319); the review then says where it came from and whether
it was checked. Load checks that each feature a setup file adds is one
this release offers at install, then goes to the review.

The stick's *Install Dukovo (serial console)* entry starts the screens
on the first serial port instead, for a computer with no screen, where
*serial-console*(8) runs them, and root's shell after them; any
terminal program at the cable's other end shows them, at the size it
says it is, or 80 columns by 24 rows, which they fit. There they are
drawn in plain ASCII, boxes of + - and |, since the terminal is not sure
to show UTF-8, and the title ends with *(serial console)*. The review's
settings then end with *serial console ttyS0*: the installed system
keeps *console=ttyS0* in its boot menu, so that its sign-in is on the
serial port too (*install-answers*(5)'s *serial_console*, ADR 0318).

Every message the screens show is in a Fluent file built into the
program, one a language, so that a translation is a copy of one file.

# ANSWERS FILES

With *--answers*, it checks the whole answers file before it touches a disk, and names
each problem by its key:

```
# dukovo-install --answers web01.json
dukovo-install: web01.json: time_zone: "America/Denvr" is not a time zone's name, such as America/Denver
```

The answers file can also be on a web server, for a fleet that keeps
one file a machine: *--answers* then takes its _https://_ address, and
*--sha256* the SHA-256 it must have, which the person who wrote it
gives with it (ADR 0319):

```
# dukovo-install --answers https://setup.example.org/web01.json \
    --sha256 4f2a3b0c95e1d8a7f6c4b3a2e1d0c9b8a7f6e5d4c3b2a1f0e9d8c7b6a5f4e3d2
dukovo-install: https://setup.example.org/web01.json: The SHA-256 does not match the downloaded file. It was not used.
```

Nothing signs an answers file, so only HTTPS is used, so that it cannot
be changed on the way: an _http://_ address, or a redirect to one, is
refused, and so is a server whose certificate is not from a root the
system trusts (*dukovo-certificates*(7)). With *--sha256*, a file that
differs is refused before a word of it is read, which also catches a
server that serves another file than the one meant. A file larger than
1 MiB is not an answers file and is refused too. The log names where the
answers came from, with the file's SHA-256 and whether it was checked;
*--sha256* checks a file on a disk the same way.

An install then runs these steps, each printed as it starts:

. *Checking the signatures of the system release*: the release in the
  stick's DUKOVO_LIVE partition or on the DVD, against the root the
  running system trusts, _/system/resources/updates/root.json_, as
  *system-release check* does (*system-release*(8)). A release that
  fails is not installed.
. *Checking* the disk, or both disks of a mirror: each must be a whole
  disk, not the stick, and its layout must fit: the root big enough for
  the system and a GiB more.
. *Shrinking Windows (C:) on* _disk_, for the layout next to Windows:
  *resize-ntfs*(8) checks Windows' volume, shrinks it by the space
  Dukovo takes, and checks it again. The volume stays marked to be
  checked, so Windows runs *chkdsk* once at its next start.
. *Partitioning* the disk, and each other disk the manual layout
  changes, as *rdisk --script* applies a layout file
  (*rdisk-layout*(5)) the installer writes and logs. The guided layout
  erases the disk and makes a 1 MiB BIOS boot partition, a 512 MiB EFI
  system partition, the swap the answers ask for, and the Dukovo root
  in the rest; the free-space layout makes the same four in the disk's
  largest free space and keeps the rest, numbering the EFI system
  partition 2, where the BIOS's GRUB finds its files, or leaving out the
  BIOS boot partition if the disk has a partition 2; the layout next
  to Windows cuts Windows' partition to its shrunk volume and makes a
  512 MiB EFI system partition of Dukovo's own, *Dukovo boot*, the
  swap and the root in the space it gave, keeping Windows' partitions;
  the mirror layout erases both disks and makes the guided layout's
  four partitions on each, the two roots of one size, the most the
  smaller disk holds; the manual layout makes the answers' *changes*.
. *Making the filesystems*: FAT32 on the EFI system partition, a swap
  area, the root's Btrfs, and the data partitions the manual layout
  formats, with *mkfs-fat*(8), *mkswap*(8), *mkfs-btrfs*(8) and
  *mkfs-exfat*(8). A mirror's second root is left for the next step.
. *Mirroring the root on both disks (Btrfs RAID1)*, for the mirror
  layout: *device add* puts the second disk's root partition in the
  root's Btrfs, and *balance start -dconvert=raid1 -mconvert=raid1*
  makes the few chunks it has RAID1, a copy on each disk (*device*(8),
  *balance*(8)). Every chunk written after is RAID1 too.
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
. *Installing GRUB*: GRUB for a BIOS in the first sector and the BIOS
  boot partition, and for UEFI in the EFI system partition, with
  *rgrub-setup*(8), so that the disk starts under either firmware, and
  the menu that starts the slot, naming the root by its UUID (ADR 0307).
  A disk given no BIOS boot partition, by the manual layout or next to
  Windows, gets GRUB for UEFI alone. Under UEFI the menu also offers
  *Windows Boot Manager* when it finds Windows, and the firmware's
  settings; it then always shows, and chooses again the system chosen
  last. Next to Windows, *efiboot*(8) also makes the firmware's boot
  entry *Dukovo*, first in its boot order, and Windows' boot files are
  left as they are (ADR 0314). A mirror gets GRUB and the menu on both
  disks, so that either starts the system when the other has failed
  (ADR 0315).
. *Saving the answers and the install log* in the new system.

It ends with the line to act on:

```
dukovo-install: Dukovo 0.14.0 is installed on vda. Remove the install stick, then restart.
```

From the DVD, it says to remove the install disc.

# OPTIONS

*--answers* _file_|_address_
	Install as the answers file says, with no screens: _file_, or the
	file downloaded from _address_, which must start with _https://_.

*--sha256* _hex_
	With *--answers*, the SHA-256 the answers file must have: 64
	hexadecimal digits, as *sha256sum*(1) prints them. A file that
	differs is refused, and nothing is installed.

*--release* _dir_
	Install the release in _dir_, a release's repository as
	*system-release*(8) describes it, instead of the one on the
	install stick or DVD.

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
	The screens also download one from a web address.

# EXIT STATUS

0 when the install finished, 1 when the answers file could not be
downloaded, differs from its SHA-256 or has problems, or the install
failed, and 2 for a usage error.

# SEE ALSO

*install-answers*(5), *system-release*(8), *rdisk*(8), *rgrub-setup*(8),
*resize-ntfs*(8), *efiboot*(8), *device*(8), *balance*(8),
*spark*(8), *consoled*(8), *serial-console*(8), *dukovo-boot*(7), *keymap*(1),
*reader*(1), *accessibility.toml*(5),
*dukovo-certificates*(7), *sha256sum*(1)
