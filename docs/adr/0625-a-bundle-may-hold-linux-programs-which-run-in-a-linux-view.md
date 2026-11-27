<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0625. A bundle may hold Linux programs, which run in a Linux view

- Status: Accepted
- Date: 2026-11-27

## Context

`linux run` starts a Linux program in a view of its own, with linuxd
beside it (ADRs 0621 and 0622), but only a program the person names by
its path. The programs Phase 18 brings are made for Linux and kept up by
others: Node.js above all, which no port of ours could follow release by
release. They should install, update and run as bundles do: from the
catalog, in lines (ADR 0387), with add-ons (ADR 0388), with the
permissions their manifest asks for, and with commands a person types.

A manifest had no way to say its programs are Linux programs. The kernel
tells the two kinds apart by the ELF note and interpreter (ADR 0610), so
a Linux program in a bundle would start, but as a sandboxed program of
Dukovo's, in a view with no `/usr`, no `/proc` and no linuxd, where it
fails at its first library.

## Decision

- **Manifest version 2.** `manifest_version` may be 2, and `runtime` is
  its one new field: `"native"`, the default, or `"linux"`. A version 1
  manifest is a version 2 manifest without it, and reads as before;
  `runtime` in a version 1 manifest is refused with the version to give.
  `bundle upgrade-manifest` changes a version 1 manifest's number in
  place and nothing else. The JSON schema `bundle schema` prints is
  `https://dukovo.org/schemas/manifest-2.json`, and takes both versions.
- **What a Linux bundle holds.** Each program is an x86-64 Linux program
  or a script: `bundle check` and `bundle sign` read each program's
  headers with the kernel's own reader (`radix-elf`) and refuse one that
  `exec` would run as Dukovo's ("a Linux bundle holds Linux programs").
  Its programs run on the Linux base, which `bundle install` names when
  it is not installed. It may not have services yet, nor ask for
  `devices.serial` or `keyring.shared`, which a Linux view does not give.
- **How its programs run.** `bundle run`, a command's link and an add-on's
  command start a Linux bundle's program through the same code as `linux
  run`: its view, linuxd beside it, the Linux base's `/usr`. The bundle's
  folder is attached read-only at its path, from the folder that was
  checked, not opened again by name. `PATH` names the folders of the
  bundle's commands first, so that a script's `#!/usr/bin/env node`
  finds the bundle's `node`.
  - A program in the bundle's sandbox gets what the person gives of what
    it asks, as a native one does: its data folder
    (`~/app-data/ID`, its `HOME`) where it starts, the folders, `/shared`
    folders, drives and other apps' data given, the network given, and
    memory.jit when it is granted (ADR 0624). It is stopped when a
    permission it holds is taken away or its version withdrawn (ADR 0593).
  - A command that runs with your access gets all of your files, the
    folder it was started in, the network, `HOME` your own, the question
    about files its arguments name outside its view, and memory.jit when
    the bundle asks for it and you allowed it. That is how `node`, `npm`
    and `npx` run in a project folder.
- **Add-ons.** A Linux line's add-ons folder is attached to change, its
  variables (`NPM_CONFIG_PREFIX`) name it, and its commands' folder is in
  `PATH`. bundled says, with each add-on command it finds, whether its
  line is a Linux one; such a command runs in the line's view with your
  access, and the commands are linked again when their folder changed.

## Consequences

- Node.js is a bundle of Node.js's own build (ADR 0627), not a port.
- A Linux bundle's program has a private `/tmp`, `/etc` and `/dev/shm`
  for each run, as `linux run`'s has; a native bundle's `/tmp` lasts
  between runs.
- Rows of docs/deferred.md: a Linux bundle's services (Phase 18 PR 5's
  runtime), and `devices.serial` and `keyring.shared` for Linux bundles.
