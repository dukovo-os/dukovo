<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0463. Backups are age-encrypted send streams, kept as files on a disk or over SSH

- Status: Accepted
- Date: 2026-11-11

## Context

The roadmap's 15b backs snapshots up incrementally to a USB disk or to
another machine over SSH, on a schedule, encrypted with rage, with
pre- and post-backup hooks for services and scheduled verification by
test restores. Snapshots are read-only and kept in `@snapshots` (ADR
0461); `subvolume send` writes one as Linux's send stream, whole or as
its changes from a parent, and `subvolume receive` makes it again (ADR
0462). The places snap keeps, `/users`, `/shared` and
`/system/settings` by default, are what a person would lose with the
disk.

A backup has to outlive the machine. The disk it goes to may be any
USB stick a person owns, formatted FAT or exFAT as they come, or a
server the person does not run Dukovo on. Whoever finds the stick, or
runs the server, must not read it. And the person who lost the machine
must be able to get their files back with nothing but the backup and
its key.

age is a small file encryption format with one audited design: a
header that wraps a random file key for each recipient, then the data
in authenticated chunks of 64 KiB, so a changed or cut file is refused
on reading. rage is its Rust implementation, from the same project as
the format's specification; Go's `age` reads the same files.

## Decision

- **A backup is a chain of encrypted send streams, kept as files.**
  For each place, the first backup is the snapshot's whole send
  stream; each one after is the stream of its changes from the one
  before, as `subvolume send -p` writes it. Each stream is encrypted
  with age and written as a file. The destination holds nothing but
  files, so it can be any filesystem Dukovo writes (FAT, exFAT, ext4,
  NTFS, Btrfs) or any server with SSH and a POSIX shell, and its owner
  sees only names, sizes and times.
- **The layout is plain.** At the destination, a folder
  `dukovo-backup/<machine>` holds `backup.toml` (the format, 1; the
  machine's ID; the backup key's public half), then a folder for each
  place (`users`, `shared`, `settings`), a folder for each
  chain, named by the time of its whole send, and in it one file for
  each backup, named by its time: `users/2026-11-11-030000/
  2026-11-12-030000.age`. A file over 1 GiB is cut into parts
  (`.age.000`, `.age.001`), since FAT holds no file of 4 GiB. A file
  is written as `.partial` and renamed when whole, so a cut-off backup
  is never taken for one; the next run removes the partial file.
- **Any Linux can restore one.** The parts in order, decrypted, are a
  send stream: `cat *.age* | age -d -i key | btrfs receive folder`
  makes each backup again, chain member by chain member, with Linux's
  own tools.
- **Each destination has its own key.** `backup add` makes an X25519
  age identity for the destination, keeps it in
  `/system/settings/backup/<name>.key`, root's and readable by root
  alone, and shows it once as the recovery key, which the person keeps
  away from the machine. `backup key` shows it again to an
  administrator. More recipients can be added, such as a key the
  person keeps elsewhere, and every file is encrypted to all of them.
  The machine keeps the identity, not only the public half, so that
  its scheduled checks can decrypt what it wrote. After release 1.2 the
  identity can live in the system key store instead.
- **One snapshot a round serves every destination.** A round of
  backups makes a read-only snapshot of each place, named
  `backup-<time>` in the place's `@snapshots` folder. A name that is
  not a time keeps it out of snap's retention (ADR 0461). Each
  destination due then gets the stream of that snapshot's changes from
  the one it last took, which its state in `/system/state/backup`
  names. A backup snapshot no destination needs as a parent any more
  is removed.
- **Chains are bounded.** A new chain starts with a whole send when
  there is none, when the parent snapshot is gone, or when the chain
  holds `chain` backups (30 by default). The destination keeps the
  newest `chains` chains (2 by default) and removes an older one only
  once a newer chain is whole, so a backup is never left with only a
  half-written chain. Restoring the newest backup reads at most one
  chain.
- **Hooks run around the snapshot, not the copy.** A service file may
  name a `before` and an `after` command in a `[backup]` table
  (service.toml(5)). Spark runs them on the control socket's `hook`
  request, as the service runs (its account, its view, its domain),
  with a time limit, and answers when the command ends. A round runs
  every running service's `before` hook, makes the snapshots, then
  runs the `after` hooks, so a database waits only for the snapshots,
  which take a moment, not for the copy. A hook that fails or runs out
  of time is reported, and the backup goes on: the snapshot is as
  consistent as after a power cut, which a database recovers from.
- **Verification restores.** `backup verify` decrypts every file at
  the destination and reads every stream through, checking the chain's
  links (each stream's parent is the backup before it) and the
  stream's own checksums. With `--restore`, as the service does every
  month by default (`verify`), it also receives the newest backup of
  each place into a scratch folder and compares it, file by file, with
  the local snapshot it was made from: names, kinds, owners, modes,
  times, extended attributes and data. A failed check is in `backup
  list` and the log.
- **SSH goes through OpenSSH's client.** A destination
  `ssh://user@host[:port]/path` is reached with the system's `ssh`,
  with a key the machine makes for backups and the server's host key,
  which `backup add` records on first contact and shows. The server
  runs only `mkdir`, `cat`, `mv`, `rm` and `find`, with names the
  backup makes, so any POSIX server does.
- **The schedule is the backup service.** `backup auto --watch` runs
  every hour and backs up each destination that is due (`every`:
  `day` by default, or `week`, or `hand`), skipping a disk that is
  not plugged in. A round holds a lock, so a run by hand and the
  service never overlap.
- **Restoring is receiving.** `backup restore` receives a place's
  chain up to the backup asked for (the newest by default) into the
  place's `@snapshots` folder as `restored-<time>`, where `snap restore
  --from` copies files back as the person who asks. With a key file
  and the destination's folder or address, it restores on any Dukovo
  machine, without the settings of the one that made it.

## Consequences

- A backup to a stick works with the stick as it came, and a backup
  over SSH works with any Linux, BSD or macOS server, without Dukovo
  on it.
- The streams are incremental, so after the first backup each one is
  the size of the changes. They are not deduplicated across places or
  machines; the restic destinations that follow in 15b are, for those
  who want that.
- Losing the recovery key and the machine together loses the backups.
  `backup add` says so when it shows the key.
- The destination's owner sees the machine's name, the places, the
  backup times and their sizes.
- Hooks that run longer than their limit are stopped; a database that
  needs longer to settle has to say so in its file.

## Alternatives

- **Receiving into Btrfs at the destination**, as `btrfs send | ssh
  host btrfs receive` does: browsable copies, but the stick must be
  Btrfs, the server must have Btrfs and a receiver run as root, and
  the copies are as readable as the disk. Encryption would then be the
  disk's, which a server does not give.
- **The restic format first:** deduplicated and encrypted, but it reads
  every file to find what changed, where a send stream comes from the
  kernel's comparison of two snapshots. It comes later in 15b, as another
  kind of destination.
- **Our own encryption of the stream:** fewer dependencies, but a new
  format to get right and to have checked; age's is specified,
  audited, and read by tools on every system.
- **A passphrase instead of a key:** age's scrypt recipient cannot be
  combined with others, and a scheduled backup would have to keep the
  passphrase on the machine anyway.
- **Hooks run by the backup, as root:** a bundle's service file would
  then run commands as root. Spark runs them as the service runs.
