<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0384. Bundles update from their catalog, go back and hold

- Status: Accepted
- Date: 2026-11-03

## Context

ADR 0383 installs bundles from the catalogs an administrator trusts and
binds an ID to the catalog it came from. Nothing updated a bundle yet:
installing a newer file replaced the old one, which was gone.

The roadmap asks 13e for app updates with rollback, holds and a history,
the old version kept beside the new, and updates that run by themselves
for those who want them. An app's data is kept as each version left it,
for 30 days, so that going back brings the data back too, and that the
health check with its rollback is for service bundles.

## Decision

- **`bundle update [ID...] [--for-everyone] [--check] [--yes]`** updates
  the asker's bundles, or everyone's through elevate(8), each from the
  catalog it is bound to, to the newest version that catalog lists. A
  bundle installed from a file that no catalog listed is bound to its
  key, not a catalog, and only a newer file updates it. `--check` lists
  what would update, with the catalog, and fetches nothing.
- **An update asks only when it asks for more.** It is an install, with
  every check of ADR 0383, and the sheet is shown and a yes asked for
  only when the new version asks for a permission the old one did not,
  for more of one (more folders, more apps, other ports), or adds a
  command that runs with the person's access. The sheet says which
  version it replaces and marks each line that is new. `--yes` answers
  for the asker.
- **The version an update replaces is kept beside it**, as
  `.previous-ID` in the same place folder, sealed as the new one is, and
  recorded in the registry file `previous`. An update moves the record
  and the folder there, in place of the one kept before. The switch is
  renames in the one folder, made while bundled holds the registry's
  lock.
- **`bundle rollback ID`** goes back to the kept version, after checking
  that its folder is bundled's, sealed, and the one recorded (its key,
  its list of sums and its version). The two change places, so that
  running it again goes forward. Going back also holds the bundle.
- **`bundle hold ID`** and **`bundle unhold ID`** say whether updates
  leave a bundle as it is: the registry file `held`, with when. `bundle
  update` says which held bundles have a newer version waiting.
- **Updating by themselves is opt in**, per person and for everyone's
  bundles: `bundle update --automatic on|off`, the registry file
  `automatic`. bundled's updater thread runs ten minutes after it
  starts, then daily, and at once when someone turns it on. It updates
  as `bundle update` does, but a version that asks for more waits for a
  yes, and says so, once per version, in the log and the history. A
  held bundle is left alone.
- **An app's data is kept as each version left it.** `bundle run` works
  as the person, so it is the one that copies: before a version first
  runs for a person, it copies `~/app-data/ID` to
  `~/app-data/.snapshots/ID/VERSION` for the version that last ran, and
  writes the new one in `.ran`. Going back to an older version whose
  copy is there, the two change places, by rename, so that what the
  newer version left is kept in turn. A copy is removed after 30 days.
  `bundle remove --purge` removes them too.
- **`bundle history [ID]`** shows what bundled did: installs, updates,
  rollbacks, removals, holds and their release, updates that wait and
  updates that failed, with who asked or that it ran by itself. It is
  `/system/state/bundled/history`, mode 0600, lines of tab-separated
  fields, kept to about the newest thousand. A person sees their own
  bundles' events and everyone's; root sees all.
- `bundle info` says when a bundle can go back, and when it is held.

## Consequences

- An update that asks for nothing new needs no one, and one that asks
  for more is never granted by itself, so updating by itself never
  widens what an app may do.
- Going back is one rename each way, so it works offline and with the
  catalog gone, and the old version is checked as it was at install.
- Only one older version is kept: a second update replaces it. Version
  lines, which keep majors side by side, come with 13e's version lines.
- A data copy is made only when a person runs the app, as that person,
  so bundled never reads anyone's home. An app run only by its service
  (13e's service bundles) is copied there instead.
- A copy costs the app's data again for 30 days. The copies are the
  person's, in their home, and they may remove them.
- A run that starts while bundled is between two renames finds no
  folder and fails; it works when run again.
- The boot test `bundle-update` signs a test catalog with two versions
  of two bundles, turns updating by themselves on, checks that the one
  that asks for more waits, updates it with a yes, goes back, checks the
  data and the hold, reads the history, and updates again.
- **Not yet:**
  - The health check of service bundles, which goes back by itself when
    a new version fails it (13e's service bundles).
  - Settings files shipped as `.default`, with `settings.d` (with the
    service bundles).
  - A bundle's own update feed (13e's security pull request).
