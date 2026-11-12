<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0454. Settings follow the two-version rule, and come back only after a fallback

- Status: Accepted
- Date: 2026-11-11

## Context

A system update replaces one thing, the system slot (ADR 0451). The
settings in `@settings` and the state in `@state`, logs included, stay,
and both versions must read them: the new one when it starts, and the
version before when the new one falls back, or when the owner rolls back
days later. Two ways were weighed:

- **Up and down migration scripts for every change.** Each release
  would carry code to undo its own changes, which is tested least where
  it matters most.
- **A rule on what a release writes,** so that the version before can
  always read it, with snapshots for the one case the rule cannot cover:
  an update that never proved itself and may have written anything.

The manifest already holds versioned standards to such a rule (13d),
and edited settings are already never overwritten: the administrator's
file stays and a new default lands beside it as `.default` (ADR 0386).

## Decision

- **The two-version rule.** Each release reads every older format of
  the settings and state it owns, and writes only what the release
  before it can read. A change that cannot follow the rule, such as a
  renamed file, takes two releases: release N writes the new file beside
  the old one and keeps both in step, and a later release drops the old
  one once N-1 can no longer be the previous version. A jump over
  several releases runs each release's migrations in order.
- **Every migration ships with a test in both directions:** old files
  in, the expected files out, and the previous release reading them.
- **Snapshots before every stage.** `sysupdate stage` snapshots
  `@settings` and `@state`, read-only, into `@snapshots/update`, with a
  note of the versions and the slot, replacing the snapshots of the
  update before.
- **A fallback puts the settings back.** When the default starts with
  `fallback=`, the update never proved itself. Spark, before any service
  starts:
  1. keeps the failed attempt's settings as
     `@snapshots/update/settings-failed`, for a look at what went wrong;
  2. makes `@settings` a writable snapshot of the settings from before
     the update;
  3. marks the update's folder `restored`.

  Each step is decided afresh from what is there, so a power cut between
  any two leaves steps that run again at the next start and end the
  same. The state is not put back: it holds the logs, and the
  two-version rule keeps it readable.
- **A rollback by hand keeps today's settings.** The version before can
  read them under the rule, so nothing the owner changed since the
  update is lost. The rule covers one step: when the update server
  offered a release between the two versions, `sysupdate rollback` stops
  and asks for `--keep-settings` or `--settings-from-before`. The
  latter writes the slot it rolls back to into
  `@snapshots/update/settings-asked`, and Spark, at that slot's next
  start, puts the settings back as after a fallback, keeping today's as
  `settings-failed`. Only the version the update came from has the
  snapshot, so only it can take them.
- **The snapshots go with the slot.** They are kept until the next
  stage replaces them, as the version before is.

## Consequences

- A rollback days after an update undoes nothing the owner did, and a
  fallback undoes whatever the failed update wrote. The boot test
  `update-fallback` changes a setting after staging a broken update, and
  checks that the fallback takes the change back and keeps it in
  `settings-failed`.
- A format change costs two releases instead of one. Releases come
  monthly (ADR 0458), so the old file is dropped a month later at the
  soonest.
- The update snapshots share blocks with the live subvolumes, so they
  cost only what changes after the stage.
- **Not yet:**
  - No release has changed a settings format yet, so no migration
    exists to test; the first one brings the runner and its tests.
  - Core apps' data in `~/app-data` follows the same rule with the first
    core app that changes its format.
