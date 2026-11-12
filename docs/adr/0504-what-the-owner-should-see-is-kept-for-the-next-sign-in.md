<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0504. What the owner should see is kept for the next sign-in

- Status: Accepted
- Date: 2026-11-12

## Context

A failed start is told on the sign-in screens until the next start (ADR
0501). Two other things the owner should know about were told only where
they had to look: a backup that failed, or a destination that has gone
days without one because its disk stays unplugged (ADR 0463), showed in
`backup list` and the log; and an installed app with known flaws (ADR
0389) showed in `bundle list` and the history. A notice on the screen's
bottom line (ADR 0212) goes after a few seconds, and is missed by anyone
not signed in when it comes.

## Decision

1. **One notice file per writer, kept in memory.** A service keeps what
   the next sign-in should say in `/system/live/sign-in/NAME`, one line
   each, and removes the file when nothing needs saying. The folder is
   in memory, so a start begins with none, and each writer puts its
   notice back on its next round if it still holds.
2. **The sign-in screens show them.** consoled and serial-console show
   the notices, by name, under the issue text and after a failed start's
   notice, each time a sign-in starts.
3. **A writer tells the consoles when its notice changes.** When a
   notice changes to one not shown before, its writer also sends a
   notice through notifyd (ADR 0212), so someone signed in sees it at
   once.
4. **backup** keeps `backup` after each round of the service, and after
   `run`, `verify --restore` and `remove`: each destination whose last
   backup or check by a restore failed, and each that backs up daily and
   has gone 3 days without a backup, or weekly and 14 days, counted from
   when it was added until its first. A destination backed up by hand is
   never overdue.
5. **bundled** keeps `bundled` after each daily check: each installed
   version whose catalog lists known flaws in it, with the first five
   advisories. A catalog that cannot be read leaves the notice as it
   was, since its bundles' flaws are not known.

## Consequences

- A backup that stops working and an app that needs updating are seen at
  the next sign-in, on the screen or the serial port, without asking.
- The notices are plain files any service can write, so later ones
  (alerts, ADR 0501's email, the web console) can read the same list.
- A notice for a removed app or destination goes at the writer's next
  round; a restart clears them all until then.
- The web console's own view of these, and which advisories an update
  fixes, wait for Phase 16 (docs/deferred.md).
