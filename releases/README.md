<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# The system releases' public keys and first root

The public half of the system's release repository is kept here (ADRs
0457, 0466, 0467 and 0538):

- `keys/online.json`: the update server's online key for the system's
  releases, which signs their snapshot and timestamp. `./ob release
  server setup` writes it the first time it sets the server up; its
  secret half never leaves the server.
- `keys/yubikey-SERIAL.json`: the public half of the release key in slot
  95 of each of the two YubiKeys, which `./ob release key yubikey` writes.
  Slot 9c of the same YubiKeys holds the catalog's key; the two are kept
  apart so that nothing signed for one repository is taken by the other.
- `metadata/1.root.json`: the first root of the releases the update
  server serves, signed on both YubiKeys in the key ceremony. `./ob
  release deploy` deploys only a repository that starts from it.

Until the ceremony, every build machine signs its releases with keys of
its own in `.cache/release-keys`, and nothing it signs is deployed.

`stand-in-root.json` is a root made once with `system-release keys`,
whose secret keys were deleted when it was made, so that no one can sign
for it and nothing that trusts it can be updated. `./ob release build`
puts it in the tree it builds, until `metadata/1.root.json` exists and is
used in its place: a tree that held a build machine's own root would differ
from the same tree built on another machine (ADR 0535). Nothing that is
shipped trusts it.

## The key ceremony

Once, on the maintainer's computer running Linux Mint, Debian or Ubuntu,
with the two YubiKeys that already hold the catalog's keys
(`catalog/README.md`). It takes about fifteen minutes. Nothing secret is
typed anywhere but the YubiKeys' own prompts, and no secret half leaves a
YubiKey.

1. Set up the server's key for the system's releases, if
   `keys/online.json` is not here yet: `./ob release server setup`. It
   asks for your password on the server, and writes `keys/online.json`.
2. With one YubiKey plugged in, check that slot 95 is free: `ykman piv
   keys info 95` must fail, saying the slot holds no key. (`ykman piv
   info` is no help here: it lists only slots that hold a certificate,
   and none of these keys has one.)
3. Make the release key on it, so that its secret half never leaves it,
   asking for the PIN once a session and a touch for every signature:
   `ykman piv keys generate --algorithm ECCP256 --pin-policy ONCE
   --touch-policy ALWAYS 95 -`. It asks for the PIN, which unlocks the
   management key the YubiKey keeps behind it.
4. `./ob release key yubikey`. It adds `keys/yubikey-SERIAL.json`.
5. Unplug it, and do steps 2 to 4 with the other YubiKey.
6. Plug both in, and run `./ob release root`. It signs root version 1
   with each, a PIN and a touch for each, writes `metadata/1.root.json`,
   and prints the root's SHA-256 last.
7. Commit `keys/` and `metadata/`. Keep the SHA-256 it printed: it is
   the root's fingerprint, by which anyone can check the root an
   install trusts (ADR 0457).
8. The second YubiKey goes back to its safe place; either one alone
   signs from then on.

The root is good for a year: sign the next one at ten months. A touch
that times out fails with `SW=0x6982`; run the step again. Touching the
wrong YubiKey types a one-time code into the terminal instead of
signing; clear the line and touch the one that asked.

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
