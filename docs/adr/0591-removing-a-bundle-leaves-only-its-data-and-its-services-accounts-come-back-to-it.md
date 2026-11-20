<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0591. Removing a bundle leaves only its data, and its services' accounts come back to it

- Status: Accepted
- Date: 2026-11-20

## Context

ADR 0385 has a removal of a bundle's services, without `--purge`, keep
their accounts and data folders, so that installing the bundle again
finds them as they were. That keeps more than data: the accounts stay in
`/system/settings/passwd` and `group`, with a socket group for each
service that offers sockets, and their folders stay in
`/system/live/services`. A removal that is meant to take an app away
leaves names and numbers that nothing uses. ADR 0223 says what a removal
takes away, and a person who removes an app expects it to be gone, but
for the data they chose to keep.

Taking the accounts away would lose the data, as ADR 0220 retires an
account's number: an install made afterwards would give a new number,
the data folder, owned by the old one, would be "in the way" (ADR 0385),
and the install would stop. So the accounts of a bundle installed again
have to take their old numbers.

This record amends ADR 0385 (removing, and the numbers a removal keeps)
and ADR 0223 (what a removal leaves).

## Decision

- **A removal of a bundle's services takes their accounts, always.**
  bundled stops each service, takes its file and its record away, has
  authd remove the service's account, which retires its number and its
  socket group's, and removes the service's folder in
  `/system/live/services`, which Spark made the account's. Its data,
  `/shared/services/ID`, stays, owned by the retired numbers, and the
  removal says so; `--purge` removes it too.
- **An account made again for a bundle takes its old number.** When
  bundled asks authd for a service's account, authd gives it the number
  that owns its data folder, `/shared/services/ID/ACCOUNT`, if the
  retired numbers hold that number under that account's name and no
  account or group has it now. Only bundled makes `/shared/services/ID`,
  so a folder owned by a retired number of the account's own name is that
  account's own data. The service then runs on it as before. A folder
  owned by another number is not the account's, and the install stops as
  ADR 0385 says.
- **A person's grants file goes with the last of its records.** What a
  person said of a bundle's permissions goes with the bundle's data, at
  `--purge` (ADR 0224). When that leaves none, bundled removes the person's
  file in `/system/state/grants`, which it made at their first answer,
  instead of leaving it empty.
- **What outlives a removal with `--purge` is records, and the folders
  they and the apps' data are in.** The paths of the system and of
  `/shared` differ from what they were before the install by these only:
  - bundled's registry files in `/system/state/bundled`: the bindings of
    an ID to its publisher's key, for everyone and for each person, and
    to the catalog it came from (ADR 0383), which outlive the bundle so
    that a later bundle of the ID from another key is refused, and
    bundled's history of what it did; the other files of the registry
    are written whole at each change and are empty when nothing is
    installed;
  - the folder bundled keeps the records of its service files in;
  - authd's retired numbers (ADR 0220), which are never given again;
  - `/shared/services`, the folder of every service bundle's data, made at
    the first install of one;
  - the system's log and the audit log in it, which keep what was said
    while the bundle was there, and the records the system's other
    services keep as it runs;
  - in a person's home, the folders `app-data`, `app-data/.snapshots`,
    `applications` and `applications/commands`, which the first install
    and the first run of an app make and which are no one bundle's.
  A removal without `--purge` leaves, besides these, the data: for a
  bundle with services, `/shared/services/ID`; for a person's bundle,
  `~/app-data/ID`, its copies and what the person said of it.

## Consequences

- The accounts and groups of a removed bundle are gone from the account
  files at once, and their names are free again.
- The numbers of a bundle's accounts are retired by every removal, and a
  bundle installed again takes them back only while its data folder is
  theirs. A bundle purged, and installed again, takes new ones; the range
  of 900 numbers (ADR 0385) is used up by purging, not by removing and
  installing.
- A reinstall after a removal without `--purge` is an install that finds
  its data, as an update does.
- The boot test `bundle-leaves-nothing` records every path of the system
  and of `/shared`, with its type, mode, owner, group and size, before
  ripgrep for a person and beacon for everyone are installed and after
  each removal, with and without `--purge`, and fails on any difference
  that the lists above and the kept data do not name. It also installs
  beacon again and checks that its account has the number it had and that
  it starts on its data. The lab test `bundles/every-catalog-app` does
  the same for every bundle the catalogs list.
