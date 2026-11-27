<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0627. Node.js is a bundle of Node.js's own Linux build, one port per line

- Status: Accepted
- Date: 2026-11-27

## Context

Node.js is what most web and tooling work on a workstation runs: npm,
build tools such as esbuild, Rollup and SWC, and the editors' language
servers. A port built for Dukovo would mean porting V8, which generates
machine code for each operating system's calling conventions and signal
frames, and keeping that port current with two release lines a year. A
bundle may now hold Linux programs, which run in a Linux view on the
Linux base (ADR 0625), and V8 may make its code once the person allows
`memory.jit` (ADR 0624). Node.js publishes a build for x86-64 Linux with
every release, linked against glibc, with npm in it, and signs the
checksums of its archives.

Projects pin a major version in `.nvmrc` or `.node-version`, and two long-
term support lines are current at once: 24 and 26 today. A package that
ships a native binary, such as esbuild or sharp, picks it by the
platform Node.js reports, `linux-x64`.

## Decision

- **The release, as Node.js made it.** `ports/node-24` and
  `ports/node-26` fetch `node-vX.Y.Z-linux-x64.tar.xz` from nodejs.org,
  pinned by the SHA-256 that the release's signed `SHASUMS256.txt` gives;
  the recipe names the releaser's key that signed it. Nothing is built.
  build.sh keeps the program, npm and the modules it comes with, the
  headers that native add-ons build against, the manual page and the
  license file.
- **One port per line.** Each line is a port of its own with the same ID,
  `org.nodejs.node`, and its own `line`, so it is packed and published as
  `org.nodejs.node@24` and `@26`, and it updates within its line (ADR
  0387). `.nvmrc` and `.node-version` choose the line in a project. `ob`
  refuses two ports of one ID and line, packing a line removes only that
  line's older files, and `ob catalog sign --next-build` takes
  `ID@line`. `series` keeps `ob ports outdated` within the line.
- **A Linux bundle, manifest version 2.** `runtime` is `linux`, and `ob`
  checks that no program in it is Dukovo's, the other way round from a
  native bundle's check. Its commands `node`, `npm` and `npx` run with
  your access, in the folder they are started in, and it asks for
  `memory.jit`, which it needs. The release's `npm` and `npx` are links,
  which a bundle cannot hold; each becomes a script for Node.js that loads
  npm's own from its place, so npm finds the rest of itself as on Linux.
  Node.js 24's corepack stays as a module and is no command, as it would
  write its links into the bundle.
- **Add-ons.** `npm install -g` installs into the line's add-ons folder,
  which `NPM_CONFIG_PREFIX` names, and its commands are in that folder's
  `bin`.
- **Licenses.** The license file goes in the bundle and its SBOM. Node.js
  is MIT, with what it includes under the MIT, Apache 2.0, BSD, ISC,
  zlib and Unicode licenses and CC0; npm is under the Artistic License
  2.0 and its modules under the ISC, MIT, Blue Oak Model, Apache 2.0 and
  BSD licenses, with CC-BY-3.0 and CC0 data. The Artistic License 2.0 and
  the Blue Oak Model License join the bundles' allow-list.

## Consequences

- A new Node.js release is a new archive and checksum in the line's
  recipe, taken from its signed list, and a new line is a new port.
- The boot test `linux-node` installs both lines, chooses one by
  `.nvmrc`, runs `npm ci` with install scripts, esbuild, Rollup, SWC and
  sharp from an npm cache, and an `npm install -g` command.
- A manifest's `source` for a port of a release archive is the GitHub
  tree of its upstream's tag, here `nodejs/node` at `vX.Y.Z`.
- Node.js's own `process.platform` is `linux`, so packages install their
  Linux binaries, which run in the same view.
