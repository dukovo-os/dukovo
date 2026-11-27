<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0780. The system's next root is signed on the YubiKeys from the keys the source tree holds, and served with the targets signed again

- Status: Accepted; signs the root versions after the first that [0457](0457-releases-are-signed-on-two-hardware-keys-after-three-builds-agree.md) and [0538](0538-the-system-releases-first-root-is-signed-on-a-slot-of-each-yubikey-of-its-own.md) left for later, and replaces the server's online key as [0467](0467-the-update-server-serves-the-systems-releases-and-signs-only-their-snapshot-and-timestamp.md) has it
- Date: 2026-11-27

## Context

ADR 0457 drops a lost YubiKey from the system's releases by having the
other sign a new root, revokes a stolen one the same day, and signs the
root again every year, at ten months. ADR 0467 replaces a stolen online
key with the release keys' next root. All of them are a root after the
first.

The catalog signs its next root on a YubiKey with `./ob catalog root`
(ADR 0381). The system's releases had only their first: `./ob release
root` signs root version 1 in the key ceremony and refuses once it is
there (ADR 0538), and `system-release keys --rotate` and `--online` sign
the next root of a build machine's own keys, which are files. No command
could sign the production root again.

A root is not served alone. The targets that the server serves are signed
by one YubiKey; under a root that no longer names it, they do not check,
and a machine would refuse every update until the next release. The
targets also delegate the rollouts to the keys the root names for
timestamp (ADR 0469), so a replaced online key would still hold the
waves. And `ob release sign` starts from what the server serves (ADR
0778), so a root the source tree has and the server does not would be
left out of the next release.

## Decision

- **`./ob release root` signs the next root once the first is there.** It
  names the keys `releases/keys/` holds then, as the first root does:
  each YubiKey's file for all four roles, and the online key's file
  beside them for snapshot and timestamp, each role with threshold 1,
  good for a year. Taking a YubiKey's file out drops it; `./ob release
  key yubikey` adds one; a new `online.json` names a new online key; and
  with nothing changed it is the yearly root. A root names one or two
  YubiKeys: one only from the day one is lost until its replacement is
  added.
- **Who signs it.** Every YubiKey plugged in whose key either root names
  for root signs it. At least one must be the root before's, as a
  machine asks (TUF 5.3.4), and every YubiKey new to the root must be
  among them, so that the root shows it works, as the first root showed
  both. A dropped YubiKey that is still at hand signs as one of the root
  before's.
- **Shown, then typed.** It shows the version and expiry, the YubiKeys
  named with the new ones marked, the online key and whether it is new,
  each key the root before named that is dropped, the YubiKeys that will
  sign, and the targets version it will sign again. It signs once `root
  N` is typed, and each YubiKey asks for its PIN and a touch.
- **Checked as a machine checks it.** The roots in `releases/metadata/`
  must each follow the one before, from `1.root.json`; the new one must
  follow the newest, signed by its keys and by its own. It is written
  whole as `releases/metadata/N.root.json`, last, once everything else
  has been signed.
- **Served with the targets signed again.** system-release fetches what
  updates.dukovo.org serves into `out/release-root/N`, with the source
  tree's roots. The root goes into it, and its targets are signed again
  as they are, as their next version, on a YubiKey the new root names, so
  that they check under it and delegate the rollouts to the keys it names
  for timestamp. `./ob release deploy out/release-root/N` serves it; the
  server signs the snapshot and timestamp with its online key, and
  refuses a deploy whose newest root does not name that key.
- **A root the server does not serve yet goes out with the next
  deploy.** `system-release fetch --roots releases/metadata` adds the
  source tree's roots that the server does not serve, each checked to
  follow the one before, and refuses when the server serves a root the
  tree does not hold, or holds otherwise. `ob release sign` fetches so,
  and a root signed before the first release goes out with it.

## Consequences

- A lost or stolen YubiKey, a stolen online key and the yearly root are
  each one `./ob release root` and one deploy. The handbook's chapter on
  keys and incidents takes each case from the start.
- A machine takes the new root the next time it checks for updates,
  walking to it from the root it trusts. From then on it refuses what a
  dropped key signs, and the timestamps and snapshots of a replaced
  online key; a kept timestamp or snapshot that its new root's keys did
  not sign no longer counts for its rollback checks. A machine that has
  not checked yet still trusts the root before.
- Images built before the new root still trust the first root and walk
  from it once online; Rescue and the installer read their own medium
  offline from the root it was made under.
- A YubiKey's PEM file goes with its JSON file: the install images'
  checksums that a dropped YubiKey signed are signed again with another
  (ADR 0779).
- **Not yet** (`docs/deferred.md`): `ob keys ceremony`, the yearly root
  signed from a Dukovo stick with networking off, both keys checked, and
  one replaced every other year (ADR 0457).

## Alternatives

- **Signing the root alone**, and leaving the targets to the next
  release, would stop every machine's updates under a root that dropped
  the YubiKey that signed them, and keep the rollouts delegated to a
  replaced online key.
- **A command for each change**, to drop, add or replace a key: the keys
  folder already holds what the root names, and signing what it holds is
  one rule, the catalog's.
