<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0499. PHP is a bundle with its extensions built in, and its own test suite

- Status: Accepted
- Date: 2026-11-13

## Context

PHP is the language of most of the web applications the catalog will
carry: WordPress, Nextcloud and Laravel's programs are PHP. The Phase 16
plan brings PHP 8.5 to the catalog as a bundle, after Python (ADR 0495):
first the command `php`, which this record is about, then PHP-FPM, the
service that serves PHP programs to nginx (ADR 0520).

PHP's release archives are on php.net, which the build machines cannot
reach; GitHub, which has the repository, they can. A tree from git has no
`configure`, no parsers and no scanners: Autoconf makes `configure`
(`./buildconf`), Bison, which the build container has, writes the parsers,
and re2c, which it does not, writes the scanners. PHP's `configure` runs
programs to answer some questions, which a cross build cannot do, and it
takes the answers for systems it knows by name: a host with `linux` in its
name gets Linux's, any other the safe ones. Its extensions are shared
libraries on most systems, loaded with `dlopen`, which Radix does not have
(ADR 0495 gives the reason). PHP finds its `php.ini` in the folder of its
program, which it learns from `argv[0]` and a search of `PATH`: a bundle's
command is run by its path in the bundle's folder, in `/applications` or in
a person's own `~/applications` (ADR 0223), with only the command's name as
`argv[0]`.

## Decision

- **Two ports.** `ports/libxml2` builds libxml2 2.15.4, from GNOME's mirror
  on GitHub, as a static library with CMake, with zlib, without iconv (it
  reads and writes UTF-8, UTF-16 and ISO-8859-1 itself, and the other
  ISO-8859 encodings from its own tables, which the XML extensions need),
  without its programs, tests, documentation and loading of modules. Its
  headers, library and `libxml-2.0.pc` go into the sysroot, for ports; none
  reaches an image. `ports/php` builds PHP 8.5.12.
- **From git, with re2c built first.** `ports/php` takes the tag's commit,
  and re2c 4.6 as an extra source, which `build.sh` builds for the build
  machine, from its bootstrap sources, as Python's builds a Python for the
  build machine. `buildconf` makes `configure`; the cross build gives it the
  answers of the cache variables below, and checks `main/php_config.h` and
  the list of extensions it chose before `make`, failing with the name of
  the first that is missing. It builds the program only (`make cli`): the
  default target would also run it on the build machine to make the phar
  tool, which a program for Radix started on Linux cannot do.
- **Every extension is built in.** `--disable-all --disable-shared
  --enable-static`, then each extension by name: Core, date, hash, json,
  lexbor, pcre, random, Reflection, SPL, standard and uri, which PHP always
  has; opcache, which PHP 8.5 always builds in; bcmath, bz2, calendar, ctype, curl, dom,
  exif, fileinfo, filter, ftp, mbstring, mysqli, mysqlnd, openssl, pcntl,
  pdo, pdo_mysql, phar, posix, session, shmop, simplexml, sockets, sysvsem,
  sysvshm, tokenizer, xml, xmlreader, xmlwriter and zlib; and libxml, which
  dom, simplexml, xml, xmlreader and xmlwriter need, and `php -m` lists. curl
  is Dukovo's libcurl on rustls; the `openssl` extension uses OpenSSL 3.5's
  libraries, which the openssl port keeps for bundles, as Python's `ssl`
  does; both read the system's root certificates.
- **What is left out**, each a row in [deferred work](../deferred.md):
  gd, intl, zip, sodium and gmp, which wait for ports of their libraries;
  pgsql and pdo_pgsql, which the PostgreSQL port brings with libpq;
  sqlite3 and pdo_sqlite, which wait for a port of SQLite; iconv, as the
  C library has no `iconv(3)`; sysvmsg, as Radix has no System V message
  queues (ADR 0490); mbstring's regular expressions, which need
  Oniguruma; the JIT of PCRE and that of opcache, as a program's memory
  is never writable and executable at once on Radix (nginx's PCRE2 is
  built without JIT as well, ADR 0491); `dl()` and extensions from PECL,
  as the program is static. PHP-FPM, which ADR 0520 adds to the bundle, is
  built with the same extensions.
