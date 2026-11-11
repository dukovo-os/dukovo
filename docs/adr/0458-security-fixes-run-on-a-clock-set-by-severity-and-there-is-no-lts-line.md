<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0458. Security fixes run on a clock set by severity, and there is no LTS line

- Status: Accepted
- Date: 2026-11-11

## Context

The implementation plan said security fixes would ship "within days",
to every supported release, and 15d planned an LTS channel from 1.0.
One person maintains Dukovo. A second supported line roughly doubles the
work of every backport, and a promise of "within days" cannot be tested
(ADR 0450). Apple built a separate mechanism for small security
patches, used it once in 2023, when it broke websites, and rebuilt it in
2025; Dukovo's release list already makes a one-file fix a one-file
download (ADR 0456).

## Decision

- **A security fix is a release with a build number,** 1.4.1 to
  1.4.1.1, through the same path, the same gates and the same waves as
  any release, only faster. It is never installed silently.
- **Severity sets the clock,** which starts when a fix exists, upstream
  or our own:

  | Severity | For example | On stable within | Soak on canaries and beta | Waves |
  | --- | --- | --- | --- | --- |
  | Critical | Remote code execution from the network, or exploited in the wild | 72 hours | 24 hours | All open within 24 hours |
  | High | Local privilege escalation; a remote crash of a service | 7 days | 3 days | Over 3 days |
  | Medium and Low | Hard to reach, or minor | The next monthly release | Normal | Normal |

- **The path:** a private report through SECURITY.md and GitHub's
  private advisories, or an issue the daily SBOM scan against OSV and
  NVD opens; triage of severity and affected versions; the fix merged to
  main first, in a private fork while embargoed, then copied to the
  release branch; every gate; and the advisory published with the
  release.
- **Advisories** are Dukovo Security Advisories in the OSV format, with
  an RSS feed. A machine matches them against its own version and SBOM
  locally (`update advisories`), so nothing about it is sent anywhere.
- **Which versions get fixes:** the newest stable gets every fix; the
  previous stable gets Critical and High fixes only, until the next
  stable ships.
- **Releases are monthly, on a fixed day.** The release branch is cut 14
  days before, and only fixes enter it while the canaries soak the
  betas. A release that is not green on release day skips that month
  rather than being rushed; security fixes keep their own clock.
- **No LTS line** while one person maintains Dukovo. It is reconsidered
  when a second maintainer joins, or after a year of monthly releases on
  time.
- **No kernel live patching.** A fix in the system takes the ordinary
  restart, at the owner's time or in a server's window; a fix in a
  catalog service restarts only that service.

## Consequences

- Backports touch at most two branches, one of them only for serious
  fixes.
- Owners of machines that must not change for years have no LTS line to
  choose; the update settings of 15d let them pin a release line and
  take only security fixes for as long as that line gets them.
- **Not yet:** the release branch and its bots, the advisories feed and
  `update advisories` (15d), and the SBOM scan, before 1.0.0-alpha.1.
  Restarting only user space when the kernel did not change, as
  systemd's soft-reboot does, is worth a look later.
