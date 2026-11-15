<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# PostgreSQL

PostgreSQL, the relational database server, comes from the catalog as a
bundle: the server, `psql` and the other client and administration
programs, libpq, the contrib modules and PostgreSQL's own test suites. Its
service, `postgresql`, runs the server as the account `postgres`, so the
bundle installs only for everyone and an administrator installs it:

```
$ elevate bundle install --for-everyone org.postgresql.postgresql
$ elevate service postgresql enable
```

The first start makes the cluster, the folder of databases the server
keeps, in the service's data folder. It takes a few seconds, and the
service is running once the server takes connections:

```
$ service postgresql status -n 0
```

says `active (running)` then. The first time you run any command of the
bundle, such as `psql`, Dukovo asks whether it may run with your access, as
for any command of the catalog's that does; answer `A` to allow it always.

## Connecting as an administrator

An administrator is the superuser of the server, with no password, through
elevate: the server takes root, which is who elevate runs a command as, for
the role `postgres` on its Unix socket.

```
$ elevate psql -U postgres
postgres=# SELECT current_user;
 current_user
--------------
 postgres
(1 row)
```

`-U postgres` names the role, as there is none called `root`. The first time
you run `elevate psql`, Dukovo asks in your terminal pane, with a warning,
whether `psql` may run with root's access, as for any command of the
catalog's that runs with its user's; answer `A` to allow it always. `psql`
looks for the server's socket in the service's folder,
`/system/live/services/postgres`, so it needs no `-h`. Its line editing,
history (`~/.psql_history`) and tab completion are libedit's.

## A role for a person

A person who has an account on the computer connects as the role of the same
name, with no password, once an administrator has made it:

```
$ elevate createuser -U postgres alice
```

Alice then runs `psql -d postgres`, since a role's own database, which
`psql` opens when it is not told another, does not exist until it is made.
A role that may make databases of its own has `--createdb`:

```
$ elevate createuser -U postgres --createdb alice
$ createdb notes
$ psql notes
```

The server learns who is at the other end of its socket from the system,
not from what the program says, so no one else can connect as `alice`.

## A role and a database for an application

A program, such as a web application, connects over the network of the
computer itself, `127.0.0.1`, with a password, and has a role and a database
of its own. The administrator makes both, and the password is asked for as
it is set:

```
$ elevate createuser -U postgres --pwprompt shop
$ elevate createdb -U postgres --owner shop shop
```

The application then connects as `shop` to `127.0.0.1` on port 5432, with the
database `shop` and the password; `psql -h 127.0.0.1 -U shop shop` asks for
it. Over the network the server has only the password to go by, which it
takes by SCRAM-SHA-256, so that no one who watches the connection can use it
again.

The cluster's databases sort text by code point, as the builtin locale
`C.UTF-8` does: `Zebra` comes before `apple`. A database that sorts as
English does is made from `template0` with a locale of the C library:

```
$ elevate createdb -U postgres --owner shop --template template0 \
    --locale-provider libc --locale en_US.UTF-8 shop
```

The locales are English's, `en_US.UTF-8`, `en_GB.UTF-8` and the others of
the `locales` port; no other language's is installed. A database records
which version of the locales' data it was made with, and PostgreSQL warns
when an update of the system changes it, so that indexes sorted by the old
rules are made again (`REINDEX`, and `ALTER DATABASE ... REFRESH COLLATION
VERSION`).

## Connecting from PHP

PHP's `pgsql` and `pdo_pgsql` extensions use the bundle's libpq. A program
that a person runs with `php` connects as the person's role over the socket,
with no host and no password; PHP-FPM's pages, which run as the account
`php`, connect over `127.0.0.1` with a role's password:

```php
$db = new PDO('pgsql:host=127.0.0.1;dbname=shop', 'shop', $password);
```

