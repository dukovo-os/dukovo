<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# When the maintainer is unavailable

Dukovo has one maintainer ([GOVERNANCE.md](https://github.com/dukovo-os/dukovo/blob/main/GOVERNANCE.md)).
This page is for the person they have named to act for them when they
cannot: ill, away with no way to reach them, or gone. It says what keeps
working with no one, until when, and what to do to keep machines safe and
updated.

## Who may act

The person the maintainer has named in writing, in a letter kept with the
YubiKey that is kept apart. The letter says who they are, what they may
reach (below), and how to tell that the maintainer cannot act: no answer,
for how long, through which channels. No one else may act, whatever a
message says; [Keys and incidents](keys-and-incidents.md) says why.

The person acting keeps the project's signatures and services running,
answers security reports, and fixes what is broken. They make no other
decision for the project, and give no one else access, unless the
maintainer does not come back (the end of this page).

## What they need

The maintainer gives them these beforehand, and checks once a year that
they still work:

- a second owner's place in the `dukovo-os` organization on GitHub, on
  their own account with two-factor sign-in;
- the place where the YubiKey kept apart is, and its PIN, kept apart
  from it;
- their own SSH key on the update server's deploying login, which is
  what `OB_UPDATE_SSH` names;
- a member's place in the DigitalOcean team that holds the server, DNS
  and website, and access to where dukovo.org is registered;
- a computer of their own, set up to build Dukovo as
  [Building Dukovo with ob](building.md) says.

## What keeps working, and until when

With no one acting:

- Every machine keeps running the release it has, and nothing on it
  stops.
- The update server signs a new timestamp every six hours, so machines go
  on reading it, for as long as the server runs: while its bill is paid,
  and dukovo.org is registered.
- The targets, what the YubiKeys signed, run out 182 days after the last
  signing, for the system's releases and for the catalog each. From then
  on machines take no update and no app, and say why. Nothing is lost:
  the next signing puts it right.
- The roots run out a year after each was signed; the first root of the
  system's releases on 2027-11-17. A machine whose root has run out takes
  the next root when one is signed, so this, too, is put right by
  signing.

`./ob updates status` says when each signature runs out, and warns 30
days before.

## What to do

1. **Look.** Run `./ob updates status`, and read the open security
   reports on the repository's Security tab, and the open pull requests.
2. **Keep the signatures good.** When a signature runs out in less than
   30 days, sign it again with the YubiKey. For the system's releases,
   `./ob release root` signs the next root, with the same keys, and the
   targets again as they are: it shows `no key of the root before is
   dropped`, and both are then good for a year and six months. Type `root
   N`, then deploy as it says. For the catalog, `./ob catalog sign
   --renew`, then `./ob catalog deploy` and `./ob catalog pr`; when its
   root runs out within 30 days, `./ob catalog root` first. Commit what
   each writes, through a pull request.
3. **Answer security reports** within the times SECURITY.md gives. A fix
   is released as `releases/README.md`'s *Signing a release* says, as a
   fix release of the newest one.
4. **Keep the bills paid**: the hosting account, and dukovo.org's
   registration.
5. **If something is broken into or lost**, follow its page: [A lost or
   stolen YubiKey](incident-yubikey.md), [A hacked update server or
   hosting account](incident-server.md) or [A hacked GitHub
   account](incident-github.md).

Write down what you did and when, in the project's issues, so that the
maintainer, or whoever comes next, can follow it.

## If the maintainer does not come back

The person acting goes on as above, and finds new maintainers, as
GOVERNANCE.md says: the project, its keys and its accounts are handed to
them, and they sign the next roots with YubiKeys of their own, as [A
lost or stolen YubiKey](incident-yubikey.md) does for a key that is
replaced.
