<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# A hacked GitHub account

The `dukovo-os` organization on GitHub holds the source, the CI that
checks it, the website's source, which dukovo.org deploys from its `main`
branch, and the security reports sent privately. Someone who has broken
into a maintainer's account, or into the organization, can change any of
it, and read the reports.

They cannot sign a release or a bundle: only the YubiKeys sign, on the
maintainer's computer. But what they change in the source goes into the
next release or batch that the maintainer signs, as the YubiKeys sign
whatever the tree builds, and CI's checks, which a release waits for,
can be changed with the rest. So the work is to find every change, put
the repositories back as they were, and sign nothing until that is done.

What tells you: a sign-in, a token or an SSH key you did not make, in
your account's security log or the organization's audit log; a commit,
tag, branch rule or workflow you did not make; a notice from GitHub.

You need the account's password and recovery codes, a clone of each
repository that you made before the break-in, and a few hours.

## 1. Take back the account

In your account's settings: change the password, set two-factor sign-in
up again with new recovery codes, and sign out every other session. Then
remove what you do not know, under each of:

- SSH and GPG keys;
- personal access tokens, both kinds;
- authorized OAuth apps and GitHub Apps.

Read the security log for what was done while it was not yours, and
write down the first time you do not know: everything after it is
suspect.

In the organization's settings, the same for the organization: its
owners and members, outside collaborators, installed GitHub Apps, and
webhooks. Read its audit log from that time. In each repository, check
the deploy keys, the webhooks, the rules on `main`, and the Actions
secrets and variables: the project keeps none, so any there is not yours.

Do not sign a release, a channel move or a batch of the catalog until
step 3 is done.

## 2. Find every change

A commit's author and date are whatever its maker typed, so compare
with your own clones, not with dates. In each clone, before you pull:

```
git fetch origin --tags
git log --oneline main..origin/main
git diff main origin/main -- .github/ releases/ catalog/ SECURITY.md
git branch -r
```

The `log` lists what reached `main` since your clone was last at it, and
the `diff` what changed in the files that matter most. A `fetch` that
says `forced update` means history was rewritten, and one that says a
tag `would clobber existing tag` means a tag was moved. A branch you do
not know is suspect too. Look hardest at:

- `.github/`: the workflows, and the scripts and actions they run;
- `releases/keys/` and `releases/metadata/`, `catalog/keys/` and
  `catalog/metadata/`: the public keys and roots that machines and the
  people who check a download trust;
- anything a release builds, which is all the rest.

Do the same for `dukovo-os/website`, whose `main` is what dukovo.org
shows, and open the site's download page and its words on checking a
download. Check the GitHub releases of each repository, and the files
attached to them.

## 3. Put it back

Undo each change you do not know by a pull request, reviewed as any
other. Where history was rewritten, push your own clone's `main` back;
an owner may need to lift the rule on `main` for that one push. Then:

- `releases/README.md`'s SHA-256 of the first root, and `sha256sum
  releases/metadata/1.root.json`, are the one you know.
- `./ob release check` and `./ob catalog check` read the server from the
  first roots and pass. They read the update server, which this break-in
  did not reach, unless the same password or key reaches it too: then
  follow [A hacked update server or hosting account](incident-server.md).

Only now sign again.

## 4. If a release carried a change

If a release or a bundle was signed from a tree that held a change you
do not know, withdraw it as malicious, so that every machine stops it,
then sign one without the change. For the system's releases, as
`releases/README.md`'s *Changing a channel by hand* says, with `pull`:

```
S=target/programs-host/debug/system-release
$S fetch --root releases/metadata/1.root.json --roots releases/metadata \
    --out DIR https://updates.dukovo.org/system/
$S pull --repo DIR --yubikey-helper tools/yubikey-piv.py \
    --version VERSION --reason TEXT --advice rollback
./ob release deploy DIR
```

For a bundle, `./ob catalog withdraw ID VERSION --kind malicious
--reason TEXT`, as `catalog/README.md` says.

## 5. The security reports

Treat every report that was open while the account was not yours as
read by someone else. Tell each reporter, through the report, and fix
those reports first: the clock of [SECURITY.md](https://github.com/dukovo-os/dukovo/blob/main/SECURITY.md)
still holds, and a report that may be public is fixed and disclosed as
fast as it can be.

The reports that `report crash --security` seals on a machine go to the
reports server, not to GitHub, and the key that opens them is not on
GitHub either.

## 6. Afterwards

Say in a GitHub security advisory what happened, from when to when,
what was changed, and what a machine owner needs to do, which is nothing
unless a release carried a change. Change the password of every other
account that shared one with this account.
