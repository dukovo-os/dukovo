<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0460. Rescue is the release kept on the EFI partition, and starts after three unfinished starts

- Status: Accepted
- Date: 2026-11-11

## Context

ADR 0459 put Rescue, the install image's live system, on the EFI system
partition, for a machine whose two slots are both broken or whose root
will not mount. It left open what Rescue's files are, how a machine
nobody is at gets into it, what it starts, who may use it, and where a
reinstall takes the system from.

The install image's live system is the boot archive of a release with
the kernel: the stick's GRUB loads both, and Spark runs `/system` from
memory with no disk as the root (ADR 0308). A slot is the same release
laid out on the root, with a startup folder of the kernel and a small
startup archive (ADR 0307). Both are checked against the signed release
list.

The slot state counts an update's tries (ADR 0452), but nothing counts
the starts of the default slot: one that never finishes starting shows
the menu, starts the same slot again, and does so for ever. An installed
system whose root no disk holds stopped in Spark, with only Spark.

## Decision

- **Rescue is made of a release.** `boot/rescue` on the EFI system
  partition holds the release's startup folder (`startup/`), a boot
  archive of every other file of the release (`system`), and the
  repository's signed metadata and release list, without its store
  (`release/`). Every file is checked against the signed list as it is
  written, and the copy is read back before it is used. The installer
  writes it from the release it installs, on each disk that starts the
  system; `ob image` writes it into installed, lab and cloud images.
- **A new copy is switched in like a slot.** It is written to
  `boot/rescue.new`, read back, then the copy there is renamed
  `boot/rescue.old`, the new one `boot/rescue`, and the old one removed.
  A power cut leaves one whole copy, which the menu starts:
  `boot/rescue`, or between the renames `boot/rescue.old`, which the
  next write puts back first.
- **Rescue starts by itself after three starts in a row that were not
  seen through, or when no slot can be read.** A count of its own,
  `boot/grub/rescue.env` beside the slot state, gains one at each start
  whose last start the health check did not clear, but not for an
  update's own tries, which their count already ends. The health check
  sets it back to 0 whenever a start is seen through, and so do
  Rescue's reinstall and its *Start Dukovo normally*. It is apart from
  the slot state, so that a write of it cut short can start Rescue or
  not, but never another slot. The menu offers Rescue always, and makes
  it the entry that starts by itself, after showing for 10 seconds,
  when the count reaches 3 or no slot's menu can be read.
- **An installed system whose root no disk holds restarts after 30
  seconds,** so that its unfinished starts reach Rescue, where it
  stopped before.
- **Rescue is the live system with `rescue=<uuid>`,** the root the menu
  names. Spark starts only the log, the firewall, notices, the screen
  reader and the consoles; networking starts when the owner turns it on.
  Every console, and the serial console where the system keeps one, runs
  `dukovo-rescue` in place of a sign-in, again whenever it ends. Nothing
  listens on the network.
- **It asks for an administrator when the root's accounts can be read:**
  a member of `admins`, checked against the root's own password hashes.
  With accounts that cannot be read, or no administrator, it asks
  nothing, since there is nothing left to protect but what the disk
  shows anyone who takes it.
- **The repairs keep data.** *Reinstall* builds the release into the
  default slot from Rescue's own copy, or from an update folder signed
  by the root Rescue trusts: from a writable snapshot of the slot so
  that only what differs is written, afresh if that fails, read back
  against the list, and sealed. The slot state then names it the
  default, with no previous version and no update. `@settings`,
  `@state`, `@users`, `@shared` and `@applications` are not written.
  *Put back the settings* runs the fallback's steps (ADR 0454) on the
  last update's snapshot. *Check the disk* runs `fsck-btrfs` read-only.
  *Copy files off* copies `@users` and `@shared` to a folder on another
  disk. A shell, a restart and power off are there too.
- **Rescue reinstalls its own version,** the release it was written
  from. Kept at most one stable release behind the slots (ADR 0459), it
  reads their settings by the two-version rule (ADR 0454), and the
  system then updates as usual.

## Consequences

- The installer writes about as much again as the system to each EFI
  system partition: about 230 MiB for a debug build, which the guided
  layout's 512 MiB holds with room for GRUB. A layout made by hand with
  a smaller partition installs without Rescue, and the install log says
  so.
- Three power cuts in a row while the system starts bring Rescue up; its
  *Start Dukovo normally* goes back with one choice.
- The boot test `rescue` installs from the stick, has an administrator
  keep a file and an app, cuts the power at three starts in a row, signs
  in to Rescue, removes the only slot from Rescue's shell so that no slot
  is left, has Rescue start by itself again, reinstalls, and finds the
  file and the app in the system that then starts.
- **Not yet:** writing a newer Rescue when an update proves itself, and
  reinstalling from the update server, come in Phase 16 with the update
  server on Dukovo; USB repair, which also rewrites GRUB and the EFI
  system partition, and the installer's reinstall that keeps data come
  with the installer's next changes; resetting the settings to their
  defaults waits for a need.
