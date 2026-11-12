<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0500. Each start is recorded, and changes compares it with the last good start

- Status: Accepted
- Date: 2026-11-12

## Context

A machine that stops starting after a change is fixed fastest by
knowing what changed. A Linux machine in the lab came up to a black
screen after a firmware package installed a week before took effect at
its next restart; finding that took the logs of earlier starts, the
package logs and file dates, read from a recovery shell. Dukovo's A/B
slots already undo a version or firmware set that fails its first start
(ADRs 0451 to 0453, 0468), but nothing said what a start ran, which
start last went well, or what changed between them, and no failure had
an error ID that leads to a page saying what to do (ADR 0420).

## Decision

1. **A record of every start.** The health check, which runs at every
   start, begins a record as it begins: the start's number, when, the
   slot, and what it runs: the version of Dukovo and when its slot was
   made, the kernel by its hash, the firmware set and when it was put in
   the store, the CPU microcode's hash, the boot options less the slot
   GRUB names, the network cards and the root's partitions, and the
   apps installed for everyone, each with when it was installed. It
   finishes the record with the outcome: *good* when the start was seen
   through or an update proved itself, *failed* with what failed and its
   error ID. A start whose health check never finished stays
   *unfinished*. The last 100 are kept in `/system/state/starts/`.
2. **The last good start** is the newest record that is good.
3. **changes(1)** compares two records and lists every difference:
   what is new, what changed and what is gone. Each new thing says when
   it was installed and which start first used it, which separates the
   day a change was installed from the restart that brought it in. By
   default it compares this start with the last good one; `--failed`,
   the newest failed start with the last good one before it;
   `--between`, two starts by number; `--list` lists the starts.
4. **Error IDs.** The health check's failures end with an error ID, in
   three new areas: `BOOT` for the health check of a start (a missing
   device, an essential service that did not stay up, a required check
   that failed), `FW` for firmware (damaged, refused, or missing CPU
   microcode) and `UPD` for an update that did not prove itself. Each
   has a page in `docs/errors/`, which the support center shows.

## Consequences

- The cause of a failed start can be named from one command, with no
  logs to read, and the record survives the fallback that undid it.
- Records are written from the root once it is mounted, so a start
  that dies before the health check runs leaves no record yet; how far
  each start got, written from its earliest stages, the summary on the
  EFI system partition that Rescue reads when the root does not mount,
  and the failure screen come with the next part of this work, each
  with a row in `docs/deferred.md`.
- The health check's messages gain an ID at their end, which the boot
  and lab tests match.
- The `firmware-fallback` boot test and the `updates/firmware-fallback`
  lab test read `changes --failed` and `changes --list` after a firmware
  set fails its try.
