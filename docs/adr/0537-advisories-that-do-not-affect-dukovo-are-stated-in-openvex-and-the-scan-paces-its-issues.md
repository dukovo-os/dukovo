<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0537. Advisories that do not affect Dukovo are stated in OpenVEX, and the scan paces its issues

- Status: Accepted
- Date: 2026-11-15

## Context

The daily scan of ADR 0534 asks OSV about each port by the git commit
of the release it pins. OSV's git ranges come from NVD's records, and
some of those name no fixed commit, so every commit of the project
matches, including releases that hold the fix. eSpeak NG's five flaws of
2023 are such records: the fix is in 1.52.0, and the scan still matched
1.52.0.

A port can also take only data from a project's source. The `locales`
port compiles glibc's locale sources with the host's `localedef` and
builds no glibc code, yet its pin is a commit of glibc, which matched
every flaw of glibc's code.

A finding that does not apply needs a decision that is recorded, has a
reason, and is reviewed like code. Closing its issue by hand records
the decision only in the tracker, where a scanner reading Dukovo's parts
never sees it.

The scan's first run opened 80 issues in 20 seconds, and GitHub stopped
it with its secondary rate limit, which allows 80 requests a minute
that create content and 500 an hour, made one after another.

## Decision

- **Statements are OpenVEX 0.2.0, in `security/vex.json`.**
  - OpenVEX is the format that Grype, Trivy and other scanners read to
    leave out what a supplier states does not apply.
  - A statement names the advisory, by its ID and any aliases, and each
    product by the package URL the scan gives it:
    `pkg:generic/<port>@<version>` for a port,
    `pkg:generic/<repository>@<commit>` for an extra source, by the
    repository's name and the commit pinned, and
    `pkg:cargo/<crate>@<version>` for a crate.
  - The version is part of the name. A statement lapses when the part
    moves to another version, and the advisory is weighed again then.
  - `fixed` and `not_affected` clear the advisory. `not_affected` has a
    justification of OpenVEX's or an impact statement. Every statement
    that clears says why, in `status_notes` or `impact_statement`.
  - `ob security check` refuses a document that breaks these rules, a
    field OpenVEX does not define, `subcomponents`, and two statements
    of one advisory and product. CI runs the check.
  - A statement is added in a pull request, so the reason is reviewed
    and kept in the history.
- **The scan applies them.**
  - A finding that a statement clears is left out of the findings and
    listed as not affected, with the statement's reason.
  - The issue of an advisory that no longer has a finding is closed
    with a comment that quotes the statement. It is closed as
    completed, so it opens again if the statement is withdrawn.
  - A statement that clears no finding is listed in the run's summary,
    so that it is removed or fixed.
- **A port that takes only data says so.** The recipe field
  `data_only` gives the reason in one line. The scan does not ask about
  the project's flaws, which are in code the port does not build, and
  lists the port as not asked about, with the reason.
- **The support center publishes the statements** at
  `security/vex.json`, beside the advisories and their feed, so a
  scanner reading Dukovo's parts reads the same decisions.
- **The bot paces its writes.** Every request that creates or changes
  an issue or label waits a second after the one before. After a
  secondary rate limit it waits the seconds of `retry-after`, or a
  minute, and tries again, three times in all.

## Consequences

- A false positive is closed by a reviewed statement with its reason,
  not by a person's memory, and scanners outside the project learn of it
  too.
- A statement does not outlive the version it was made for.
- A first run with many findings takes about a second for each issue it
  opens, and runs to its end.
