<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Security policy

## The alpha, beta and release candidates

Dukovo 1.0.0-alpha.1 is the first release for anyone outside the project
to install, and every release before 1.0.0 is a test release.

- **No one outside the project has audited it.** Its security rests on
  the project's own reviews, tests, fuzzing and proofs, and nothing more
  yet. Do not run it where a break-in would cost you: not with data you
  cannot lose, and not on a machine that others depend on.
- **It takes its updates from the project's server.** A system installed
  from the project's install stick or ISO follows the beta channel of
  updates.dukovo.org, where each pre-release goes. Every release is
  signed on the project's hardware keys, and the server can sign nothing
  a machine would install (ADR 0457). The images' checksums are signed
  the same way, so that a download can be checked before it is written
  to a stick ("Creating USB install media" in the handbook).
- **Report what you find**, as below: which test releases get fixes is
  under [Supported versions](#supported-versions).

## Reporting a vulnerability

Please report security problems privately through GitHub's private
vulnerability reporting: open the repository's
[Security tab](https://github.com/dukovo-os/dukovo/security) and
choose **Report a vulnerability**, or go straight to
<https://github.com/dukovo-os/dukovo/security/advisories/new>.
Only the maintainers ([GOVERNANCE.md](GOVERNANCE.md)) can see the report.

Do not open a public issue, pull request or discussion about a
vulnerability before it is fixed and disclosed.

A useful report says which commit or release is affected, what an attacker
can do, and how to reproduce it: an `./ob run --cmdline` line, a boot test,
or a program, if you have one.

## What happens next

| When | What we do |
| --- | --- |
| Within 3 working days | Acknowledge the report. |
| Within 14 days | Confirm whether it is a vulnerability, tell you how severe we think it is, and agree on a disclosure date with you. |
| Within 90 days of the report | Release a fix and publish a GitHub security advisory, requesting a CVE where one applies. |

If a fix needs longer, for example because it depends on a third party, we
tell you why and agree on a new date; we do not keep a report secret
indefinitely. If the vulnerability is being exploited or is already public,
we fix and disclose it as fast as we can. If 90 days pass without a fix or
an agreed extension, you are free to disclose it yourself.

We credit you in the advisory unless you ask us not to. We do not run a
bug bounty.

## Security advisories

Each fixed flaw gets a Dukovo Security Advisory, DKSA (ADR 0534), beside
the GitHub security advisory and the CVE where one applies. An advisory is
an [OSV](https://ossf.github.io/osv-schema/) file in
[`security/advisories/`](security/advisories/README.md) named
`DKSA-YYYY-NNNN.json`, with its severity, the releases that have the flaw,
the release that fixes it and what to do. It is published in two forms:

- **For machines,** signed by the release keys with the release that
  carries the fix, on the update server. `update advisories` matches them
  on the computer, against the release it runs and the apps installed, and
  says which apply, how severe they are and how to take the fix. Nothing
  about the computer is sent. `update check` has a Security part, and a
  critical or high advisory that applies is named in the notice that
  `updated` shows.
- **For people,** in the support center, which has a page for each
  advisory, newest first, at
  <https://dukovo.org/support/security/index.html>, and an RSS 2.0 feed,
  <https://dukovo.org/support/security/feed.xml>, to follow in a feed
  reader.

Advisories of other projects that match a part of Dukovo but do not affect
it, because the flaw is fixed in the version Dukovo ships or its code is
not used, are stated in `security/vex.json` (ADR 0537), which the support
center publishes at <https://dukovo.org/support/security/vex.json> in the
OpenVEX format that scanners read.

Severity sets the clock for the fix, which starts when a fix exists,
whether upstream's or Dukovo's own. ADR 0458 has the table:

| Severity | A fix is on the stable channel within |
| --- | --- |
| Critical | 72 hours |
| High | 7 days |
| Medium and Low | The next monthly release |

A fix is a release like any other, with a build number such as 1.4.1.1,
through the same gates and the same waves, only faster. It is never
installed silently. The runbook
[Publish a security advisory](docs/runbooks/security-advisory.md) goes from
a report to a published advisory.

## Supported versions

Before 1.0, the newest pre-release and the `main` branch are supported.
A fix lands on `main` and reaches testers in the next pre-release. A
Critical or High fix comes sooner, as a fix release of the newest
pre-release, named with one more number: 1.0.0-alpha.1 is fixed by
1.0.0-alpha.1.1, which comes before 1.0.0-alpha.2. An older pre-release
gets no fix; update to the newest. The clock above holds from 1.0.0, and
before it a fix ships as soon as it is ready. There are no nightly
releases before 1.0 (ADR 0781).

From 1.0 on, the newest stable release gets every security fix, and the previous
stable release gets Critical and High fixes until the next stable release
ships. There is no long-term-support line (ADR 0458).

A fix lands on `main` first. A maintainer then labels its pull request
`backport` to copy it to the newest release branch, `release/X.Y`, and
`backport-previous` as well for a Critical or High fix, which copies it to
the branch before. The clock of ADR 0458 runs from the day a fix exists:
72 hours to the stable release for Critical, 7 days for High, and the next
monthly release for Medium and Low. A bot keeps the clock in one pinned
issue and marks a late fix `overdue`; the release train, the backport
labels and the clock are described in `docs/handbook/src/release-train.md`.

## Scope

In scope: everything in this repository, including the Radix kernel, the
`ob` build tool and the images it builds. A vulnerability in a third-party
component that Dukovo ships, such as GRUB, is also welcome; we will pass it
on to that project and update the component.
