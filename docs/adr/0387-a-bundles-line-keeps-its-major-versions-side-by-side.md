<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0387. A bundle's line keeps its major versions side by side

- Status: Accepted
- Date: 2026-11-03

## Context

A bundle has one version installed in a place, which an update replaces
(ADR 0384). Language runtimes and databases do not work so: a project
written for Node.js 20 or Python 3.12 needs that one, while the next
project wants 22 or 3.13, and each major version is supported for years
beside the others. On Linux, distributions package them as separate
packages (python3.12, python3.13), Homebrew as separate formulas
(`node@20`, `node@22`), and nvm, pyenv and their kind choose between
them for each project from a file the project keeps: `.nvmrc` or
`.node-version`, `.python-version`. The roadmap asks 13e for a
manifest's `line`, which keeps major versions side by side as separate
bundles, with `bundle default` and a project's `.nvmrc` or
`.node-version` choosing between them, and the security design's section
on Node.js (2026-10-26) says how it looks: Node.js 20, 22 and 24 are
separate bundles, each updated within its own line; `node` runs the
version a project names, otherwise your default, set with
`bundle default node 22`; `node-20` and `node-22` always run that
version; and a project that names a version not installed says so
plainly.

## Decision

- **`line` in the manifest** names the major version line a bundle's
  version belongs to: numbers separated by dots, such as `"22"` or
  `"3.13"`. Its version must start with it: 22.11.0 is of line 22, 3.13.1
  of line 3.13. `line_files` names the files a project keeps that say
  which line it wants, such as `[".nvmrc", ".node-version"]`; it needs a
  line.
- **A bundle with a line is a bundle of its own, `ID@LINE`**, as
  Homebrew names its formulas: `org.nodejs.node@22`. That full ID is
  what bundled keys it by: its folder, its data folder, its grants, its
  history, its hold, the version kept to go back to, and the catalog's
  entry for it, which `ob catalog` lists under the full ID. So an update
  stays in the line, line 24 installs beside 22 and never replaces it,
  and removing one line leaves the others. The key and catalog an ID is
  bound to bind all its lines. `bundle install ID` from a catalog takes
  the newest line when the ID has lines and no bundle without one.
- **Lines may add the same commands**, which no two other bundles may,
  and each line's command is linked a second time as `NAME-LINE`,
  `node-22`, which always runs that line. When several installed lines
  of an ID add the command run, the one that runs is, in order:
  1. the one the project asks for: from the working folder up to `/`,
     the nearest folder holding one of the lines' `line_files`; its
     first word, a `v` before it dropped, is a version or a line, and
     the installed line it is of, or the newest of it (`3` takes 3.13),
     runs. A version no installed line is or holds stops the command,
     saying which file asks for it and how to install it, rather than
     running another; a word that is no version, such as `lts/iron` or
     `system`, is passed over.
  2. the default: the person's, then everyone's, set with
     `bundle default NAME LINE [--for-everyone]`, where NAME is the ID or
     one of its commands.
  3. the newest line installed.
  The person's own bundles come before everyone's, as for any command.
  `bundle default NAME` says which line runs in the working folder, and
  why.
- **No services yet** in a bundle with a line: two lines would run
  services of the same names.

## Consequences

- A project's `.nvmrc` works as it does with nvm, with nothing to
  install but the lines and nothing added to the shell's start files.
- An update never moves a project to a new major version: that is a new
  bundle, installed by name.
- A line is not a version range: `^22` or `>=3.12` in a project file is
  passed over, and the default runs.
- The boot test `bundle-lines` installs two lines of a test bundle, runs
  the newest, then the default, then the one a project file names, and
  updates one line without touching the other.
- **Not yet:**
  - A language's package manager installing into a folder of each
    person's for its line (`npm install -g` without elevate), whose
    commands come and go with the line (13e, the next part).
  - Services in a bundle with a line.
