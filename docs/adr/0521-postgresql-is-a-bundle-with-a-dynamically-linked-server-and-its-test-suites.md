<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0521. PostgreSQL is a bundle with a dynamically linked server and its test suites

- Status: Accepted
- Date: 2026-11-14

## Context

Web applications and the programs people write keep their data in a
database, and a server that is to host them needs one. PostgreSQL is the
catalog's: it runs as a service of its bundle (ADR 0522), and PHP's
`pgsql` and `pdo_pgsql` extensions are built against its libpq (ADR 0499).
Dukovo 1.0 carries PostgreSQL 19, the major release current at its
release, so that a cluster made on it need not be moved to the next major
version soon after.

PostgreSQL is a large C program, and the system asks of it what it has not
had to give any port before:

- **Its `configure` knows no Radix** and stops for a system it does not
  know; it takes the name of a template for one it does not.
- **It waits for events with epoll and signalfd** when the C library has
  their headers, and Radix has neither call (radix-posix(7)).
- **It loads its modules with `dlopen`.** PL/pgSQL, the encoding
  conversions and the Snowball dictionaries are shared libraries, `initdb`
  needs the first and the last, and every module calls functions of the
  server that loaded it, so the server's symbols must be there for the
  dynamic linker (ADR 0071) to resolve them.
- **It finds its library and share folders from where its program is**
  (`find_my_exec`), from `argv[0]`, searched for in the PATH when it has no
  slash. A bundle's command has only its name in `argv[0]`, and the PATH
  holds the link bundled(8) made, which does not lead to the bundle.
- **Its source comes from git,** as ftp.postgresql.org, which serves the
  release archives with the parsers and the catalogs already written, is
  not reachable from the build machines. A tree from git needs bison, flex
  and Perl to write them.

## Decision

- **A bundle, `org.postgresql.postgresql`,** in the catalog and not the
  image, with the server, the client and administration programs, libpq,
  the contrib modules and the test suites. Its service is ADR 0522's.
- **The source is PostgreSQL's git tree,** pinned to the commit of a tag:
  the release candidate `REL_19_RC2`, whose bundle is the pre-release
  19.0-rc.2, which a catalog orders before 19.0, until the pin moves to
  `REL_19_0`.
- **The linux template.** Radix's ABI is Linux's, so `configure` takes
  `--with-template=linux`: unnamed POSIX semaphores, `-fPIC` for shared
  code, Linux's shared library rules and its `fdatasync` for the log. The
  macro `__linux__` stays undefined, as for every program built with the
  compiler wrapper, which leaves out
  what PostgreSQL guards with it and needs nothing of: the processes'
  titles, the huge page size from `/proc/meminfo`, `posix_fallocate` for the
  memory of a parallel query (`ftruncate` takes its place), the processor's
  clock rate and the reflinks of `pg_upgrade --clone` and
  `pg_combinebackup --clone`.
- **`poll` and the self-pipe.** `configure` is told there is no epoll or
  signalfd header, and PostgreSQL waits as it does on a system without
  them. The C library has no `sync_file_range`, so the `*_flush_after`
  settings are 0, as on any system without it. The C library's
  `backtrace(3)` looks for `libgcc_s`, which a bundle does not have, so the
  server is built without backtraces.
