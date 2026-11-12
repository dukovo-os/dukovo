<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0502. One update command and one updater service

- Status: Accepted
- Date: 2026-11-12

## Context

Three things update on a Dukovo machine: the system, in its two slots
(ADR 0466); the firmware set the system names (ADR 0468); and the apps,
from their catalogs or their own feeds (ADR 0384). Each had its own
command, sysupdate(8) for the first two and bundle(1) for the apps, and
nothing checked for updates unless someone ran one of them. The owner
should see every update in one place, decide when each is taken, and be
able to go back on one, while a server that nobody watches should be
able to keep itself current.

## Decision

1. **One command, `update`.** `update` (or `update check`) says what
   waits for the system, the firmware set and the apps. `update apply`
   stages the system update into the spare slot, or the firmware set
   when no system update waits, as the two share that slot, and updates
   the apps installed for everyone. `update undo system|firmware|ID`
   goes back on one thing: the system or firmware set by making the
   previous slot the default, an app as `bundle rollback` does.
   `update hold ITEM` skips the version offered now, keeping newer ones
   coming, with the system's and firmware's holds in updates.toml(5)
   and an app's as `bundle hold` keeps it; `update unhold` lifts it.
   `update history` lists each change of the system and the firmware
   set by the start that first ran it, from the starts' records (ADR
   0500), then the apps' history. sysupdate(8) and bundle(1) stay as
   the tools underneath.

2. **One service, `updated`.** Ten minutes after each start, then once
   a day, it checks as `update` does and does what updates.toml(5)
   says. The owner is in charge: by default it only tells whoever is
   signed in, through notifyd(8), that an update is ready and that
   `elevate update apply` stages it. `check = "download"` fetches the
   system update ahead; `apps = "auto"` updates the apps by itself;
   `check = "off"` stops the checks. Nothing is shown at shutdown or at
   start: a staged update starts at whatever restart the owner chooses.

3. **Unattended machines, by choice.** `unattended = true` has updated
   stage each system or firmware update by itself, and go back to the
   version before when the release it runs is withdrawn with that
   advice (ADR 0469). It restarts the machine only when a `restart`
   time is set as well, and then at that time of day. The two slots,
   the health check and the fallback (ADR 0460) are what make this
   safe: an update that fails its check is gone at the next start.

## Consequences

- Every update is seen and taken in one place, and a machine left alone
  can keep itself current without anyone at it.
- A held version gets no security fixes; update says so when it holds
  one, and a newer release is taken as usual.
- `update undo firmware` works only when the previous slot runs the
  same system with another firmware set; otherwise it says to undo the
  system, which brings its firmware set back with it.
- Not yet: unattended updates limited to security fixes, and the
  advisories each update fixes, which need the channel to list them;
  respecting metered connections, which netd does not know of yet;
  snoozing a reminder; pinning the system so that no newer version is
  taken; and the updater's page in the web console. docs/deferred.md
  lists each.
