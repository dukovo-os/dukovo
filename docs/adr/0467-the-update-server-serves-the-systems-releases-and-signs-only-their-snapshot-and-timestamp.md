<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0467. The update server serves the system's releases and signs only their snapshot and timestamp

- Status: Accepted
- Date: 2026-11-12

## Context

ADR 0466 made the update repository: every release a server offers,
the channels that name one each, and every root by version. A
timestamp is good for a day and a snapshot for a week, so the server
that serves the repository must sign them again every few hours, with
nobody there.

ADR 0457 keeps every key that can sign a release off servers and out of
CI: root and targets live on the two hardware keys. The catalog already
splits its signing this way (ADRs 0381 and 0382): root and targets are
signed on the signer's computer, and the update server signs the
snapshot and timestamp with an online key that never leaves it. The
system's releases are served from the same server, updates.dukovo.org.

The nightly channel (ADR 0456) would be signed every merge, which no
person with a hardware key does. How it should be signed is not decided
yet, so the server offers no nightly release for now.

## Decision

- **The server has an online key of its own for the system's releases**,
  made there by `system-release online-key`, readable by one system
  user, `dukovo-system`, and never copied off. Its public half goes to
  the source tree as `releases/keys/online.json`, and the release keys'
  next root names it for snapshot and timestamp beside their own keys,
  each role with a threshold of one (`system-release keys --online`).
  So install sticks and update files, signed offline, still check, and
  the server signs its snapshots and timestamps alone. The online key
  signs no release and no channel: those are targets.
- **The server takes from a deploy only what the release keys signed.**
  `system-release publish`, run when a deploy says it is ready, takes:
  - the numbered roots and targets, `N.root.json` and `N.targets.json`,
    and nothing else of the deploy's metadata: the snapshot and
    timestamp are the server's own;
  - the release lists, the channels and the files, each named by its
    SHA-256 and checked against it as it is copied. None is ever
    replaced: a file already there with other bytes is refused.
  A first root other than the one the server first published is
  refused, so every later root, and every targets, must follow from it.
  Then it signs a snapshot and timestamp, and reads every release list
  and channel back with the TUF client from the first root, expiry
  checked, as a new computer would. Every file each release lists must
  be in the store at its size, and every channel must offer a release
  the repository offers.
- **What is served changes in one rename.** Each publish and refresh
  writes into a copy of the served repository made of hard links, and
  system-release writes every file anew and renames it into place, so
  the files the two share never change. The link
  `/srv/dukovo-system/system` is then pointed at the copy; a repository
  that fails its check is never served. Caddy serves it as `/system/`.
- **A timer signs again every six hours:** a timestamp alone while the
  snapshot lists the newest targets, is good for two more days and no
  new root has been deployed; a snapshot and a timestamp otherwise.
- **`ob release deploy` deploys, over the maintainer's own SSH login**,
  as `ob catalog deploy` does. It copies an update repository's
  metadata, targets and files to `/var/lib/dukovo-system-incoming`,
  skipping files the store holds already, and waits for the server's
  result; then `ob release check` reads the releases served over https,
  with `system-release served`, from the first root. A repository whose
  first root is not `releases/metadata/1.root.json`, the root the key
  ceremony signs on the hardware keys (ADR 0457), is not deployed: until
  that ceremony, nothing a build machine signed reaches the server.
- **One setup for the server.** `ob catalog server setup` and `ob release
  server setup` copy `tools/update-server/`, catalog-repo and
  system-release, and run `setup.sh`, which sets up both the catalog's
  side and the system's: its own user, key, folders, publish unit and
  refresh timer each.
- **Stable and beta only.** The server offers the stable and beta
  channels. A machine may choose nightly, and is told the server has no
  such channel. How nightly releases are signed is decided before
  1.0.0-alpha.1.

## Consequences

- A stolen online key can sign snapshots and timestamps, so it can keep
  serving an old set of releases as current; it cannot add a release, a
  channel move or a file. The release keys' next root replaces it, and
  `setup.sh` makes a new one once the old file is removed.
- The tests run the server's own scripts. The boot tests' update server
  is published by `system-publish.sh` and signed again by
  `system-refresh.sh`, in a folder standing in for the server's `/`; a
  deploy with a file that is not what its name says is refused there,
  and what was served before stays served. The machine walks from the
  root the build trusts to the third: the second names the server's
  key, and the third replaces the release keys' own timestamp key.
- A deploy carries only the files the server lacks, but each one is
  hashed as the server takes it, so a deploy of a new release waits up
  to ten minutes for the result.
- **Not yet:**
  - The nightly channel, and how its releases are signed.
  - The production first root, signed in the key ceremony (ADR 0457).
  - The online keys kept encrypted at rest, as ADR 0457's table has
    them: they are files only their system users read, as the catalog's
    has been since ADR 0382.

## Alternatives

- **The server signing targets too**, with a key of its own, would
  serve a nightly release every merge with no person there, but a
  stolen server could then publish a release to every machine. ADR 0457
  rules that out.
- **The deploy carrying the snapshot and timestamp**, signed on the
  maintainer's computer, needs no key on the server, but a timestamp is
  good for a day: someone would have to sign one every day.
- **One online key for the catalog and the system's releases** is one
  key fewer to keep, but a server user that publishes one could then
  sign for the other.
