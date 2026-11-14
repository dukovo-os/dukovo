<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0522. PostgreSQL runs as a service that stops with its own signal and time

- Status: Accepted
- Date: 2026-11-14

## Context

The PostgreSQL bundle (ADR 0521) carries the server and its programs. A
database is mostly used as a service, which a service bundle runs as an
account of its own once an administrator enables it (ADRs 0385 and 0386),
as Valkey (ADR 0492) and PHP-FPM (ADR 0520) are run. What the server asks
of the system that is not there yet:

- **A stop of its own.** Spark ends every service with SIGTERM and gives
  it 5 seconds. PostgreSQL takes SIGTERM for a *smart* shutdown, which
  waits for every client to disconnect: with one connected, Spark's 5
  seconds pass, Spark ends the whole domain, and the next start recovers
  from a crash. Its fast shutdown, which ends the sessions, rolls back
  what they had open, writes a checkpoint and stops, is SIGINT, and the
  checkpoint writes every changed buffer, which can take longer than 5
  seconds. PostgreSQL's own systemd unit says `KillMode=mixed`,
  `KillSignal=SIGINT` and `TimeoutSec=infinity`. Spark already signals only
  the service's own process, which is the postmaster, as `KillMode=mixed`
  does; it lacks the signal and the time, per service.
- **A bundle's hooks around a backup.** A service file can name programs
  that run before and after a backup (`[backup]`, ADR 0463), but a bundle's
  manifest has no such key, so a bundle's database cannot checkpoint before
  a snapshot.
- **A socket folder.** libpq's default folder for the Unix socket is
  `/tmp`. The service's socket belongs in its live folder
  (`/system/live/services/postgres`, ADR 0386), which Spark makes for the
  account and which only that account may enter. PostgreSQL authenticates
  each connection itself, by the account that connects (`SO_PEERCRED`,
  radix-posix(7)), as Debian's `/var/run/postgresql`, which every account
  may enter, lets it.
- **A version for the C library's collations.** PostgreSQL records the
  version of a collation when a database or collation is made, and warns
  when it differs later, so that indexes sorted by the old rules are
  rebuilt. For the C library's collations it asks `gnu_get_libc_version()`
  when `__GLIBC__` is defined; the C library of Radix is not glibc, so the
  version is empty and nothing is caught. The sorting rules are the
  `locales` port's (glibc 2.42's data, ADR 0194), which the C library
  follows, and an update of that port can change the order of strings, and
  with it make an index on text silently wrong.
- **A check after an update that PostgreSQL can pass.** ADR 0386 has a
  service bundle's new version answer on the health socket its manifest
  names, within 60 seconds, or the version before comes back with its data.
  The socket PostgreSQL makes is `.s.PGSQL.5432`, whose name a manifest
  cannot hold (a name starts with a letter or a digit); and a socket named
  in `health` is one the service offers to others, which waits for the
  sockets a bundle provides (docs/deferred.md), and may be one that Spark
  holds for the service, which PostgreSQL cannot take over from it. Its
  health command, `pg_isready`, tells what is needed, and Spark runs it.
- **libpq for PHP.** PHP's `pgsql` and `pdo_pgsql` extensions link libpq,
  which a port finds in the sysroot.

This record changes three earlier ones: the health check after an update
(ADR 0386), the hooks around a backup (ADR 0463) and the keys of a service
file (ADR 0470). Their status lines say so.

## Decision

- **The service `postgresql`** runs as the account `postgres`, which owns
  `/shared/services/org.postgresql.postgresql/postgres`. Its script makes
  the cluster in `data` there at the first start, with `initdb` into a
  folder of its own that it moves into place when it is whole, so that a
  start that stopped half way starts the cluster over. The cluster's
  settings stay in the data folder, where `initdb` writes them sized for
  the machine, as Red Hat's and upstream's packages keep them, and
  updates of the bundle do not change them; this differs from Valkey's and
  PHP-FPM's `conf` folder on purpose, as PostgreSQL rewrites its own
  settings (`ALTER SYSTEM`). The server runs in the foreground, with the
  script's process as its postmaster.
- **Who may connect.** The shipped `pg_hba.conf` takes the account root,
  inside `elevate`, and the account postgres as the superuser by peer
  authentication on the socket (`pg_ident.conf` maps them), any other
  account as the role of its own name by peer authentication, and any
  account over `127.0.0.1` and `::1` with a password, by SCRAM-SHA-256.
  `listen_addresses` is `localhost`. The script opens the live folder to
  every account, since the server decides who connects, and Spark's own
  default for every other service stays as it is.
- **Stop.** The service stops on SIGINT and has 300 seconds, with the
  `stop` of its manifest (service.toml(5) `[stop]`, manifest.json(5)
  `stop`), and its `before` hook of `backup` runs `CHECKPOINT`, so that a
  snapshot of `/shared` holds the cluster as a crash would leave it a
  moment after a checkpoint, which PostgreSQL documents as a valid backup
  ("File System Level Backup").
