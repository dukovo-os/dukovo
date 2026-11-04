<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0390. A bundle from no catalog updates from a feed its publisher signs

- Status: Accepted
- Date: 2026-11-03

## Context

The roadmap asks 13e for sideloaded apps to update like catalog apps: "A
bundle may name its own signed update feed in its manifest". The
manifest has had the field since ADR 0221, `update` with `feed`, an https
address, and `key`, a minisign public key, but nothing read it. A bundle
from a file no catalog lists is bound to the key that signed its first
install (ADR 0223); an update from a catalog (ADR 0384) is an install
with every check an install has.

The updates design that Chase accepted for 13e (2026-11-03, its section
10) says how: "One TUF client for every feed. A third-party catalog or a
bundle's own feed is a small TUF repository rooted in the publisher key
the owner accepted at install, and a publisher may use one key for all
its roles. Sideloaded apps get the same protection against old, frozen
and faked updates." A catalog is such a repository already (ADRs 0380
and 0383), made with catalog-repo (ADR 0381).

## Decision

- **The feed is a catalog of one publisher.** `update.feed` is the
  address of a TUF repository, `metadata/` and `targets/` under it, laid
  out and signed as a catalog is: each bundle a target
  `bundles/ID-VERSION.dkv` with its ID, line and version. It may list
  other bundles of the same publisher. It may be served by https or
  http, since everything it serves is signed, as for a catalog.
- **Rooted in the manifest's key.** `update.key`, a minisign key, is an
  Ed25519 key, and TUF's key is the same one. bundled starts a feed's
  client from `metadata/1.root.json`, which must name that key alone for
  root, threshold one, and be signed by it. From there the client is the
  catalogs' own: root rotation, rollback, freeze and mix-and-match
  protection, and each file checked by its length and hashes.
- **Which key.** bundled reads the feed that the installed version's
  manifest names. It keeps a client for each place and bundle in
  `/system/state/bundled/feeds/PLACE/ID`, with the key and address it
  started from. A manifest that names another address starts it over;
  another key starts it over too, unless the root it trusts already
  names that key, which the publisher rotated to.
- **The file.** bundled fetches the newest version the feed lists of the
  bundle's ID and line, and installs it as any file: it must be signed
  by the key the bundle's ID is bound to, and an update that asks for
  more waits for a person, as from a catalog. A bundle bound to a
  catalog updates from the catalog alone.
- **Where it shows.** The sheet says "It updates from its own feed at"
  the address for a bundle from no catalog; `bundle update --check`
  names "its own feed" where it names a catalog; the history and the
  log say "from its feed".
- **The publisher's tool.** `bundle feed FOLDER [FILE.dkv...]` makes and
  signs the repository with catalog-repo's code: `repository` holds the
  key and signed metadata, `store` the files, `site` what is served. One
  key, `--key` or the developer key, signs every role and must be the
  key the files' manifests name. Each run adds the files given and signs
  targets, snapshot and timestamp anew, good for 30 days unless
  `--days` says, at most 180; the root is good for a year and signed
  again when it would run out first.

## Consequences

- A sideloaded app updates, by itself too, without a catalog, and gets
  the guarantees a catalog app gets, from the same client code and its
  TUF conformance tests (ADR 0380).
- A publisher who serves no new version must still run `bundle feed`
  before the feed's time ends, or updates stop with TUF's own message.
- A publisher who loses the feed's key can no longer update through the
  feed: people install a new version, signed by the bundle's key, from
  its file, and that version may name a new feed key.
- A feed's file signed by another key than the bundle's is refused, as
  a file is; the feed's key alone cannot change what runs.
- `bundle` links catalog-repo's code, to sign feeds.
- The boot test `bundle-feed` serves almanac's feed, signed by `bundle
  feed` on the host, over HTTP: an update from it, the feed as it was
  signed before and a changed file refused, and `bundle feed` signing
  one and refusing a key the manifest does not name.
