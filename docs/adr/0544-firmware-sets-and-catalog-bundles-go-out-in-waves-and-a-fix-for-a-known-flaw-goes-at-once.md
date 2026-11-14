<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0544. Firmware sets and catalog bundles go out in waves, and a fix for a known flaw goes at once

- Status: Accepted
- Date: 2026-11-14

## Context

ADR 0469 sends each system release out in waves: the canaries on the day
a channel offers it, then machines by the number each keeps, on days 1,
3, 5 and 7, held while the release is paused or a canary is red. It left
two kinds of update out, each with a row in `docs/deferred.md`: the
firmware set a channel offers (ADR 0468), and the catalog's bundles
(ADR 0384), which bundled takes by itself each day.

A bad firmware set can stop a machine from starting; a bad bundle can
stop a server's database or web server. Both reach every machine on the
day they are offered. Yet a bundle's update is also how a known flaw in
it is fixed (ADR 0389), and a seven-day ladder must not hold a fix back
from the machines that run the flawed version.

## Decision

1. **Firmware sets.** The update server signs a rollout of each firmware
   set a channel offers, `rollout/firmware-VERSION.json`, beside the
   releases', from the same plan and by the same rollout role (ADR
   0469). Its waves are its own: a pause of the release does not hold
   the set, and `system-release rollout pause firmware-VERSION REASON`
   holds the set alone. `update` says a set waits for its wave, and does
   not stage it, until this machine's wave opens; `sysupdate firmware
   stage` without a version refuses it until then; with `--version` it
   takes it at once, as `sysupdate stage --version` does a release.
2. **Catalog bundles.** The catalog's targets delegate the paths
   `rollout/ID/VERSION` alone to the role `rollout`, held by the catalog
   server's online key, as they delegate the list of flaws. The catalog
   server keeps a plan, never served, and signs a rollout of each
   bundle's newest version at each publish and refresh, with the waves,
   days, pauses and canaries of ADR 0469; `ob catalog rollout pause ID
   VERSION REASON`, `resume ID VERSION` and `canary NAME red|green` run
   its `catalog-rollout.sh` over SSH. A catalog whose targets delegate no
   rollouts reaches every machine at once, as before.
3. **One number per machine.** bundled uses the machine's number and
   canary of ADR 0469, `/system/state/sysupdate/wave` and `canary` in
   `updates.toml`, so that a machine is in the same wave for its system,
   its firmware and its apps.
4. **Who waits.** bundled's own daily updates and `bundle update` wait
   for the wave; `bundle update ID`, which names the bundle, takes it at
   once; `bundle install` of a bundle not installed takes the newest at
   once, since waves are for updates. A bundle that updates from its own
   feed (ADR 0390) has no waves. `bundle update --check`, and so
   `update`, says which updates wait, until which day, and why the waves
   are held.
5. **A fix goes at once.** When the catalog's list of known flaws names a
   High or Critical flaw in the version a machine runs, and not in the
   newer version, the machine takes the newer version whatever its wave.
   The machine decides, from the signed list it keeps: the server cannot
   tell which machines run which version, and a machine that runs no
   flawed version keeps its wave.

## Alternatives

- **The server opens every wave of a version that fixes a flaw.** Every
  machine would take it on its first day, though most do not run the
  flawed version; and the server would need to tell a fix from a
  version that only lists fewer flaws because its components are newer.
- **A wave number of bundled's own.** A machine would then be early for
  its apps and late for its system, and two numbers would need two
  places to set a canary.

## Consequences

- A bad firmware set or bundle reaches the canaries and at most 1% of
  machines in its first two days, and a pause stops it, with no key.
- A stolen online key can hold or open the waves of bundles and firmware
  sets as it can of releases (ADRs 0456 and 0469), and nothing else: the
  role signs rollouts alone, and the bundles and sets are signed by the
  catalog's and the release keys.
- The catalog's waves begin with the next targets the catalog's keys
  sign, which carry the delegation.
- The boot tests `update-online` (a firmware set waiting for its wave,
  and offered to a canary) and `catalog-waves` (a bundle's update waiting,
  held, offered to a canary and taken when named) check it.
