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

`stand-in-root.json` is a root made once with `system-release keys`,
whose secret keys were deleted when it was made, so that no one can sign
for it and nothing that trusts it can be updated. `./ob release build`
puts it in the tree it builds, until `metadata/1.root.json` exists and is
used in its place: a tree that held a build machine's own root would differ
from the same tree built on another machine (ADR 0535). Nothing that is
shipped trusts it.

## Building a release twice

`./ob release build` builds the system as the commit makes it and prints
the SHA-256 of its release list, unsigned; CI's `reproducible.yml` runs it
twice in different places and compares (ADR 0535), and `ob release sign`
compares its own third build with the last line it printed (ADR 0457).
Two builds of one commit make the same list, so the keys sign it only
when they do.

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

## Canaries

A release's waves open on schedule only while every canary is green
(ADR 0533). The canaries are the project's own machines: each reports to
the update server every hour, and the server holds the waves of every
release and of the catalog's bundles while one is red by hand, has never
reported, has been silent for two hours, failed its health check, fell
back from an update, has a failed service or failed `dukovo-check`, or
has not taken a release its channel has offered for more than six hours.
The list of canaries is root's on the server, and `ob` changes it:

```
./ob release canaries                         # colour, why, last report of each
./ob release canaries --json
./ob release canary add NAME KEY              # KEY is what `sysupdate canary enrol` prints
./ob release canary NAME red REASON           # by hand; wins over its reports
./ob release canary NAME green REASON
./ob release canary NAME auto                 # back to its reports
./ob release canary remove NAME
```

The server signs the rollouts again only when the set of canaries that
are not green changes. With no canary enrolled, nothing is held for
canaries. `./ob release rollout canary` no longer exists, and says so.
ob(1) says more, and `tools/update-server/dukovo/README.md` says how a
Dukovo server runs the jobs.
