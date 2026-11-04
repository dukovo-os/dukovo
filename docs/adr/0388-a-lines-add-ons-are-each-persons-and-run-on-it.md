<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0388. A line's add-ons are each person's, and run on it

- Status: Accepted
- Date: 2026-11-03

## Context

A language's package manager installs tools for everyone who uses the
language: `npm install -g typescript`, `pip install --user`,
`cargo install`, `gem install`. On Linux, `npm install -g` writes into
the folder Node.js was installed in, so it needs `sudo`, and what it
installs then runs as root's or breaks at the next Node.js update; nvm
and Volta move that folder into the person's home for each Node.js
version instead. The roadmap asks 13e for the same: a language's package
manager installs into a per-user add-ons folder, `npm install -g` needs
no elevate, and its commands appear on PATH and leave with that version.
The security design (2026-10-26) adds three rules: PATH never changes, as
three command folders on it from the first boot that only the runtime
writes; a tool keeps the version it was installed with, as Volta pins
one; and commands that add-ons create cannot hide a system or bundle
command. Lines (ADR 0387) keep each major version a bundle of its own,
`ID@LINE`, which is what the add-ons belong to.

## Decision

- **`add_ons` in the manifest** of a bundle with a line names the
  environment variables that tell its package manager where to install,
  `{"variables": ["NPM_CONFIG_PREFIX"], "commands": "bin"}`, and the
  folder in it where commands go. It needs a line and a command that
  runs with your access, and names none of the variables the system sets
  (`PATH`, `HOME`, `LD_*`, `LC_*` and the like).
- **The folder is the person's own for the line**:
  `~/applications/add-ons/ID@LINE`, by full ID, as app data is, so two
  publishers' bundles never share one. The security design wrote it as
  `node-22`; the full ID is that, made unique. When a command of the
  line that runs with your access starts, the launcher makes the folder
  if it is not there and sets the variables to it, so
  `npm install -g` installs there with no elevate. Through `elevate`, an
  administrator's add-ons are everyone's, in
  `/applications/add-ons/ID@LINE`.
- **Commands are linked as they appear.** A line with add-ons is the one
  case where the launcher starts the command and waits instead of
  becoming it: when the run ends and the folder of the add-ons' commands
  changed, it asks bundled to link them. bundled, which alone writes the
  command folders, links each name there beside the place's commands,
  reading only names, through folders each the place owner's and never
  through a link, and links none named as a system command. Every link
  points at the launcher, as a bundle's do.
- **An add-on runs on its line.** Asked for a command no bundle adds,
  bundled answers with the add-on of that name of the asker's lines,
  their own before everyone's, so a bundle's command always comes first.
  The launcher runs it with the person's access, as a program of theirs,
  with its line first in `BUNDLE_LINES`; when it runs a command of that
  ID with several lines, that line runs, before a project's file or a
  default. `tsc` added with Node.js 22 keeps running on 22 when 24
  becomes the default.
- **Add-ons are the line's data.** Removing the line takes their
  commands away; `--purge` removes the folder too, and `--keep-data`
  keeps it for the line's next install.

## Consequences

- `npm install -g` works as it does with nvm, without elevate and
  without editing PATH or a shell's start files.
- A run of a line's command with add-ons costs one process more than
  others, which waits for it; others still become the program.
- A command that an add-on adds while the package manager is still
  running, or that a person adds by hand, is linked when the next run of
  the line's command ends.
- The boot test `bundle-lines` adds a command with line 1's package
  manager, runs it on line 1 while line 2 is the default, checks that an
  add-on named as a system command is not linked, updates line 1 with
  it, and removes line 1 with its data and its add-ons.
- **Not yet:** removing an everyone's line removes no person's add-ons
  for it; they come back with the line.
