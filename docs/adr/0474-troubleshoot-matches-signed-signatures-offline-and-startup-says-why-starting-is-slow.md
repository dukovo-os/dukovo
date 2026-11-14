<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0474. troubleshoot matches signed signatures offline, and startup says why starting is slow

- Status: Accepted
- Date: 2026-11-13

## Context

When a start fails, Dukovo already keeps a record of it (ADR 0500),
compares it with the last good one, and shows a failure notice with an
error ID and its support page (ADR 0501). What it does not do is name
the likely cause: the person reads the log and the support pages
themselves. `service explain` (ADR 0471) says why a service is in its
state, but its fix is generic.

When a start is slow, nothing says why. Spark knows when each service
started and became ready, and the firmware often records how long it
and the boot loader took (ACPI's FPDT), but nothing puts them together,
and nothing notices that starting got slower than it was.

## Decision

1. **troubleshoot(8) matches known signatures, offline.** It gathers
   the facts of one start: its record and the last good one's, its log
   (the kernel's lines among them), the services' states, and how full
   each mounted filesystem is. Each signature whose every condition
   holds is a match; it prints the cause, the error ID, the fix and the
   support page, best first: the start's own error ID first, then by how
   many conditions held. Nothing is sent anywhere. With no start named it
   takes the newest that is not good, but not this boot's while the
   health check is still checking it, since that record has no outcome
   yet: it says so and takes the one before.
2. **Signatures are signed data.** The release ships
   `/system/resources/troubleshoot/signatures.toml`. The daily update
   check fetches the update server's target `troubleshoot/signatures.toml`
   through the TUF client, as it fetches a channel's file, and keeps it
   in `/system/state/troubleshoot/` when its `version` is newer than both
   the shipped one and the one kept; an update file carries one too. An
   older file is never taken, and a file that does not parse is refused
   whole and logged once.
3. **The format is data, with nothing that runs.** TOML, `format = 1`,
   a `version`, and `[[signature]]` tables of `id`, `cause`, `fix` and
   `match`, a list of conditions: a log line by glob (`*` and `?` only,
   matched in linear time), optionally of one service and level; the
   start failed or did not finish, optionally with an error ID; an item
   that changed since the last good start (firmware, kernel, options,
   card, root, app, system); a service that failed, or an essential one;
   a filesystem at least so full. Limits: 256 KiB, 512 signatures, 8
   conditions each and 64 on the log in all, 200 bytes a glob. Matching
   also has a budget of work, counted in lines looked at and bytes a glob
   reads; when it is spent, the conditions on the log that are left do not
   hold and `troubleshoot` says it was cut short, so that a file cannot
   make a match run long. A condition of a kind this release
   does not know skips its signature, so a newer file still helps an
   older system.
4. **The first signatures** name the firmware failures FW-101 to 103,
   an essential service that failed (BOOT-102), a full system disk
   (BOOT-104), a root that did not mount (BOOT-105), disk I/O errors in
   the kernel's log (BOOT-107) and a failure after a boot option changed
   (BOOT-108), each with its support page.
5. **Where it shows.** The failure notice gains a line, "Likely cause:
   CAUSE (ID). Fix: FIX", from the best match, and Rescue's menu gains
   "Find the likely cause". `service explain` asks the matcher about the
   service at the root of its chain, and a match's fix replaces its
   generic one.
6. **startup(8) says why starting is slow.** Each start record gains
   `times`: the firmware's and the loader's (from the FPDT's Firmware
   Basic Boot Performance Table, which the kernel reads only where the
   UEFI memory map keeps it for the firmware, at `/kernel/boot/times`),
   when the kernel started Spark, each service's start and ready times
   and what it started after, when the services settled, and the first
   sign-in. The health check sees a start through once the essential
   services run, which is often before the last service is ready, so it
   takes the times once no service started at boot is pending or
   starting, waiting at most two minutes. `startup` prints each step from firmware to sign-in, the
   chain of services that set the total, and the ten slowest; `--compare`
   puts a start beside the last good one.
7. **A slow start is noticed.** When the services settle more than 20%
   and at least two seconds later than the median of the last five good
   starts, the health check logs and notifies which step grew and which
   service most (BOOT-106). Five good starts are needed first.

## Consequences

- A failed start names its likely cause on the screen that reports it,
  from the machine's own records with no network and no guessing, and
  the cause is one a test proved: each shipped signature matches the
  facts recorded from its planted fault, and no other fault's.
- Signatures improve between releases without a system update, and
  only through the same signed channel as the system itself.
- A start that grows slower is reported the first time, with the
  service to look at, rather than when someone notices.
- What Dukovo does better: systemd-analyze shows blame and the critical
  chain, but not the firmware beside it on most machines, and never
  compares with the last good start or says when a start got slower.
  Windows' troubleshooters are code, shipped with the system, and go
  online; Dukovo's signatures are signed data, run offline, and each is
  proved by a planted fault before it ships.
