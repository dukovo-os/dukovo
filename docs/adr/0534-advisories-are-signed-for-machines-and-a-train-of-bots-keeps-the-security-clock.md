<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0534. Advisories are signed for machines, and a train of bots keeps the security clock

- Status: Accepted
- Date: 2026-11-15

## Context

ADR 0458 set the clock for security fixes, by severity, and left these
for later:

- the release branch and its bots
- the advisories feed and `update advisories`
- the daily scan of the system's SBOM

With one maintainer, a clock that is kept by memory is not kept: the
work that stays manual is the work that slips.

Other projects publish advisories in many places:

- Debian publishes DSAs and Ubuntu USNs.
- GitHub publishes GHSAs, and the OSV schema lets one scanner read all
  of them.
- Microsoft ships its fixes on the second Tuesday of each month, a day
  many teams already keep for patching.

What no common system does is let a machine check an advisory against
itself without telling anyone what it runs. Package managers that check
send a list of packages, or need an account.

## Decision

- **Dukovo Security Advisories are named DKSA-YYYY-NNNN.**
  - The prefix is not DSA, which OSV already uses for Debian's.
  - Each is one OSV 1.6 file in the source tree,
    `security/advisories/`, so osv.dev and any OSV scanner can read
    them as they are.
  - The system is the package `dukovo` in the ecosystem `Dukovo`, a
    catalog app is named by its ID, and ranges follow Dukovo's version
    order.
  - `database_specific.severity` is critical, high, medium or low,
    as ADR 0458's table has them.
  - `./ob security advisory new` starts one and `./ob security check`
    checks them all. CI runs the check.
- **Machines get them signed.**
  - The release keys sign `advisories/index.json` and each advisory
    as targets of the system's repository. They do it in the same touch
    as the channel that offers the fix (`./ob release channel` brings
    the tree's advisories), or alone with `./ob release advisories`.
  - `update advisories` fetches them through the same TUF client as
    the releases. It then matches them on the machine, against the
    release it runs and the apps it has, with the catalog's list of
    known flaws for the apps.
  - Nothing about the machine is sent.
  - `update check` adds a Security section, and a Critical or High
    advisory that applies is named in updated's notice.
- **People get pages and a feed.** The support center, built from the
  tree, has a page for each advisory and an RSS feed. The website takes
  the support center from main within six hours.
- **Releases are monthly, on the second Tuesday.**
  - A branch `release/X.Y` is cut 14 days before, and only fixes enter
    it while the canaries soak the betas (ADR 0533).
  - A release that is not green on its day skips the month.
- **Bots keep the calendar and the clock.** They are GitHub workflows,
  whose scripts are tested in CI and whose actions are pinned by commit.
  - **release-train** cuts the branch and opens the release's issue
    with its checklist. On release day it says whether the release is
    green.
  - **backport** cherry-picks a merged pull request labelled
    `backport`, onto the newest release branch. With
    `backport-previous` it also cherry-picks onto the one before, which
    takes only Critical and High fixes.
    - It opens the pull request, and starts CI on it by dispatch,
      since pushes made by a workflow's own token start no workflows.
    - On a conflict it labels the original and writes out the commands.
  - **security-clock** keeps one pinned issue with every open
    security issue and when it is due.
    - Each issue's clock starts when `fix-available` is added, and runs
      by its `severity:` label against ADR 0458's table.
    - A late issue is labelled `overdue`.
  - Until 1.0.0 the release train only says what it would do.
- **A daily scan of what the system is built from.**
  - `cargo audit` reads every Cargo.lock: the kernel's, the
    programs', and the catalog's ports'.
  - OSV is asked about each C port by the git commit of the tag it
    pins. That covers projects with no package ecosystem through OSV's
    git ranges, which come from NVD, with no CPE to map by hand.
  - A finding opens one issue labelled `security` and `sbom-scan` with
    its severity, and never a second issue for the same ID. The scan
    closes the issue when the finding no longer matches.

## Consequences

- An owner learns from their own machine, without telling anyone what
  it runs, whether a published advisory applies to it and what to do.
- The calendar and the clock are kept by bots that say when something
  is late, rather than by memory.
- The previous stable line costs backports only for serious fixes, as
  ADR 0458 intends.
- **Not yet:**
  - The release-train, backport and security-clock bots, and the daily
    scan, which come in the next pull request.
  - The release train cuts its first branch at 1.0.0.
