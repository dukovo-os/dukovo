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
as Valkey (ADR 0492) and PHP-FPM (ADR 0520) are run. What the server
needs of the system:

- **A stop of its own.** Spark ends a service with SIGTERM and gives it 5
  seconds unless the service's `[stop]` says otherwise (ADR 0477).
  PostgreSQL takes SIGTERM for a *smart* shutdown, which waits for every
  client to disconnect: with one connected, the 5 seconds pass, Spark ends
  the whole domain, and the next start recovers from a crash. Its fast
  shutdown, which ends the sessions, rolls back what they had open, writes
  a checkpoint and stops, is SIGINT, and the checkpoint writes every
  changed buffer, which can take longer than 5 seconds. PostgreSQL's own
  systemd unit says `KillMode=mixed`, `KillSignal=SIGINT` and
  `TimeoutSec=infinity`. Spark signals only the service's own process,
  which is the postmaster, as `KillMode=mixed` does.
- **A checkpoint before a backup.** A snapshot of a running cluster is a
  valid backup, as after a crash, and the less log there is to replay
  since the last checkpoint, the sooner a cluster restored from it
  starts. A bundle's manifest names programs that run around a backup's
  snapshot (`backup`, ADRs 0463 and 0477).
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
  cannot hold (a name starts with a letter or a digit), and the server
  takes a connection on it while it starts up or recovers, only to refuse
  the session, so a socket that answers says little. Its health command,
  `pg_isready`, tells what is needed, and Spark runs it.
- **libpq for PHP.** PHP's `pgsql` and `pdo_pgsql` extensions link libpq,
  which a port finds in the sysroot.

This record changes the health check after an update (ADR 0386), whose
status line says so.

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
  `stop`, ADR 0477), and its `before` hook of `backup` runs `CHECKPOINT`,
  so that a snapshot of `/shared` holds the cluster as a crash would leave
  it a moment after a checkpoint, which PostgreSQL documents as a valid
  backup ("File System Level Backup"). 300 seconds, not the upstream
  unit's endless wait: a machine's shutdown waits as long for the service,
  and that is time for a checkpoint of gigabytes of changed pages on a slow
  disk; a stop that takes longer ends the domain, and the next start
  replays the log, which loses no committed transaction.
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
- **TLS with a certificate certd keeps (ADR 0512).** An administrator who
  opens the server to other machines names the certificate in the
  service's file (`certificates = ["this-machine"]`, or one from `cert
  get` or `cert add`) and sets `ssl = on`, `ssl_cert_file` and
  `ssl_key_file` to the copies Spark gives the service, in
  `/system/live/certificates/postgresql/NAME`. The server reads them again
  on the SIGHUP Spark sends when certd renews the certificate, with no
  restart. The bundle's settings leave TLS off, as the server listens on
  `localhost` alone.
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
- Other bundles' services, PHP-FPM's pages among them, connect over
  `127.0.0.1` with a password: a bundle offers others only a socket that
  Spark makes and holds for its service (ADR 0477), and the server makes
  its own (docs/deferred.md).
- **Not yet:**
  - `pg_upgrade` between two lines of the bundle (ADR 0387): there is one
    major version.
  - A standby tested in the lab: `libpqwalreceiver` is built and
    `pg_basebackup` is in the bundle.
