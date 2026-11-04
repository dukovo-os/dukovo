<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0391. An unsigned bundle installs only with an override that signs it here

- Status: Accepted
- Date: 2026-11-04

## Context

The roadmap asks 13e to keep sideloading open: "a bundle from outside
any catalog installs if a developer key signed it, after a warning that
shows the publisher and the permissions it asks for; an unsigned bundle
needs an explicit per-bundle override". The Phase 13 exit asks that
"unsigned bundles are refused under the default policy". ADR 0223 left
the override for later.

Every installed bundle is bound to the key that signed it (ADR 0223):
its grants, its updates and who may install its ID again all rest on
that key, and bundled checks the signature and every file in an
unpacker that holds nothing else (ADR 0222). A bundle with no signature
has no key to bind to. Today such a file is refused with "it is not
signed: it has no signature", and a folder with none is signed by
`bundle pack` with the developer key, which only this machine trusts
and only for the person who made it (ADR 0222).

## Decision

- **The override is `bundle install --unsigned FILE.dkv`**, given for
  each bundle and each install. Without it, a file with no signature is
  refused as before, and bundle says that `--unsigned` installs it if
  the person trusts where it came from.
- **It signs the file here.** bundle checks the file first: one that is
  signed installs as any file does, and one whose signature fails is
  refused, since someone changed it after it was signed. One with no
  signature and no list is read whole, given an SBOM if it has none, and
  signed with the person's developer key into a copy in their temporary
  folder, which only they see; that copy is installed and then removed.
- **It warns first.** bundle says that nothing vouches for the file,
  neither who made it nor that it is as they made it, and that it is
  signed by the developer key, which only this computer trusts. The
  install sheet then shows its permissions and asks, as for any file.
- **bundled is unchanged.** It installs only signed bundles: the copy
  is checked, unpacked and sealed as any file is, bound to the
  developer key.

## Consequences

- One trust model stays: every installed bundle is bound to a key, and
  bundled, its unpacker and its grants never see an unsigned bundle.
- The bundle installs as the person's own build: bundle list and the
  sheet show their developer key. A later version of it needs
  `--unsigned` too, and the publisher's own signed version installs only
  once this one is removed, since the ID is bound to the developer key.
- An administrator can install one for everyone through elevate, as
  with any bundle their developer key signs.
- `dkv::signed::sign_archive` signs an archive a file at a time, and
  refuses one that carries a signature or a list.
- The `bundles` boot test zips a bundle by hand with Info-ZIP's zip: it
  is refused, then installs with `--unsigned`, after its warning, and
  runs.
