<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0380. TUF checks what catalogs and updates say, with a client of our own

- Status: Accepted
- Date: 2026-11-03

## Context

Phase 13e brings catalogs, bundle updates and the system's own updates
(the updates design, section 8). Each comes from a server, and each
server can be taken over or replayed. The Update Framework (TUF) is the
design that stands up to that: a root's keys vouch for the keys of four
roles (root, timestamp, snapshot and targets); timestamp and snapshot
expire within days, so an old repository cannot be served again; and
targets roles can delegate parts of the repository to others, such as
the catalog's, firmware's and rollout's roles in the updates design.
Bundles' minisign signatures (ADR 0222) say who made a bundle; TUF says
whether it is the one the catalog lists now.

Chase chose (2026-11-03) to use tough, AWS's TUF library, and to write
Dukovo's own client if tough would not do, with the TUF conformance
suite in CI either way.

tough builds on aws-lc-rs, AWS's build of BoringSSL's C code, which
does not build for Radix. Its API is ring's, and a 60-line patch builds
tough on ring, which passes tough's own tests. The conformance suite
then failed 32 of its 119 tests. Some were its storage (it writes
re-encoded metadata, and a file of its own beside the roles), but most
were how it follows delegations: it loads every delegated role up front,
and does not walk them depth first, in order, stopping at a terminating
role, as the specification's client workflow does (TUF 5.6.7). That walk
is what keeps a role from listing targets that were delegated to
another, so it matters for the delegated roles the updates design uses.
Fixing it would have been a fork of tough, and tough also brought 55
crates.

## Decision

- **The `tuf` crate is Dukovo's TUF client**, for catalogs, the system's
  updates and a bundle's own feed alike. It follows the specification
  (version 1.0) and the client workflow as python-tuf's `ngclient`, the
  reference client the suite was written around, runs it:
  - **The trusted set** holds what one update has checked, root first.
    Each new root is signed by the old root's keys and its own and is
    one version on, up to 256 at once; the final root must not have
    expired. A timestamp, snapshot or targets role joins only once it is
    signed by its threshold of keys, is no older than what is trusted,
    and matches the length and hashes the role above it gives. An
    expired timestamp or snapshot still joins, so that it guards a newer
    one against rollback, but nothing below it loads.
  - **Delegations** are walked depth first, in their order, at most 32
    roles, stopping at a terminating one; a role is delegated paths by
    shell patterns, part by part, or by hash prefixes, or is a hash bin
    of succinct roles (TAP 15).
  - **The directory** of a repository keeps the metadata exactly as it
    came, one file per role by its name quoted as python-tuf quotes it,
    each written whole. The next update starts from it, and fetches a
    snapshot or targets role again only when it is no longer the one.
  - **Limits**, python-tuf's: 512 KB for a root, 16 KB for a timestamp,
    and 2 MB for a snapshot and 5 MB for a targets role when the role
    above does not give their length. A fetch longer than its limit is
    cut off and refused.
  - **Keys and hashes** are checked with ring: Ed25519, ECDSA on P-256
    and P-384, and RSA with PSS or PKCS #1 v1.5 padding and SHA-256, 384
    or 512, as securesystemslib names them; SHA-256, SHA-384 and SHA-512
    hashes. A key of another type is read, but a signature by it never
    counts. Signatures are over the OLPC canonical JSON of the role's
    `signed`, made from the file's own JSON.
  - **Fetching** is its own: HTTP/1.1 over TCP or rustls on ring with
    the system's roots (ADR 0192), one connection per file, five
    redirects at most, and 403, 404 or 410 as "not there", which ends
    the search for a next root; and file URLs. What a file says is
    checked by TUF, not by how it came.
- **It brings no new crate**: ring, rustls, serde, serde_json and base64
  are already the programs'. Its URLs, HTTP, canonical JSON and DER are
  small parts of its own.
- **`ob test tuf`** runs tuf-conformance, pinned by its commit, against
  the client's conformance CLI, `tuf-conformance`, built for the host,
  in a CI job of its own. The tests the client is expected to fail are
  listed, each with why, in `userspace/programs/tuf/tuf-conformance.xfails`;
  one that passes fails the run. Only ML-DSA is listed: ring has none,
  and Dukovo's repositories sign with Ed25519 and P-256. All 116 others
  pass.
- **Its unit tests** run with the programs' host tests: canonical JSON,
  times, keys, the metadata's checks, path patterns and hash bins, and
  HTTP's framing and redirects against a local server.

## Consequences

- The suite now tests the client against every case it has: rollback
  and freeze attacks, key rotations, expired and mismatched metadata,
  delegations, odd role names and endless data.
- The updates design's delegated roles, and a third party's feed, are
  read by the same code as the catalog's.
- A new key type needs ring to have it, then a line here.
- **Not yet** (13e's next pull requests): the repository's side, signing
  with the YubiKeys and the key-setup and batch-sign commands; the
  update server on updates.dukovo.org; catalogs, installs and updates
  that use the client; the DSSE envelope, which no Dukovo repository
  uses.
