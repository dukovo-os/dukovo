<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0370. Ports the catalog carries are packed as signed bundles, kept up with their upstreams, and stay out of the image

- Status: Accepted
- Date: 2026-11-03

## Context

The plan's Catalog ports track (Chase, 2026-11-03) brings popular
console tools to Dukovo as bundles of the catalog, never in the base
image: ripgrep and bat first, which 13e seeds its catalog with, then fd,
hyperfine, dust, delta, eza, zoxide, tealdeer and starship, and the C
and C++ tools after them. Phase 13d already reads manifests (ADR 0221),
signs and packs `.dkv` files (ADR 0222), installs them (ADR 0223) and
runs them in a sandbox or, for a command-line tool, with the person's
access, after asking (ADR 0225). What was missing is the way from a
port, which `ob` builds into the sysroot for the image, to a bundle, and
a way to keep each port's pin up with its upstream once dozens of them
are in the catalog.

The signing and packing code lived in systools, a program for Radix,
which `ob` cannot run on the build machine.

## Decision

- **The `dkv` crate** holds what a bundle is as files: the list of its
  files and minisign's signature (`signed`, `sums`, `minisign`), the SBOM
  and the zip. bundle(1) uses it on Dukovo and `ob` on the build
  machine, so both write the same files; the code moved unchanged.
- **A port with a `[bundle]` table** (`id`, `name`, `commands`) is the
  catalog's. Its build.sh installs into `$PORT_BUNDLE`, a program
  `bin/<command>` for each command, and nothing into the sysroot or the
  image. After the ports are built, `ob` copies that folder and adds:
  - `manifest.json`, made from the recipe: its version, summary, license
    and home page, its upstream repository as `source`, the publisher
    "Dukovo catalog", requires_os 0.12, and each command running with
    the access of whoever runs it, as the security design has
    command-line tools run (manifest.json(5)); the install sheet says so
    and the first run asks, with a warning (ADR 0225);
  - `sbom.spdx`, an SPDX 2.3 document of every component the port builds
    in, as the image's license manifest lists them (ADR 0021): the
    source, and for a port in Rust each crate `cargo tree` lists, with
    its crates.io SHA-256;
  - `licenses/`, with the text of each license named and each
    component's notices.
  It checks the licenses against the ports' allow-list and MPL-2.0,
  which is copyleft file by file and whose source is the crate on
  crates.io the SBOM names, signs the folder and packs it into
  `out/bundles/<id>-<version>.dkv`.
- **The key** is the build's, `out/bundles/build.key`, made the first
  time, or the minisign key `OB_BUNDLE_KEY` names. Dukovo trusts neither:
  installing one warns that the key is unknown. The catalog (13e) signs
  what it publishes with its own keys.
- **Licenses decide how a tool is built.** bat and delta call
  ansi_colours, which is under the LGPL; linked statically, it would
  oblige every bundle to ship objects to relink with. Chase chose
  (2026-11-03) that Dukovo write its own: `ports/bat/ansi_colours`,
  Apache-2.0 OR MIT, written from xterm's 256-colour palette alone,
  which bat's build takes with Cargo's `[patch]`. bat is built without
  its Git markers, whose libgit2 carries LGPL code (a question for delta,
  which needs libgit2), and without `--diagnostic`, whose report reads
  Linux's /proc.
- **Keeping up.** `ob ports outdated` asks each port's git repository
  for its tags and lists the ports whose upstream has a newer release of
  the same series (a letter release such as tzdata's 2026f counts;
  snapshots and development versions do not). A workflow runs it each
  Monday and opens, or brings up to date, one issue per port. A person
  moves the pin in a pull request, which CI tests as any other: a bot
  cannot sign off a commit (ADR 0025), and a pull request a workflow
  opens with its own token starts no CI.
- **Advisories.** `ob ports audit` checks each Rust bundle's Cargo.lock,
  with the port's patches, against RustSec with cargo-audit 0.22.1, each
  day. A vulnerability fails it; a crate that is only unmaintained or
  unsound is shown. A port fixes one by moving its pin or with a patch
  to its Cargo.lock, as bat's moves five crates to fixed releases.
- **Tested** by the `catalog-bundles` boot case: with Dukovo's root on a
  disk and the `.dkv` files on another, alice installs ripgrep and bat,
  is told rg runs with her access, allows each on its first run, and
  both work.

## Consequences

- A catalog bundle costs a recipe, a build.sh that installs into
  `$PORT_BUNDLE`, and a line in the boot case; licenses, SBOM, manifest
  and signature come from `ob`.
- `ob ports` builds and packs the bundles too, so CI does, and a pull
  request that breaks one fails.
- The bundles are built for Dukovo 0.12 and say so; the catalog will
  rebuild them for each release.
- 13e's publishing starts from the `dkv` crate and these `.dkv` files,
  re-signed with the catalog's keys.
- **Not yet:** publishing (13e); manual pages inside bundles are kept in
  `man/` but not yet on MANPATH; the C and C++ ports and their shared
  libraries inside bundles (ADR 0071) come with their batches.
