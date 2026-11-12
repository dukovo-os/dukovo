<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0466. Updates come from a repository of releases, through a channel, or from one update file

- Status: Accepted
- Date: 2026-11-12

## Context

A release is a signed list of content-addressed files (ADR 0308), and
`sysupdate stage` builds one into the spare slot from a repository
folder that holds the files the running slot lacks (ADR 0451). ADR 0456
decided how releases reach machines: a machine fetches only the files
it lacks, from any version to any newer one; stable, beta and nightly
channels, each one signed file naming its current release, and moving
from beta to stable never downgrades; one signed update file for
offline machines and mirrors; a release names the oldest version that
updates to it directly, and an older machine goes through a bridge
release first; downloads resume, retry, are hashed as they land and
carry no machine ID.

A repository made by `system-release make` holds one release and is
signed once, for an install stick. An update server holds every
release it offers and is signed again every few hours, so that a
machine can tell it still vouches for them. Its online keys will be
replaced over the years, and a machine that last looked a year ago must
still find its way to the newest root.

## Decision

- **The update repository** holds every release the server offers, laid
  out as an install stick's release is and read by the same TUF client:
  each release's list as the target `system/<version>.json`, a channel
  as the target `channels/<name>.json` holding `{"channel", "version"}`,
  every file once in `files/<sha256>`, and every root by version.
  `system-release` gains its commands:
  - `add` adds a release that `make` signed, after reading it from the
    first root and checking every file, and refuses a version the
    repository offers with another list: a number is never used twice.
  - `channel` points stable, beta or nightly at a release it offers.
  - `refresh` signs a new snapshot and timestamp.
  - `keys --rotate ROLE` replaces a role's key and signs the next root
    with the root key before it, and with the new one when the root key
    is the one replaced (TUF 5.3.4); every root is kept by version, and
    `make` and `add` publish them all.
  Each signs the next version of what it changes and writes the
  timestamp last, so that a client reading the repository while it
  changes sees the old one or the new one.
- **`sysupdate check`, `download` and `stage`** read the server through
  the TUF client from the root the system was built with, the first
  time, and from the trusted metadata kept in
  `/system/state/sysupdate/metadata` after that, so that a server, or
  anyone between, can never take a machine back to older metadata;
  expiry is checked, as it is online. The server and the channel are
  `/system/settings/sysupdate/updates.toml`'s `server` and `channel`,
  by default `https://updates.dukovo.org/system/` and stable;
  `sysupdate channel NAME` changes the channel.
- **Never back:** a channel that offers the running version or an older
  one offers nothing: the system waits until the channel passes it, so
  moving from beta to stable never downgrades. `--version` asks for a
  release by its version, under the same rule.
- **Bridges:** when the release offered updates only from a version
  after the running one, the release it names comes first, and so on,
  at most 16 steps; a bridge that is not between the two is refused.
  The next update then takes the newer release.
- **Only the files the running slot lacks are fetched.** A file at the
  same place with the same size and SHA-256 is kept by the stage; a file
  the slot holds elsewhere is copied; only files whose size matches a
  wanted one are read. The rest are fetched from `files/<sha256>` into
  `/system/state/sysupdate/files`, each hashed as it lands, refused if
  it is longer than its list says or not its SHA-256, and kept whole or
  not at all. A fetch cut short goes on from where it stopped with a
  range request, up to four tries with waits of 2, 4 and 8 seconds; a
  file the server does not hold, or one that is not the file listed, is
  not tried again. Requests carry no machine ID, account or cookie. A
  stage removes the files once the slot holds them, before the state is
  snapshotted for a fallback.
- **The update file** is a release's repository as one POSIX tar
  archive (ustar), uncompressed: its metadata with every root, its list
  and every file it lists, once, in an order that is always the same,
  so that one repository always makes the same bytes. It is read in
  place, each file at its offset, with nothing unpacked but the
  metadata, and checked offline as an install stick is: every
  signature, version and hash, but not expiry. `system-release pack`
  writes one and reads it back; `system-release check`, `sysupdate
  stage` and Rescue's reinstall take one wherever they take a
  repository folder. Only folders and regular files with relative names
  are read; anything else in the archive is refused. Linux's `tar`
  lists and unpacks it, which makes a mirror.
- **Tested against a local server.** The boot test `update-online` serves
  a repository over HTTP from the test harness: three releases, two
  channels, and the timestamp and snapshot keys replaced twice since
  the root the build trusts, signed with the build machine's keys. It
  refuses a changed file, a changed list and a replayed timestamp,
  resumes a fetch cut short, counts the files fetched against those the
  slot lacks, stages a bridge and proves it. `update-offline` stages the
  bridge's update file with no network.

## Consequences

- A machine a year behind walks the roots to the newest and takes each
  bridge in turn; each step is a release that was tested on its own.
- A check reads every file of the running slot whose size matches a
  wanted one, as a stage does; keeping each slot's list, so that a
  check reads nothing, is deferred.
- The trusted metadata lives in `@state`, which a fallback restores to
  what it was before the update: older trusted metadata, which the next
  check brings up to date.
- An update file is about as large as the system, 210 MiB, since it is
  not compressed; Btrfs compresses it on a disk, and a USB stick holds
  it.
- **Not yet:** the server side on updates.dukovo.org, with its online
  keys for snapshot and timestamp and the nightly channel's delegated
  role, comes with the next pull request, and the production root on
  the hardware keys before 1.0.0-alpha.1 (ADR 0457). Waves, pause and
  pull are ADR 0456's and come later in Phase 15.

## Alternatives

- **A compressed update file** (zstd) would be about half the size, but
  could not be read in place: a stage would unpack 210 MiB first, or
  read it through twice.
- **Each file a TUF target** would sign every file's hash in the targets
  metadata, 2,100 entries a release, which every machine fetches at
  every check; the release list does the same once, as one target.
- **Hashing the whole running slot at each check** finds moved files by
  hash too, but reads 210 MiB where a size match reads only what may be
  the same.
