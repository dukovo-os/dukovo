<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# The catalog's TUF repository

What the catalog's YubiKeys sign is kept here (ADR 0381):

- `keys/`: the public half of each key, with the roles it signs for and,
  for a YubiKey's, its serial number. Secret halves are never here.
- `metadata/`: every version of root, `N.root.json`, the newest also as
  `root.json`, and the top-level targets, `targets.json`, which list each
  bundle the catalog carries by its length and SHA-256, with its ID and
  version.

Root and targets are signed offline, on the maintainer's computer, by
either of the two YubiKeys alone. Snapshot and timestamp, which only say
which targets are current and that they still are, are signed on the
update server by its own key. `./ob catalog` does all of it; ob(1) says
how.

## Setting up a YubiKey

Once for each of the two YubiKeys, on a computer running Debian, Ubuntu or
Linux Mint, one YubiKey plugged in at a time. This uses the YubiKey's PIV
application only, so its sign-ins (FIDO2, and the codes of Yubico
Authenticator) are not touched.

1. Install Yubico's tool: `sudo apt install yubikey-manager`.
2. `ykman info` shows its serial number. Write it on a label.
3. Set a PIN and a PUK of your own: `ykman piv access change-pin`
   (the factory PIN is 123456) and `ykman piv access change-puk` (the
   factory PUK is 12345678). Three wrong PINs lock the key until the PUK
   unlocks it; keep the PUK somewhere other than the YubiKey.
4. Replace the factory management key with a random one the YubiKey keeps
   behind the PIN: `ykman piv access change-management-key --generate
   --protect` (the factory one is 010203…08, three times).
5. Make the signing key on the YubiKey, so that its secret half never
   leaves it, asking for the PIN once a session and a touch for every
   signature: `ykman piv keys generate --algorithm ECCP256 --pin-policy
   ONCE --touch-policy ALWAYS 9c -`.
6. From the top of the source tree: `./ob catalog key yubikey`. It adds
   `keys/yubikey-SERIAL.json`, the key's public half, for root and
   targets.

The update server's key, for snapshot and timestamp, is made on the
server itself, and its public half added to `keys/` the same way, with
`./ob catalog key new`.

## The first root

With both YubiKeys plugged in, `./ob catalog root` signs root version 1
with each, a PIN and a touch for each. Commit `keys/` and `metadata/`.
The second YubiKey then goes to its safe place; either one alone signs
from then on.

## Signing a batch

`./ob catalog sign` packs the catalog's bundles into `out/bundles/`, as
`./ob ports` does, then lists what would change, asks, and signs the next
targets: one PIN and one touch for the whole batch. Commit
`metadata/targets.json`. Targets are good for half a year, so sign at
least that often, even when nothing has changed; `./ob catalog status`
warns a month before.

## When a YubiKey is lost

Plug in the other one, take the lost key's file out of `keys/`, add a
new YubiKey's with `./ob catalog key yubikey`, and sign the next root
with `./ob catalog root`: clients take a new root only when the old
root's keys sign it, so the lost key can sign nothing more once they
have it.
