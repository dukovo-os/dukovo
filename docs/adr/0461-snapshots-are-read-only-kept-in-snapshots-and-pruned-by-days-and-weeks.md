<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0461. Snapshots are read-only, kept in /snapshots, and pruned by days and weeks

- Status: Accepted
- Date: 2026-11-11

## Context

Btrfs subvolumes and snapshots came with ADR 0138, and `subvolume`(8)
makes them by hand. Every Dukovo root has had an `@snapshots`
subvolume since the installer's first layout, unmounted, and atomic
updates keep the settings from before the last update in
`@snapshots/update` (ADR 0454). The roadmap's 15b adds `snap`, which
makes, lists, restores and removes snapshots of `/users`, `/system` or
any subvolume, `/volumes` targets included, and prunes them by a
retention policy of 7 daily and 4 weekly snapshots. Its exit criterion
is a deleted file in `/users` restored from a snapshot.

ADR 0442 left `ls` without a mark for snapshots: telling one needs the
subvolume's parent UUID, which Linux gives with
`BTRFS_IOC_GET_SUBVOL_INFO` and Radix gave no program.

Apps run in a view of their own (ADR 0225): the system's folders, their
bundle, their data folder `~/app-data/ID` and their own `/tmp`, and
nothing else of `/users` or `/volumes`.

## Decision

- **Snapshots live in the filesystem's `@snapshots`, a folder for each
  subvolume.** A snapshot of a subvolume is
  `@snapshots/NAME/SNAPSHOT`, where `NAME` is the subvolume's name in
  the folder that holds it, without a leading `@`: `users`, `shared`,
  `settings`, `state`. The top of a filesystem is `top`. `update` stays
  the update's (ADR 0454). On Dukovo's root, Spark mounts `@snapshots`
  read-only at **`/snapshots`**, so `/snapshots/users/2026-11-11-0300`
  holds `/users` as it was then. On another Btrfs filesystem, a
  `/volumes` target, `snap` makes `@snapshots` at its top the first
  time it needs one.
- **Every snapshot is read-only,** by the subvolume's own flag, so a
  write into one fails with `EROFS` whoever makes it, an administrator
  through `/volumes` included. `/snapshots` is a read-only mount as
  well.
- **A snapshot is named by its time, or by a name it is given.** The
  local time to the minute, `YYYY-MM-DD-HHMM`, with `.2` and on for a
  second one in the same minute. A snapshot made with `--name` keeps
  that name, and is kept until it is deleted.
- **The `snap` service makes one snapshot a day of each place
  `snap.toml` names,** `/users`, `/shared` and `/system/settings` unless
  changed: when it starts and then every hour, a place with no snapshot
  made today gets one, and then each place is pruned. A place that is
  not there, as a disk not plugged in, is skipped and logged. Spark has
  no timers, so the service waits between rounds, as `diskhealth` does.
- **Retention keeps, of the snapshots named by their time,** the newest
  of each of the `daily` (7) latest days that have one, the newest of
  each of the `weekly` (4) latest weeks (Monday to Sunday) that have
  one, and the newest of all, as restic's `--keep-daily` and
  `--keep-weekly` count them. Days and weeks are local, by when the
  kernel made each snapshot. Every other snapshot named by its time is
  removed. Snapshots given a name, the update's snapshot and the slots
  are never removed by `snap`: the slot before an update is the
  previous version that `sysupdate rollback` and the boot menu start
  (ADR 0451).
- **Restoring is by file or folder, as the person who asks.** `snap
  restore PATH` copies `PATH` back from the newest snapshot that holds
  it, or from the one `--from` names, to `PATH` or to `--to`, and never
  over a file that is there. The copy is a clone where it can be, so it
  takes no room until either changes. The modes and owners the snapshot
  kept decide what can be read, so a person restores their own files
  without an administrator. Making and removing snapshots needs one:
  `@snapshots` is root's.
- **Snapshots keep the owners and modes of their moment.** A person
  reads in a snapshot what they could read then. Account numbers are
  never given again (ADR 0220), so a removed account's files in a
  snapshot are no one else's.
- **Apps never reach a snapshot.** An app's view holds neither
  `/snapshots`, nor `/volumes`, nor anything of `/users` beyond its own
  data, so an app reads no snapshot, of its own data or another app's,
  and writes none.
- **The kernel tells a subvolume's details to anyone who can read it.**
  The directory method `subvolume` writes its ID, its parent's ID and
  folder, its flags, its transaction numbers and times, its UUID,
  parent UUID and received UUID, and its name, as Linux's
  `BTRFS_IOC_GET_SUBVOL_INFO` gives them without privilege. The C
  library maps that request to it, so `ls` and programs from Linux ask
  as they do there, and `subvolume show` prints them, as `btrfs
  subvolume show` does.
- **`ls -l` marks a snapshot `[snapshot]`, or `[snapshot, read-only]`,**
  in place of the subvolume mark of ADR 0442: a subvolume with a parent
  UUID. Why each is kept is in `snap list`, which knows the retention.
- **The service holds `@snapshots` and the default places** (ADR 0473),
  where it first held the whole tree, as the update's health check did:
  it reads and snapshots the places of the default `snap.toml`, reads
  `/snapshots`, and changes `@snapshots` at the top of the root and
  nothing else there. A place of `snap.toml` outside that view is skipped
  and logged, with the fix, and the owner adds it to the service's file.

## Consequences

- A snapshot takes room only for what changes after it. A deleted
  file's blocks are freed when the last snapshot that holds it is
  removed, up to four weeks later; `snap delete` frees them sooner, and
  the handbook says so where it tells how to free space.
- A file deleted to be rid of it stays in the snapshots until they are
  pruned or deleted. The settings snapshots keep old password hashes,
  readable only by root, as the live file is.
- The boot test `snapshots` has a person delete a file and restore it
  with `snap restore`, moves the clock through 40 days with a round of
  `snap auto` on each and checks that exactly the retention set is
  left, and checks that a write into a snapshot fails, that a person
  cannot read another's files in one, and that an app sees no
  `/snapshots`. The lab's `snapshots` tests do the same on real disks.
- `ls` patch 0013 asks for the parent UUID only for folders that are
  the top of a subvolume, so a listing costs nothing more otherwise.
- **Not yet:** rolling a whole place back to a snapshot, which Rescue
  offers later; apps restoring their own data from a snapshot through a
  portal; retention set place by place; the web console's snapshots
  page (Phase 16).

## Alternatives considered

- **Snapper's `.snapshots` inside each subvolume.** It needs no folder
  per filesystem, but puts snapshots inside what they copy, where a
  listing of `/users` shows them, and a subvolume inside another must
  be removed before it.
- **Hourly snapshots, as Snapper's timeline keeps them.** More
  metadata changes on every machine for a loss of at most a day, which
  a person can make smaller with `snap create` before a risky change.
- **Retention by the time in the name.** The kernel's time is the one
  `snap` cannot get wrong when a snapshot is renamed by hand.
- **A writable `/snapshots`, guarded by modes alone.** The read-only
  flag refuses a write from anyone; a mount that writes would rest on
  every folder's mode being right.
- **Pruning the spare update slot with the snapshots.** It is the
  previous version, kept for a rollback, and costs only what differs
  from the running slot.
