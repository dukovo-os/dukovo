<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0503. Spark marks how far a start got, and the install stick repairs startup

- Status: Accepted
- Date: 2026-11-12

## Context

The record of a start (ADR 0500) begins when the health check runs, so
Rescue (ADR 0501) could say of the starts that stopped before it only
that they stopped "in the kernel, mounting the root or starting the
services", which leaves the owner to guess which Troubleshooting start
to try. And when the EFI system partition loses GRUB or its menu, or the
firmware loses its boot entry, as a firmware update or another system's
installer can leave them, no Dukovo menu shows at all, so neither Rescue
nor Troubleshooting can be reached; reinstalling was the only way back.

## Decision

1. **Spark marks the start that reached its services.** Once the root
   and the state are mounted, and before any service starts, Spark
   writes `reached` among the start records. The health check removes
   it as it begins the start's record. While it is there, a start after
   the newest record got as far as its services but never to its health
   check. Rescue then says that at least one of the starts not seen
   through reached its services, so that a service may be to blame and
   the essentials-only start is the one to try; without it, that none
   did, so that they stopped in the kernel or mounting the root, and
   the start with all of the kernel's messages is the one to try.
   Rescue's own start marks nothing.

2. **Repair startup on the install stick.** The stick's menu gains
   *Repair startup of an installed Dukovo*, on the hotkey R, which runs
   `dukovo-install --repair-startup`. For each Dukovo root it finds on
   the computer's disks, by the root's partition type, it:
   - puts GRUB back on the disk for UEFI, and for a BIOS where the disk
     has a BIOS boot partition, from the images in the slot that starts;
   - writes the start menu again from that slot's template, for the
     root's UUID, with Rescue's entry, GRUB's environment and the serial
     console the old menu had;
   - keeps the slot state when a whole copy names a default slot that is
     there, and otherwise makes the slot that is there the default;
   - sets the count of starts not seen through back to none;
   - makes Dukovo's firmware boot entry first in the boot order again
     where the firmware has one.

   Nothing in the root changes: it is mounted read-only. When it finds
   no Dukovo it says that a disk controller in RAID mode hides the
   disks from every system but Windows, and that the firmware's SATA
   mode should be AHCI. It ends by restarting, once the stick is
   pulled.

## Consequences

- Rescue now names the likely Troubleshooting start for starts that
  never reached their health check.
- A machine whose menu or GRUB is gone starts again from the stick in a
  minute, with its slots, settings and files as they were.
- Not yet: re-making an EFI system partition whose FAT cannot be read,
  with Rescue's files from the stick; reading the firmware's Secure Boot
  and SATA mode settings; a QEMU boot test, which needs a third boot from
  the stick after the install; and marks before Spark, in the kernel or
  mounting the root. docs/deferred.md lists each.
