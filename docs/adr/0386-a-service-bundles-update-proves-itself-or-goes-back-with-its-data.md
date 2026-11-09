<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0386. A service bundle's update proves itself, or goes back with its data

- Status: Accepted
- Date: 2026-11-03

## Context

ADR 0385 runs a bundle's services, each as an account of its own, and
writes their service files. An update wrote those files again and
restarted the services: a new version that failed stayed failed, the
data it changed was not kept as the version before left it, and an
administrator's change to a service file was lost.

The app rules of section 10 of the updates design come in 13e: a service
bundle's new version must pass its declared health check within 60
seconds of starting, or the previous version and its data snapshot come
back at once and the owner is told; and a snapshot is kept until the
next update or for 30 days, whichever is later. The roadmap adds, from
2026-10-26, that updates never ask about edited settings: the
administrator's file stays, the new default lands beside it, and our own
TOML files take override files in a `settings.d` folder, so most changes
never touch the shipped file.

## Decision

- **Override files.** Spark and `service check` read
  `/system/settings/spark/services/settings.d/NAME.toml` over
  `NAME.toml`: its keys take the place of the file's, a table's key by
  key, any other value whole (a list too). It is checked once it is set
  over the file, and an error names both. Spark keeps a good copy of it
  beside the file's (ADR 0216). An administrator changes a service this
  way without touching the file bundled or the system wrote.
- **A changed service file stays.** bundled's record of each service
  file it wrote holds a SHA-256 of it, read with its `enabled` line set
  one way, as `service enable` changes only that. An update or going
  back that finds the file changed since keeps it, writes the new one
  beside it as `NAME.toml.default`, and says so; `service check` says
  which settings differ between the two. A file no one changed is
  written anew, and a `.default` beside it removed.
- **The data is copied before each update.** bundled stops the
  services the old version ran and waits until Spark says they have
  stopped, so that the copy is whole. Then, as `bundle run` does for an
  app's data (ADR 0384), it copies each account's data folder to
  `/shared/services/ID/.snapshots/ACCOUNT/VERSION` for the version that
  ran, keeping each file's owner, and notes the new version; going back
  to a version whose copy is there, the two change places, so that what
  the newer one left is kept in turn. A copy goes once it is 30 days old,
  at the next update; `--purge` removes them with the rest.
- **Each service has a live folder**, `/system/live/services/ACCOUNT`,
  which Spark makes as the service starts, the account's, and which
  `RUNTIME_DIRECTORY` names, as systemd's does. The sockets it provides
  go there.
- **The health check.** After an update for everyone, bundled connects to
  the socket each started service's manifest names in `health`, in its
  live folder, until each answers, for up to 60 seconds in all. A
  service without one is not checked. When one does not answer, bundled
  goes back by itself, as `bundle rollback` does: the version before,
  its services' data as that version left it, and a hold, so that no
  update tries the failed version again unasked. The update fails with
  why, and `bundle history` keeps it, so the administrator is told. Going
  back by hand is not checked: that version ran before.
- **Going back for everyone brings the data back at once**, when the
  bundle has services, rather than at the next run as an app's does.

## Consequences

- An update of a service bundle that fails leaves it as it was, data
  and all, within about a minute and a half, with no one there.
- A service is down while its data is copied, as a package's daemon is
  while dpkg upgrades it. The copy is a whole copy, not a reflink yet.
- An administrator who changed a service's file keeps the old settings
  across updates until they take the new ones from the `.default`; the
  health check catches an old file that no longer fits the new version.
- A program must remove a socket a stopped version left in its live
  folder before it listens there, as on Linux; a socket no one listens
  on fails the check, so a stale one never passes it.
- The boot test `service-update` installs lighthouse, sets a greeting
  in an override file, changes the file bundled wrote, updates to a
  version that answers and then to one that never does, and checks the
  `.default`, the copies, the data that came back, the hold and the
  history.
- **Not yet:**
  - The required and wanted checks of the system's health check (15a).
  - Override files for the system's other TOML files.
  - Sockets a bundle provides to others, and `on-connection` starting a
    service at its first connection.
