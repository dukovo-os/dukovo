<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0778. A release is signed on a YubiKey from what the server serves, once its gates and three builds agree

- Status: Accepted; carries out the signing of [0457](0457-releases-are-signed-on-two-hardware-keys-after-three-builds-agree.md) and the gates of [0450](0450-updates-keep-eleven-promises-each-a-gate-before-a-release-is-signed.md)
- Date: 2026-11-26

## Context

ADR 0457 has `ob release sign VERSION` build a release a third time on
the maintainer's machine, refuse unless it matches CI's two builds and
every gate of ADR 0450 is green for the exact commit, show what it is
about to sign, sign on a YubiKey, and record the signature in Sigstore's
Rekor log. The key ceremony of ADR 0538 signed the first root: both
YubiKeys' slot 95 keys for every role, either of which signs alone, and
the update server's online key for snapshot and timestamp.

Until now system-release signed only with keys in a folder, and an
update repository grew in one folder of the machine that signed it:
`add` put a release that `make` signed into it, and `channel` pointed a
channel at one. A folder kept on one machine can fall behind what the
server serves, and nothing in it is what a computer reads. CI's
reproducible workflow uploaded each build's lists in one artifact with
its whole tree, so a third build had to download both trees to compare
two small files.

## Decision

- **The repository starts from what the server serves.** `system-release
  fetch --root releases/metadata/1.root.json` reads updates.dukovo.org
  with the TUF client, as a computer online does, from the first root,
  expiry checked, and writes a new folder: every root by version, the
  newest targets as served, and the file of each target by its hash. A
  server that serves nothing yet gives the first root alone. The
  releases' files stay on the server; a deploy sends only the new
  release's.
- **One targets version, one touch.** `system-release offer` puts the
  new release's files into the store from the third build's tree, each
  checked against the list as it is copied; its list among the targets,
  byte for byte as `./ob release build` wrote it; the channel pointed at
  it, keeping the firmware set the channel offers; and the security
  advisories of `security/advisories`. It signs them as the next targets
  version with the release key of the YubiKey plugged in. The newest root
  must name that key for targets, so a YubiKey without one is refused
  before it is asked for a touch. A version that is not newer than every
  release offered is refused: a version is never used twice, and a
  channel never goes back.
- **Shown, then typed.** Before the YubiKey asks for its PIN and touch,
  `offer` shows the version, the list's SHA-256, the commit, the number
  of files and their size, the channel and the release it offered until
  then, what changes among the advisories, and the targets version and
  when it expires; and it asks for the version to be typed. Anything
  else signs nothing. The signed targets are then checked against the
  newest root, and against what was shown.
- **The gates, for the exact commit**, read with `gh` from GitHub's API:
  - the checkout has nothing changed, and builds the version asked for;
  - the commit is on main or a `release/` branch;
  - `CI passed` succeeded on it;
  - a full run of the update test matrix passed on it, which the
    workflow names `Update matrix, full` (ADR 0536);
  - a run of the reproducible workflow passed on it, and the third
    build's `system.json.sha256` and `files.txt` are those of both of
    its builds (ADR 0535).

  There is no override. A GitHub account that is not the maintainer's
  could show a false green, but not change what the third build makes,
  which is what is signed.
- **The record.** `offer --record FILE` writes a Rekor entry of the kind
  `hashedrekord`: the SHA-256 of the targets' canonical JSON, which is
  what the YubiKey signed, the signature, and the YubiKey's public key.
  The targets are what is recorded, as they name the release list by its
  hash and carry the channel's move with it. `ob release sign` sends the
  entry to `rekor.sigstore.dev` and keeps the log's answer;
  `ob release record VERSION` sends it again when the log could not be
  reached, and a log that holds the entry already answers with it.
- **Deployed as any repository.** `ob release sign` leaves the repository
  in `out/release-sign/VERSION`, and `ob release deploy` sends it; the
  server takes the new targets beside the ones it published, refuses
  any root or targets that differ from those, and signs the snapshot
  and timestamp.
- **The same signer for the rest.** `system-release channel`,
  `advisories` and `pull` take `--yubikey-helper` in place of `--keys`,
  on a fetched repository: a release moved from beta to stable, an
  advisory corrected without a release, a withdrawal.
- **CI keeps the trees apart.** The reproducible workflow's artifacts
  `reproducible-first` and `reproducible-second` hold the three files a
  build is compared by, and `reproducible-first-tree` and
  `reproducible-second-tree` the trees, which the comparison downloads
  only when the builds differ.

## Consequences

- The machine that signs needs `gh` signed in, the YubiKey helper as the
  key ceremony used it, and the time of one release build.
- Nothing the signing machine kept from before is signed again: two
  machines that sign the same commit sign the same targets, but for the
  time.
- `ob release sign` removes the fetched folder when it stops, so that
  signing again starts from what the server serves; a repository it
  signed is kept until it is deployed or removed.
- What Rekor holds is public: the release key's public half, the
  signature and the hash. Nothing of a computer's goes there.
- The first entry sent to the live log is the first release's.
  `DUKOVO_REKOR=https://rekor.sigstage.dev` sends to Sigstore's staging
  log instead, for a trial.
- **Not yet:**
  - Install images that trust the release root and hold a release
    signed on a YubiKey, so that an install takes updates from
    updates.dukovo.org (`docs/deferred.md`).
  - `ob` commands for a channel's move, a withdrawal and the advisories
    alone on a YubiKey; until then they are `system-release fetch`, the
    command with `--yubikey-helper`, and `./ob release deploy`.
