<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0468. Firmware sets ride in the slots, and a new set is tried as an update

- Status: Accepted
- Date: 2026-11-12

## Context

Closed firmware stays out of `/system` (ADR 0007): CPU microcode and
the firmware some cards load come in an optional package. Until now the
only firmware was microcode that `ob image --microcode` put on the EFI
system partition (ADR 0014), the same for every start, and changed only
by writing a new image.

Firmware breaks machines as updates do. A microcode update the
processor refuses, or a card whose firmware no longer loads, leaves a
system that starts but lacks something it had. The A/B slots (ADRs 0451
to 0454) already undo a version that does not prove itself; firmware
should be undone the same way, and a system update should never change
the firmware a machine runs by the way.

## Decision

1. **Sets.** The firmware package is published as firmware sets: a
   signed TUF target `firmware/<version>.json` beside the releases, in
   the same update repository (ADR 0466), listing each file with its
   SHA-256, size, how it is loaded and its license. A set's version is a
   date, such as 2026.11.12, compared as a version. `system-release
   firmware` adds one, and `system-release channel --firmware` names
   the set a channel offers. For now a set holds CPU microcode only, at
   most one file of it.
2. **The store.** On the machine, sets live in the root's top-level
   subvolume `@firmware`, outside every slot: each file once, by its
   SHA-256, read-only, and each set's signed list. A file is checked
   against its hash before it takes its name. Sets no slot names are
   removed after each stage.
3. **Each slot names its set.** A slot's `startup/firmware.env`, a GRUB
   environment block, names its set and the hash of the set's
   microcode. GRUB loads that microcode from the store in place of any
   on the EFI system partition. A system stage carries the running
   slot's naming into the new slot, so a system update keeps the
   firmware, and a Rescue rebuild names none.
4. **A new set is an update.** `sysupdate firmware stage` snapshots the
   running slot into the spare, makes it name the new set, and stages it
   as any update is staged: its first start is a try, with three tries,
   and it falls back to the slot before it, with the set before it, if
   it does not prove itself. `sysupdate firmware remove` stages a slot
   that names none. GRUB's titles say "firmware update" for a slot of
   the same version that names another set.
5. **The kernel says what became of the microcode.** The kernel reports
   it in `/kernel/firmware/microcode`: the module's size, how many
   updates it holds, and the result: applied, current, unmatched,
   unsupported, hypervisor, damaged or refused, with the revision. A
   module with damaged data is not used at all.
6. **Per-device checks.** The health check gains two checks that are
   required during a try: the microcode of the set the slot names
   reached the kernel and was neither damaged nor refused, and every
   device the last good start had is still there. The devices are the
   network cards the kernel drives itself, by where they are and their
   MAC address, and the partitions the root is on, kept after each start
   that passes in `/system/state/sysupdate/devices`. At a start that is
   no try, a missing device is only logged: a machine that lost a card
   still has to start.

## Consequences

- A bad firmware set costs a restart, not a machine: its try falls back
  by itself, as a bad version does.
- The firmware a machine runs changes only when its owner stages a set,
  never with a system update.
- The installer does not yet install the install stick's set, update
  files do not yet carry firmware, cards that load firmware through a
  driver wait for a driver that does, and AMD microcode waits for AMD's
  loader: each has a row in `docs/deferred.md`. Disks other than the
  root's are left out of the device record, as their owners plug them in
  and out.
- The `firmware-fallback` boot test stages a good set, then a newer set
  whose try misses a card and a set whose microcode is damaged, and sees
  both fall back.
