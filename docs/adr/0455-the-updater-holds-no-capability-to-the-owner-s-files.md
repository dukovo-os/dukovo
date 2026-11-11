<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0455. The updater holds no capability to the owner's files

- Status: Accepted
- Date: 2026-11-11

## Context

Promise 3 of ADR 0450 is that an update never touches the owner's files.
Windows 10's October 2018 Update deleted users' Documents folders and was
pulled within days; code review and tests had not caught it. Dukovo's
services run with only the capabilities their service files grant
(service.toml(5)), so what an updater cannot reach, no bug in it can
harm.

## Decision

- **What a system update may reach:**

  | Area | What an update may do | Guard |
  | --- | --- | --- |
  | `/system` | Replace it with the new release | Built aside and checked; the old slot kept (ADR 0451) |
  | `/system/settings` | Add new defaults beside the administrator's files; migrate formats under the two-version rule | Snapshotted first; put back after a fallback (ADR 0454) |
  | `/system/state` | Add log lines; migrate formats under the two-version rule | Snapshotted first; logs never rolled back |
  | `/applications`, `~/app-data` | Nothing: apps update through the catalog | No capability to them |
  | `/users`, `/shared`, `/volumes` | Nothing, ever | No capability to them |
  | The EFI system partitions | Write the slot state | Two copies (ADR 0452) |

- **The updater's capabilities** are the system subvolumes, `@settings`,
  its own folder in `@state`, the update snapshots, and the slot state,
  and nothing else.
- **Settings migrations run as programs of their own,** each holding
  only the files it migrates.
- **An update never deletes the owner's files, snapshots or Trash to
  make room.** When space is short, it says how much it needs and what
  the owner could free.

## Consequences

- A bug in the updater or a migration cannot delete a home folder,
  because nothing it holds reaches one.
- The upgrade-path tests of ADR 0450 compare homes full of files byte
  for byte before and after every update and rollback.
- **Not yet:** sysupdate and its health check run as services with the
  administrator's access (`runs-with-your-access`). Reaching the root's
  top level at `/volumes/<disk>` needs a capability to the Btrfs root
  that does not also reach `@users`, which the view of a subvolume set
  brings with the unified updater (15d). Until then, the boundary is
  kept by code review and the boot tests.
