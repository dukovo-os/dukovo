<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0592. A catalog that vouches for an app's publisher may take its ID, and a publisher may move its app to a new key

- Status: Accepted
- Date: 2026-11-20

## Context

ADR 0383 binds an app's ID to the key that signed its first install, and
to the catalog it came from, and refuses every other key. A file that
someone installs by hand, claiming an ID before the real app is
installed, therefore binds the ID to its own key, and the real app from a
catalog is refused afterwards: "only its publisher's key may install it".
Nothing can change a binding once it is made. The catalog's index lists a
file's ID, version and hashes, and says nothing of who owns an ID, so
bundled cannot tell a catalog that vouches for an ID's publisher from one
that lists a file.

A publisher that loses a key, or retires it, has the same problem from the
other side: its ID is bound to a key it can no longer use, and no new
version can be installed.

This record amends ADR 0383 (catalogs and the bindings of an ID) and
builds on ADR 0390 (a bundle from no catalog) and ADR 0591 (what a
removal leaves).

## Decision

- **A catalog says which IDs it vouches for.** A target of the catalog's
  index may carry `"owner": "verified"`, which says the catalog itself
  vouches for the publisher of that ID. Every bundle Dukovo's catalog
  lists is built from Dukovo's own ports, so `ob catalog sign` signs each
  of them as verified, and marks a bundle it signed before as verified in
  the same batch, with no new bytes. The index bundled keeps has a field
  for it.
- **A file of an ID a catalog vouches for installs only from that
  catalog.** A file of such an ID that comes from any other key is
  refused, naming the catalog, unless the asker's own developer key
  signed it as a build of their own. Where the ID is bound to a key
  already, the same refusal says to install it from the catalog.
- **The catalog takes an ID from a key it does not vouch for.** When the
  ID is bound here, in the place the install is for, to another key, and
  the first catalog that lists the file vouches for the ID's publisher,
  the install is a take-over. The sheet warns, naming the old key and the
  day it was bound, that the app of that key is replaced and its data set
  aside. On a yes, or `--yes`, the old bundle is removed with its
  services' accounts (ADR 0591), the version it replaced is not kept,
  the ID is bound to the new key and to the catalog, and what people
  allowed the old key is forgotten. A person's install never takes an ID
  from an everyone binding; an administrator takes it, for everyone, as
  every install for everyone is.
- **An old publisher's data is set aside, never deleted.** For a person,
  `~/app-data/ID` and `~/app-data/.snapshots/ID` move to
  `~/app-data/.set-aside/ID-YYYY-MM-DD/`, as `data` and `snapshots`; for
  everyone, the same in each home, and `/shared/services/ID` moves to
  `/shared/services/.set-aside/ID-YYYY-MM-DD`. A name already there gets
  `-2`, `-3`. The folders bundled makes are the owner's of the folder they
  are in, with mode 0700. Each move is a rename through descriptors of
  the folders, opened without following a link, never a walk of a path.
  The audit log names the folders.
- **A publisher moves its app to a new key with a rollover file.**
  `rollover`, at the top of the bundle beside `files.sha256`, is signed
  with the rest of its files by the new key, and holds one or more links,
  each signed by the key it moves from. A link says that the ID moves
  from one key to another as of a time. A chain of at most 8 links, from
  the key the ID is bound to to the key that signed the bundle, with no
  key twice and the times rising and none after the bundle was signed,
  moves the ID: the sheet says `A new key for` the ID, with the old key
  and the day of the move, and the install binds the ID to the new key
  and moves what people allowed to it. The old key is refused from then
  on. `bundle key rollover` writes a link.
- **The rules, in order,** for a file signed by a key: an ID bound to a
  catalog installs from that catalog's files; a file of the key the ID
  is bound to installs; a rollover chain from the bound key to the
  signing key is a move; a catalog that lists the file and vouches for
  the ID takes it; any other key is refused. An ID not bound installs as
  the catalog that lists the file, or by the asker's developer key, or,
  if no catalog vouches for the ID, as an unknown key does, with the
  warning.
- **The proof of a domain is not part of this.** A catalog that takes
  apps of third parties would check that the publisher controls a domain
  by a file at `https://DOMAIN/.well-known/dukovo-app-ids`. It waits for
  the first third-party app the catalog takes (`docs/deferred.md`).

## Consequences

- A person who installed a file of an ID before the catalog listed it can
  install the real app after, with a warning that says what is replaced,
  and loses nothing but the app: its data stays in `.set-aside`, for them
  to read, move back or remove.
- A file someone installs by hand can no longer hold an ID the catalog
  vouches for.
- A publisher that loses a key before it made a rollover file cannot move
  its ID: the binding outlives the bundle (ADR 0591), so the rollover
  file is made while the old key is still there.
- Every audit event of a move or a take-over names the ID, the old key,
  the new key and, for a take-over, the catalog and the folders where the
  data went.
- The index of a catalog written before this record has no `owner`
  field and vouches for no ID, so the rules change nothing for it.
- The boot test `catalog-install` installs a file of ripgrep's ID signed
  by an unknown key before a test catalog that vouches for ripgrep is
  added, has a second such file refused for everyone naming the catalog,
  installs ripgrep from the catalog over the first, warned and with the
  data set aside, and moves a bundle of another ID to a new key with a
  rollover file, after which a file of the old key is refused.
- **Not yet:** the proof of a domain for a third-party publisher.
