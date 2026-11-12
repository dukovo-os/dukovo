<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0456. Releases reach machines in waves that canaries open and a pause closes

- Status: Accepted
- Date: 2026-11-11

## Context

A release that passes every test can still fail on machines CI never
saw. CrowdStrike's update of July 19, 2024 reached every machine at once
and left more than 8 million Windows systems restarting in a loop;
afterwards it moved to rings with canaries. Ubuntu phases its updates,
raising the share of machines that take one every 6 hours unless a
regression sets it to zero. Bottlerocket's updater withholds an update
until the wave its host falls in is open.

Dukovo collects no telemetry (ADR 0450), so its early warning must come
from machines the project owns and people who chose to test. Updates
arrive as static files from any server or mirror, and TUF decides
whether to trust them (ADR 0380).

## Decision

- **A release is one signed list** of content-addressed files (ADR
  0308). A machine fetches only the files it lacks, from any version to
  any newer one, with no delta built for each pair of versions. Every
  release also comes as one signed update file, for offline and
  air-gapped machines and mirrors, and as the install images.
- **TUF roles**, as in ADR 0381 with one more:

  | Role | Signs | Key | Expires after |
  | --- | --- | --- | --- |
  | root | The keys of every other role | Offline, on the hardware keys (ADR 0457) | 1 year |
  | targets | Channels, release lists, withdrawals | The hardware keys, a touch each signing | 3 months |
  | rollout (delegated) | Which waves of a listed release are open | Online, on the update server | 1 day |
  | snapshot | The current version of every metadata file | Online | 7 days |
  | timestamp | The newest snapshot | Online, refreshed every 6 hours | 1 day |

  The rollout role is Dukovo's addition: it can open or close the waves
  of a release that targets already lists, and nothing more. A stolen
  online key can pause or speed waves, or freeze updates for a day, and
  cannot add a release.
- **Channels:** nightly (every merge), beta (two weeks before each
  stable) and stable, each one signed file naming its current release.
  Moving from beta to stable never downgrades: the machine waits until
  stable passes its version.
- **Canaries** are machines the project owns, so their reports are not
  telemetry: the reference machine, the Framework Desktop once supported,
  and three small cloud machines in different regions and providers.
  Each takes every beta and stable first, runs the acceptance suite every
  hour, and reports to the maintainer. During Phase 15 they are QEMU
  machines in CI.
- **Waves.** Each machine draws a number from 0 to 999 at its first
  start, so that cloned cloud images do not share one, and keeps it to
  itself. Wave 0, the canaries, opens on release day; wave 1 (numbers 0
  to 9) on day 1; wave 2 (to 99) on day 3; wave 3 (to 499) on day 5; and
  everyone on day 7. A wave opens on schedule only while every canary is
  green and no one has paused the release. An owner who runs
  `update install` skips the wait, because they asked.
- **Pause, with no key:** `rollout pause VERSION "reason"` on the update
  server closes every wave not yet open, at each machine's next check. A
  machine with an update staged checks again just before the restart
  that would switch to it.
- **Pull, with one touch:** `ob release pull VERSION --reason "..."
  --advice rollback` signs a withdrawal with the release key. The
  release leaves every channel; a machine that staged it unstages it; a
  machine running it shows the reason and offers to go back, and only a
  server set to Unattended goes back by itself. The number is never used
  again: the fix ships as the next build number or release.
- **Failure reports** are offered after a fallback, show exactly what
  would be sent (versions, the failing step, a log excerpt, no
  identifiers), and are sent only when the owner chooses.
- **Every kind of update goes through the waves:** the system, the
  optional firmware package and the catalog.
- **Delivery:** HTTPS through rustls, resumed with range requests,
  retried with back-off, any mirror, an update proxy and a bandwidth
  limit, and every file hashed as it lands. Requests carry no machine ID
  or account. The clock is set over NTS before a check; a clock earlier
  than the running release's build date is moved forward to it, and one
  years ahead stops the check with a plain message. Each release names
  the oldest version that updates to it directly, and an older machine
  goes through a named bridge release first.
- **A drill each quarter** on the beta channel: publish a harmless beta,
  pause it, pull it, and time it, against 15 minutes to a pause and an
  hour to a signed pull.

## Consequences

- A bad release reaches at most the canaries and 1% of machines before a
  person can stop it, and stopping it needs no key.
- Machines older than a release's `updates_from` take a longer path, each
  step of which was tested.
- **Not yet:** all of it is built in Phase 15's later pull requests: the
  release list fetched through the TUF client, the channels and the
  offline update file in the sixth (ADR 0466), with the server side in
  `tools/update-server/` beside the catalog's; the waves, the rollout role, the canaries, pause and pull
  in the seventh; and the reports with the unified updater in the
  eighth. The canary cloud machines come before 1.0.0-alpha.1.
