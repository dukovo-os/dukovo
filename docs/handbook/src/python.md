<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Python

Python comes from the catalog as a bundle: CPython, the reference
interpreter, with its standard library and pip. Install it for yourself,
or, as an administrator, for everyone:

```
$ bundle install org.python.python
$ elevate bundle install --for-everyone org.python.python
```

Its commands are `python3` and `pip3`, also named `python` and `pip`.
The first time you run one, Dukovo asks whether it may run with your
access, as for any command of the catalog's that does; answer `A` to
allow it always.

```
$ python3 -c 'import sys; print(sys.platform, sys.version)'
radix 3.15.2 ...
$ python3
>>> 6 * 7
42
```

`sys.platform` is `radix`, the name of Dukovo's kernel: a program that
checks for `linux` to choose what to do takes another path on Dukovo.
Programs that check for a POSIX system, with `os.name == 'posix'`, work
as they do on Linux.

Ansible manages a Dukovo machine over SSH with this Python: give it
`ansible_python_interpreter=python3` in its inventory if it does not
find it by itself.

## Installing packages

The bundle's folder is sealed, so `pip install` installs a package for
you, in `~/.local`, and says so. For a project, make a virtual
environment, which has pip of its own and installs into itself:

```
$ python3 -m venv ~/projects/site/.venv
$ ~/projects/site/.venv/bin/pip install flask
```

Packages written in Python alone install and work. A package with code
in C, which pip builds or downloads as a library for Linux, does not:
Dukovo links every program as a whole and loads no library at run time,
so Python's own modules in C are part of `python3` itself.

## What it has and what it lacks

The standard library is all there, with its modules in C built in:
`ssl` and `hashlib` with OpenSSL, `zlib`, `bz2`, `lzma` and
`compression.zstd`, `decimal`, `curses`, `socket`, `select`,
`multiprocessing`, `pyexpat` and the rest. TLS checks certificates
against the system's root certificates (dukovo-certificates(7)).

These modules are missing, as Dukovo does not have the library each
needs yet: `ctypes` (libffi), `sqlite3` (SQLite), `dbm.gnu` and
`dbm.ndbm`, `readline` (the interactive interpreter edits lines without
it) and `tkinter`, with IDLE and `turtle`. `uuid` makes its identifiers
in Python instead of with libuuid.

## Its test suite

The bundle carries CPython's test suite, which runs as it does on any
system:

```
$ python3 -m test -j0 test_json test_os
```

Some tests fail on Dukovo for reasons that are known: the lab's test of
Python runs the whole suite and lists those it leaves out, each with its
reason, in `lab/tests/services/python-skips.txt` of Dukovo's repository.
