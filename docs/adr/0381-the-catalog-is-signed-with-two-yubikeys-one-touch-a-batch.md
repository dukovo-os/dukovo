<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0381. The catalog is signed with two YubiKeys, one touch a batch

- Status: Accepted
- Date: 2026-11-03

## Context

The catalog is a TUF repository that the client of ADR 0380 reads. TUF
asks for its keys to be chosen with care. The keys of root and targets,
which say what Dukovo publishes, should be offline. The keys of snapshot
and timestamp, which only say which targets are current and that they
still are, must sign every few hours, so they are online.

The chosen design:

- His two YubiKeys, either of which signs alone, the second kept away
  from the first.
- One touch of a YubiKey for each batch of changes. Nothing that signs
  what the catalog carries is kept on the server.

A YubiKey 5 signs in three ways:

- **FIDO2.** Its signatures cover what the authenticator adds, so they
  are not signatures of the metadata itself.
- **OpenPGP.** Its signatures are not ones TUF's clients check.
- **PIV.** It signs with a key made on the YubiKey, which cannot be
  copied off it. On a YubiKey 5 the key is ECDSA on P-256 or P-384, or
  RSA. Ed25519 needs firmware 5.7 and tools newer than Linux Mint 22 has.

The tools to sign with PIV differ:

- yubico-piv-tool in Ubuntu 24.04, which Mint 22 is built on, cannot sign
  data it is handed.
- PKCS #11 through ykcs11 shows a key only once a certificate is stored
  beside it, and Rust would need a crate for it.
- Yubico's own Python library, yubikit, comes with yubikey-manager and
  signs with a few lines.

The bundles of ports ADR 0370 packs carry the build's minisign
signature. That ADR expected the catalog to sign them again with its own
keys, but minisign is Ed25519, which these YubiKeys do not do in PIV.

## Decision

- **The catalog is a TUF repository of its own.** It is served from the
  update server (13e's next pull request), apart from the system's
  updates, which come in 15a. A third party's feed is another such
  repository.
- **Root and targets are signed by an ECDSA P-256 key in each YubiKey's
  Digital Signature slot (9c).**
  - The key is made on the YubiKey with `ykman`, and its PIN is asked for
    once a session.
  - Every signature needs a touch.
  - Each role's threshold is one, so either YubiKey signs alone.
  - `catalog/README.md` gives the setup steps, from a new PIN to the
    first root.
- **Snapshot and timestamp are signed by an Ed25519 key** whose secret
  half is a file on the update server, made there.
- **How long each role's metadata is good for:**
  - Root: a year.
  - Targets: half a year, so that a batch is signed at least that often.
    `ob catalog status` warns a month before.
  - Snapshot: a week.
  - Timestamp: a day, re-signed by the server every six hours.
- **Consistent snapshots.** Metadata is served by its version and
  targets by their SHA-256, so a client never mixes two publishes.
- **What is signed offline is kept in the source tree, in `catalog/`:**
  - `keys/`: the public half of each key, with its roles and its
    YubiKey's serial number.
  - `metadata/`: every root, and targets.

  Each change is a commit that can be reviewed and traced back.
- **A bundle is a target, `bundles/ID-VERSION.dkv`.**
  - Targets list its length and SHA-256, and its ID, version and name as
    `custom`.
  - A path once signed is never signed again as other bytes: a new build
    is a new version.
  - The catalog vouches for a bundle by signing its hash, so a bundle
    from the catalog is trusted through TUF, not through the minisign
    key that signed it. That key stays the build's. This replaces ADR
    0370's re-signing with the catalog's keys.
- **`ob catalog`** builds catalog-repo, a program in the user programs'
  workspace that also runs on the server, and runs it with the source
  tree's paths:
  - `key yubikey` adds a YubiKey's public key.
  - `key new` makes an Ed25519 key.
  - `root` signs the next root. It is signed by every YubiKey plugged in
    that holds a key of the old root or the new one, and is checked
    against both, as a client checks it.
  - `sign` packs the bundles as `ob ports` does, lists what a batch
    changes, asks, and signs the next targets with one signature: one PIN
    and one touch.
  - `publish` signs a snapshot and timestamp with the online key and lays
    out what the server serves. It then reads all of it back with the
    client, from the first root, as a new computer does.
  - `status` shows each role's version and when it expires, and lists the
    keys.
- **The YubiKey signs through `tools/yubikey-piv.py`**, a short script on
  yubikit, run by the system's Python:
  - `list` names the YubiKeys plugged in.
  - `public` prints a slot's public key. It refuses a key that was
    imported rather than made on the YubiKey, and warns when a key signs
    without a touch.
  - `sign` asks for the PIN on the terminal, asks for the touch, and
    prints the signature.
- **Every signature is checked before it is written**, with the client's
  own code, whatever made it. The `tuf` crate's `repo` module makes each
  role's metadata and signs it, so the client and the tools share one
  canonical JSON and one way of checking keys.

## Consequences

- No key that can say what the catalog carries is ever online. A stolen
  server can sign snapshots and timestamps that change nothing in what
  targets list, and only until they expire.
- With a threshold of one, a lost YubiKey and its PIN could sign a root.
  Three wrong PINs lock it. The other YubiKey signs the next root
  without it, and a third key can raise the threshold later.
- The tests sign with a stand-in for a YubiKey: openssl's P-256 key,
  which signs as the YubiKey does, in DER. The client then reads back a
  catalog that has been signed, published, given a new root, and had a
  bundle taken out.
- **Not yet** (13e's next pull requests):
  - The update server: the online key made there, and timestamps
    re-signed every six hours.
  - Catalogs on Dukovo: search, install and the trusted root in the
    image.
  - Delegated roles for other publishers.
