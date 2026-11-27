<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0626. A Linux view's shell and core commands are Nukesh and uutils

- Status: Accepted
- Date: 2026-11-27

## Context

A Linux program's view (ADR 0622) holds the Linux base's `/usr`, which
has libraries and no programs. Linux programs expect a few programs at
fixed paths: `system()`, `popen()` and Node's `child_process.exec` run
`/bin/sh -c`; a script starts with `#!/bin/sh`, `#!/bin/bash` or
`#!/usr/bin/env node`; and npm's install scripts, Makefiles and build
tools run `mkdir`, `cp`, `rm`, `cat` and `uname` by name. Without them
`npm install` fails at its first package with an install script.

The programs could come from a distribution in the base bundle, but
Dukovo ships no GPL code in its base system, and two shells and two sets
of core commands would differ in the small ways scripts trip on. Dukovo
has its own: Nukesh, which is POSIX's shell as `sh` and bash's as `bash`
(ADR 0083), and uutils' coreutils and findutils, which are MIT-licensed
and made to match GNU's behaviour.

## Decision

- **What every view holds.** Of `nukesh`, `coreutils`, `find` and
  `xargs`, those that `/system/commands` has are attached read-only at
  their own paths, with `/system/libraries`, the dynamic linker and C
  library they load. The links are in `/usr/bin`, which `/bin` leads to
  as on Linux: `sh` and `bash` to Nukesh, `env` and each utility the
  coreutils port builds to its one program, `find` and `xargs` to theirs.
  A static program's view has them too.
- **A base keeps its own.** A Linux base with a `usr/bin` of its own
  gives the view its commands, and nothing is added beneath it. The base
  Dukovo builds has none. With the shell's links beneath `/usr`, the base
  is attached entry by entry, as it is for Dukovo's zone database.
- **The programs run natively.** The kernel starts each by its ELF note
  as Dukovo's (ADR 0610), in the view, with the view's files. A Linux
  program they start runs as a Linux program in the same view, as a
  process keeps the slot of the view's linuxd (`SLOT_LINUX`) through
  `fork` and `exec`.
- **Nukesh in a view.** The launcher sets `DUKOVO_LINUX_VIEW=1`. Nukesh
  runs a Linux program it cannot start through `linux run` (ADR 0622)
  only outside a view: a view has no `linux` command, and a second view
  would lose the first one's files.
- **`/system` is the view's own.** No path beneath it is the person's to
  give with `--allow`, as for `/usr` and `/etc`.

## Consequences

- `sh -c`, `#!/usr/bin/env node`, npm's install scripts and the core
  commands work in every Linux view and in Linux bundles (ADR 0625).
- The coreutils port's list and the view's are one: a unit test reads
  the port's build script.
- The commands a person types in a view are Nukesh's and uutils', not
  GNU's; differences between them are bugs to fix in those programs.
