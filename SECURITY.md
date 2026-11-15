<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Security policy

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

Before 1.0 only the `main` branch is supported, and fixes land there. From
1.0 on, the newest stable release gets every security fix, and the previous
stable release gets Critical and High fixes until the next stable release
ships. There is no long-term-support line (ADR 0458).

## Scope

In scope: everything in this repository, including the Radix kernel, the
`ob` build tool and the images it builds. A vulnerability in a third-party
component that Dukovo ships, such as GRUB, is also welcome; we will pass it
on to that project and update the component.
