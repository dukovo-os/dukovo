<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0595. A rebuild ships as its version's next build

- Status: Accepted
- Date: 2026-11-20

## Context

ADR 0594 keeps a version the catalog lists as it was signed when a port
packs it again as other bytes, and a rebuild reached computers only under
a new version. This record amends that: a version packed again with other
files ships as its next build.

A version once signed is never signed again as other bytes (ADR 0381):
computers that took the signed bytes would otherwise see a file that no
longer matches the digest they hold. So a rebuild needs a version of its
own. A recipe's version is the upstream one, and a fix to the C library
changes the files of a port with no upstream release to name it.

Every port packs as other bytes after every commit, since a bundle is
signed at the commit's time and the time is in the bytes. A rebuild
cannot be told from a repack by its bytes; only the files in the bundle
tell them apart.

## Decision

- **A version may end in a build number.** `VERSION_N`, with N from 2 to
  9999, written last, after any pre-release and metadata: `15.2.0_2`,
  `19.0-rc.2_2`, `1.0.0+meta_3`. Build 1 is the plain upstream version,
  written with no `_1`. `_0`, `_1`, `_01`, `_10000`, a bare `_`, `__2`,
  `_2_3` and `_x` are refused. Neither a semantic version's pre-release
  nor its metadata may hold `_`, so the last `_` splits a version without
  doubt.
- **Order.** Versions order by semantic versioning's precedence first
  (numbers, a missing one counting 0; a pre-release before its release;
  metadata ignored), then by build, a missing build counting 1. So
  `15.2.0 < 15.2.0_2 < 15.2.0_10 < 15.2.1-rc.1`, and `19.0-rc.2_2 <
  19.0`. One function, `manifest::bundle::compare_versions`, orders every
  version: `bundle update`, the catalog and bundled.
- **The catalog gives the number when it signs.** `./ob catalog sign
  --next-build ID --rebuild-reason TEXT` (and with `--preview`) packs the
  recipe version of each named ID again as one past the newest build the
  targets list of it, a withdrawn build counted so that a number is never
  given twice. The bundle is `out/bundles/ID-VERSION_N.dkv` and the reason,
  one line of at most 200 bytes, is `ID-VERSION_N.reason` beside it. A
  recipe states no build number.
- **Files decide.** catalog-repo compares a pack of a listed version with
  the newest listed build of it file by file, by the signed `files.sha256`
  lists, reading the signed bytes from the store. The SBOM is left out
  because it holds the packing time, and the manifest counts only when it
  differs in more than its version. A pack with the same files is not a
  rebuild and is refused as one; a pack with other files is listed with the
  files that differ and publishes as the next build. When the signed bytes
  are not in the store the files are not compared, and the line says so.
- **The reason.** A rebuild of the catalog is signed with a reason, which
  the target records as `custom.rebuild.reason`; catalog-repo's `sign`
  refuses one without. Computers show it where they offer the update.

## Consequences

- A signer reads the `=` lines of a signing, packs the rebuilds it wants
  with `--next-build`, reviews the plan, and signs it with `--expect` as
  for any batch. The plan marks such a target `rebuild` and names the
  version it replaces, its files and its reason; the digest and the plan's
  format are unchanged. Dukovo Ops will let the signer tick rebuilds and
  give the reason.
- On computers `bundle update --check` ends the row of an update that
  differs only in build with `(rebuild)` and prints the reason under it;
  the web console calls it a rebuild beside a fix only and a new version.
  A rebuild rolls out in waves (ADR 0544), can be withdrawn (ADR 0593) and
  is rolled back, with the data its older version left (ADR 0384), as any
  version is. A feed publisher (ADR 0390) may write `1.2.0_2` by hand;
  `bundle feed` compares its files with the build it follows in the
  feed's store, and signs a reason when one is beside the file.
- Left out: epochs, since no version has needed to go below an earlier one;
  build numbers for the system's own versions, whose fourth number is the
  OS build; and numbering of a build taken out of the targets with
  `--remove`, which could be given its number again (docs/deferred.md).
- `manifest_version` stays 1, which is a preview until 1.0.0.
- Unit tests in manifest cover the grammar and the order, in catalog-repo
  the file comparison, the plan and the refusals, and in xtask the numbers
  given; the `bundle-feed` boot test installs a rebuild from a feed.
