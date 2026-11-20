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

Every image built after that carries `metadata/1.root.json` as the root
of the `dukovo` catalog, among its default settings, so `bundle search`
and `bundle install` read the catalog from it (ADR 0383).

## Signing a batch

`./ob catalog sign` packs the catalog's bundles into `out/bundles/`, as
`./ob ports` does, then lists what would change, asks, and signs the next
targets: one PIN and one touch for the whole batch. Commit
`metadata/targets.json`. Targets are good for half a year, so sign at
least that often, even when nothing has changed; `./ob catalog status`
warns a month before.

The PIN is asked for on the terminal. With `DUKOVO_PINENTRY` set, or with
no terminal attached, `tools/yubikey-piv.py` asks through pinentry, the
desktop's own PIN window, instead (`sudo apt install pinentry-gnome3` on
Linux Mint); `DUKOVO_PINENTRY` may name the pinentry program. Dukovo Ops
sets it, so that no PIN passes through the app.

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

## Waves

The newest version of each bundle goes out to computers in waves (ADR
0544), as a system release does: the project's canary computers on its
first day, then on days 1, 3, 5 and 7 the computers numbered below 10,
100, 500 and 1000. The targets delegate the role `rollout` to the
server's key for the paths `rollout/ID/VERSION.json`, which the next
`./ob catalog sign` puts in; until then no rollout is signed and every
computer takes its updates at once. The server signs each rollout when
it publishes and every six hours, keeping its plan in
`/var/lib/dukovo-catalog/rollout.json`, which is never served. A version
counts from the day the server first sees it; the versions listed when
the server first signs rollouts have every wave open, since every
computer could take them already.

If a new version misbehaves, hold its waves, with a reason computers
show, and free them when it is fixed or known to be fine:

```
$ ./ob catalog rollout pause io.github.burntsushi.ripgrep 15.3.0 "crashes on start"
$ ./ob catalog rollout resume io.github.burntsushi.ripgrep 15.3.0
```

It asks for your password on the server, and needs no YubiKey. A
computer running a version with a known high or critical flaw takes the
version that ends it at once, whatever its wave, and `bundle update
ID` takes any version at once. Paused waves do not take back what the
computers reached already: `bundle rollback` does that, one computer at
a time.

### Canaries

The canaries (ADR 0533) are the project's own computers that take a new
version first. Each reports to the update server every hour, and the
server holds every wave not yet open, for the newest version of every
bundle and for every system release, while one is not green: it is red
by hand, has never reported, has been silent for two hours, failed its
health check or fell back from an update, `dukovo-check` found failures
or a service failed. A canary that has not taken a newer system release
holds the system's waves only, not the catalog's. The rollout then says
`canaries not green: NAME`, and `bundle update --check` shows it. The list
is the same for both, and `ob release` changes it:

```
$ ./ob release canaries
$ ./ob release canary add my-canary KEY
$ ./ob release canary my-canary red "swapping to disk"
$ ./ob release canary my-canary auto
$ ./ob release canary remove my-canary
```

`canaries` shows each canary's colour and why, when it last reported
and what it runs; `add` enrols one with the key `sysupdate canary
enrol` prints on the canary; `red` and `green` set a colour by hand,
which wins over its reports until `auto`; `remove` takes it off the
list. The server signs the rollouts again only when the set of canaries
that are not green changes: `dukovo-catalog-canaries` looks when a
report or the list changes and every 15 minutes (every 5 on a Dukovo
server, which has no way to watch a file), for a canary that went
quiet. `./ob catalog rollout canary` is refused, naming these commands.
With no canary enrolled nothing is held for canaries.

## Withdrawing a version

Pausing the waves does not take back what computers have reached. A
version that is broken, or harms, is withdrawn (ADR 0593):

```
$ ./ob catalog withdraw io.github.burntsushi.ripgrep 15.3.0 --kind broken --reason "crashes on start"
$ ./ob catalog deploy
```

`withdraw` takes the bundle's ID as the targets list it, which for a
bundle with a line is `ID@LINE`, and the version. It lists the change,
asks, and signs the next targets with a YubiKey, as `sign` does; the
target stays listed, with the `kind`, the `reason` (at most 200 bytes,
one line) and the time. Commit `metadata/targets.json` and deploy as
after `sign`. It refuses a version no target lists, and one withdrawn
already as the same kind or a worse one: a version withdrawn as
malicious is never made broken again, but a broken one may be made
malicious.

Every computer that reads the catalog keeps the withdrawal for good,
installs and updates to that version nowhere, and leaves it: a broken
version is replaced by the version kept beside it, or by the next good
one at once, ahead of the waves; a malicious one is stopped first.
`bundle list` and `bundle info` say which of a computer's bundles were
withdrawn, and why.

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
