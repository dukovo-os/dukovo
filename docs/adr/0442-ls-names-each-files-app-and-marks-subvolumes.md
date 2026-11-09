<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0442. ls names each file's app and marks subvolumes

- Status: Accepted
- Date: 2026-11-09

## Context

This is the third part of the new `ls` features (ADRs 0440 and 0441
have the first two). Those came from eza;
these two come from Dukovo itself, and eza has nothing like them.

A Dukovo app is a bundle installed sealed in `/applications/ID` or a
person's `applications/ID` (ADR 0223), keeps its data in their
`app-data/ID`, and its temporary files in `/tmp/.app-ID` (ADR 0225).
Looking at a file there, nothing in GNU's listing says which app it is,
or whether its folder is still sealed, so that the bundle runtime will
start it. The root is Btrfs subvolumes, each mounted at its folder
(ADR 0137), and nothing in the listing says which folders those are, or
that `/system` cannot be changed.

## Decision

Two additions to `ls`, in a third patch of the port
(`ports/uutils-coreutils/patches/0011`, code in `apps.rs` and
`subvolumes.rs`).

- **`--app`** adds a column to the long listing, just before the name:
  the app each file belongs to, and a word for how. **sealed** is a
  file inside an installed bundle whose top folder and the file itself
  are root's and read-only, as bundled leaves them (a link's own mode
  counts for nothing); **not sealed** is one
  inside a bundle folder that is not, which `bundle` will not start
  (ADR 0225); **data** is in an app's data folder; **temp** is in its
  `/tmp/.app-ID`; and **-** is a file of no app. A listing with no
  file of an app has no column, as `--git`'s. `applications` and
  `app-data` count only in `/` and in a home, which is a folder that is
  its owner's home by the password file. Its title under `--header` is
  App.
  The app is named as its manifest's `name` gives it (ADR 0221), as
  "Notes", read from the sealed bundle in the same place as the file
  (a person's own, else everyone's), and only from a sealed one: anyone
  can write a manifest into a folder of their own, so an unsealed
  folder or one with no bundle installed is named by its ID. A
  manifest is read only up to 64 KiB, the name is cut to 24 columns,
  and a control character in it shows as `?`, as does one that turns
  the text's direction or takes no room, so that no manifest can write
  to the terminal, stretch the column or make its name read as another.
  Each bundle is read once a listing.
- **`--subvolumes[=WHEN]`** puts `[subvolume]` after each folder of the
  long listing that is the top of a Btrfs subvolume, and `[subvolume,
  read-only]` after one where nothing can be changed, as `/system`.
  `WHEN` is `always`, `never` or `auto`, the default, which is when
  standard output is a terminal: `ls -l /` at the console shows the
  marks, and a pipe gets GNU's text. A subvolume's top folder is inode
  256 of its own tree, which Linux shows as `st_ino` 256 and Radix as
  256 in the low 40 bits, and the folder must be on a Btrfs mount by
  the mount table, as other filesystems use 256 too. Read-only is what `access(2)` says to writing: `EROFS`, whether
  the subvolume or its mount is, and not a want of permission.

The field colours (ADR 0440) gain four keys: `app-sealed` (green),
`app-unsealed` (bold red), `app-data` (faint, for data and temp) and
`subvolume` (cyan); a `-` takes `none`. Nukesh's colors.toml `[ls]` has
them under the same names, and the themes change them: high-contrast
bright, colour-blind with blue and yellow for green and red.

Which subvolumes are snapshots is left out. It needs each subvolume's
parent UUID, which Linux gives with `BTRFS_IOC_GET_SUBVOL_INFO` and
Radix gives no program yet; Phase 15b's `snap` adds that query with the
snapshots it makes, and `ls` can mark them, with their retention, then.

## Consequences

- `ls -l --app` answers "whose is this file?" for apps without asking
  bundled: the answer is by path and by who owns what, so it works for
  a bundle bundled no longer lists, and needs no socket.
- A name or word is only as true as the files: a person may make a
  `/tmp/.app-ID` of their own, which then shows as that app's temp. The
  owner column still says whose it is.
- `ls -l` at a terminal shows the marks with no flag. It is the only
  change of this ADR to output not asked for, and it is only on a
  terminal.
- The lab's `commands/ls-apps` test installs a bundle as the lab
  account and checks each word, the column's title, the marks of
  `/users` and `/system`, and that a listing without the flags is GNU's;
  the `ls-apps` boot test does the same on a Dukovo root at the console,
  and unseals the bundle as an administrator to see it shown so.

## Alternatives considered

- **Asking bundled which bundle owns a file.** bundled knows what is
  installed, but `ls` would need its socket, which a sandboxed app has
  no reach to, and a listing would wait on a service; the paths say the
  same for an installed bundle.
- **Reading the name from any manifest.** A folder anyone can write is
  a name anyone can choose, such as another app's; only a sealed one is
  bundled's.
- **Marking snapshots now, with a kernel query of our own.** The query
  is ABI, kernel and Btrfs work, and `snap` defines what a snapshot of
  Dukovo is (its names, its retention), so it belongs with Phase 15b.
- **`--subvolumes` off unless given, as the other flags.** The marks
  cost a check of the inode and one read of the mount table, say what
  a listing of `/` and `/users` leaves out, and change nothing into a
  pipe.