- **The server is linked dynamically and exports its symbols** (the compiler
  wrapper's `-dynamic`, ADR 0071, and `-Wl,--export-dynamic`), and every
  module is a shared library in `lib/postgresql`: PL/pgSQL, the encoding
  conversions, the Snowball dictionaries, `pgoutput`, `libpqwalreceiver`
  and the contrib modules. This is how PostgreSQL loads its modules on every
  system, and how an extension is built for it apart from the server, so an
  extension made for this server loads without the server built again. The
  run paths are relative to the file's own folder (`$ORIGIN`), as a
  bundle's folder is `/applications/ID` or one in a person's own
  applications folder.
- **Every program links `libpq.so`, the bundle's.** `libpq.a` does not link on
  its own: libpq's objects call the encoding functions by their public
  names, such as `pg_char_to_encoding`, which only `libpgcommon_shlib`
  defines, and the programs' own `libpgcommon` names them with the suffix
  `_private` (`src/include/mb/pg_wchar.h`). So the programs and the modules
  that connect out (`libpqwalreceiver`, `dblink`, `postgres_fdw`) use the
  `libpq.so.5` in `lib`, as they do on Linux, and a program that has no
  `libpgcommon` of its own, PHP, links `libpq.a` with `libpgcommon_shlib.a`
  and `libpgport_shlib.a`, which the build installs into the sysroot with
  the headers and a `libpq.pc`.
- **libpq has an OpenSSL of its own,** the openssl port's static libraries,
  and does not export it, as it is loaded into programs that have none
  (`psql`) and into the server, which has its own and exports it: exported,
  each would answer the other's calls. `pgcrypto` and `sslinfo` work on the
  server's own OpenSSL objects and link none, so their calls are the
  server's.
- **The time zones are the system's,** read from `/system/resources/zoneinfo`
  when the programs run (`--with-system-tzdata`), so that an update of the
  `tzdata` port reaches PostgreSQL and the bundle has no copy.
- **Line editing is libedit's.** A new library port, `libedit`, gives `psql`
  its editing, history and completion, which PostgreSQL's `configure` takes
  in place of readline's.
- **What is built in:** OpenSSL for TLS, zlib, lz4, zstd and libxml2. What
  is not: ICU, LLVM's JIT, PL/Perl, PL/Python, PL/Tcl, `liburing`, native
  language support, and the contrib modules that need a library the system
  lacks (`uuid-ossp`, `xml2`) or SELinux (`sepgsql`). Collations are the C
  library's, which the `locales` port provides, and the builtin provider's.
- **Patch 0001** takes the program's path from the kernel's auxiliary vector
  (`AT_EXECFN`) when it is absolute, before the search of the PATH, as
  Python's and PHP's patches do. Patches 0002 and 0003 are ADR 0522's.
- **No huge pages.** The C library's `<sys/mman.h>` has no `MAP_HUGETLB`,
  so the server is built as for a system without huge pages:
  `huge_pages = try` maps ordinary pages and `huge_pages_status` says off,
  and `huge_pages = on` stops the server with PostgreSQL's own message.
  `mmap` refuses Linux's number for the flag with `ENOMEM`, as Linux does
  when no huge page is reserved.
- **The test suites are in the bundle,** in `tests`: `pg_regress`,
  `pg_isolation_regress` and `isolationtester`, with the main suite's files
  and its test module, the isolation suite's, PL/pgSQL's and each contrib
  module's. The build writes a schedule for each suite from the lists in
  PostgreSQL's makefiles, an isolation schedule for each of the six contrib
  modules that have isolation tests (`ltree`, `pg_logicalinspect`,
  `pgrowlocks`, `postgres_fdw`, `tcn` and `test_decoding`) among them, and
  names in `own-server` the settings of a module whose tests start a server
  of their own. The lab test runs them on real
  hardware, as it runs those of PHP and Python (ADRs 0499 and 0495), leaving
  out only the tests that its skip list names, each with the reason it fails
  on Dukovo. Under emulation every test passes, the core's 243, PL/pgSQL's
  13, the 139 isolation tests and the 49 suites of the contrib modules, so
  the list starts empty. The TAP tests (`src/test/*/t`) are Perl scripts and are not in
  the bundle.
- **Tests of the bundle:** the `postgresql` boot test runs the server by
  hand, as a person developing would, with its cluster and socket in `/tmp`
  (`-k /tmp` and `-h /tmp`, as libpq's default socket folder is the
  service's, ADR 0522); the `dynamic` test of `posix` checks that a plugin
  calls a function of the program that opened it, which every module of
  PostgreSQL does; the lab test `services/postgresql` runs the suites.

## Consequences

- An administrator installs the bundle for everyone, as it has a service
  (ADR 0385), and any account then runs its programs: `initdb` and `pg_ctl`
  make and run a server of a person's own, `psql` and `pg_dump` talk to one.
- The bundle's file is about 29 MB, of which the test suites' files, which
  are text for the most part, are a few MB, and the contrib modules and
  the programs the rest.
- A server without huge pages, the processes' titles and backtraces,
  with `poll` where Linux uses epoll and `*_flush_after` off, differs from
  PostgreSQL on Linux in what an administrator can see and tune. The reasons
  are in `build.sh` and docs/deferred.md.
- A failure of the suites on Dukovo that is no gap in the skip list is a
  bug of the system, found by the lab test on hardware.
- **Not yet,** each a row of docs/deferred.md: ICU collations, until an ICU
  port exists (PHP's `intl` waits for the same); PL/Perl, PL/Python and
  PL/Tcl, as there is no Perl or Tcl port and Python's has no `libpython`
  to embed; LLVM's JIT; `uuid-ossp`, `xml2` and `sepgsql`; `io_uring`, as
  Radix has its own rings; the TAP tests; and messages in other languages
  than English.
