<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Security advisories

This folder holds the Dukovo Security Advisories, DKSA (ADR 0534): one
file for each flaw that a release or an app update fixes. It holds nothing
until the first advisory is published. The runbook
[Publish a security advisory](../../docs/runbooks/security-advisory.md) goes
from a report to a published advisory.

## Format

An advisory is one [OSV](https://ossf.github.io/osv-schema/) 1.6 file, so
that osv.dev and any OSV scanner read it as it is. The prefix DKSA is
Dukovo's own: OSV already uses DSA for Debian's.

| Field | Meaning |
| --- | --- |
| `schema_version` | Always `"1.6.0"`. |
| `id` | `DKSA-YYYY-NNNN`, the file's name without `.json`. |
| `modified`, `published` | RFC 3339 times in UTC, such as `2026-11-10T09:00:00Z`. |
| `summary` | What is wrong, in one line of at most 120 characters. |
| `details` | What the flaw is, which releases have it, who can use it and what it does, in plain sentences. |
| `affected` | The packages and versions with the flaw (see below). |
| `database_specific.severity` | `critical`, `high`, `medium` or `low`. |
| `database_specific.action` | Optional: what to do, in a sentence of at most 300 characters. |
| `aliases` | Optional: other names for it, such as a CVE or a GitHub advisory. |
| `references` | Optional: links to the report, the fix and the discussion. |
| `severity` | Optional: CVSS vectors. |
| `credits` | Optional: who found it. |

An `affected` entry names one package of the ecosystem `Dukovo`: `dukovo`
is the system, and a catalog app is named by its ID. Its ranges are of
the type `ECOSYSTEM`, with the events `introduced`, `fixed` and
`last_affected`, in Dukovo's version order: 1.4.1 comes before 1.4.1.1,
which comes before 1.4.2, and a pre-release comes before its release. An
`introduced` of `"0"` means from the first version. A range with no
`fixed` and no `last_affected` has no fix yet.

```json
{
  "schema_version": "1.6.0",
  "id": "DKSA-2026-0001",
  "modified": "2026-11-10T09:00:00Z",
  "published": "2026-11-10T09:00:00Z",
  "summary": "A local user can read another user's files",
  "details": "...",
  "affected": [
    {
      "package": { "ecosystem": "Dukovo", "name": "dukovo" },
      "ranges": [
        {
          "type": "ECOSYSTEM",
          "events": [{ "introduced": "1.4.0" }, { "fixed": "1.4.2" }]
        }
      ]
    }
  ],
  "database_specific": {
    "severity": "high",
    "action": "Update to Dukovo 1.4.2 with `update apply`, then restart."
  }
}
```

## Numbering

`YYYY` is the year of the advisory's first publication, and `NNNN` counts
from 0001 within that year. A number is used once: an advisory that turns
out to be wrong is corrected in its file, with a new `modified`, and keeps
its number. `./ob security advisory new` writes the next free number from
a template; text in it that still says TODO has to be written before
`./ob security check` accepts the file.

## Publishing

`./ob security check` reads every `DKSA-*.json` here as the release keys'
signing does: the file is valid OSV with these fields, its `id` is its
file name, no number is used twice, the versions are of Dukovo's order,
and no text says TODO. CI runs it on every pull request.

The release keys sign the advisories as targets of the update
repository, `advisories/index.json` and `advisories/DKSA-YYYY-NNNN.json`,
in one targets version:

- `./ob release channel` signs them with the channel that offers the
  fix, so that a machine that takes the release also reads the advisory.
- `./ob release advisories` signs them alone, for a corrected advisory,
  or one for an app, that goes out without a release. It signs nothing
  when they are as the repository has them.

A file removed from this folder leaves the repository's targets at the
next signing. Machines read the index and the advisories through their TUF
client and match them against what they run: `update advisories` says
which apply, and sends nothing about the machine.

People read them in the support center, which `./ob docs site` builds from
this folder: a page for each advisory, newest first, and the RSS feed at
<https://dukovo.org/support/security/feed.xml>.
