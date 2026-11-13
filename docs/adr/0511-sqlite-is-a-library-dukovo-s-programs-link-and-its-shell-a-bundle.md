<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0511. SQLite is a library Dukovo's programs link, and its shell a bundle

- Status: Accepted
- Date: 2026-11-13

## Context

The web console's admin service, admind, keeps its sessions, its audit
log and the history of its tasks and settings in SQLite, a database that
is one library and one file. Dukovo has no SQLite. Python's `sqlite3`
module is missing for that reason (ADR 0495), and there is no shell to
look into a database file with.

Dukovo's programs are linked statically and load no library at run time,
so a program that uses SQLite carries its own copy of the library. Left
alone, each would bring a copy of its own: Python's, the shell's and
admind's, and a security fix would be one change for each. SQLite's own
test suite, the best evidence that a port works, is a Tcl program
(`testfixture`) that needs Tcl, which Dukovo does not have either.

SQLite's unix VFS has two ways to use a file that Radix has only in part.
It keeps the index of a database in WAL mode in the `-shm` file beside
it, as a shared writable mapping, which every connection of every
process shares; and it can read a database through a shared mapping
instead of `read`. Radix refuses a shared writable mapping of a file
that is not in `/dev/shm` (ENODEV), and a read-only shared mapping of
one is the process's own copy, which does not see later writes (ADR
0490; the posix design note's known limits). Its other needs, record
locks between processes (`fcntl`, `flock`), `fsync`, `ftruncate` and the
rest, are there.

## Decision

- **The library is a port, that programs link.** `ports/sqlite` builds
  SQLite 3.54.1 from its repository's tag as `libsqlite3.a`, with
  `sqlite3.h`, `sqlite3ext.h` and `sqlite3.pc` in the sysroot, and
  nothing in the image of its own: a program that links it carries it,
  and the image's license manifest lists the port. The repository has no
  amalgamation, so the port's configure runs as a cross build and
  `make` generates `sqlite3.c`, the shell's `shell.c` and the headers,
  as for a release. One set of compile options builds the library, the
  shell and the test program, so that the suite tests what is
  shipped: serialized threading (`SQLITE_THREADSAFE=1`, as admind's
  threads need), no extension loading (a static program loads nothing),
  FTS4 (which gives FTS3), FTS5, R-Tree, the dbstat table, column
  metadata and the math functions, which are what Dukovo's programs and
  Python's `sqlite3` module expect, and `SQLITE_MAX_PATHLEN=4096`, as
  SQLite sizes the buffer for the working folder by `FILENAME_MAX`, which
  the C library sets to 256, and a database could not be opened by a
  relative name in a longer folder; the rest are SQLite's defaults.
- **The shell is a bundle of the catalog.** `ports/sqlite-cli` builds
  `org.sqlite.sqlite`, whose command `sqlite3` is the generated
  `shell.c` linked to that library, with line editing from linenoise and
  the shell's own manual page. Its SBOM lists SQLite, zlib and the C
  library, and linenoise, whose license (BSD-2-Clause) joins the
  bundle's. The port has no source of its own but linenoise's, a port of
  extra sources alone, which `ob` accepts.
- **No memory-mapped I/O, and no WAL index in a file.** The build sets
  `SQLITE_MAX_MMAP_SIZE=0`: with memory-mapped reads on, SQLite would
  read stale pages through the copy, its own connection's writes
  included. `HAVE_MREMAP` stays 0, as mlibc's `mremap` has no system call
  under it. The WAL index is left as SQLite builds it. Two ways round
  the missing mappings were turned down:
  - Pointing `SQLITE_SHM_DIRECTORY` at `/dev/shm`, where mappings work.
    SQLite names each index there by the database's device and inode,
    `/dev/shm/sqlite-shm-<inode>-<device>`, a name any account can work
    out and create first in a folder every account may write, so a
    process of one account (a compromised web application, say) could
    tamper with or block the index of another's database, admind's
    included. On Linux the file sits beside the database, where the
    database's folder guards it.
  - A patch to SQLite's VFS, which is code to carry and prove for every
    release. The kernel is the place to close the gap, by mapping the
    page cache's pages shared; the deferred work says so.

  What works is the rollback journal, SQLite's default, between
  connections and processes, as `fcntl` byte-range locks hold the
  database; and WAL in exclusive locking mode, which a connection asks
  for with `PRAGMA locking_mode=EXCLUSIVE` before `PRAGMA
  journal_mode=WAL`, and which keeps the index in the heap and makes no
  `-shm` file.
