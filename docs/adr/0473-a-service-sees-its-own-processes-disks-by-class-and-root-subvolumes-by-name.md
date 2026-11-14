<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0473. A service sees its own processes, disks by class and the root's subvolumes by name

- Status: Accepted
- Date: 2026-11-13

## Context

A service's root is a view of what its file declares (ADR 0217), but
three things every view held or lacked were coarser than the services
need:

- Every service read the whole of `/processes`, so a service learnt
  what every other one ran, with what arguments, and when.
- A service that manages disks, volumed or diskhealth, had either the
  whole of `/devices` (the keyboard, the framebuffer, the kernel log) or
  no disk at all, and disks come and go after it starts, so naming them
  one by one could not work.
- The updater and the snapshot service need the top of the root's Btrfs,
  where the system's subvolumes are, but `@users` and `@shared` are
  beside them there. Holding the top meant holding the owner's files, so
  sysupdate and snap ran with the whole tree (`runs-with-your-access`),
  and ADR 0455's promise that the updater holds nothing that reaches a
  home folder was kept by code review alone. The updater also read
  partitions raw to find which one held the root.

## Decision

1. **A service sees its own processes.** Spark attaches at `/processes`
   a list of the processes of the service's resource domain and the
   domains beneath it, from the kernel's new `processes` method on a
   domain (radix-resource-domain(2)). The machine-wide files (`stat`,
   `meminfo`, `uptime`, `mounts`, `net/*`) are there as before. Another
   process's number is not found, by lookup or listing, and a number
   reused by a process outside the domain reads as not found. A file
   that names `/processes` with `files.read` gets the whole list; a
   service with `runs-with-your-access` has it too. Sessions keep the
   whole list, as on Linux and every desktop, with ADR 0214 keeping
   another user's details to that user. Apps run in no domain of their
   own yet, so theirs is unchanged until the catalog's sandbox gives
   them domains.
2. **Disks by class.** `files.disks = "read"` or `"read-write"` gives a
   service a `/devices` with every disk and partition, as they come and
   go, the `disk` link, and the four devices every program has (`null`,
   `zero`, `random`, `urandom`), and nothing else. `files.efi-system`
   gives the EFI system partitions alone, by partition type (MBR type
   `0xef` or GPT's `C12A7328-F81F-11D2-BA4B-00A0C93EC93B`). The kernel
   makes each class as a filesystem (`dev_disks`, `dev_efi_system`),
   which Spark mounts once, at first need, in
   `/system/live/spark/devices/`, and attaches at `devices`, read-only
   for `"read"`. `/devices/null` and `/devices/zero` take writes on a
   read-only place, since what is written to them goes nowhere. A file
   naming either key may not also name a place in `/devices`, nor both
   keys at once.
3. **The root's top level by name.** `files.root-subvolumes = [names]`
   attaches the top of the Btrfs the system started from at
   `/volumes/<disk>`, where Spark mounted it, with only those names in
   it. The kernel's new `attach_names` (radix-directory(2)) wraps the
   directory so that any other name is not found by lookup or removal,
   left out of a listing, and not permitted to be made or renamed into;
   `..` from beneath a name comes back to the wrapper, and a capability
   opened on it is the wrapper too. The wrapped directory is never a
   snapshot's source, which would be the whole top level. The names are
   plain, at most 16, and `@users`, `@shared` and `@applications` are
   refused when the file is read: an update or a snapshot never needs
   the owner's files.
4. **Spark records the root it started.** Once Dukovo's root on a disk
   is mounted, Spark writes `/system/live/spark/root` with the disk it
   is mounted from, its Btrfs UUID and every partition the Btrfs is on.
   The updater reads it in place of probing partitions raw, and Spark
   reads it to attach the root's top. A system with no root on a disk,
   the live system or Rescue, has no record and no top to attach.
5. **`service status` says what a service holds** of these: its devices
   class and how, the root's subvolumes, and whether it sees every
   process.

The service files follow: volumed holds `files.disks = "read-write"`,
diskhealth `files.disks = "read"`, sysupdate's health check the root's
two slots, the staging place and the firmware store and the EFI system
partitions, and snap `@snapshots` with read access to the places it
snapshots. Staging an update runs as a service of its own,
`sysupdate-stage`, which updated and `update apply` start: it holds the
same, and `@settings`, `@state` and `@snapshots` besides, for the
snapshots a fallback needs. The stage's `@state` reaches
`/system/state/spark/secrets`, which no service is given by path. That
is accepted: the stage writes the system slots, which can do more than
read a service's secrets. The services that still hold the
whole tree (authd, backup, bundled, cloud-setup, consoled, firewall,
guest-agent, serial-console, and updated for the apps it updates) have
one row in `docs/deferred.md`, to be narrowed one by one.

## Consequences

- ADR 0455's promise is kept by the kernel's walk: the updater holds no
  name that reaches a home folder, and the boot tests try to reach one
  through its view and fail.
- A disk plugged in after volumed started appears in its `/devices`,
  and nothing but disks and the four devices ever does.
- A service that read other services' processes to work must name
  `/processes` in its file; `service status` says which do.
- The kernel gains one VFS node type that wraps a directory, written
  method by method so that a method added to the node trait later does
  not pass through unchecked; the ABI gains one directory method, one
  domain method and two filesystem kinds.
- What Dukovo does better: systemd's `ProtectSystem`, `ReadWritePaths`
  and `PrivateDevices` hide paths, but a service with `CAP_SYS_ADMIN` or
  raw disk access still reaches everything, and `/proc` shows every
  process unless `ProtectProc=invisible` and a separate user are set.
  Dukovo's updater cannot name a home folder, its health check reads
  only the EFI system partitions raw and no other disk, every service sees only its own processes by default, and
  `service status` says what each one holds.
