<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0451. An update is built into the spare slot and sealed before anything names it

- Status: Accepted
- Date: 2026-11-11

## Context

An installed Dukovo runs from `@system.a`, a read-only subvolume of its
Btrfs root that holds all of `/system` (ADR 0307). GRUB on the EFI
system partition loads the kernel and the startup archive from it. The
install stick carries a signed release: a list of every file of
`/system` with its hash, and the files, each kept once under its SHA-256
(ADR 0308). Phase 15 must move a running system to a newer release so
that a power cut at any instant leaves the old system or the new one,
never a mix (promise 2 of ADR 0450).

Two layouts were weighed:

- **Both slots' kernels and archives on the EFI system partition**, as
  the implementation plan said. Every update would then write files on
  FAT, which has no journal, and the boot files would change apart from
  the rest of the slot.
- **The boot files in the slot.** GRUB reads Btrfs, zstd and mirrors
  included, so a slot's kernel, archive and menu appear at the same
  instant as the rest of it.

Android's Virtual A/B writes an update into a snapshot rather than a
full second partition, and Fuchsia fetches only the content-addressed
blobs a device lacks. A Btrfs snapshot gives both savings without a
merge step.

## Decision

- **Two slots, `@system.a` and `@system.b`.** The system runs read-only
  from one. The other, the spare, holds the version before or a staged
  update. The kernel's command line says which slot runs (`slot=a`,
  ADR 0452), and Spark mounts that slot on `/system`.
- **Everything GRUB starts is in the slot**, in `/system/startup`: the
  kernel `radix`, the startup archive `archive`, the boot menu
  `menu.cfg`, and `slot.env`, which says the slot's version and any
  kernel flags its release always starts with. The EFI system partition
  keeps GRUB, a fixed `grub.cfg` that the install writes once, the slot
  state and, from the next pull request, Rescue (ADR 0459). An update
  writes only the slot state there.
- **The menu comes from the slot.** The fixed `grub.cfg` finds the root,
  reads the slot state and sources the default slot's
  `startup/menu.cfg`. A release can change its menu, and the change
  arrives and goes back with the slot. If the default slot's menu cannot
  be read, the other slot's is sourced and that slot starts as the
  default.
- **`sysupdate stage REPOSITORY` builds the spare slot aside:**
  1. It refuses unless the running slot is the default (an update
     staged earlier is replaced), the release is signed by the root the
     system trusts
     (`/system/resources/updates/root.json`), its version is strictly
     newer than the running one by semantic-version precedence, and the
     running version is at least the release's `updates_from`.
  2. It snapshots the running slot, writable, as `@system.new`, so that
     every unchanged file shares its blocks with the running slot.
  3. It removes what the release no longer lists and writes each file
     whose hash differs, from the repository's store. A repository needs
     only the files that changed since the versions it updates from.
  4. It reads every file back from the disk and checks its kind, size,
     SHA-256, link target and mode against the signed list, and that
     nothing else is there.
  5. It takes the spare slot out of the slot state if any copy names it
     (ADR 0452), removes it, and seals `@system.new` as a read-only
     snapshot in its place.
  6. It snapshots the settings and the state (ADR 0454).
  7. Last, it writes the slot state that names the spare as the update
     to try. Until that write, the running slot is the one that starts,
     whatever happens.
- **A slot left half built is removed.** A stage cut short leaves
  `@system.new`, which nothing names. The health check removes it at the
  next start (ADR 0453), and the next stage removes one it finds.
- **Versions compare by precedence.** Build metadata is ignored, and a
  build of the same version is not newer.
- **One stage at a time.** The stage and the health check take a lock in
  `/system/live`, which a restart frees.

## Consequences

- An update costs the disk only the files it changes, and a power cut at
  any step before the last write leaves the running system as it was.
  The boot test `update-power-cut` restarts the machine with
  `@system.new` built and not sealed, and checks that slot A starts, the
  half-built slot is removed, and a stage then succeeds.
- The version before stays in the spare slot until the next stage
  replaces it, so going back needs no download.
- A release whose menu is broken breaks only its own slot's starts,
  which count as failed tries.
- Installs made before 0.15.0 keep their old `grub.cfg`, which starts
  `@system.a` directly and cannot count tries. They are reinstalled to
  take updates; a migration is not worth its risk before the first
  public release.
- `sysupdate verify` reads the running slot back against its signed
  list, as a stage reads the slot it builds: from a repository folder or
  update file the owner names, or from Rescue's copy on the EFI system
  partition when it holds the same release, or from the update server.
  updated(8) runs it once a week and tells the consoles when the slot is
  not the release. Btrfs's scrub, which checks every block against its
  checksum, stays the owner's to run (`scrub`, scrub(8)).
- **Not yet:**
  - Fetching the changed files from an update server, and the offline
    update file (15c). `sysupdate stage` reads a repository folder, as
    `ob release` makes one, until then.
  - Checking each file against a signed root hash as it is read, with
    Secure Boot and measured boot (release 1.2).
