<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# The release train and the security bots

Dukovo ships a stable release every month. Four bots in the repository
keep the calendar and the security clock, so that a person does the part
that needs judgment: deciding what goes in, and whether a release is
good. This chapter is for the people who maintain the project. It says
what each bot does, what a maintainer does in return, and what to do when
a bot reports a problem. ADR 0458 sets the policy, ADR 0534 the tooling,
and `docs/runbooks/security-advisory.md` goes from a private report to a
published advisory.

## The calendar

A release day is the second Tuesday of a month, in UTC. The release
branch `release/X.Y` is cut from `main` 14 days before, which is the
Tuesday two weeks earlier, so a release has 14 days on the beta channel
for the betas to soak on the canaries (`./ob release canaries`). `X.Y` is the
version in `[workspace.package]` of the root `Cargo.toml` when the branch
is cut. After the cut, `main` moves to the next minor version, and only
fixes enter the release branch.

| Month | Cut (UTC) | Release day (UTC) |
| --- | --- | --- |
| November 2026 | Tuesday 27 October | Tuesday 10 November |
| December 2026 | Tuesday 24 November | Tuesday 8 December |
| January 2027 | Tuesday 29 December | Tuesday 12 January |

A release that is not green on release day skips the month. There is no
long-term support: the newest stable release gets every fix, and the
previous stable release gets Critical and High fixes until the next one
ships.

## The bots

| Bot | Runs | Does |
| --- | --- | --- |
| Release train, `.github/workflows/release-train.yml` | Every day at 04:41 UTC, and by hand | Cuts the branch on cut day, opens the release's issue, and reports on release day |
| Backport, `.github/workflows/backport.yml` | When a pull request is merged or labelled | Copies a merged fix onto the release branches |
| Security clock, `.github/workflows/security-clock.yml` | Every day at 05:23 UTC, when a security issue changes, and by hand | Keeps the pinned "Security clock" issue and the `overdue` label |
| SBOM scan, the `sbom-scan` job of `.github/workflows/upstream.yml` | Every day at 06:17 UTC, and by hand | Opens an issue for each known flaw in what the system is built from |

Each bot is a script in `.github/scripts/` that GitHub runs with
`actions/github-script`, and CI tests the scripts with `node --test` and
checks the workflows with actionlint. The bots create the labels they
use when a label is missing, and leave an existing label as it is, so a
maintainer can change its colour or its words.

### The release train

On cut day the bot creates `release/X.Y` at the head of `main`. If the
branch exists already it leaves it. It opens one issue, "Release X.Y",
labelled `release`, with a checklist of what a release needs:

- only fixes are merged into the branch, each by the `backport` label;
- the check "CI passed" is successful on the head of the branch;
- the release is in the update repository and the beta channel offers
  it;
- the betas have soaked on the canaries, and the canaries are green;
- the release's security advisories are checked and signed;
- the release notes are written;
- the stable channel offers it, and the waves are watched;
- the release is tagged `vX.Y.Z` on the branch;
- the version on `main` is moved on.

Each step names its `./ob` command in the issue. Tick a box when
the step is done.

On release day the bot comments on the issue. The release is green when
every box is ticked and the check "CI passed" is successful on the head
of the branch; the comment then says so. Otherwise the comment says what
is missing and that the release skips the month, and gives the next
release day. A branch gets CI on each push, so that the check exists on
its head.

The train starts disabled. While `.github/release-train.json` says
`{"enabled": false}`, the bot writes what it would do into the job's
summary and changes nothing. It is enabled at 1.0.0, by setting
`"enabled": true`. The bot reads the file at the head of `main`.

A run by hand, from the Actions tab or `gh workflow run release-train.yml
-f date=2026-12-08`, acts as on that UTC day, which shows what a day
does, or catches up on a day the schedule missed. It does what the
configuration allows, so a disabled train only writes a summary.

### Backports

A fix goes to `main` first. After it is merged, a label on the pull
request copies it to a release branch:

- `backport` copies it onto the newest `release/X.Y`;
- `backport-previous` copies it onto the one before as well, and only for
  a pull request that also has the label `severity:critical` or
  `severity:high`. Another pull request gets a note in the job summary
  that the previous release takes Critical and High fixes only.

