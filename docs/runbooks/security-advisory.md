<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Publish a security advisory

A report of a flaw in Dukovo or in a catalog app becomes a Dukovo
Security Advisory, DKSA (ADR 0534): a file in `security/advisories/` that
machines read signed from the update server, and people read in the
support center and its feed. This page goes from a private report to the
published advisory, with the `ob` commands of each step. The clock it
works to is ADR 0458's, and SECURITY.md has the policy.

You need the source tree and the release keys on your laptop, a login on
the update server (`$OB_UPDATE_SSH`), and access to the repository's
private vulnerability reports. Do the steps in order. A step that fails
changes nothing a machine can see: only step 6 does.

## 1. Take the report

The report arrives through GitHub's private vulnerability reporting.
Acknowledge it within 3 working days, as SECURITY.md promises. Do not
open a public issue, pull request or discussion about it.

## 2. Triage

Decide three things, and write them in the report's private thread.

- **Whether it is a vulnerability,** by reproducing it.
- **Which versions have it.** The system has one version number a release,
  so name the first release that has the flaw and the newest one that
  still does. For an app, name the catalog app's ID and its versions.
- **How severe it is.** The table is ADR 0458's, and it sets the clock,
  which starts when a fix exists:

  | Severity | On stable within | Waves |
  | --- | --- | --- |
  | Critical | 72 hours | All open within 24 hours |
  | High | 7 days | Over 3 days |
  | Medium and Low | The next monthly release | Normal |

  For the public `security` issues, the security clock bot starts the time
  when an issue is labelled `fix-available`, and keeps the pinned issue
  "Security clock" with what is due and what is overdue
  (`docs/handbook/src/release-train.md`). A report that stays private is
  timed by the person who triages it.

  The newest stable release gets every fix. The previous stable release
  gets Critical and High fixes until the next stable release ships. Before
  1.0 only `main` is supported.

## 3. Fix it, privately

While the flaw is embargoed, work in the report's private fork. The fix
merges to `main` first, and is then copied to the release branch
`release/X.Y`, and to the previous one when the severity says so. A
security fix is a release with a build number: 1.4.1 becomes 1.4.1.1.

The copy is made by the backport bot when the merged pull request has the
label `backport`, and `backport-previous` too for a Critical or High fix
(`docs/handbook/src/release-train.md`). The bot works on the repository's
public pull requests: while the fix is embargoed, copy it by hand in the
private fork with `git cherry-pick -x`.

## 4. Write the advisory

In the same private fork, so that it becomes public with the fix, not
before:

```console
$ ./ob security advisory new
Edit security/advisories/DKSA-2026-0007.json (the runbook, docs/runbooks/security-advisory.md, says how), then run ./ob security check.
```

The file starts with the next free number of the year, today's time and
the severity `medium`, and its text says TODO where it needs writing.
Edit it, following `security/advisories/README.md`:

- `summary` is one line, and `details` says what the flaw is, which
  releases have it, who can use it and what it does.
- `database_specific.severity` is `critical`, `high`, `medium` or `low`,
  as decided in step 2, and `database_specific.action` says what to do.
- Each `affected` entry names `dukovo` for the system, or an app's ID,
  with an `ECOSYSTEM` range: `introduced` is the first version with the
  flaw, `"0"` for all of them, and `fixed` is the version with the fix.
  Give one range for each release branch that gets the fix.
- `aliases` lists the CVE or the GitHub advisory, once one exists, and
  `credits` the reporter, unless they asked not to be named.

Then check it:

```console
$ ./ob security check
system-release: checked 1 security advisory in /home/you/dukovo/security/advisories, the worst high
```

The check refuses text that still says TODO, an id that is not the file's
name, a number used twice, and versions that are not Dukovo's. CI runs it
on the pull request.

To see the page and the feed before they go out:

```console
$ ./ob docs site --check
```

The page is `out/docs/site/security/DKSA-2026-0007.html`, and the feed
`out/docs/site/security/feed.xml`.

## 5. Merge and release

On the day of the release, merge the fix and the advisory together. Then
build the release as for any release, and sign it with the advisory in
the same targets version, with the release keys:

```console
$ ./ob release channel releases-repo stable 1.4.1.1
```

`releases-repo` is the update repository that `system-release add`
added the release to. The command signs `security/advisories/` with the
channel that offers the fix, so a machine that takes the release also
reads the advisory. For a fix that waits for a monthly release, the
channel is signed then, and the advisory goes out with it.

For an advisory about an app, or for one that goes out without a release,
sign the advisories alone:

```console
$ ./ob release advisories releases-repo
```

Nothing is signed when the advisories are as the repository has them.

## 6. Deploy

```console
$ ./ob release deploy releases-repo
```

The server takes the advisories with the metadata and targets, signs its
snapshot and timestamp, reads everything back from the first root, and
only then serves it. Check what it serves:

```console
$ ./ob release check
```

It reports how many security advisories the repository offers and the
newest. Then watch the waves as for any release (`./ob release canaries`).
A fix that is bad is withdrawn as any release is, with `system-release
pull` and a deploy.

## 7. Check it on a machine

On a machine that runs an affected release, `update advisories` says which
advisories apply, how severe they are and what to do, and ends with
status 1. The machine sends nothing:

```console
$ update advisories
update: Dukovo: DKSA-2026-0007, high: A local user can read another user's files
update:   Dukovo 1.4.1.1 has the fix and is offered: `update apply` stages it
update:   Update to Dukovo 1.4.1.1 with `update apply`, then restart.
```

After `elevate update apply` and a restart, the same command says that no
advisory applies, and ends with status 0. `update check` has the same
in its Security part, and `updated` tells whoever is signed in once a day
while a Critical or High advisory applies and its fix is not staged.

## 8. Publish and credit

- The pull request that merged the advisory puts its page and its feed
  item in the support center, which the website takes from `main` within
  six hours.
- Publish the GitHub security advisory of the report, with the CVE where
  one applies, and credit the reporter in it.
- Close the private thread.

## Correcting an advisory

A wrong summary, range or action is corrected in the file, with a new
`modified`. The number stays. Run `./ob security check`, merge, then sign
and deploy the corrected file as in steps 5 and 6, with `./ob release
advisories releases-repo`. Machines read the new file the next time they
check. An advisory is never removed once it is public: a file taken out
of `security/advisories/` leaves the update repository at the next
signing, and the support center's page with it.
