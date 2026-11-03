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
update server by its own key (ADR 0382). `./ob catalog` does all of it;
ob(1) says how.

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

## Setting up the update server

Once, from the same computer, before the first root, since the root
lists the server's key. The server is updates.dukovo.org, reached over
SSH with your own login there; if that is not the name you use with
`ssh`, set `OB_UPDATE_SSH`, such as `OB_UPDATE_SSH=you@updates.dukovo.org`.

1. Open ports 80 and 443 to everyone in the hosting provider's firewall
   (on DigitalOcean: Networking, Firewalls), for the certificate and for
   clients. SSH stays as it is.
2. `./ob catalog server setup`. It copies the server's files there and
   runs their setup through `sudo`, which asks for your password on the
   server. It installs Caddy, which serves the catalog over https, makes
   the server's own key for snapshot and timestamp, and lets your login
   deploy.
3. It writes the key's public half to `keys/online.json`. Commit it.

Run it again whenever `tools/update-server/` or catalog-repo changes;
the key is kept.

## The first root

With both YubiKeys plugged in and `keys/online.json` in place,
`./ob catalog root` signs root version 1 with each, a PIN and a touch for
each. Commit `keys/` and `metadata/`. The second YubiKey then goes to its
safe place; either one alone signs from then on.

## Signing a batch

`./ob catalog sign` packs the catalog's bundles into `out/bundles/`, as
`./ob ports` does, then lists what would change, asks, and signs the next
targets: one PIN and one touch for the whole batch. Commit
`metadata/targets.json`. Targets are good for half a year, so sign at
least that often, even when nothing has changed; `./ob catalog status`
warns a month before.

## Publishing

`./ob catalog deploy` copies `keys/`, `metadata/` and the bundles just
signed to the server, which signs a snapshot and timestamp, checks the
whole catalog as a new computer would, and only then serves it. It says
whether that worked; when it did not, the server keeps serving what it
served. Then it reads the catalog back from https://updates.dukovo.org.
`./ob catalog check` does that last part alone, at any time.

The server signs a new timestamp every six hours by itself. Keep the
bundles in `out/catalog/store/` until they are deployed: they exist
nowhere else.

## When the server or its key is lost

Remove `/var/lib/dukovo-catalog/online.key` on the server, or start a
new server, and run `./ob catalog server setup`. It makes a new key and
stops, because `keys/online.json` is the old one: replace that file with
the new key, from `/usr/local/share/dukovo-catalog/online.json` on the
server, sign the next root with a YubiKey, and deploy. Clients stop
trusting the old key once they read that root.

## When a YubiKey is lost

Plug in the other one, take the lost key's file out of `keys/`, add a
new YubiKey's with `./ob catalog key yubikey`, and sign the next root
with `./ob catalog root`: clients take a new root only when the old
root's keys sign it, so the lost key can sign nothing more once they
have it.