The label can be added before or after the merge; the bot works on a
merged pull request alone, and ignores one that was closed unmerged. It never checks out the pull request's code
and never runs it: it takes the default branch, fetches the merge commit
and the release branch, and runs `git cherry-pick -x`, so the commit
says which commit it came from. The title and text of the pull request
are data, and reach the bot only through the environment.

For each target the bot pushes `backport/<number>-to-<X.Y>`, opens a pull
request titled `[release/X.Y] <title>` that links the original, and
starts CI for the branch with a `workflow_dispatch` of `ci.yml`, since
GitHub does not start a workflow for a push made with the workflow's own
token. The commit keeps the original's author and its sign-off, and the
committer is `github-actions[bot]`, so it passes `./ob signoff` when the
original does. The bot checks the rule before it pushes; a commit that
would fail it is pushed all the same, and the pull request says so and
how to add the sign-off.

Doing it twice does nothing: a backport pull request for the target is
found by its branch and left alone. Before the first release branch
exists, the bot says so in the job summary and ends without an error.
The bot runs when a label is added, so a label added before the first
release branch exists does nothing: remove it and add it again.

When the cherry-pick does not apply, the bot aborts it, adds the label
`backport-conflict` to the original and comments on it. The job summary
has the exact commands for a person:

```console
$ git fetch origin release/1.4 <merge commit>
$ git switch -c backport/1234-to-1.4 origin/release/1.4
$ git cherry-pick -x <commit>
(fix the conflict, then git add and git cherry-pick --continue)
$ git push origin backport/1234-to-1.4
```

Open the pull request against `release/1.4` with the title
`[release/1.4] <title>`, and start CI with `gh workflow run ci.yml --ref
backport/1234-to-1.4`. Remove `backport-conflict` when the pull request
is open.

### The security clock

ADR 0458's clock starts when a fix exists, and not when a flaw is found:
the label `fix-available` on a `security` issue starts it, and the
bot reads the time from the issue's events. If the label is removed, the
clock stops, and starts again when it is added again.

| Severity label | Due |
| --- | --- |
| `severity:critical` | 72 hours after the clock starts |
| `severity:high` | 7 days after |
| `severity:medium` and `severity:low` | The end of the next release day, which is the second Tuesday after the clock starts |

One pinned issue, labelled `security-clock`, has a table of the open
`security` issues: the issue, its severity, when the clock started, when
it is due, and its state: due in so many days or hours, overdue by so
much, clock not started, or no severity label. Overdue issues come
first, then the rest by due time. The bot creates the issue once and pins
it, and finds it by its label afterwards, open or closed; a closed one
is opened again. It labels a late issue `overdue` and removes
the label when the issue is no longer late. An issue with two severity
labels is held to the more severe, and the table says so.

A security issue stays out of the table when it is closed. Reopening it
puts it back.

### The daily scan

`./ob security scan` looks for known flaws in what the system is built
from. It runs `cargo audit --json` on every tracked `Cargo.lock`, without
changing one, and on the lock of every Rust port that `./ob ports audit`
audits. It then asks OSV, in one batch, about:

- each port whose recipe pins a git commit, by that commit, and each of
  the port's `[[extra]]` sources, by theirs;
- each port whose recipe pins a release archive, by the commit of the
  tag its `[upstream]` names;
- each crate a recipe lists (`[[crate]]`), by its name and version on
  crates.io, unless a `Cargo.lock` that cargo-audit read lists it, since
  the RustSec database has answered for those.

The `sbom-scan` job runs it every day and reads its JSON.

For each advisory ID, the job keeps one issue, labelled `security` and
`sbom-scan` with the advisory's severity, or `needs-triage` when the
advisory gives none. The title starts with the ID. An ID never has a
second issue, open or closed:

- a finding with no issue gets one;
- an issue that is closed gets opened again when its ID matches again,
  with a comment, unless it was closed as not planned, which is a
  decision to live with the finding;
- an open issue whose ID no longer matches is closed with a comment that
  says why: the part that had it has moved to a version without it, the
  advisory was withdrawn, the scan does not ask about the part, or a
  statement says Dukovo is not affected (see below). The job reopens an
  issue that was closed this way when its ID matches again.

The job writes one issue, comment or label at a time, a second apart, as
GitHub asks of a client that creates content. When GitHub answers with
its secondary rate limit, the job waits for the seconds of the response's
`retry-after` header, or a minute when there is none, and tries the write
again, three times at most. A limit that does not lift fails the job. The
issues made before the limit stay, and the next run carries on from them.

