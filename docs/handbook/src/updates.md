<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Updates, snapshots, backups and recovery

This chapter describes how Dukovo updates its system, and what happens
when an update goes wrong. The full references are sysupdate(8),
health.toml(5), spark(8) and dukovo-boot(7); ADRs 0450 to 0459 record
the design.

An update never leaves the machine unable to start, never touches your
files, and never installs or restarts without your say. A new version is
built beside the running one, tried at a restart you choose, and kept
only if it proves itself; anything short of that brings the version you
had back, by itself.

## Two slots

An installed Dukovo keeps its whole system, everything under `/system`
except the settings and the state, in a slot: a read-only Btrfs
subvolume, `@system.a` or `@system.b`. One slot runs. The other holds
the version before, or an update waiting for a restart. Each slot holds
its own kernel, startup archive and boot menu, so a slot changes all at
once.

`sysupdate status` shows where things stand:

```
$ elevate sysupdate status
Running:  slot A, Dukovo 0.15.0
Slot A:   Dukovo 0.15.0, the default
Slot B:   Dukovo 0.14.1, the previous version
Next start: slot A
Last health check: passed, 2026-11-11 09:14: slot A (Dukovo 0.15.0)
Slot state: on nvme0n1p1 (generation 1)
```

## Staging an update

`sysupdate stage` builds a release into the spare slot while the system
runs. It starts from a snapshot of the running slot, so unchanged files
cost no space, writes only the files that changed, reads every file
back against the release's signed list, and seals the slot read-only.
It also snapshots the settings and the state. Until its very last step,
a power cut leaves the machine exactly as it was.

```
$ elevate sysupdate stage /volumes/UPDATES/dukovo-0.15.1
sysupdate: checking the release against the root this system trusts
sysupdate: building slot B from slot A and the release's changes
sysupdate: wrote 41 changed files; reading the slot back against the signed list
sysupdate: snapshotting the settings and the state, for a fallback
sysupdate: staged Dukovo 0.15.1 in slot B: it starts at the next restart, with 3 tries to prove itself, and Dukovo 0.15.0 starts again if it does not
```

Nothing else happens until you restart. To change your mind first,
`elevate sysupdate unstage`.

A release must be signed by the root your system trusts and newer than
the version that runs. Releases come from a repository folder today, as
`ob release` makes one; downloading them from the update server comes
with the update command.

## Trying and proving

At the restart, the boot loader starts the update as a try, and counts
the try before it starts it, so a hang, a panic or a power cut each
spend one of its three tries. Then the health check watches the
essential services: logging, sign-in, the consoles, the network and SSH.
When they have all run for two minutes with no restart, and any checks
you require pass, the update proves itself. It becomes the default, and
the version before stays in the other slot as the previous version.

You can ask more of an update in `/system/settings/sysupdate/health.toml`,
such as your web site answering:

```toml
[[check]]
name = "web site"
command = ["curl", "-fsS", "http://localhost/"]
required = true
```

health.toml(5) describes every setting.

## When an update fails

An update falls back to the version you had when:

- an essential service keeps restarting, or stops after a failure;
- the essential services are not all running within ten minutes;
- a required check fails;
- or it runs out of tries: a kernel that panics restarts by itself after
  10 seconds, and a system that hangs is restarted by the hardware
  watchdog, where the machine has one.

The version before then starts with no keypress, and before any service
starts it puts back the settings from before the update. The settings
the failed update had are kept in `@snapshots/update/settings-failed`,
for a look at what went wrong, and the log says why it failed:

```
$ elevate log sysupdate
sysupdate: slot B (Dukovo 0.15.1) failed its health check: logd has restarted 3 times
sysupdate: the update gives up its tries; restarting into slot A (Dukovo 0.15.0)
```

## Going back later

If you find a problem days later, `elevate sysupdate rollback` makes the
previous version the default from the next restart on. Your settings
stay as they are: each release writes only what the release before it
can read, so nothing you changed since the update is lost.

## The boot menu

The boot menu stays hidden unless you hold Shift as the computer starts,
or the last start did not get as far as the health check seeing it
through; then it shows for 10 seconds and starts Dukovo by itself.
Besides the version that starts, it offers the previous version, which
starts once without becoming the default, and, while an update is being
tried, the version without the update. Its last entry, Rescue, is the
next section.

## Rescue

Rescue is a small, whole Dukovo kept on the EFI system partition, the
release the system was installed from, for a computer whose system will
not start at all: both slots broken, or a root that will not mount. It
needs nothing from the root to start, and changes nothing until you
choose.

The boot menu starts Rescue by itself when three starts in a row did not
finish, or when it finds no slot to start; you can also choose it from
the menu (hold Shift as the computer starts). Networking stays off, and
Rescue asks for one of the computer's administrators before anything
else:

```
Dukovo Rescue 0.15.0
The Dukovo root is 5f0c2d1e-..., on sda4.
Rescue needs one of this computer's administrators.
Name: alice
Password:

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
```

**Reinstall** builds the system again from Rescue's own copy, or from an
update folder on a USB stick, every file checked against the signed
list. Settings, apps and everyone's files are not touched. Once it says
the system is back, choose Restart. **Put back the settings** undoes
the settings of the last update, as a fallback would. **Check the disk**
looks for damage to the root's filesystem; if it finds any, copy your
files off first, with **Copy files off**, onto another disk or stick.
**Start Dukovo normally** clears the count of unfinished starts, for when
the cause, such as a loose cable, is fixed.

The installer puts Rescue on each disk that starts the system; it takes
about 230 MiB of the 512 MiB EFI system partition the guided layout
makes. See dukovo-rescue(8) for the details.

## Snapshots

A snapshot holds a folder as it was at a moment, read-only, and takes
room only for what changes after it. Dukovo makes one of `/users`,
`/shared` and `/system/settings` every day, keeps the newest of each of
the last 7 days and of each of the last 4 weeks, and removes the rest.
They are in `/snapshots`, a folder for each:

```text
$ ls /snapshots/users
2026-10-25-0300  2026-11-05-0300  2026-11-07-0300  2026-11-09-0300  2026-11-11-0300
2026-11-01-0300  2026-11-06-0300  2026-11-08-0300  2026-11-10-0300
```

A file you deleted or changed by mistake comes back with `snap
restore`, from the newest snapshot that has it, without an
administrator: your files in a snapshot are yours, and no one else can
read them there either.

```text
$ snap restore ~/notes.txt
Restored /users/alice/notes.txt from /snapshots/users/2026-11-11-0300/alice/notes.txt
```

It never writes over a file that is there: `--to` restores beside it,
and `--from` picks an older snapshot. Folders are restored the same
way. You can also open a snapshot like any folder and copy from it.

An administrator makes a snapshot before a risky change, and gives it a
name so that it is kept until deleted:

```sh
elevate snap create --name before-the-move /users
snap list /users
elevate snap delete /users before-the-move
```

`snap list /users` says why each is kept. The places and how many days
and weeks are kept are in `/system/settings/snap.toml`, which can also
name a Btrfs disk in `/volumes`; see snap.toml(5). Apps never see
`/snapshots`, and nothing in a snapshot can be changed.

A deleted file keeps its room on the disk until the last snapshot that
has it is removed, up to four weeks later. To free it sooner, delete
those snapshots with `snap delete`. See snap(1) for the details.

## What is on the way

Backups; downloading updates from the update
server in waves, through one `update` command for the system, firmware
and apps, which also keeps Rescue up to date; and repairing GRUB and the
EFI system partition from an install stick. This chapter grows with
each.
