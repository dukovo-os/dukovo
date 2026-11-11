<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0464. A whole machine is restored at a start, from its backups and its record

- Status: Accepted
- Date: 2026-11-11

## Context

Backups (ADR 0463) keep each place's snapshots as encrypted send
streams, and `backup restore` brings one back as a read-only snapshot,
from which `snap restore --from` copies files. That serves a person who
lost a file. A person who lost the machine, its disk dead or the
machine stolen, needs more: a new machine, or a fresh install, that is
the old one again. That means its accounts and settings, everyone's
files, the apps installed for everyone and for each person, and what
each person allowed each app (the grants, ADR 0224), so that nothing
asks again and nothing gets more than it was given.

The roadmap's 15b says that a backup carries the app list and the
grants, and that a restore on a fresh install brings the apps back
with them.

Two more things matter once a backup can rebuild a whole machine:

- age encrypts a file to a public key, and anyone who has the public
  key can write a file that decrypts. A destination's `backup.toml`
  names the key's public half. Whoever runs the server, or finds the
  stick, could put a backup of their own there. Restored as a whole
  machine, its settings would be theirs: its accounts, its passwords,
  its SSH keys.
- The places are subvolumes the running system has mounted and in
  use. Replacing `/system/settings` or `/users` under running services
  is not something to do to a live system.

## Decision

- **The apps are in the places.** Apps installed for one person are in
  their home folder's `applications` folder, in `/users`. Apps
  installed for everyone are in `/applications`, which every
  destination now backs up besides snap's places, unless its settings
  name its places. A restored app is the same version, sealed as it
  was, with or without a network, and an app installed from a file
  comes back as well as one from a catalog.
- **Each round writes the machine's record.** Beside the places'
  backups, in the folder `machine`, each round writes one record,
  `machine/<time>.age`, encrypted to the destination's keys: the
  machine's name, each place by its folder at the destination and its
  path on the machine, and the files of `/system/state` that say which
  apps are installed and what each person allowed them, bundled(8)'s
  registry and the grants, as they are. A record carries nothing else
  of the state, and a record that names any other file is refused. A
  record stays while a backup from its time or after it can be
  restored, with the newest one from before.
- **Every file is tagged.** Each backup and record has a tag beside
  it, `<time>.tag`: an HMAC-SHA256 of its path from the machine's
  folder and its bytes, keyed with HKDF-SHA256 from the destination's
  own age key, the recovery key. Only the machine and whoever holds the
  recovery key can make one, and a file moved to another place, chain
  or time no longer matches its own. `backup verify`, `backup restore`
  and the scheduled checks read each file's tag with its bytes and
  refuse a file whose tag is missing or does not match. The tag is
  read at the end of the file, so a stream is received before it is
  known to be good; one that is not is removed. A restore with another
  of the destination's keys, which cannot make the tag key, needs
  `--unchecked`, and says so.
- **`backup restore --machine` stages; a start restores.** With a
  destination's name, or a machine's folder and the recovery key,
  `backup restore --machine` reads and checks the newest record (or
  the newest from no later than `--at`), then receives each of the
  root's places it names, `/system/settings`, `/applications`,
  `/users` and `/shared`, as of that record's time, into
  `@snapshots/machine-restore/staged`, with the state's files the
  record carries. It writes `about`, what it staged, last. A place
  that is not on the root, such as a disk's in `/volumes`, is restored
  as `restored-<time>` in its own `@snapshots` folder, as `backup
  restore` restores it, if the machine has it. Nothing in use changes
  until the next start.
- **Spark puts it in place before any service starts.** At the next
  start, after the settings an update may put back (ADR 0454) and
  before it mounts the root's places, Spark puts each staged subvolume
  in the place of the one there, which it keeps, read-only, in
  `@snapshots/machine-restore/before-<time>`, shown in `/snapshots`.
  Then it empties the grants and writes the registry and the grants
  the record carried into `@state`, each root's with its mode, and
  marks the restore done. Btrfs cannot rename a subvolume, so each
  step is a snapshot or a removal, decided afresh from what is there,
  as the settings after an update are: a power cut between any two
  leaves the next start to carry on, and the steps end the same. Then
  bundled starts with the registry, the apps and the grants of the
  machine that was backed up.
- **The rest of the state stays the new machine's.** `/system/state`
  holds the slot state, the logs, the services' own state and each
  destination's last backup; none of it is restored. The next backup
  of a restored machine starts new chains, in the same folder, since
  its settings name the same destinations and it has the same name.
  The network settings name cards by name or by MAC address, and a
  card no section names gets an address by DHCP (network(5)), so a
  machine restored on other hardware comes up on the network.

## Consequences

- A machine is rebuilt from a stick or a server with one command and a
  restart, after a fresh install. It comes back with its name, its
  accounts and passwords, its SSH host keys, its files, its apps and
  their grants.
- A backup cannot be forged by whoever holds the destination: a
  record, a stream or a tag of theirs is refused before anything of it
  is used. Linux's tools still restore a backup (ADR 0463), but do not
  check the tag.
- Backups grow once by the size of the apps installed for everyone,
  then only by their changes.
- Restoring needs room on the root for everything restored, while the
  subvolumes it replaces are kept. Those are removed with `subvolume
  delete` once they are not wanted.
- A whole-machine restore replaces everything in the places since the
  backup, on the same machine as on a new one; what it replaced is kept
  until removed.

## Alternatives

- **Reinstalling the apps from their catalogs:** smaller backups, but
  each restore would need the network, would bring the newest version
  rather than the one that was running, and could not bring back an
  app installed from a file.
- **Copying the files into the running places:** no restart, but the
  services and the session that runs the restore would see their
  settings and files change under them, and a power cut half way would
  leave a machine that is neither.
- **Restoring from the installer:** the right place for a person with
  no Dukovo at all, and a later step; the installer can stage the same
  restore and leave the same start to finish it.
- **Signing each file with an Ed25519 key:** the same protection, but
  the person restoring would need the public key from somewhere other
  than the destination. A key derived from the recovery key needs
  nothing more than what they already keep.
- **Backing up `/system/state` whole:** it would bring the old
  machine's slot state and logs to a new disk, which they do not
  describe.
