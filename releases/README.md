<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# The system releases' public keys and first root

The public half of the system's release repository is kept here (ADRs
0457, 0466 and 0467):

- `keys/online.json`: the update server's online key for the system's
  releases, which signs their snapshot and timestamp. `./ob release
  server setup` writes it the first time it sets the server up; its
  secret half never leaves the server.
- `metadata/1.root.json`: the first root of the releases the update
  server serves, signed on the two hardware keys in the key ceremony
  (ADR 0457). `./ob release deploy` deploys only a repository that
  starts from it.

Neither is here yet: the server is set up for the system's releases
before the key ceremony, and the ceremony comes before 1.0.0-alpha.1.
Until then, every build machine signs its releases with keys of its own
in `.cache/release-keys`, and nothing it signs is deployed.

## Setting up the update server

Once, from the maintainer's computer, the same way as for the catalog
(`catalog/README.md`, which says how to open the server's ports and
reach it over SSH):

1. `./ob release server setup`, which is `./ob catalog server setup`:
   it updates the server's files and programs, and makes the online key
   for the system's releases the first time.
2. It writes the key's public half to `keys/online.json`. Commit it.

Run it again whenever `tools/update-server/`, catalog-repo or
system-release changes; the keys are kept.

## Naming the online key in a root

The root names the online key for snapshot and timestamp beside the
release keys' own, either of which signs alone, so that install sticks
and update files, signed offline, still check:

```
system-release keys --online releases/keys/online.json KEYS
```

signs the next root from the keys in `KEYS`. A deploy whose newest root
does not name the server's key is refused by the server, which says so.

## Deploying

`./ob release deploy REPOSITORY` copies an update repository made with
`system-release add` and `channel` to the server, which publishes it and
says whether that worked; then `./ob release check` reads what it serves
from `metadata/1.root.json`. ob(1) says more.

## A lost or stolen online key

Remove `/var/lib/dukovo-system/online.key` on the server and run
`./ob release server setup` again: it makes a new key, and stops because
`keys/online.json` is not the new one. Replace `keys/online.json` with
`/usr/local/share/dukovo-system/online.json` from the server, sign a
root that names it, and deploy.
