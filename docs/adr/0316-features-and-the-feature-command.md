<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0316. Features and the feature command

- Status: Accepted
- Date: 2026-11-08

## Context

The plan's *Install profiles and features* (Chase, 2026-10-30) lets the
person decide what each machine does: features are named groups such as
Web server, Databases or File sharing, profiles are starting sets of
them, and nothing is locked in, since the `feature` command adds and
removes them at any time. Phase 14 PR 5c builds features and the
command; 5d builds the installer's profile step on top.

What a feature is made of already exists: services that Spark runs from
their files (ADR 0216), with the ports each listens on in its file, so
the firewall opens them only while it runs (ADR 0334); and bundles that
bundled(8) installs from the catalogs an administrator trusts (ADR
0383).

## Decision

- **A feature is a file of the signed system**,
  `/system/resources/features/ID.toml` (feature.toml(5)): its name, a
  sentence about it, the services it turns on and the bundles it
  installs. Being in the system, it is signed with the release (ADR
  0308) and changes only with an update; no second signature is needed.
- **No ports.** A service's file already names the ports it listens on,
  and the firewall opens them while it runs, so adding a feature changes
  no firewall rule and removing it leaves none behind.
- **No record of what was added.** A feature is *added* when each of its
  services starts at boot and each of its bundles is installed for
  everyone; *partly* when some are, as after `share off`. The state can
  never disagree with the system, whatever changed it.
- **`feature add`** installs the missing bundles through bundle(1)'s
  own path, with their install sheets unless `--yes`, then turns the
  services on, now and at each boot, as `service NAME enable` does.
- **`feature remove`** turns the services off and removes the bundles,
  but those another feature still needs: one added, or partly added, by
  a part of its own, not only by the parts it shares with the feature
  removed. Bundles keep their settings and data unless `--purge`, as
  `bundle remove` does.
- **The first feature is File sharing** (smbd and nfsd), whose parts are
  in the base system. Web server, Databases, Containers, Mail and
  Monitoring follow as their ports become bundles in the catalog: a
  feature naming bundles that do not exist yet would only fail.

## Consequences

- `feature list`, `info`, `add` and `remove`, with feature(8) and
  feature.toml(5). The boot test `feature` adds File sharing, sees smbd
  and nfsd running, and removes it; the lab test
  `services/features` does the same on a lab machine.
- The installer's profile step (5d) reads the same files, and the web
  console's Features page (16c) runs the same command.
- A feature's bundles install from the network until the stick carries
  a catalog of its own.

## Alternatives considered

- **A state file of added features**, as dnf's groups keep. It drifts
  from the system as soon as a service is turned off by hand.
- **Signed feature files apart from the system**, fetched from the
  catalog. Features then change between updates, behind the system's
  back; the catalog can carry them later if that is ever needed.