- **The signal and the time belong to each service,** and Spark's 5
  seconds stay the default for the rest. A longer wait for every service
  would hold a machine's shutdown for as long as any one that does not end
  can be waited for, and a service that ends in a moment gains nothing from
  it. A service says what it needs: a signal from `SIGTERM`, `SIGINT`,
  `SIGQUIT`, `SIGUSR1`, `SIGUSR2` and `SIGWINCH`, the ones daemons take as a
  request to stop (nginx's is `SIGQUIT`, PostgreSQL's `SIGINT`), and
  a time of 1 to 3600 seconds.
- **Ready and healthy.** The service is ready once `pg_isready` passes,
  with the longest `ready.timeout`, 3600 seconds: a start after a crash
  replays the log first and must not be stopped half way, as the upstream
  unit's `TimeoutSec=infinity` says. `pg_isready` fails while the server
  starts up, recovers or shuts down, and passes once it takes connections,
  even with every connection slot in use. The service names no health
  socket.
- **The check after an update takes a health command in place of a
  socket (this changes the health check of ADR 0386).** For a started
  service whose manifest has a health command and no health socket,
  bundled asks Spark for the service's state and has it pass when the
  service is running with its last health check passed, within the same 60
  seconds for all of a bundle's services. A service that is starting,
  waiting to start again or failing its check has the time; one that has
  stopped for good, or that Spark gave up on, fails at once, with why. A
  service with a health socket keeps the socket check, and a service with
  neither is not checked. Going back is as ADR 0386 says.
- **Patch 0002** makes `/system/live/services/postgres` the default folder
  of the Unix socket, in the server, libpq and so in `psql` and every
  program built on libpq, as Debian's packages make it
  `/var/run/postgresql`. A server run by hand passes `-k`, or sets
  `unix_socket_directories`, and its clients `-h` or `PGHOST`.
- **Patch 0003** gives the C library's collations the contents of
  `/system/libraries/locale/VERSION` as their version, which the `locales`
  port installs, `glibc-2.42` today; C and POSIX keep none, as on Linux,
  and so does a system whose file is missing.
- **libpq goes into the sysroot** as a static library with its headers and
  a `libpq.pc`, with the two archives of PostgreSQL's code that libpq
  needs, and PHP is built with `pgsql` and `pdo_pgsql` against it.

## Consequences

- `elevate bundle install --for-everyone` and `elevate service postgresql
  enable` give a server that takes connections: `elevate psql -U
  postgres` opens a superuser session with no password, `elevate createuser
  -U postgres alice` lets alice run `psql -d postgres` as herself, and a
  role with a password connects over `127.0.0.1`.
- `service postgresql stop` with a session open stops the server in
  seconds and the next start needs no recovery. A machine's shutdown can
  wait up to 300 seconds for it.
- A backup holds every database in the snapshot of `/shared`, taken after a
  checkpoint, and makes no dump. A cluster restored from it starts with a
  short recovery, as after a crash a moment after a checkpoint.
- A database that is to sort as English does is made from `template0` with
  a locale of the C library (`createdb --template template0
  --locale-provider libc --locale en_US.UTF-8`), which the `locales` port
  has for English alone; the cluster's own databases sort by code point.
  Its collation's version is the data's, so that an update of the `locales`
  port that changes the order is reported by PostgreSQL, as a new C
  library is on Linux.
- An update of the bundle copies the whole data folder while the server is
  stopped (ADR 0386), which takes time for a large cluster, and until
  copies are reflinks (docs/deferred.md) that time grows with the cluster.
- The check after an update covers every service whose manifest has a
  health command and no socket, not only PostgreSQL's. A service that is
  ready only after its first check passes is running as soon as the check
  does; one whose `ready.when` is not `healthy` is first checked
  `health.every` seconds after it is ready, 30 unless set, so a service
  with a longer interval than the 60 seconds cannot pass this check in time
  and should name a socket or be ready when healthy. PostgreSQL, started
  after a clean stop, takes connections within a few seconds, well inside
  the 60; a start that must replay the log first, after the stop failed to
  be clean, may not, and bundled then goes back.
- A server that other bundles' services reach over the socket, as PHP-FPM's
  pages do, waits for the sockets a bundle provides; they connect over
  `127.0.0.1` with a password meanwhile (docs/deferred.md).
- **Not yet:**
  - TLS with the machine's certificate: the server listens on `localhost`
    only, and an administrator who opens it sets `ssl_cert_file` and
    `ssl_key_file` by hand.
  - `pg_upgrade` between two lines of the bundle (ADR 0387): there is one
    major version.
  - A standby tested in the lab: `libpqwalreceiver` is built and
    `pg_basebackup` is in the bundle.
