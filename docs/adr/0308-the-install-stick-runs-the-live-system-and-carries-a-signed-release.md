<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0308. The install stick runs the live system and carries a signed release

- Status: Accepted
- Date: 2026-11-06

## Context

Phase 14a, the second step of its third part. The plan's stick has an
EFI system partition with the kernel and the boot archive, and a
DUKOVO_LIVE partition with "the read-only live system and the signed
system image the installer writes to disk"; its menu starts the
installer, the live console, the installer on a serial console or with
speech, or the first disk. The installer must not trust the stick: a
stick can be damaged, written wrongly or changed by someone else, and
what it installs is a whole system that will run as root.

Until now the stick carried a Dukovo root, the third partition, which
Spark took as the system's root like any other disk's. Booted beside a
machine's own Dukovo disk, a stick's Spark could take either, and the
live system wrote to whichever it took.

The updates design (approved 2026-11-05) describes a release as a list
of every file of `/system` with each file's SHA-256, `system.json`, a
target of The Update Framework (its section 4), and the files fetched
by their SHA-256. It says expired metadata is accepted only for a file
the owner named. The catalog already has a TUF client of our own
(ADR 0380).

## Decision

- **A release is the updates design's release list and its files.**
  `system.json`, format 1, names the version, the commit and toolchain
  it was built from, when it was built, the oldest version that updates
  to it directly, and every directory, file and link of `/system` with
  its mode, and for a file its size and SHA-256. The files are kept
  once each under their SHA-256. The repository is laid out as the
  update server will serve it: TUF metadata with consistent snapshots in
  `metadata/`, the list as the target `system/<version>.json` in
  `targets/`, and the files in `files/`. Phase 15's updates read the
  same format; the stick is the first release that is not online.
- **`system-release`**, a new crate, makes and checks releases:
  `system-release keys` makes Ed25519 keys for the four roles and the
  first root, `make` lists a staged slot, signs it and reads it back,
  and `check` reads a release from the root the system trusts, every
  signature, version and hash, then every file's size and SHA-256,
  naming each file that does not match. It is one of the native tools
  (systools), so the live system and the installer have it, and `ob`
  builds it for the build machine.
- **Offline checks skip expiry and nothing else.** The TUF client gains
  `Updater::offline`: signatures, thresholds, versions, rollback and
  hashes are checked as online, and expiry is not. Expiry says that a
  server still vouches for the metadata, which a stick cannot know; the
  owner who boots a stick or runs `system-release check` on one has
  named that release, as the updates design requires. Online refreshes
  are unchanged and still refuse expired metadata.
- **Each build machine signs with keys of its own** until the project's
  YubiKeys sign releases (the updates design's section 11). `ob` makes
  them the first time it needs them in `.cache/release-keys`, and
  carries their root in the boot archive at
  `/system/resources/updates/root.json`, the root `system-release check`
  trusts by default. A stick checks against the root built into it;
  nothing online trusts these keys.
- **The stick is three partitions**: the BIOS boot partition, the EFI
  system partition with GRUB, the kernel and the boot archive, and
  DUKOVO_LIVE, a Btrfs labelled `DUKOVO_LIVE` whose partition type is a
  new GUID, 5894C662-AF90-4B47-BAAF-2E3396443B25, "Dukovo live", whose
  read-only subvolume `release` holds the signed release. The stick no
  longer carries a Dukovo root. `rdisk` knows the type as `dukovo-live`.
- **The live system** is the boot archive, run from memory: the menu
  entry *Try Dukovo (live console)* adds `live` and
  `console-sign-in=off` to the kernel's command line. With `live`, a
  flag for Spark, not the kernel, Spark takes no disk as the root,
  mounts the boot archive on `/system` and tmpfs over settings, state
  and `/system/live` as with no root, and mounts only the stick's
  DUKOVO_LIVE, read-only. The machine's own disks stay unmounted, for
  the installer or a rescue shell; `mount` mounts one by hand.
- **A Dukovo live partition is always read-only and never a root.**
  Spark and volumed mount it read-only whatever the command line says,
  without compression, and Spark never takes it as Dukovo's root.
- **_Boot from first disk_** starts the machine's own system as if the
  stick were not there. Under UEFI GRUB chain-loads
  `EFI/BOOT/BOOTX64.EFI` from the first of the next disk's first four
  GPT partitions that has one; under a BIOS it swaps the first two BIOS
  disks with `drivemap` and chain-loads the next disk's MBR. GRUB gains
  the `chain` and `drivemap` modules for it. GRUB 2.12's `exit` cannot
  hand a UEFI back to its boot order: it returns success, and OVMF then
  shows its own menu.
- **The installer's entries come with the installer**: *Install
  Dukovo* and *Install with serial console* with dukovo-install, the
  next part of this PR, which installs from the stick's release and
  checks every file as it writes it; *Install with speech* with the
  screen reader.
- **The ISO runs the live system and carries no release** yet: it has
  no partition for one.
- **The boot tests `live-uefi` and `live-bios`** boot the stick from USB
  with a Dukovo disk beside it under each firmware: Spark must take no
  root and mount nothing but DUKOVO_LIVE, read-only;
  `system-release check` must pass on the stick's release, all of it;
  and nothing may write to it. The other boot tests keep their own menu,
  `image/grub/grub.cfg.in`, and the Dukovo root some of them carry.

## Consequences

- The release is 2104 files of 207 MiB, and the stick carries the
  system twice, as the boot archive the live system runs and as the
  release the installer installs: Btrfs's zstd compresses the second.
  The live system and the installer can then be updated without the
  release and the other way round, and the installer never copies the
  system from the memory it is running in.
- A damaged or changed stick is refused, file by file, before anything
  is written to a disk, and `system-release check` tells its owner to
  write it again.
- A stick a year old still installs: its metadata has expired, which an
  offline check does not ask. The installed system then updates online,
  where expiry is checked.
- A stick built on one machine verifies only against the root built
  into it, so a developer's stick installs systems that trust only that
  developer's keys, until the YubiKeys' root replaces it.
- No boot test chooses *Boot from first disk* yet: the install tests of
  the next part, which boot the stick beside the disk they install, will.
- Booting a stick beside a Dukovo disk no longer risks starting the
  disk's system with the stick's Spark, or writing to the disk.
