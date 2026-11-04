<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0389. A catalog lists its bundles' known flaws, which the daily check reads

- Status: Accepted
- Date: 2026-11-03

## Context

The roadmap asks 13e for two things about flaws: the catalog flags
bundles whose SBOM lists a known flaw, and the daily vulnerability check
reads installed bundles too. Every bundle carries an SPDX SBOM
(`sbom.spdx`, 13d) that lists what it holds, and for a port's bundle `ob`
writes it from the same data as the image's license manifest. Advisories
come from many databases, which OSV (<https://osv.dev>) gathers in one
format, by ecosystem: RustSec and the GitHub Advisory Database for
crates.io, and the like for npm, PyPI and the others. It names a
component by its ecosystem and name, as a package URL does, and the
versions it affects as ranges or lists.

A catalog's targets are signed offline, by the YubiKeys of ADR 0381,
when bundles are added: advisories change every day. Chase also asked
for a `support` field in the manifest (2026-11-02), an email or web
address, so that problem reports about a catalog app reach its
publisher.

## Decision

- **Components by package URL.** An SBOM names a component an advisory
  can match with an `ExternalRef: PACKAGE-MANAGER purl` line, as SPDX
  2.3 says; `ob` now writes one for each crate from crates.io. `dkv`
  reads them, and matches each with OSV advisories: ranges where
  versions order as semantic versioning orders them (`SEMVER` ranges,
  and the crates.io and npm ecosystems), lists of versions in every
  ecosystem. A withdrawn advisory, or one only informational (a crate no
  longer maintained), is no flaw. How bad a flaw is comes from the
  advisory's CVSS 3 vector, scored as CVSS 3.1 scores it, else from its
  database's word, else it is unknown; malicious code, as OpenSSF's
  database lists it, is critical.
- **The update server lists the flaws each day.** The targets delegate
  a role `flaws` to the online key, the one that signs snapshot and
  timestamp: it may list `flaws/*` alone, and delegates nothing, so the
  key on the server can never list a bundle. Each day the user
  `dukovo-flaws` fetches OSV's export of each ecosystem the bundles need,
  crates.io for now; it alone of the server's jobs reaches the web, and
  it cannot read the key. Then `dukovo-catalog`, with no network, matches
  every bundle the targets list against them, signs
  `flaws/flaws.json` with the role, good for a week, and publishes it
  with a new snapshot and timestamp, after reading all of it back as a
  client does. Advisories more than two days old are not used. A list
  that has expired is left out of the snapshot: the catalog stays good,
  and clients say they cannot tell.
- **The list says what it checked.** It names each flaw by bundle and
  version, advisory and aliases, component, how bad, a line of what it
  is, and the first version of the component without it; and each
  bundle and version it looked at. Of a bundle not looked at, such as
  one added since or one without an SBOM, it tells nothing, rather than
  that it has no flaw.
- **bundled keeps the list with the index.** Each time it reads a
  catalog, it fetches the list through the TUF client, checked by its
  length and SHA-256, and keeps it while it was made less than eight
  days ago; one it cannot fetch is not kept. `bundle search` and
  `bundle list` show a FLAWS column: `-` when nothing tells, `none`, or
  how many and the worst, such as `2, high`. The sheet of a bundle from
  a catalog says what its list knows, and warns of the five worst flaws;
  `bundle info` names them all.
- **The daily check reads installed bundles.** Some minutes after it
  starts, then daily, bundled reads anew each catalog that installed
  bundles come from, whether or not they update by themselves, and
  writes each installed version's flaws to its log and, once for each
  change, to `bundle history` as `flawed`. It changes nothing: removing
  or updating a flawed bundle stays the person's to decide.
- **`support`** in the manifest is an email address or an `https://`
  address, which the sheet shows as where the publisher takes problem
  reports.

## Consequences

- No new request tells anyone what is installed: the daily check reads
  the same metadata every client reads.
- OSV's crates.io export is 3.8 MB, about 2,900 advisories, which `dkv`
  reads in about a second.
- The boot test `catalog-install` signs two test advisories into its
  catalog and checks the flags in `bundle search` and `bundle list`, the
  sheet's warning, and the daily check's history.
- The advisories' texts are credited: RustSec's are CC0-1.0, the GitHub
  Advisory Database's CC-BY-4.0, and OpenSSF's malicious packages'
  Apache-2.0.
- **Not yet:** ecosystems other than crates.io, as bundles need them
  (npm's export has more files than `dkv`'s zip reader takes, which
  needs ZIP64); telling the person of a new flaw beyond the history and
  the log, which 15d's notifications do; and the advisories an update
  fixes, which 15d shows.
