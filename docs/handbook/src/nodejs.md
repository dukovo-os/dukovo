<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Node.js

Node.js comes from the catalog as Node.js's own build for Linux, with
npm, one bundle for each long-term support line: `org.nodejs.node@24` and
`org.nodejs.node@26`. They are Linux bundles (ADR 0625): Dukovo runs
their programs in a Linux view on the Linux base, the bundle
`org.dukovo.linux-base`, which `bundle install` names if it is not there
yet. Install the base and the lines you need, for yourself or, as an
administrator, for everyone:

```
$ bundle install org.dukovo.linux-base
$ bundle install org.nodejs.node@26
$ bundle install org.nodejs.node@24
```

When it installs a line, `bundle install` lists what the bundle asks for,
and one of them reads "Make machine code as it runs, as JavaScript engines
do". That is memory.jit (ADR 0624): V8, Node.js's JavaScript engine,
compiles the code it runs into machine code in memory that is writable
and executable at once, which no other program is given. Node.js does not
start without it.

The commands are `node`, `npm` and `npx`. They run with your access, in
the folder you start them in, and the first time you run one Dukovo asks
whether it may, as for any command of the catalog's that does; answer `A`
to allow it always.

```
$ node -p 'process.version + " " + process.platform'
v26.13.1 linux
```

`process.platform` is `linux`, and `process.arch` is `x64`: Node.js runs
as it runs on a Linux machine, and so do the packages that ship programs
for Linux, which npm picks by those two names.

## Choosing a line

With both lines installed, `node` is the newest, 26, and `node-24` and
`node-26` name each. In a project, `node`, `npm` and `npx` are the line
its `.nvmrc` or `.node-version` names, in the nearest folder up from the
one you are in that has either, as nvm and other version managers read
those files:

```
$ cd ~/projects/site
$ cat .nvmrc
24
$ node --version
v24.22.3
```

Outside a project, `bundle default node 24` makes line 24 yours, and an
administrator's `elevate bundle default node 24 --for-everyone` makes it
everyone's; plain `bundle default node` says which line runs in the
folder you are in, and why (bundle(1)).

Each line updates within itself: Node.js 24 stays 24 when 26 has a new
release.

## Installing packages

`npm install` and `npm ci` work as on Linux, in the project's
`node_modules`. Packages with code of their own for Linux, which npm
downloads for `linux-x64` (esbuild, SWC, Rollup, sharp and the like), or
which an install script builds, run in the same Linux view. An install
script's `sh -c`, and a script's `#!/usr/bin/env node` line, find the
view's shell and commands, which are Dukovo's own (ADR 0626).

`npm install -g` puts a package in the line's add-ons folder,
`~/applications/add-ons/org.nodejs.node@26` for line 26, as the line's
own folder is sealed, and its commands in that folder's `bin`, which is
in your `PATH`:

```
$ npm install -g semver
$ semver 1.2.3 -i minor
1.3.0
```

A command of a package installed so runs with the line it was installed
with.

## What it has and what it lacks

Node.js is the release as Node.js made it, so what it has is what it has
on Linux. The Linux calls it makes are answered by Dukovo's kernel or by
linuxd (ADRs 0610, 0618); the `linux-node` boot test checks that each call
Node.js and npm make on Linux is one Dukovo has before each change goes
in. io_uring is not there: libuv, Node.js's event library, finds that out
and uses threads for files instead, as on a Linux without it.

A Linux bundle cannot have services yet (ADR 0625), so a Node.js
program does not run as a service of Dukovo's own.

How close the Linux view comes to Linux is measured on LTP's system call
tests and on generated seccomp filters, which CI runs on Linux and on
Dukovo for each change to it (`ob test linux-suite`, ADR 0628).
