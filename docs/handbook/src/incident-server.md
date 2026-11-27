<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# A hacked update server or hosting account

The update server, updates.dukovo.org, serves what the YubiKeys signed,
and holds the online keys of the system's releases and of the catalog,
which sign the snapshots, timestamps and rollouts. The hosting account,
DigitalOcean, holds that server, dukovo.org's DNS and the website. Someone
who has broken into either can:

- keep old releases and bundles current for machines, until the targets
  the YubiKeys signed run out, six months after they were signed;
- hold or open the waves, and take the server down;
- with the account, point updates.dukovo.org anywhere, and change the
  website, with what it says about checking a download.

They cannot sign a release, a bundle, a channel move or a file: the
YubiKeys sign those, and every machine checks them. So the work is to put
the server's keys out of use, and to serve from a server that only you
reach.

What tells you: a sign-in you did not make, in the server's log or the
account's security history; `./ob release check` or `./ob catalog check`
failing where they passed; a DNS record or a page of the website you did
not change; a notice from the host.

You need the source tree on the computer that signs releases, a YubiKey,
the hosting account, and about two hours.

## 1. Take back the account

If the account was broken into: change its password, set its two-factor
sign-in up again, sign every other session out, and remove each API
token, SSH key and team member you do not know. Check dukovo.org's DNS
records: each is one you made, `updates` leads to the server's address,
and the website's records lead to its app. Check that the website's app
deploys `dukovo-os/website` from `main`. Do the same where the domain is
registered.

If the way in was your own login, from the computer that signs releases,
stop here: that computer may be the one broken into. Sign nothing on it
until it is installed again. A YubiKey asks for a touch for each
signature, so nothing was signed there without your touch.

## 2. Take the server off the network

Power the server off from the hosting account. Machines then cannot
reach updates.dukovo.org: they keep running the release they have, say
that updates cannot be checked, and install nothing. Keep the server's
disk, or a snapshot of it, in the account to look at later, and do not
download it: a copy of its disk holds its keys (ADR 0782).

## 3. Make a new server with new keys

Follow [Move the update server to Dukovo](https://github.com/dukovo-os/dukovo/blob/main/docs/runbooks/update-server-move.md),
its steps 1 to 5, with OLD being the server that was broken into, and
these differences:

- **Skip its step 4.** Nothing comes from OLD.
- **Before its step 5**, take the old keys' public halves out of the
  source tree:

  ```
  git rm catalog/keys/online.json releases/keys/online.json
  ```

  Step 5 then prints `Making the catalog online key.` and `Making the
  system online key.` where the move says it takes them, and writes the
  new keys' public halves to the two files.

Then come back here. Do not carry anything over from OLD, as the move's
step 6 does: not the metadata, the plans nor the canaries. Once the next
roots name the new keys, a machine no longer holds OLD's timestamps and
snapshots against NEW's, as TUF says of a key that is replaced.

## 4. Sign the next roots, naming the new keys

For the system's releases, the server cannot be read, so the next root
is signed into the folder of the newest release you deployed. Find it:

```
ls out/release-sign/*/metadata/*.targets.json out/release-root/*/metadata/*.targets.json
```

The folder whose `T.targets.json` has the highest number `T` is the one,
unless you deployed a channel move by hand since, from a folder of your
own.
Then, with a YubiKey plugged in:

```
./ob release root --repo FOLDER
```

It shows the next root. Check what it shows:

- `snapshot and timestamp also:` names the online key marked `new`.
- One `dropped:` line, the old online key.
- `every role:` names the YubiKeys you have, none of them `new`.
- `then the targets again as they are, as version`: one more than the
  `T` you found.

If all of that is so, type `root N`, `N` being the version on its first
line, give the PIN and touch the YubiKey.
It signs the targets again under the new root, so that they delegate the
rollouts to the new key.

For the catalog, sign its next root, then its targets again, so that
they delegate its rollouts and its list of flaws to the new key:

```
./ob catalog root
./ob catalog sign --renew
```

Commit `catalog/` and `releases/keys/` and `releases/metadata/` on a
branch, and open a pull request.

## 5. Deploy to NEW

As the move's step 7 says, with `/etc/hosts` leading updates.dukovo.org
to NEW:

```
./ob release deploy FOLDER
./ob catalog deploy
```

NEW must hold every file of every release the targets list, and every
bundle. If the system's deploy says a file is missing, it is one of an
earlier release: copy them in from the folders of the earlier releases,
`cp -n out/release-sign/*/files/* FOLDER/files/`, and deploy again. The
catalog's bundles come from `out/catalog/store/`, where `./ob catalog
sign` put them. A file or bundle can also come from OLD's disk: each is
named by its SHA-256 and checked against it as NEW takes it, so one that
was changed is refused.

Then, before the DNS record moves:

- **The system's waves** start again on NEW, from the canaries, for each
  release a channel offers. Pause again each release you had paused:
  `./ob release rollout pause VERSION REASON`.
- **The catalog's waves** are all open on NEW, as on a new catalog: a
  bundle you had paused reaches every computer. If one must not, withdraw
  it, as `catalog/README.md` says, before the DNS record moves.
- **Enrol the canaries again.** On each, as an administrator, `sysupdate
  canary enrol` prints the line to run, `./ob release canary add NAME
  KEY`. A canary on OLD's list is not taken on trust.

Then follow the move from its step 8 to its end: check NEW as a client
would, let the refresh jobs run once, switch the DNS record, and give
NEW the certificate that is renewed.

## 6. Check

```
./ob updates status
./ob release check
./ob catalog check
```

Each reads NEW as a computer online does, from the first roots, and
walks to the new ones. A machine does the same at its next check for
updates: it takes the new root, and from then on refuses whatever OLD's
keys sign.

## 7. Afterwards

Look at OLD's disk, in the hosting account, for how the way in was
made, and close it on NEW. Say in the next release notes what happened
and what a machine owner needs to do, which is nothing unless the notes
say otherwise. Remove OLD and its snapshots once you are done with them.