An issue labelled `needs-triage` waits for a maintainer, who reads the
advisory and adds `severity:critical`, `severity:high`, `severity:medium`
or `severity:low`. The severity is never guessed: it is the one the
advisory names, or the base score of its CVSS 3 vector (Critical from
9.0, High from 7.0, Medium from 4.0, Low below).

Add `fix-available` when a fix exists, upstream or in a Dukovo patch, and
the clock starts. Warnings of `cargo-audit` about a crate that is
unmaintained or yanked are not findings.

A scan that could not run, because cargo-audit is missing, OSV cannot
be reached or the tag of a release cannot be resolved, fails its job and
changes no issue.

#### Release archives and what the scan does not cover

A recipe that pins a release archive, with `url` and `sha256`, names no
commit. Its `[upstream]` table gives the project's own git repository and
the tag of the release the archive is:

```toml
[upstream]
git = "https://github.com/curl/curl.git"
tag = "curl-8_23_0"
```

The scan resolves the tag with `git ls-remote`, once for each port, and
takes the commit an annotated tag points to, which is the commit OSV's
records name. The archive stays what is built. The recipe check refuses
an archive port with no `[upstream]`, and a tag that is not the release of
the recipe's `version`, so a pin moved without its tag fails `./ob ports`
before it can make the scan look at the wrong release. The same table
tells `./ob ports outdated` where to look for a newer release.

A project with no public git repository says why instead, in one line:

```toml
[upstream]
none = "Info-ZIP publishes release archives and keeps no public repository"
```

The scan cannot ask OSV about such a port. It lists the port as not asked
about, with the reason, in its output and in the job summary, and an
advisory against it is found by reading the project's release
announcements. `docs/deferred.md` keeps the list of these ports.

A port that takes only data from its source, such as the locale tables of
glibc and none of its code, says so with `data_only`, and a reason of one
line:

```toml
data_only = "only the locale sources and character maps are taken, and the host's localedef compiles them; no glibc code is built"
```

The flaws of the project are in code the port does not build, so the scan
does not ask about them. It lists the port as not asked about, with the
reason, as it does a port with no repository. A port with `data_only` has
a source, and lists no crates.

#### Findings that do not affect Dukovo

