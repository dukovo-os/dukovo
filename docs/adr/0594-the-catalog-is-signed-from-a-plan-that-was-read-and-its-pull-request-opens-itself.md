<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0594. The catalog is signed from a plan that was read, and its pull request opens itself

- Status: Accepted; a version packed again with other files ships as its next build, since [0595](0595-a-rebuild-ships-as-its-versions-next-build.md)
- Date: 2026-11-20

## Context

ADR 0381 has the catalog's targets signed offline with one touch of a
YubiKey a batch, and ADR 0382 has them published. Four things in that flow
went wrong.

A full `./ob catalog sign` packed every port again and then refused the
whole batch if any listed version came out as other bytes, since a version
once signed is never signed again as other bytes. After the C library
changed, every port packs to new bytes, so every full signing failed, and
so did `./ob catalog status --json`, which lists the same changes.

A tool that signs for a person, Dukovo Ops, ran `sign --yes`: the change
list scrolled by in a log and the YubiKey asked for its touch with no
pause in which to read it. Nothing tied what was signed to what anyone had
seen.

The commit of the signed `targets.json` and its pull request were made by
hand. Committing in the checkout moved it to a branch, and a later
`git pull --ff-only` stopped on the unstaged copy of the merged file.

A pull request that changed only `catalog/metadata/targets.json` ran every
code job, though no build reads that file: an image carries only the first
root.

This record builds on ADR 0381 (the signing), ADR 0382 (the update server
and `deploy`) and ADR 0593 (`withdraw`, which signs the same way).

## Decision

- **A repacked version is kept as signed.** When a bundle's ID and version
  are listed already and the file has other bytes, catalog-repo sets it
  aside, lists it on a line that starts `= PATH: packed again as other
  bytes`, signs the rest, and writes nothing of it to the store. It
  changes nothing, so a batch of such versions alone counts as empty.
  `status --json` lists them as `rebuilt` beside `verified` in `unsigned`.
  `sign` also refuses a bundle whose bytes changed after the list was
  made.
- **A signing has a plan.** `--preview FILE` lists the batch, writes the
  plan (`out/catalog/plan.json` from `ob`) and signs nothing. The plan
  names the action (`sign`, `renew` or `withdraw`), the targets version it
  follows, each change, the keys that may sign, and a digest: the SHA-256
  of the action, that version, each target added as listed, the paths
  taken out and newly marked verified, and each withdrawal's kind and
  reason. The digest leaves out the clock (a withdrawal's `since` and the
  expiry) and the kept versions, so that a batch listed again a minute
  later has the same digest. `--expect DIGEST` lists the batch again and
  signs it with no question only if its digest is that one; otherwise it
  fails with "prepare again" before any key is looked for. Only one of
  `--yes`, `--preview` and `--expect` is allowed, on `sign` and on
  `withdraw`.
- **A renewal reads no bundle.** `sign --renew` signs the targets again
  as they are. The ports are packed first only when no bundle is named and
  neither `--renew` nor `--expect` is given, because packing could change
  the bytes that were read.
- **Signing follows the server.** Before `sign` or `withdraw`, ob reads
  what updates.dukovo.org serves and refuses when it serves a newer
  targets or root version than `catalog/` holds. A server it cannot read
  is warned of, since signing is offline by design.
- **`./ob catalog pr` records what is live.** It checks the catalog
  offline, takes the changed files of `catalog/keys/` and
  `catalog/metadata/`, refuses when the checkout is behind main or
  the server does not serve exactly these versions, builds one commit on
  `origin/main` in an index of its own with a sign-off from git's
  `user.name` and `user.email`, pushes it without force to
  `catalog-targets-N` (`catalog-root-M` for a root alone), opens the pull
  request with `gh`, and turns on auto-merge with squash. HEAD, the
  branch and the working tree are never touched, and only the signed files
  are staged, so a later `git pull --ff-only` takes main's copies. An
  older open catalog pull request is named in the new one, not closed.
  Its last two lines are the "is live" line and the URL, which Dukovo Ops
  reads.
- **`./ob catalog verify` checks the catalog offline.** Every root from
  the first is signed by its own keys and by the one before, the targets
  are signed by the newest root's keys and have not expired, and every
  target's path follows its ID and version. `catalog-repo since` lists
  what the targets changed since an older copy, for commit messages.
- **CI.** A pull request whose only non-documentation file is
  `catalog/metadata/targets.json` skips the code jobs. The job "Licenses,
  sign-off, TUF and lab coverage", which every pull request runs, runs
  `./ob catalog verify`.

## Consequences

- A change of the C library no longer blocks signing: new versions sign,
  renewals sign, and the rebuilt versions are named and kept.
- A rebuild of a version the catalog lists reaches computers only under a
  new version. A recipe's version is the upstream one, with no build
  number, and a build number would change version order on every machine;
  that is left for its own decision (docs/deferred.md).
- What the YubiKey signs is what was read: a list that moved between the
  preview and the touch signs nothing and says to prepare again. A tool
  can show the list, hold the signing to its digest, and run the same
  `./ob` commands a person types.
- `./ob catalog sign` with no option still lists, asks and signs, as
  before.
- A signing leaves the checkout on its branch with only the signed files
  staged, and its pull request merges itself when its checks pass. `gh`
  and git's identity are needed to open it.
- `./ob catalog verify` in CI fails once the catalog's targets or root
  expire, whatever else the pull request changes. The 30-day warning in
  `./ob catalog status` and in Dukovo Ops is there to prevent that.
- A pull request of signed metadata alone takes one light CI round, with
  its signatures checked.
- Dukovo Ops' prepare step packs the ports in `out/`, and does not see a
  lab round reading it (docs/deferred.md).
- The plan's digest does not cover the expiry, so two signings of one plan
  differ in their dates only.
- Unit tests in catalog-repo cover the kept versions, the digest, the
  preview, a plan that moved, renewals, `status`, `since` and `verify`;
  tests in xtask cover the commit builder on a temporary repository and
  the rules for packing, the server and main. No image changes, so no boot
  test.
