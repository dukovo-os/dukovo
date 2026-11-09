<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0382. The update server publishes what the signer deploys over SSH

- Status: Accepted
- Date: 2026-11-03

## Context

ADR 0381 signs the catalog's root and targets offline, with a YubiKey,
and its snapshot and timestamp with an online key on the update server.
A timestamp is good for a day, so the server must sign one every few
hours, with nobody there.

The server is updates.dukovo.org: one small virtual machine, running
Ubuntu, set up with a personal administrator login, an SSH key, and no
root login.

13e's plan had CI deploy to it with a key of its own. But what a deploy
carries exists only on the signer's computer:

- Targets list each bundle's SHA-256, and the bundles are built and
  signed there. A build on CI is not yet the same bytes.
- Root and targets are signed there, with the YubiKey plugged in.

A key on CI that can write the server would be a secret to keep and
rotate, and it would bring nothing that is not already on the signer's
computer.

## Decision

- **The server serves static files over https with Caddy**, from
  Ubuntu's own packages:
  - Caddy gets and renews its certificate from Let's Encrypt itself.
  - Metadata named by version and targets named by hash may be kept by
    caches for a year. `timestamp.json` and `root.json` may not.
  - Strict Transport Security, and no type sniffing.
- **`ob catalog server setup` sets the server up, over the signer's own
  SSH login.** It copies `tools/update-server/` and catalog-repo, built
  for release on the signer's computer, and runs `setup.sh` through
  `sudo`. `setup.sh` can run again, to update the server:
  - It installs Caddy and rsync.
  - It makes a system user, `dukovo-catalog`, that alone can read the
    online key. The key is made there, by catalog-repo, the first time,
    and never leaves.
  - It puts that login in a group, `dukovo-deploy`, that may write one
    folder, `/var/lib/dukovo-catalog-incoming`, and nothing else of the
    catalog's.
  - It installs the units that publish and refresh.

  `ob` then copies the online key's public half to
  `catalog/keys/online.json`, for the next root to list. If a different
  one is there, it stops: a root must list the new key first.
- **`ob catalog deploy` deploys, over the same login.** It copies
  `catalog/keys/`, `catalog/metadata/` and the signed bundles to the
  incoming folder and leaves a file, `ready`, there.
  - A systemd path unit sees `ready` and runs `publish.sh` as
    `dukovo-catalog`, with no network and nothing else writable.
  - `publish.sh` makes the repository anew from the public keys and
    metadata it was given, with the server's own online key. It adds new
    bundles to its store and never replaces one.
  - It refuses a first root that is not the one it published before.
    Every later root must follow from that one, and every targets from
    the roots, so only what a YubiKey signed is ever published, even by
    a login that may deploy.
  - catalog-repo signs a snapshot and a timestamp and lays the catalog
    out in a copy, made of hard links, of the folder being served. It
    reads the copy back as a new computer would, from the first root.
    Only then is the copy served: `/srv/dukovo/catalog` is a link, and
    pointing it at the copy is one rename. A client sees the old
    catalog or the new, never part of each.
  - If anything fails, the server keeps serving what it served, from
    the repository it came from.
  - What happened is written to a file, `result`, which `ob` waits for
    and shows. `ob` then reads the catalog back over https, from the
    source tree's first root.
- **A timer runs `refresh.sh` every six hours.** catalog-repo refresh
  signs a timestamp alone while the snapshot is good for two more days,
  and the served root and snapshot are the repository's. Otherwise it
  signs everything again, as a publish does. It too writes a copy and
  serves it once it reads back. Publish and refresh take a lock, so one
  runs at a time.
- **`ob catalog check`** reads any served catalog as a client would,
  by default the update server's.

## Consequences

- No secret is kept on CI, and the server has no key that can say what
  the catalog carries.
- A stolen server, or a stolen login that may deploy, can stop updates
  until the timestamp expires, a day later. It cannot make a client
  take a bundle a YubiKey did not sign, as ADR 0381 says.
- A lost or stolen online key: run `setup.sh` again after removing it,
  and sign a root that lists the new one. `ob catalog server setup`
  says so.
- catalog-repo is built on the signer's computer and runs on the server,
  so the server's C library must be as new as that computer's: Ubuntu
  24.04 or later for Linux Mint 22.
- The tests check catalog-repo's refresh and its reading of a served
  catalog. `setup.sh`, `publish.sh` and `refresh.sh` were tried with a
  stand-in for SSH and systemd, through deploys that worked, a tampered
  targets and a different first root.
- **Not yet:**
  - Mirrors, or a content delivery network, in front of the server.
  - The catalog's root in the image, so that Dukovo's own catalog
    command trusts it (13e's next pull request).
  - The system's updates, served the same way (15a).
