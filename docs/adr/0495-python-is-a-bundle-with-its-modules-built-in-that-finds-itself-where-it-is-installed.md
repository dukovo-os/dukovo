<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0495. Python is a bundle with its modules built in, that finds itself where it is installed

- Status: Accepted
- Date: 2026-11-12

## Context

Python is the runtime that Ansible, which manages servers over SSH,
needs on each machine it manages, and that many programs and scripts
are written for. The Phase 16 plan brings CPython, the reference
interpreter, to the catalog as a bundle after nginx and Valkey (ADRs
0491 and 0492).

CPython builds for another system only for those its configure script
knows. A cross build runs a Python of the same version on the build
machine. Its standard library has about 70 modules written in C, which
other systems build as shared libraries and load with `dlopen`; Radix
links its programs statically and loads no library at run time. Python
finds its library from where its program is, and a bundle's command is
run by the program's path in the bundle's folder, with only the
command's name as `argv[0]`, from `/applications` or from a person's
own `~/applications` (ADR 0223). Its newer allocator, mimalloc, is
built in by default, and the files of an installed bundle are sealed:
nobody may write beside its modules, where Python keeps their compiled
form.

## Decision

- **A port of the catalog.** `ports/python` builds CPython's newest
  stable release, 3.15, for Radix. It builds a Python for the build
  machine from the same source first, for the cross build to run.
  Three patches fit it to Radix:
  - configure knows Radix (`x86_64-pc-radix`): `sys.platform` is
    `radix`;
  - `strerror_r` is the GNU one, which the C library declares with
    `_GNU_SOURCE`, as glibc does;
  - the interpreter's own path comes from the auxiliary vector
    (`AT_EXECFN`), as macOS's comes from `_NSGetExecutablePath`, so
    that Python finds its library in the bundle's folder wherever the
    bundle is installed, and `sys.executable` is a path that runs it.
    For a script run by its `#!` line, the kernel gives the script's
    path instead, as Linux does, so a file that starts with `#!` is
    not taken: the interpreter's path from the `#!` line is, as on
    Linux.
- **Every module is built in.** The modules in C are part of `python3`
  (`MODULE_BUILDTYPE=static`), and Python has no `dlopen` to import one
  with. Those whose library Dukovo has are built: `ssl` and `hashlib`
  with OpenSSL 3.5's libraries, which the openssl port keeps for
  bundles, `zlib`, `bz2`, `lzma` and `compression.zstd`, `curses` with
  ncurses, `decimal` with the mpdecimal CPython carries, `pyexpat`, and
  the rest of the library's. A package from PyPI with code in C cannot
  be imported; one in Python alone can.
- **Without mimalloc.** Python is built with its own allocator,
  pymalloc, which it uses by default anyway: with mimalloc built in, it
  stopped with SIGABRT as it started on Radix.
- **Commands.** `python3` and `pip3` are the bundle's commands, with
  `python` and `pip` as other names for them (ADR 0492); each runs with
  the access of whoever runs it. `pip3` is a script that runs the
  `python3` beside it as `python3 -m pip`.
- **What the bundle holds.** The interpreter, its library, compiled
  once when the bundle is built, its test suite and pip, from the wheel
  CPython carries for `ensurepip`. The compiled modules use hashes that
  are not checked, as a sealed bundle's files do not change. The test
  suite is left to compile as it runs, which keeps the bundle smaller.
  Left out: what serves only a program built against Python's C
  library (`libpython`, its headers, `python3-config`), which nothing
  could load, and Tk's modules and IDLE, as Dukovo has no Tk.
- **Installing packages.** As the bundle's folder is sealed, `pip
  install` installs for the person, in `~/.local`, as pip does when it
  cannot write to the library; `python3 -m venv` makes a virtual
  environment, with pip in it, in which it installs instead.
- **Licenses.** CPython is under the PSF's license and those before it
  (`Python-2.0`), which joins the ports' allow-list; what it builds in
  is under MIT, BSD-2-Clause and BSD-3-Clause terms. pip carries
  libraries under Apache-2.0, BSD-3-Clause, BSD-2-Clause, ISC and, for
  certifi, MPL-2.0, whose source is the wheel in CPython's repository at
  the commit the SBOM names. Their notices are in pip's `dist-info`
  folder, in the bundle.
- **Tested.** The boot test `python` installs the bundle for Alice
  from its `.dkv` file and checks that `python3` asks, with a warning,
  whether it may run with her access, and then runs; that its modules
  in C import, and that TLS finds the system's root certificates; that
  a virtual environment works and pip runs; and a part of CPython's
  test suite. The lab test `services/python` runs the whole suite on
  real hardware, leaving out the tests that fail on Radix, listed in
  `lab/tests/services/python-skips.txt` with the reason for each.

## Consequences

- Ansible can manage a Dukovo machine: what it runs there needs Python
  and its standard library alone.
- `ctypes`, `sqlite3`, `dbm`, `uuid`'s C part, `readline` and `tkinter`
  are missing, as Dukovo has no libffi, SQLite, Berkeley DB or GDBM,
  libuuid, GNU Readline or Tk; the interactive interpreter edits lines
  itself, without `readline`. Each waits on a port, in
  [deferred work](../deferred.md).
- CPython's suite, about 45,600 tests in 558 files, passes on Radix
  but for about 260 tests, which find gaps in the C library and the
  kernel: multiprocessing's tests stop at a `sysconf` the C library
  does not answer, and the rest are socket options, `mmap` of tmpfs
  files, pipe sizes, file names that are not UTF-8, which Dukovo
  refuses, and smaller differences. The skip list names them; the
  gaps are in [deferred work](../deferred.md), and each fix removes its
  lines.
- A package with code in C waits until Radix can load libraries, or
  until it is built into a bundle of its own.
- The bundle takes 29 MB to download and about 100 MB installed, most
  of it the library and its test suite.
