dukovo-rescue(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

dukovo-rescue - repair an installed Dukovo whose system will not start

# SYNOPSIS

*dukovo-rescue* [*--ssh* _name_]

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
when it is turned on here, or when SSH comes up, which it does only if
the system had SSH on (see SSH). Every console, and the serial console where
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
unless you choose otherwise.

The Dukovo root is 5f0c2d1e-..., on sda4.
Networking is off until you turn it on, and SSH stays off: this computer had it off.
Rescue needs one of this computer's administrators.
Name: alice
Password:
Rescue: alice is an administrator of this computer.

The last 3 starts were not seen through.
Start 42 on 2026-11-12 08:14, Dukovo 0.15.0, slot A: unfinished
Its services started, but its health check never finished: it stopped, hung or lost power after that.
From the last good start, 41 on 2026-11-11 21:03, Dukovo 0.15.0, to start 42:
  app org.example.notes: new, 0.1.0; installed 2026-11-12 08:02; first used 2026-11-12 08:14 (start 42)

What would you like to do?
   0  Why the last start failed, and what changed
   1  Find the likely cause
   2  Find the change that broke the start
   3  Reinstall Dukovo, keeping settings, apps and files
   4  Put back the settings from before the last update
   5  Check the root's disk
   6  Copy the users' and shared files to another disk
   7  Check the hardware
   8  Turn networking on
   9  Check the network
  10  Open a shell (type exit to come back)
  11  Start Dukovo normally
  12  Restart
  13  Power off
Choose 0 to 13:
```

A choice that needs the root, or a last update to undo, says *(not here)*
when there is none. The line after the root says what Rescue did about SSH,
whichever way it went; when the root does not mount, it says only that
networking is off until it is turned on.

Rescue never takes two repairs at once: see ONE REPAIR AT A TIME.

Once the administrator has signed in, Rescue says why it may have started
(ADR 0501): how many starts in a row were not seen through, as the boot
menu counts them, and the newest start that failed or never finished,
from the root's records of each start, with what changed since the last
good start, as *changes*(1) says it, and the support page of its error
ID. When the newest record is of a good start, the starts counted after
it stopped before the health check began. Spark marks a start that
reaches its services (ADR 0503): when one of them did, Rescue says so,
and that the start with only the essentials, on the start menu's
Troubleshooting, is the one to try; when none did, they stopped in the
kernel or mounting the root, and the start with all of the kernel's
messages shows where. When no menu shows at all, *Repair startup* on the
install stick puts GRUB and the menu back (*dukovo-install*(8)).

# SSH

When the installed system had SSH on, Rescue turns it on too, before it
asks at the screen for an administrator (ADR 0540), so that a computer
with no screen can be repaired from where its owner reached it before. It
does so only if the root's *sshd* service is enabled, its accounts can be
read and name an administrator, and its host key is there. It then:

- links the root at _/system/live/rescue/root_, in a folder only root may
  write, for *authd*(8) to read;
- copies *sshd*(8) the five files it reads, _sshd_config_, _passwd_,
  _group_, the host key and its public half, into
  _/system/state/sshd/rescue_, in Rescue's memory, so that the program that
  faces the network never sees the password hashes or a home;
- takes the root's network settings, the cards' addresses, the DNS, the
  firewall's rules and _blocking.toml_, where the root has them, in place
  of Rescue's own, and loads the firewall, so that the computer is where
  its owner reached it before;
- turns networking on, and starts *authd* and *sshd*.

Rescue says what it did in one line, whichever way it went:

```
SSH is on, as it was on this computer: its administrators can reach Rescue over it.
Networking is off until you turn it on, and SSH stays off: this computer had it off.
```

The reasons SSH stays off are that the computer had it off, that its
*sshd* service file does not read or is not there, that the root's
accounts cannot be read, that it has no administrator, and that the root's
host key cannot be read. Turning networking on by hand, with choice 8,
never starts *sshd*. A console that starts Rescue again finds SSH running
and starts nothing.

Over SSH, the root's administrators sign in with the keys of their
_authorized_keys_ and, only if the root's _sshd_config_ lets passwords
in, with their passwords; its *AllowUsers*, *AllowGroups*, *DenyUsers*
and *DenyGroups* apply, root never signs in, and the host key is the
installed system's, so that a client sees the fingerprint it knows.
Addresses that guess passwords are blocked as in the installed system
(*authd*(8), *blocking.toml*(5)), in Rescue's memory only. A root whose
accounts cannot be read gives no SSH at all, though the screen then asks
for no password.

*authd* starts this program for each administrator who signs in, as root
and on their terminal, with *--ssh* _name_, and with no password asked
again. In place of the sign-in it says who signed in and from where:

```
Dukovo Rescue 0.15.0
Rescue repairs this computer's Dukovo. Files, apps and settings stay as they are
unless you choose otherwise.

The Dukovo root is 5f0c2d1e-..., on sda4.
alice signed in over SSH from 192.168.1.5.
```

The menu is the one at the screen, and its *Open a shell* choice is the
shell. A command, SFTP and forwarding are not allowed over SSH in Rescue.
authd answers nothing else in Rescue and writes nothing to the root.

# ONE REPAIR AT A TIME

The choices that change something, 3 (reinstall), 4 (put back the
settings), 5 (check the disk), 6 (copy files off), 11 (start normally),
12 (restart) and 13 (power off), take a lock, so that a person at the
screen and administrators over SSH do not repair at once. A second person
who chooses one is told who holds it, what for and from where, and chooses
again when it is done:

```
Rescue: alice is reinstalling Dukovo, over SSH from 192.168.1.5: wait for it to finish, then choose again
```

A person at the screen is at *the console*; one who was asked for no
password is *someone*. The other choices run side by side. The lock is
*flock*(2) on _/system/live/rescue/lock_, which the kernel lets go
however its holder ends, so that no repair is held for good by a program
that died. The line it holds names the person, what they do and where.

# OPTIONS

*--ssh* _name_
	Skips the sign-in: _name_ is the administrator who signed in over
	SSH, which *authd*(8) has checked, and the address is the first word of
	*SSH_CONNECTION*. It is *authd*'s, which starts the program as root for
	each administrator; run by anyone but root, it fails, saying so.

# CHOICES

*0 Why the last start failed*
	Shows again what Rescue said of the last starts after the sign-in.

*1 Find the likely cause*
	Names the likely cause of what went wrong, as *troubleshoot*(8) does
	(ADR 0474): of the root's newest start that did not go well, from its
	record and what changed since the last good start, or, when the root
	does not mount, of that. Each match says its cause, its fix and the
	support page of its error ID; with none, it says that no known cause
	matches what Rescue can see. It works when the root is not mounted,
	which is when the other choices that need it say *(not here)*. It goes
	by the signatures Rescue's own copy of the release ships, or the newer
	file the root keeps, when it mounts.

*2 Find the change that broke the start*
	Says which start failed and which was the last good one before it,
	and what to run to find the change between them: *elevate changes
	bisect* (*changes*(1)), once the system is up, even with a service
	failing. Rescue cannot run the search itself. Each try is a start of
	the installed system, which judges it with its own health check, and
	the search keeps its state in the system's _/system/state_, which
	in Rescue is memory. Nothing is changed. It says why when no start
	has failed or no good start is kept.

*3 Reinstall Dukovo*
	Builds the system into the default slot, or slot A when no slot is
	left, from Rescue's own copy of the release, or from an update folder
	or update file on a disk (*sysupdate stage* reads the same), signed
	by the root that Rescue trusts. It starts from a snapshot of the
	slot, so that only what differs is written, and afresh when that
	fails, reads every file back against the signed list, and seals the
	slot read-only. The slot state then names the slot the default, with
	no previous version and no update, and the count of starts not seen
	through goes back to 0. The settings, the state, every user's files
	and apps, and shared files are not written. Rescue reinstalls its own
	version; the system then updates as usual.

*4 Put back the settings*
	Runs the fallback's steps (ADR 0454) on the last update's snapshot,
	_@snapshots/update_: the settings the system has now are kept in
	_settings-failed_, and those from before the update take their place.

*5 Check the disk*
	Unmounts the root and runs *fsck-btrfs*(8) on it, read-only, then
	mounts it again. When it finds problems, copy the files off before
	anything else.

*6 Copy files off*
	Copies _@users_ and _@shared_ to a new folder,
	_dukovo-rescue-_*time*, on another disk or partition, which Rescue
	mounts and unmounts.

*7 Check the hardware*
	Runs *hardware check* (*hardware*(8), ADR 0488): the memory, the
	processors, the disks' self-tests and the other parts it checks,
	for a start that fails in ways that point at a part. Rescue shows
	its results and waits for Enter before it shows the choices again.
	It needs no root and no network.

*8 Turn networking on*
	Starts the network services the system has, behind the firewall
	already running: for a reinstall from an update folder or file on a
	network share, or to copy files off over the network from the shell.
	It does not start *sshd*: SSH is turned on only when Rescue starts
	(see SSH).

*9 Check the network*
	Turns networking on as choice 8 does, waits up to 20 seconds for a
	card to have an address, then runs *netcheck* (*netcheck*(8), ADR
	0489), which says at which layer the network stops working. Rescue
	waits for Enter before it shows the choices again. Nothing leaves
	the local network: *netcheck* is run without *--internet*.

*10 Open a shell*
	Runs root's shell, with the root mounted at _/volumes/_*disk*. Exit
	the shell to come back. Over SSH, it is the shell of the session.

*11 Start Dukovo normally*
	Sets the count of starts not seen through back to 0 and restarts, so
	that the menu starts the default slot again.

*12 Restart*, *13 Power off*
	End Rescue.

# EXIT STATUS

0
	The console's input ended.

1
	Three wrong answers to the administrator's sign-in; or it was not
	started by the boot menu's Rescue entry, with *rescue=* on the
	kernel's command line; or *--ssh* was given by someone other than
	root; or the command line was wrong.

# FILES

_boot/rescue_ on each EFI system partition
	Rescue: _startup/_ with the kernel, _system_, the boot archive of
	the rest of the release, and _release/_, its signed metadata and
	list. _boot/rescue.new_ and _boot/rescue.old_ are there only while a
	new copy is switched in.

_/system/live/rescue/root_
	In Rescue, with SSH on, the link to the root that Rescue mounted,
	which *authd*(8) reads the installed system's accounts through. In a
	folder of mode 0700, root's: only a system started with *rescue=* on
	the kernel's command line reads it.

_/system/live/rescue/lock_
	The lock a repair holds while it runs, and its line: the person, what
	they do, and where, a tab between them.

_/system/state/sshd/rescue/_
	The five files *sshd*(8) reads of the installed system, copied from
	the root as SSH comes up, mode 0600, in Rescue's memory.

_boot/grub/rescue.cfg_, _boot/grub/rescue.env_ on each EFI system partition
	Rescue's menu entry, and the count of starts in a row that were not
	seen through, *rescue_starts*.

# SEE ALSO

*sysupdate*(8), *system-release*(8), *spark*(8), *consoled*(8), *authd*(8), *sshd*(8), *blocking.toml*(5),
*dukovo-install*(8), *dukovo-boot*(7), *fsck-btrfs*(8), *changes*(1),
*hardware*(8), *netcheck*(8)