An advisory can match a part of the system that it does not affect: its
record names no fixed commit, though the commit the port pins has the fix,
or the vulnerable code is in a part of the project the port does not
build. A statement in `security/vex.json`, an
[OpenVEX](https://openvex.dev) 0.2.0 document, says so. A pull request
adds it, with:

- the advisory's ID, as the issue's title gives it;
- the product, as the issue's table names it in the column Product: a
  package URL with the recipe's version, `pkg:generic/<port>@<version>`
  for a port, `pkg:generic/<repository>@<commit>` for an extra source,
  with the commit pinned, or `pkg:cargo/<name>@<version>` for a crate;
- the status, `not_affected` or `fixed`;
- the reason, in `status_notes` or `impact_statement`. A statement of
  `not_affected` also has a `justification`, one of OpenVEX's, or an
  `impact_statement`.

For example, a statement that espeak-ng 1.52.0 holds the fix of CVE-2023-49990:

```json
{
  "vulnerability": { "name": "CVE-2023-49990" },
  "products": [{ "@id": "pkg:generic/espeak-ng@1.52.0" }],
  "status": "fixed",
  "status_notes": "Fixed upstream by commit 58f1e0b6 (espeak-ng #1846, for issues #1823 to #1827), which release 1.52.0 holds. OSV's record names no fixed commit, so it matches every commit of the project."
}
```

`./ob security check` checks the file, and CI runs it. The next scan
leaves the finding out of its `findings` and lists it under
`not_affected`, with the statement. The `sbom-scan` job closes the
issue as completed, with a comment that quotes the statement. The
support site publishes the file as `security/vex.json`, so that a scanner
that reads OpenVEX reads the same statements.

The product names a version. A statement lapses when the port moves to
another one, and the advisory is weighed again for the version the port
moves to. A statement that clears no finding is listed under `unused_statements`, and
in the job summary, to be corrected or removed. The issue that a statement
closed is opened again when the statement is withdrawn, or when the ID
matches in another product.

Run it on a machine with `cargo-audit` installed:

```console
$ ./ob security scan
$ ./ob security scan --json | jq '.findings[] | select(.severity == null)'
```

## The release is built twice

A release is signed only when the builds of its commit agree (ADR 0457),
so CI builds it twice before anyone signs. The workflow
`.github/workflows/reproducible.yml` runs every Monday at 04:23 UTC, on
each push to a `release/**` branch, by hand, and on a pull request that
changes the workflow, the identifiers, the images, the release build or
one of the recipes that keep paths and times out of a build (ADR 0535).

Both builds run `./ob release build`. The first is made in the runner's
own checkout with the ports that `ci.yml` cached. The second is made on
another runner in `/srv/dukovo-second`, by another user with a home of
its own, under `umask 0002`, in the time zone of Auckland, with
`LC_ALL=C.UTF-8` and one processor fewer, and builds every port from its
source. It takes about two hours. A third job compares the lists of
files, and when they differ it names each path that does, runs diffoscope
on the first twenty that differ in content, uploads the reports as the
artifact `reproducible-report`, and fails.

To build a release on a machine of one's own, and to compare it with a run:

```console
$ ./ob release build --out out/mine
...
<the SHA-256 of the release list, 64 hexadecimal digits>
$ cat out/mine/system.json.sha256
```

The job "Compare the builds" prints both builds' values, and
`system.json.sha256` is in the artifacts `reproducible-first` and
`reproducible-second`; the three are the same when the builds agree. `files.txt` lists every folder, file and
link of the tree, with its mode, SHA-256, size and time, sorted, so that
`diff` of two of them shows what differs. The tree itself stays in
`out/release-build-tree`.

The build trusts the repository's own root, not the machine's release
keys, and signs nothing; `releases/README.md` says which root.

## Troubleshooting

**The reproducible workflow failed.** Open the job "Compare the builds":
its summary lists the paths that differ, and the artifact
`reproducible-report` has diffoscope's report on each of the first twenty
that differ in content. A path in a binary with a string such as
`/home/runner/work` or `/srv/dukovo-second` is a compiler that was not
told to write the path down under another name: `tools/xtask/src/repro.rs`
has the names, and the recipe or build script that runs the compiler needs
its flag. A path that differs in mode or time only has `files.txt`'s
lines to show it. A first or second job that failed did so building, and
its log says where.

**A pull request ran the reproducible workflow and took two hours.** It
changes a file the workflow watches, which are the files that decide what
goes into a release's tree. The run is cancelled by the next push.

**The release-train job's summary says it is disabled.** The train is
enabled at 1.0.0. Set `"enabled": true` in `.github/release-train.json`
on `main`.

**The branch is missing on cut day.** Run the workflow by hand with the
cut day as `date`. The bot creates the branch and the issue if they are
missing, and leaves them if they exist.

**The release issue says the release is not green.** The comment lists
the boxes that are not ticked, or that "CI passed" is not successful on
the head of the branch. Fix it and tick the box before the next release
day, or let the release skip the month. A branch's CI is started again
with `gh workflow run ci.yml --ref release/X.Y`.

**A backport pull request has no checks.** The bot starts CI with a
dispatch. If the dispatch failed, its job summary says so; run
`gh workflow run ci.yml --ref backport/<number>-to-<X.Y>`.

**A backport was not made.** The pull request must be merged, and
labelled `backport`. The summary of the run says why it did nothing. A
label that was added before the first release branch existed does
nothing: remove it and add it again.

**The security clock issue is missing or twice.** The bot finds the issue
by the label `security-clock` and makes a second only when no issue has
the label. To start over, remove the label from the old issue; the next
run makes a new one and pins it. A pin is made once, so a person who
unpins the issue keeps it unpinned.

**The scan fails with "could not find the commit of the release of".**
The message names each port whose tag it could not resolve, and why: the
repository could not be reached, or it does not have the tag. Run `git
ls-remote --tags <repository> <tag>` with the `git` and `tag` of the
port's `[upstream]`. A repository that was down is asked again by the
next run. A project that moved its repository or named a release
differently needs its `[upstream]` corrected in the recipe.

**A scan issue is opened again every day.** The issue was closed as
completed while the finding still matches. Close it as not planned, to
live with the finding, or fix what the finding is about.
