<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0593. A catalog or a bundle's feed may withdraw a version, and machines leave it

- Status: Accepted
- Date: 2026-11-20

## Context

A catalog lists each version of a bundle for good. A version that proves
bad, a crash on start or a program that sends keys away, stays listed, and
every machine that took it keeps it: ADR 0544's waves hold back a version
that has not reached a machine yet, a pause is all it has, and ADR 0384's
`bundle rollback` is for a person who knows. ADR 0469 lets the update
server withdraw a system release, so that machines go back; a bundle has
no such word. A bundle that updates from its own feed (ADR 0390) has none
either.

A bad version must also stop where it is installed: a version that does
harm should not run until a person notices. Yet a word that stops
programs is a strong one, so it must be signed by a key that can already
change what runs, never by the online key that signs the flaw list (ADR
0389) and the rollouts, and a field nobody can read must stop nothing.

This record builds on ADR 0544 (waves, which it does not wait for), ADR
0390 (a bundle's feed), ADR 0384 (the version kept beside, going back and
holds) and ADR 0592 (a catalog that vouches takes an ID from its
publisher).

## Decision

- **A target may carry a withdrawal.** The `custom` of a catalog's target,
  or of a target of a bundle's own feed, may hold `withdrawn`: an object
  `{kind, reason, since}`, where `kind` is `broken` (the version does not
  work as it should) or `malicious` (it does harm), `reason` is text, and
  `since` is the time, in seconds since 1970, of the withdrawal. The
  target stays listed, so that every machine that reads the catalog or the
  feed learns of it. The targets role signs it, whose keys are offline for
  a catalog, and the publisher's key for a feed. The flaw list, which the
  online key signs, never carries it.
- **A field that does not read fails closed for installs only.** One that
  is there and is not an object, names another kind, or has no integer
  `since`, withdraws the version as `broken`, with the reason "its
  withdrawal does not read: WHY". Such a field stops nothing that runs,
  since nobody can say what it meant.
- **Every machine keeps what it learns, for good.** Each read of a
  catalog (search, sheet, install, update and the daily check) or of a
  feed (update, and the daily check, which reads the feed of each bundle
  from no catalog as it reads the catalogs) adds the withdrawals that
  bundled has not got to
  _/system/state/bundled/withdrawn_, a line each: `SOURCE ID VERSION
  SHA256 KIND SINCE REASON`, where SOURCE is `catalog:NAME` or
  `feed:PLACE`. A line is never changed or removed by a later read, so a
  catalog that drops the field, or says `broken` where it said
  `malicious`, changes nothing; a `malicious` line may be added beside a
  `broken` one, never the reverse. The file holds at most 4,096 lines;
  past that, the oldest line of a version that is installed nowhere and
  kept nowhere goes. A feed withdraws only versions of its own bundle's
  ID. When a catalog that vouches takes an ID from its publisher (ADR
  0592), what that publisher's feed withdrew is forgotten, as that key no
  longer speaks for the ID. The file is one of bundled's registry files,
  so backups carry it.
- **Which line applies.** To an install of ID at VERSION in a place: a
  line of the catalog the ID is bound to there, or for everyone, or of
  a feed there or for everyone. To a file offered for install: a line of
  any catalog, or of the place's feed, whose SHA-256 is the file's, so a
  withdrawn file does not come back under another name or from another
  source.
- **A withdrawn version is never taken.** `bundle install` refuses it:
  "ID VERSION was withdrawn by catalog C on DATE: REASON". Searches,
  sheets and updates do not offer it, and `bundle rollback` refuses to go
  back to it.
- **What bundled does,** on its updater thread, at once after it learns a
  new line, when it starts, and in the daily check:
  - **malicious**: the version is stopped. Its services are dropped, their
    accounts and data kept; each run of it is told to end on its `watch`
    connection with `Answer::Withdrawn(reason)`; its folder is made
    mode 0500, so that no person's or service's run can start. Its
    commands stay, and say why it does not run. Then bundled goes back to
    the version kept beside it (ADR 0384, with that version's data),
    without holding it, if that one is not withdrawn too. With no good version the bundle
    stays installed and stopped, until it is removed or a good version
    installs; `bundle update ID` takes one at once, ahead of the waves.
    A good version is a newer one: bundled never goes to an older version
    by itself but to the one kept, whose data that version left.
  - **broken**: bundled goes back the same way. With no good version kept,
    the version keeps running, and the next good version is taken at once,
    ahead of the waves: by itself in a place that updates by itself, else
    when the person updates.
  - A held bundle goes back, or is stopped, all the same, and stays held:
    the next good version waits for the person.
- **A run ends.** `bundle run` follows bundled on a `watch` connection for
  every run, not only for one that holds permissions. On `Answer::Withdrawn`
  it prints `bundle: ID was stopped: WHY` and ends the program and what it
  started as a revoke of its permission does: SIGTERM, and SIGKILL two
  seconds later. `bundle run` of a stopped app, and its commands, fail with
  "stopped by catalog C: REASON; remove it, or `bundle update ID` takes a
  fixed version".
- **It is never silent.** Each new withdrawal of an installed version is a
  history event `withdrawn` (`bundle history` says "VERSION was
  withdrawn: NOTE"), an audit line, a notice at the next sign-in, and a
  notice from notifyd. `bundle list` and `bundle info` say "withdrawn by
  catalog C: REASON", or for a stopped version "stopped by catalog C:
  REASON; remove it, or `bundle update ID` takes a fixed version".

## Consequences

- A catalog's keys can pull a bad version off every machine that took it,
  and stop a malicious one at once, with no wave to wait for. A feed's
  publisher can do the same for its own bundle.
- A stolen online key cannot withdraw anything, nor undo a withdrawal: it
  signs neither the targets nor the feed.
- A catalog whose keys are stolen can stop any app it lists, which it
  could already replace; the stop keeps the data, and a withdrawal is
  never undone by the catalog, so the person removes the bundle or takes
  a version that is not withdrawn.
- A machine that was off learns of a withdrawal at its next read, and acts
  on it when bundled starts.
- A machine's file grows by a line for each withdrawal and stays within
  4,096 lines.
- Nothing is deleted: a stopped version stays in its folder, its data stays,
  and a person who must can read it as root.
- The boot test `catalog-withdraw` has a test catalog withdraw a version
  of each of two bundles: a broken one goes back to the version kept,
  with its history event; a malicious one is stopped, its run ended with
  why and its next run refused. The withdrawn file is refused, and so is
  going back to it; a fixed version takes the stopped one's place. Unit
  tests cover the feed's rules.
