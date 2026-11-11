<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0459. Rescue on the EFI system partition replaces the Recovery entry

- Status: Accepted
- Date: 2026-11-11

## Context

The implementation plan gave 15a a Recovery entry that starts the
previous slot. It fails exactly when it is needed most: when both slots
are broken, or the Btrfs root will not mount. Promise 10 of ADR 0450 is
that such a machine can be repaired without losing data.

The install stick already runs a live system from memory, the boot
archive with no root, which mounts none of the machine's disks unasked
(ADR 0308). After the CrowdStrike outage of July 2024, Windows added
Quick Machine Recovery, which connects from its recovery environment to
find a fix.

## Decision

- **Five layers stand between a broken machine and lost data,** each
  working without the ones above it:

  | Layer | When it acts | What it can do | Where it lives |
  | --- | --- | --- | --- |
  | 1. Automatic fallback | A new version fails to prove itself | Starts the version before and puts its settings back (ADR 0453) | The slot state and the previous slot |
  | 2. The previous version | The owner finds a problem days later | Starts it once from the boot menu, or makes it the default with `sysupdate rollback` | The previous slot |
  | 3. Rescue | Both slots are broken, or the root will not mount | Checks and repairs disks, restores snapshots, reinstalls the system from the network or an update file keeping settings, apps and data, resets settings to defaults, and copies data off | The EFI system partition |
  | 4. USB repair | The boot area or Rescue itself is broken | All of Rescue, plus rewriting GRUB and the EFI system partition | Any install stick |
  | 5. Reinstall keeping data | All of the above failed | The installer's reinstall that keeps users, shared data and apps | Any install stick |

- **Rescue is the install image's live system,** the same build, kept on
  the EFI system partition with its own menu entry, so it adds nothing
  new to maintain. It replaces the Recovery entry.
- **It is updated at most once per stable release,** after its own boot
  test, written beside the old copy and switched like a slot.
- **It is careful by default:** networking stays off until the owner
  turns it on; it runs only at the machine's own console; and it asks
  for an administrator's password when the system's settings can be
  read.
- **Data is never the price of a repair.** No layer formats `@users`,
  `@shared` or `@applications` unless the owner types the disk's name,
  as the installer already requires to erase a disk.

## Consequences

- The fixed `grub.cfg` on the EFI system partition already sources a
  `rescue.cfg` beside it when there is one (ADR 0452), so adding Rescue
  changes no menu in a slot.
- Rescue must fit the 512 MiB EFI system partition with one copy. If two
  copies are needed for the switch, the guided layout's partition grows
  to 1 GiB for new installs.
- **Not yet:** Rescue itself, its menu entry and its boot test, in the
  next pull request; USB repair and the reinstall that keeps data come
  with it or after it.