- **Tcl is a toolchain port, for the suite alone.** `ports/tcl` builds
  Tcl 9.1.0 static, with no run-time loading and no zipfs (its script
  library is a folder), into the toolchain, where only the sqlite port's
  `testfixture` looks. Nothing of it reaches an image, a bundle's SBOM
  or the license manifest. The sqlite port packs `testfixture`, stripped,
  with the test scripts and Tcl's library as `for-lab/sqlite-tests.tar.gz`
  in its `PORT_INFO`, and `ob lab` names the folder of the ports'
  builds to the lab in `DUKOVO_LAB_PORTS`.
- **Rust programs link the port's copy.** `std_cargo` gives libsqlite3-sys
  `SQLITE3_LIB_DIR`, `SQLITE3_INCLUDE_DIR` and `SQLITE3_STATIC` for the
  sysroot's library, so that rusqlite, without its `bundled` feature,
  links `libsqlite3.a` and there is one SQLite to update. The first
  program is `sqlite-test`, in `/system/tests/posix`.
- **Python links it too.** The python port depends on and links the
  sqlite port, so `_sqlite3` is built in and `import sqlite3` works;
  without it, CPython's configure would find the library in the
  sysroot whenever it ran after the sqlite port, and not before.
- **Tested.** The boot test `sqlite` runs `sqlite-test` on a Btrfs root:
  the library's version and options, a rollback journal, a reader
  beside a writer, a second writer's busy timeout, another process taking
  the locks and waiting, exclusive WAL, and FTS5 and JSON. The lab test
  `services/sqlite` runs the shell, a backup and two shells at once; a
  step of `services/python-commands` runs Python's module; and
  `services/sqlite-suite` runs SQLite's own `veryquick` suite on Radix,
  leaving out the test files `lab/tests/services/sqlite-skips.txt` lists,
  each group with its reason.
- **Licenses.** SQLite is in the public domain, with a blessing in the
  place of a license, which `blessing` names in the ports' allow-list.
  Tcl's license is BSD-style, and Tcl is a toolchain port, which the
  license check leaves out.

This record supersedes the line of ADR 0495 that says `sqlite3` is
missing from Python's modules.

## Consequences

- A SQLite security fix is one change of the port's pinned commit, and a
  rebuild of the programs that link it, whose stamps follow the port's.
- Programs that set `journal_mode=WAL` without exclusive locking get
  `SQLITE_IOERR_SHMMAP` ("disk I/O error") until Radix maps page-cache
  pages shared; Dukovo's own programs use the rollback journal or
  exclusive WAL. The same holds for a person's `sqlite3` and Python's
  `sqlite3` module. The deferred row for shared writable mappings
  names SQLite's WAL between connections and its memory-mapped I/O.
- admind's host unit tests need SQLite on the build machine: it declares
  `libsqlite3-sys` with the feature `bundled` for hosts only, so tests
  compile the crate's copy and Radix builds link the port's. Its readers
  and writers take the rollback journal, or WAL in exclusive mode for a
  database one connection holds, until WAL between connections works.
- Each gap in Radix that SQLite's suite finds is a line of
  `sqlite-skips.txt` with its reason and a row of the deferred work,
  and its fix removes the line.
- A port of Tcl is in the build for one program's sake. It is built
  once and costs the image nothing.
- The library's code is about 1.6 MB in each program that links it; the
  shell is 2.5 MB, and the test archive 11 MB.
