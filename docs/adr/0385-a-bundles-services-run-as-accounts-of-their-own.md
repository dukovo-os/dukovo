<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0385. A bundle's services run as accounts of their own

- Status: Accepted
- Date: 2026-11-03

## Context

A bundle's manifest may declare services (ADR 0221): long-running
programs, with a service file's settings, an account, when they start and
how to tell they work. Nothing ran them yet. The roadmap asks 13e for
service bundles that Spark runs under the bundle's grants, with a restart
policy and resource limits, updating through the catalog like any app
without touching /system, and Phase 13's exit asks that a sample service
bundle install, start at boot and be started again after a crash. 13d put
a service's data in `/shared/services/ID`.

Spark ran every service as root, without root's bypass right, in a view
of what its file declares (ADR 0217). A bundle's service is a
publisher's program, not the system's: it should not act for root at
all, as a Debian package's daemon runs as a system user of its own.

## Decision

- **Services run for everyone.** A bundle with services installs only
  for everyone, through elevate(8); `bundle install` for oneself refuses
  it and says so. Its services run for no one, so a person's
  permissions (their folders, their home, their drives, other apps'
  data, serial ports, running with their access) never reach them.
- **Each service runs as an account of its own**: the one its manifest
  names, or the service's name. bundled(8) asks authd(8) for it as it
  installs the bundle, with a new root-only request, `service`: a number
  from 100 to 999, below people's, a group of its own, a password entry
  that is disabled, no shell (`/system/commands/false`), and its data
  folder as its home. authd removes it as it removes any account, and a
  service's account may be removed by root alone. An account of that
  name with that home is taken up again; any other account of that name
  stops the install.
- **Its data folder is `/shared/services/ID/ACCOUNT`**, the account's
  alone (mode 0700), in `/shared/services/ID`, root's. Two services that
  name one account share it.
- **Spark gains `account`** in service.toml(5). A service with one gets
  Spark's root stamped with the account's name (ADR 0214), and its
  process named so: its program is opened as the account, its view is
  built from the stamped root, so each place in it is opened as the
  account and the owners and modes of the files narrow it as they would
  a person, and what it makes is the account's. Its `/tmp`, the places it
  may change that Spark makes for it, and its secrets are given to the
  account. Root's own number is refused, as is `account` beside
  `runs-with-your-access`.
- **bundled writes each service's file**,
  `/system/settings/spark/services/NAME.toml`, as service files are
  written, checked as Spark reads it: the program in the bundle's sealed
  folder, its arguments, environment (with `HOME`, `USER` and `LOGNAME`
  the account's), restart, limits and secrets as the manifest gives them,
  order 70, after the system's own, and what it may do: read the bundle's
  folder, read and change its data folder, and of what the bundle asks
  for and the program keeps, connect out (`network.outbound` or
  `network.local`), listen on the ports of `network.listen`, and the
  folders of `/shared` that `files.shared` names, read-only or not as
  asked. An administrator's yes to the install sheet gives these; the
  sheet lists each service with its account and when it starts. bundled
  keeps a record of each file it wrote in `/system/state/bundled/services`;
  a file no record names is the system's or an administrator's, and no
  bundle's service takes its name.
- **When it starts**: `at-boot` is enabled, and Spark starts it at once;
  `when-enabled` waits for `service NAME enable`, which the install's
  message says. `on-connection` starts at boot for now. An update or a
  rollback writes the files again, keeps what an administrator enabled or
  disabled, starts again each enabled service, takes away the services
  the version before ran that this one does not, and counts a service the
  version before did not run as asking for more (ADR 0384).
- **Removing** stops each service and takes its file and record away.
  The accounts and data folders stay, so that installing the bundle again
  takes them up; `bundle remove --for-everyone --purge` removes them too.

## Consequences

- A bundle's service acts for an account no person or other service
  has: it reaches only what its view holds, and of that only what the
  owners and modes let its account, so the data of another service, the
  system's state and people's homes stay out of its reach, as on Linux
  with a daemon's own user.
- An account's number is retired when it is removed (ADR 0220), so a
  purged service's files never become another's. There are 900 numbers;
  a removal without `--purge` keeps the account and its number.
- A person cannot run a service of their own: their bundles have
  commands, which run while they do.
- The boot test `service-bundle` refuses an install for oneself,
  installs beacon for everyone through elevate, checks its account, that
  it runs as it in its data folder, which Alice cannot read, that Spark
  starts it again after it crashes and at the next boot, and that
  `--purge` removes its account and data.
- **Not yet:**
  - The health check, which goes back by itself when a new version
    fails it, and the copies of a service's data before an update (13e's
    service updates).
  - Settings files shipped as `.default` beside the administrator's, and
    `settings.d` (13e's service updates).
  - Sockets a bundle provides to others, and `on-connection` starting a
    service at its first connection; `needs`, which waits for interfaces;
    and `open_files`, which Spark does not set yet.
