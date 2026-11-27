<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Keys and incidents

Whoever can sign a Dukovo release can run code on every Dukovo machine.
So the keys that sign are few, kept apart, and each one can be replaced
without the others. This chapter says where each key and account is,
what a thief could do with it, and what the maintainers do, step by
step, when one is lost or broken into. ADRs 0457, 0467, 0538, 0780 and
0782 are the decisions behind it.

The pages that follow are the runbooks:

- [A lost or stolen YubiKey](incident-yubikey.md)
- [A hacked update server or hosting account](incident-server.md)
- [A hacked GitHub account](incident-github.md)
- [When the maintainer is unavailable](incident-maintainer.md)

## The keys

| Key | Where it is | What it signs | What a thief could do with it |
| --- | --- | --- | --- |
| The release key, slot 95 of each YubiKey | Two YubiKeys, either signing alone: one at hand, one kept apart | The system releases' roots and targets: every release, channel move, withdrawal and advisory | With the PIN, and a way to serve machines, sign anything a machine installs. Without the PIN, nothing: three wrong PINs lock it |
| The catalog's key, slot 9c of the same YubiKeys | The same two | The catalog's roots and targets: every bundle | With the PIN, as for the release key, for apps |
| An age key on each YubiKey | The same two | Opens the security team's key, which security reports are sealed to (ADR 0531) | With the PIN and the team key's wrapped file, read security reports |
| The system releases' online key | The update server, a file only its jobs read (ADR 0782) | Snapshots, timestamps and rollouts | Keep old releases current, or hold and open the waves. Never add a release or a file |
| The catalog's online key | The update server, the same way | The catalog's snapshots, timestamps, rollouts and list of known flaws | As for the system's, for the catalog |

The public halves are in the source tree: `releases/keys/` and
`releases/metadata/` for the system's releases, `catalog/keys/` and
`catalog/metadata/` for the catalog. The first root's SHA-256 is in
`releases/README.md`; it is how anyone checks the root an install
trusts.

## The accounts

| Account | What it holds | What a thief could do with it |
| --- | --- | --- |
| GitHub, the `dukovo-os` organization | The source, CI, the website's source, the private security reports | Change code, CI and the website, and read reports. Not sign a release, but what they change goes into the next one signed, unless it is found first |
| The hosting account (DigitalOcean) | The update server's machine, dukovo.org's DNS, and the website | All the server holds, point updates.dukovo.org anywhere, and change the website |
| The update server's login (`$OB_UPDATE_SSH`) | Deploys, and `elevate` on the server | All the server holds |
| Where the domain is registered | dukovo.org itself | Move the domain, and with it every name |

Keep two-factor sign-in on each, with its recovery codes kept apart
from the computer that signs in.

## Who may act

The maintainers in [GOVERNANCE.md](https://github.com/dukovo-os/dukovo/blob/main/GOVERNANCE.md)
may follow any of these pages. While there is one maintainer, the person
they name in writing may act for them when they are unavailable, as
[When the maintainer is unavailable](incident-maintainer.md) says; no one
else. A message that claims to come from a maintainer and asks for a
key, a PIN, a recovery code or an account is not one until they confirm
it in person, or through a channel agreed on before.

## What always holds

- No key that signs a release is ever on a server or in CI. A machine
  installs only what the release keys signed, and checks each file
  against the SHA-256 the signed list gives.
- A machine takes a new root only when the root it trusts signs it, so a
  key dropped from a root signs nothing that machine takes again.
- Every release signed on a YubiKey, and every install image's
  checksums, is also recorded in Sigstore's Rekor log, so that a release
  signed in secret stands out (ADRs 0778 and 0779).

## Checking what is served

`./ob updates status` reads the catalog and the system's releases as a
new computer does, from the first roots in the source tree, and says
when each signature runs out and what needs a maintainer. `./ob release
check` and `./ob catalog check` read each in full. Run them at the end
of every runbook, and whenever something looks wrong.