[Connecting to PostgreSQL](php.md#connecting-to-postgresql) in the PHP
chapter has more.

## Its settings

The cluster's settings are in its folder, the service's data folder,
`/shared/services/org.postgresql.postgresql/postgres/data`:

- `postgresql.conf`, which `initdb` wrote and sized for the computer, with
  PostgreSQL's own comment on each setting, and `postgresql.auto.conf`,
  which `ALTER SYSTEM` writes;
- `pg_hba.conf`, which says who may connect and how each proves who it is,
  and `pg_ident.conf`, which maps accounts to roles for it. Both are the
  bundle's: root and `postgres` are the superuser on the socket, any other
  account is the role of its name on the socket, and any program on the
  computer is the role it names, with its password, over `127.0.0.1` and
  `::1`. Each line of them has its comment.

An update of the bundle leaves them as they are. Edit them through elevate,
and have the server read them again; most settings take a reload, and a few,
such as `shared_buffers` and `max_connections`, a restart, which the server's
log says when it reads a setting that needs one:

```
$ elevate nano /shared/services/org.postgresql.postgresql/postgres/data/postgresql.conf
$ elevate service postgresql reload
$ elevate log postgresql | tail -n 3
```

PostgreSQL's documentation, at <https://www.postgresql.org/docs/19/>,
describes every setting. What differs on Dukovo:

| On a Linux distribution | On Dukovo |
| --- | --- |
| `/etc/postgresql/19/main/` | `/shared/services/org.postgresql.postgresql/postgres/data/`, which has the data too |
| `/var/run/postgresql`, the socket's folder | `/system/live/services/postgres` |
| `sudo -u postgres psql` | `elevate psql -U postgres` |
| `pg_createcluster`, or `initdb` run by hand | The service makes the cluster at its first start |
| `systemctl reload postgresql` | `elevate service postgresql reload` |
| `journalctl -u postgresql` | The system's log: `elevate log postgresql` |
| Runs as `postgres` | Runs as the account `postgres` |

The server listens on `localhost` only. To take connections from other
computers, an administrator gives the server a certificate that certd keeps,
the machine's own or one from an authority, as
[Certificates](certificates.md#postgresql) shows; sets `listen_addresses`,
such as `'*'`; adds a `hostssl` line for them to `pg_hba.conf`, with
`scram-sha-256`; and restarts the service. The firewall lets other
computers reach what a service listens on, so it needs no rule of its
own.

## Stopping and starting

```
$ elevate service postgresql stop
$ elevate service postgresql start
```

A stop ends the sessions that are open, rolls back what they had open and
writes a checkpoint, as `pg_ctl stop -m fast` does, and takes seconds; the
service has up to five minutes for it, which a large cache of changed pages
can need, and so a computer's shutdown waits as long for the server. The next
start finds the cluster shut down and needs no recovery. After a crash, or a
power loss, the server replays the log first, which can take a while for a
large cluster, and the service is ready when the server takes connections
again.

## Backups

A backup of the computer (see [Updates, snapshots, backups and
recovery](updates.md)) takes a snapshot of `/shared`, with the cluster in
it. Before the snapshot the system has the server write a checkpoint, so that
the snapshot holds the cluster as a crash would leave it a moment after one,
which PostgreSQL documents as a valid backup of the files. A cluster
restored from it starts with a short recovery. The backup holds every
database and makes no dump.

A dump is for another version of PostgreSQL, or another computer, and for a
single database. `pg_dump` writes a database as SQL, the redirection puts
it in a file of yours, and `psql` reads it into a database made the same
way:

```
$ elevate pg_dump -U postgres shop > shop.sql
$ elevate createdb -U postgres --owner shop shop2
$ psql -h 127.0.0.1 -U shop -f shop.sql shop2
```

`pg_dumpall` dumps the roles and every database, and `pg_restore` reads the
other formats of `pg_dump`.

An update of the bundle stops the service, copies its data folder, installs
the new version and starts it again, as for every service. The new version
has 60 seconds to take connections, or the one before comes back with its
data; the copy of a large cluster takes time, during which the server is
down. A cluster of another major version of PostgreSQL is refused by the
server, with a message in its log that says so.

## A server of your own, for development

The bundle's programs run for anyone, so a person developing makes a server
of their own, in a folder of their own, with no service and no administrator.
`initdb` makes the cluster, with the settings the service uses, and `pg_ctl`
starts and stops the server:

```
$ initdb -D ~/pgdata --locale-provider=builtin --builtin-locale=C.UTF-8 \
    --locale=C -E UTF8
$ pg_ctl -D ~/pgdata -o '-k /tmp' -l ~/pgdata/server.log start
$ psql -h /tmp postgres
```

`-k /tmp` puts the server's socket in `/tmp`, which this account may write,
and `psql -h /tmp` (or `PGHOST=/tmp`, which every libpq program reads) looks
there: PostgreSQL's default folder is the service's, which only the service
may write to. When the service is running, which holds port 5432 of the
computer, give the server another port, `-o '-k /tmp -p 5433'`, and the
programs `-p 5433`.

```
$ pg_ctl -D ~/pgdata stop
```

stops it with a fast shutdown. `pg_ctl -D ~/pgdata status` and `reload` do
what they say. The programs find their libraries and the files they read
from where they are, so the bundle may be in `/applications` or in
somebody's own applications folder; `pg_config --bindir` says where.

## What it has and what it lacks

The server and its programs are PostgreSQL 19, built as on Linux: the
client programs (`psql`, `pg_dump`, `pg_dumpall`, `pg_restore`, `createdb`,
`dropdb`, `createuser`, `dropuser`, `pg_isready`, `vacuumdb`, `reindexdb`,
`clusterdb`, `pgbench`), the administration programs (`initdb`, `pg_ctl`,
`pg_basebackup`, `pg_verifybackup`, `pg_amcheck`, `pg_controldata`,
`pg_resetwal`, `pg_rewind`, `pg_upgrade`, `pg_checksums`, `pg_waldump`,
`pg_walsummary`, `pg_archivecleanup`, `pg_combinebackup`, `pg_receivewal`,
`pg_recvlogical`, `pg_createsubscriber`, `pg_test_fsync`, `pg_test_timing`,
`oid2name`, `vacuumlo`) and `postgres`. They have TLS (OpenSSL), compression
with zlib, lz4 and zstd, XML (libxml2) and PL/pgSQL. The contrib modules are
there as extensions, each made with `CREATE EXTENSION`: `pgcrypto`,
`pg_trgm`, `hstore`, `pg_stat_statements` (which `shared_preload_libraries`
loads), `postgres_fdw`, `dblink` and the others of PostgreSQL's contrib
folder. Time zones are the system's, so an update of the system's time zone
data reaches the server. Data checksums are on.

These are missing, each as Dukovo does not have what it needs yet:
collations from ICU, which wait for an ICU port; the procedural languages
PL/Perl, PL/Python and PL/Tcl; JIT compilation with LLVM; the contrib
modules `uuid-ossp`, `xml2` and `sepgsql`; asynchronous reads through
`io_uring`, as the server reads through its worker processes; messages in
languages other than English; and the processes' titles in `ps`, where
each process of the server shows the command that started it and not what it
does. The server uses ordinary memory pages, so `huge_pages = try` says
`off` and `on` is refused, and it writes no backtraces for its errors.

PostgreSQL's own test suites are in the bundle's `tests` folder, with the
programs that run them; the lab's test of PostgreSQL runs all of them and
lists the tests it leaves out, each with its reason, in
`lab/tests/services/postgresql-skips.txt` of Dukovo's repository.
