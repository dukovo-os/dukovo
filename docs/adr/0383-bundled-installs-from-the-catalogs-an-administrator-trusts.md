<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0383. bundled installs from the catalogs an administrator trusts

- Status: Accepted
- Date: 2026-11-03

## Context

ADR 0380 gave Dukovo a TUF client, ADR 0381 signs Dukovo's catalog with
it, and ADR 0382 serves the catalog. Nothing on Dukovo read a catalog
yet: a bundle installed only from a `.dkv` file, trusted by its key
(ADR 0223), the asker's developer key or a key it was bound to at its
first install, and any other key after a warning.

The roadmap asks for several catalogs, Dukovo's and others', and for
sideloading to stay open. Chase decided that a third-party catalog, and
a file from anywhere, go through the same checks as Dukovo's own.

## Decision

- **A catalog is a folder of `/system/settings/catalogs`**, by its name:
  a lowercase letter, then up to 31 lowercase letters, digits and `-`.
  It holds `url`, where the repository is (`https`, `http` or `file`),
  and `root.json`, the root the client starts from. Dukovo's own,
  `dukovo`, comes among the default settings, with the first root of
  `catalog/metadata`, once there is one. A file `off` there turns a
  catalog off, so that a default one stays off when Spark copies the
  defaults in again.
- **bundled reads catalogs, with the client of ADR 0380, as root.** Its
  service may now connect out. It keeps each catalog's client in
  `/system/state/bundled/catalogs/NAME`: the metadata it trusts, the
  root it started from, and an index of the bundles the targets list
  (path, SHA-256, length, ID, version and name) with when it was read
  and when the soonest of the root, timestamp, snapshot and targets it
  came from expires. A changed `root.json` starts the client anew from
  it. bundled writes the index only after a refresh passes every check.
- **A catalog vouches for a file, not a key.** A target is
  `bundles/ID-VERSION.dkv`, and it lists the archive's SHA-256 and
  length. bundled sums the archive it is given, and a catalog lists it
  if its current index has that ID with that sum and length. So a file
  copied from a catalog's server by hand, from a disk or a friend, is
  the catalog's as much as one bundled fetched.
- **`bundle install ID [--from CATALOG]`** reads the catalogs anew, in
  order, the system's first, and fetches the newest version the first
  one that lists the ID has, through the client, which checks its length
  and sums as it comes. Then it installs as a file does, with the
  catalog's name.
- **`bundle search WORD...`** reads the catalogs anew and lists the
  newest version of each bundle whose ID or name holds every word. A
  catalog that cannot be read is left out, and bundle says so.
- **`bundle catalog`** lists the catalogs, on or off, how many bundles
  each lists and when it was last read. **`bundle catalog add NAME URL
  ROOT-FILE`** and **`bundle catalog remove NAME`** are an
  administrator's, through elevate(8). Adding reads the catalog from its
  root before it keeps anything. Removing a default catalog writes
  `off`; removing another removes its folder. Both write a security
  event to the audit log.
- **Trust, in order:**
  - An ID installed from a catalog is bound to that catalog in that
    place, as an ID is bound to its key. From then on it installs only
    from a file that catalog lists, whatever key signed it, so a
    catalog's publisher can move to a new key, and nothing else can
    take the ID.
  - Otherwise an ID bound to a key installs only as that key signs.
  - A file a catalog lists installs as that catalog's, without a
    warning. The sheet says which catalog lists it.
  - Then, as before: the asker's developer key, and any other key after
    the warning, which now also says that no catalog this computer
    trusts lists the file.

## Consequences

- A file is trusted the same way however it came. A catalog only says
  which files are good; what may install them stays bundled's.
- A third-party catalog is as safe as its root's keys. Only an
  administrator adds one, with its root file, and its bundles install
  for anyone without a warning. Its bundles cannot take an ID that is
  already bound to another catalog or key.
- An ID bound to a catalog that is later removed cannot install again
  until a catalog of that name is added back. The binding outlives the
  bundle, as a key's does.
- A catalog whose timestamp expired vouches for nothing, so its bundles
  install with the warning until it is read again.
- bundled now connects out, as root. The client reads only the
  catalogs' addresses and checks every byte against a key before it
  uses it, and a download goes to a file of its own that is removed
  after the install.
- The boot test `catalog-install` signs a test catalog with throwaway
  keys, puts it on a disk, adds it, searches it, installs from it by ID
  and from the disk by file, and checks the warning and the binding.
- **Not yet:**
  - Updates, rollback and holds (13e's next pull request).
  - A bundle's own update feed in its manifest.
  - Dukovo's catalog's first root in the image: it is added when the
    first root is signed with the YubiKeys.
