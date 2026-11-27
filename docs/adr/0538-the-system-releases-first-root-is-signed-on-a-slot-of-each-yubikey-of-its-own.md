<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0538. The system releases' first root is signed on a slot of each YubiKey of its own

- Status: Accepted; the roots after the first by [0780](0780-the-system-s-next-root-is-signed-on-the-yubikeys-from-the-keys-the-source-tree-holds-and-served-with-the-targets-signed-again.md)
- Date: 2026-11-17

## Context

ADR 0457 signs the system's releases on the same two YubiKeys as the
catalog, either signing alone, and leaves the first root of the system's
releases to a key ceremony before 1.0.0-alpha.1. Until that root exists,
`ob release deploy` refuses, and every build machine signs with keys of
its own (ADR 0308).

The catalog's key is the P-256 key in each YubiKey's slot 9c (ADR
0381). Were the system's root to name the same keys, root metadata
signed for one repository would carry valid signatures for the other: a
catalog root, served in place of a system root by whoever controls the
server, is signed by keys the system's root trusts for root, and a
machine would take it as its next root. Separate keys keep each
repository's signatures its own. A YubiKey's PIV application has room:
twenty retired key slots, 82 to 95, take signing keys as 9c does.
age-plugin-yubikey, whose keys unwrap the security team's key that
problem reports are encrypted to (ADR 0531), takes the first free one
from 82 upward.

## Decision

- **Keys.** Each YubiKey holds the system releases' key in slot 95, an
  ECDSA P-256 key made on the YubiKey with a PIN once a session and a
  touch for every signature, as the catalog's in 9c is. The public half
  of each is `releases/keys/yubikey-SERIAL.json`, with its slot; the
  YubiKey helper, `tools/yubikey-piv.py`, takes the slot after the
  serial number, 9c by default, and refuses any other than 9c and 95.
- **The first root** names both YubiKeys' keys for root, targets,
  snapshot and timestamp, and the update server's online key beside
  them for snapshot and timestamp, each with threshold 1. The server
  refreshes snapshot and timestamp; an install stick or an update file,
  made offline, has them signed on a YubiKey. It is good for a year
  (ADR 0457), with consistent snapshots, as the build machines' roots.
- **The ceremony** is two commands: `ob release key yubikey`, once for
  each YubiKey, and `ob release root`, with both plugged in.
  `ob release root` is system-release's `first-root`: it refuses unless
  both YubiKeys' files and the online key are in `releases/keys/`, both
  YubiKeys are plugged in and hold the keys their files name, and
  `releases/metadata/1.root.json` is not there yet. Both sign, though
  either alone would do, so that the ceremony proves each works. It
  checks each signature as a machine would, writes the root whole under
  another name before it takes its own, and prints its SHA-256 last.
  `releases/README.md` is the runbook.
- **One signer.** The code that signs through the helper is
  system-release's, and catalog-repo signs the catalog through it too.

## Consequences

- Once `releases/metadata/1.root.json` is committed, `ob release build`
  puts it in the tree it builds in place of the stand-in root, `ob
  release deploy` and `check` read from it, and every image built after
  that trusts it.
- The YubiKeys' 9c keys sign only the catalog, and their 95 keys only
  the system's releases.
- Releases, channels, advisories and pulls are signed on the YubiKeys
  through the same helper (ADR 0778).
- **Not yet:** the runbooks for lost or stolen keys, before
  1.0.0-alpha.1; root versions after the first and `ob keys ceremony`
  for the yearly root, before 1.0.0, as the first root expires on
  2027-11-17 (`docs/deferred.md`).