- **Eight patches** fit PHP to Radix, each with its reason in its header
  (0008 and 0009 are PHP-FPM's, ADR 0520):
  - `0001-executable-from-auxv`: `PHP_BINARY`, and so the folder PHP looks
    for its `php.ini` in, is the path the kernel gave the program in the
    auxiliary vector (`AT_EXECFN`), as Python's patch 0003 does, and a file
    that starts with `#!` is not taken, as there the path is the script's;
  - `0002-php-ini-in-the-bundle`: after the program's own folder, PHP
    looks in the `etc` folder beside it, so a bundle finds `etc/php.ini`
    wherever it is installed; the
    folder PHP was configured with stays the last place, as upstream has
    it;
  - `0003-fnmatch-on-radix`: configure takes `fnmatch` to be there in a
    cross build for hosts with `linux` in their names only; Radix has it;
  - `0004-operating-system-of-the-target`: `PHP_OS`, which configure took
    from the build machine's `uname`, can be given, as `PHP_UNAME` can;
  - `0005-sockets-without-packet-and-filter-headers`: the sockets extension
    keeps its code for packet sockets and BPF filters for every system
    whose headers name them; Radix's C library names `AF_PACKET` and
    `SO_ATTACH_REUSEPORT_CBPF` and has neither Linux header, as the kernel
    has neither;
  - `0006-sockets-multicast-on-radix`: the C library has `MCAST_JOIN_GROUP`
    and not `MCAST_LEAVE_GROUP`, so the extension joins and leaves groups
    as IPv4 does (`IP_ADD_MEMBERSHIP`), which the kernel has, and learns
    an interface's address with `getifaddrs` where `SIOCGIFADDR` is not
    there;
  - `0007-zend-extensions-without-dlopen`: PHP compiles the mechanism a Zend
    extension registers with only where it can load one from a file, and
    OPcache, built in, is one: without the patch it is registered to
    nothing, never starts, and the program's shutdown frees what its start
    never made, so every run of `php` ended in a fault. The mechanism stays,
    and loading a file fails;
  - `0010-accepted-sockets-are-blocking`: a socket that `accept(2)` gives
    is blocking on Radix, as on Linux, whatever the listening socket is,
    and PHP copied the listening stream's record of it to the new stream
    on every system but Linux, in its plain TCP transport and in
    OpenSSL's, which takes `tcp://` when the `openssl` extension is built
    in; a server that made its listening socket non-blocking got streams
    it took to be non-blocking that were not, and their reads waited with
    no timeout.
- **Answers for configure**, one line each in `build.sh`: no `dlopen` (the
  three variables `ac_cv_func_dlopen`, `ac_cv_lib_dl_dlopen` and
  `ac_cv_search_dlopen`, which is the one that sets `HAVE_LIBDL` in PHP), so
  there is no `dl()`; `php_cv_func_getaddrinfo` and `php_cv_have_write_stdout`
  yes, as the C library answers as POSIX says; `ac_cv_func_which_gethostbyname_r`
  no, as the C library's `gethostbyname_r(3)` links and finds no name, so
  that PHP's `gethostbyname()` and the sockets extension's host names take
  `gethostbyname(3)`, which finds them, and which a PHP built without
  threads calls from one thread only; and `php_cv_shm_ipc`,
  `php_cv_shm_mmap_anon` and `php_cv_shm_mmap_posix` yes, which are the
  three ways opcache can allocate the memory its processes share, each of
  which the kernel gives, so that opcache has some; `php_cv_have_stack_limit`
  yes, as the stack grows down: configure runs a program to find out, and
  in a cross build takes it not to, which builds PHP without its check of
  how deep calls go, so that a function calling itself without end ends the
  process with SIGSEGV where PHP throws an Error; `PHP_OS`, `PHP_UNAME` and
  `PHP_BUILD_PROVIDER` say Dukovo. The answers left as configure takes them
  in a cross build are right for Radix: no `sched_getcpu`, no JIT.
- **A `php.ini` in the bundle**, found beside the program. It is upstream's
  `php.ini-production`, kept whole in `ports/php/php.ini`, with one
  change: `openssl.cafile` and `curl.cainfo` name the system's root
  certificates. `date.timezone` and `session.save_path` are unset, as
  upstream's are, and PHP-FPM's service sets its own.
- **The test suite is in the bundle** and runs on real hardware. The bundle's
  `tests` folder has `run-tests.php` and every `tests` folder of `Zend`,
  `tests`, `sapi/cli` and each extension the program has, whole, in the
  tree's own layout. The lab test `services/php` installs the bundle for
  everyone, copies the suite to a writable folder in `/tmp`, as the bundle's
  files are sealed and `run-tests.php` writes beside each test, and runs
  all of it with a worker for each processor, leaving out the tests that
  `lab/tests/services/php-skips.txt` lists, each with the reason it fails
  on Dukovo. All of it is 18,941 tests, too many for a boot test, so the
  list comes from runs of all of them in the machine a boot test makes
  (below), and the lab test adds a line for each test that fails on
  hardware once its reason is known.
- **Licenses.** PHP is under the PHP License 3.01 and the Zend Engine
  License 2.00, which, with `TCL` (scanf), `Apache-1.0` (snprintf),
  `OLDAP-2.8` (tables derived from OpenLDAP's), Solar Designer's
  crypt_blowfish, which is in the public domain with a permissive
  fallback, and re2c, which is in the public domain, join the bundles'
  allow-list. What it builds in is under BSD-2-Clause, BSD-3-Clause,
  BSD-4-Clause, MIT, Zlib, BSL-1.0 and Apache-2.0 terms, PCRE2's under its
  exception, and libmbfl and libbcmath under the LGPL 2.1 or later, which
  the allow-list had. The LGPL asks that a person can link the program
  again with a changed library: the SBOM names PHP's source at its
  commit, which holds both, and `ports/php` builds it. The bundle's
  notices are the files `README.REDIST.BINS` points to; ob names those
  that share a file name by their folder as well (`Zend-LICENSE`).
- **Tested.** The boot test `php` installs the bundle for everyone, through
  elevate, from its `.dkv` file, and checks that `php` asks, with a warning,
  whether it may run with Alice's access; that `php -m` lists the
  extensions, and that `php.ini` and `PHP_BINARY` are in `/applications`; and
  a script of checks, one for each of twenty-four things the extensions do:
  XML, multibyte strings, hashes, OpenSSL, compression, files,
  `pcntl_fork`, `proc_open`, a socket server and client, System V and
  shared memory, opcache and curl. It then runs seventeen tests of PHP's
  own suite that check what the port changes: `gh8472`, of the accepted
  socket that patch 0010 makes blocking; the tests of host names and of
  `socket_create_listen()` that take `gethostbyname(3)`; and `gh18572`,
  whose calls nested too deep end in an Error with the check of how deep
  calls go.

## Consequences

- Web applications written in PHP that need nothing of the missing
  extensions run from the command line: Composer's packages that are PHP
  alone install and work, and PHP-FPM (ADR 0520) serves them to nginx.
- The bundle has a service, PHP-FPM (ADR 0520), and a bundle with services
  installs only for everyone: an administrator installs PHP, and a person
  cannot install it for themselves.
- Each missing extension waits on a port or on the kernel, in deferred work.
  A package with its own extension in C waits until Radix loads libraries or
  a bundle of its own is built with it.
- Both JITs are off: PHP and opcache run at the speed of their
  interpreters.
- The bundle takes 38 MB to download and has 20,240 files of 116 MB (175 MB
  on a disk), of which PHP's test suite is 103 MB and `bin` 69 MB: `php`
  and `php-fpm` (ADR 0520) are 36 MB each, as each has every extension
  built in. Installing it on a machine of 2 GiB first stopped with
  `no_memory`, as the kernel's free memory was in pieces too small for a
  table the install grew; since ADR 0524 it installs there, and the boot
  tests give theirs 2 GiB.
- The suite's results, and the gaps it found, are below.

## The suite on Dukovo

The suite first ran in part: the 3,492 tests that ask the system for
something, those of `tests/` and `sapi/cli/tests/`, of
`ext/standard/tests/` for files, the network, general functions and
streams, and of the sockets, posix, pcntl, sysvsem, sysvshm, shmop,
session, openssl, curl and ftp extensions. They ran in a boot test's
machine, in 12 runs of at most 300 tests with four workers, taking 33
minutes in all: 2,760 passed, 661 were skipped by their own `SKIPIF`
sections (an extension the program does not have, a test that is `online`,
a privilege), 2 failed as their files say they do (`XFAIL`), and 69 did
not pass: 62 failed and 7 were borked.

Each failure was read against what it printed. Five of the seven that
borked did so because their `SKIPIF` sections include a file of a `tests`
folder the bundle did not have (`ext/dl_test/tests/skip.inc` and
`sapi/cgi/tests/include.inc`): the bundle has those two files now, and four
more of that kind that tests in the rest of the suite include
(`sapi/fpm/tests/skipif.inc` and `tester.inc`, and two of `ext/xsl`), and
the five skip. The other two, `mcast_ipv4_send` and `mcast_ipv4_send_error`, are
borked by a warning their `SKIPIF` sections print when the system cannot
give an interface's address by its number. A second run of the 69, one
at a time, gave the same failures, so none of them is caused by the load.

Sixteen of the 64 that were left were PHP's own on Radix, not the
system's, and the port fixes them: `gh8472`, whose accepted socket blocked
(patch 0010), and fifteen tests of host names and of
`socket_create_listen()`, which took the C library's `gethostbyname_r(3)`
(the answer to configure above). The boot test `php` runs those sixteen.

The 48 that are left are in `lab/tests/services/php-skips.txt`, grouped by
what they need of the system: in the C library, `getprotobyname(3)`,
`getprotobynumber(3)` and `wait3(2)`, a `getservbyport(3)` that ignores
the protocol, and a `sysconf(3)` that lacks names; in the kernel,
Unix-domain datagram sockets, the socket options `TCP_CONGESTION`,
`SO_ZEROCOPY`, `IPV6_PKTINFO` and `IPV6_RECVPKTINFO`, the ioctl for an
interface's address, a SIGCHLD's sender's user number, a tmpfs folder of
more than 32,768 entries, `fsync(2)` of a descriptor that is not a file, a
slash after a file's name, `utimes(2)` of a time before 1970, and
`getcwd(3)` in a removed folder; and, by choice, TLS in `curl`, which is
rustls's, and `openssl.cafile`, which `php.ini` sets. The gaps are rows in
`docs/deferred.md`; when one closes, its lines leave the list. PHP's
`dl()` and `dns_*` functions are missing for reasons given above, and
their tests are in the list for that.

The other 15,449 tests, the language's own, `Zend/tests` among them, then
ran in the same machine, in 14 runs of about 1,100 with four workers,
taking an hour and a half. `run-tests.php` counts 15,580 of them, as some
tests run others: 14,095 passed, 1,467 were skipped by their own `SKIPIF`
sections, 5 failed as their files say they do, and 13 did not pass: 11
failed and 2 were borked. Each was read against what it printed:

- `Zend/tests/gh18572` ended the process with SIGSEGV where it expects an
  Error, as PHP had been built without its check of how deep calls go: the
  answer to configure above builds it in, and the boot test `php` runs the
  test.
- Eight are in the list: two open a file whose name is not UTF-8, which
  Radix refuses, as its names are UTF-8 (ADR 0127), with EINVAL where they
  expect ENOENT; two read or write XML in Windows-1251 and in SHIFT_JIS,
  which libxml2 converts only with `iconv(3)`, which the C library does not
  have; and four run `locale(1)`, which Radix does not have, to list the
  locales.
- `ext/standard/tests/directory/gh10992` makes a folder whose name is 265
  characters less the length of the working folder's path, which is over
  the 255 a name may have when that path is shorter than 10 characters, on
  Linux as well; the run's was `/tmp/t`, and the lab test's,
  `/tmp/php-tests`, leaves it 251.
- `ext/fileinfo/tests/cve-2014-3538-nojit` times itself, and emulation is
  too slow for the time it allows.
- The two borked are the two tests of `ext/opcache` that run PHP-FPM,
  `preload_user_004` and `preload_user_005`: their `SKIPIF` sections include
  PHP-FPM's tester, which includes four more of PHP-FPM's helpers that the
  bundle did not have. It has all of them now, and the two skip, as their
  search finds no `sbin/php-fpm`.

PHP's own tests of PHP-FPM (`sapi/fpm`) and of `php-cgi` are not in the
bundle, as `php-cgi` is not, and PHP-FPM is in `bin`, where its tests do
not look for it: the boot test `php-fpm` and the lab test
`services/php-fpm` test the service.
