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

`-U postgres` names the role, as there is none called `root`; only a
`database restore` makes one, for as long as it runs (see "Putting one database
back"). The first time you run `elevate psql`, Dukovo asks in your terminal
pane, with a warning, whether `psql` may run with root's access, as for any
command of the catalog's that runs with its user's; answer `A` to allow it
always. `psql` looks for the server's socket in the service's folder,
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

## Databases for programs and websites

A program that keeps its data in PostgreSQL, such as a website, needs a database
and a role that owns it. `database` makes both in one command, gives the role a
password that it makes itself, and lets no one else connect to the database. It
signs in as the superuser through the socket, so only root runs it, and an
administrator types `elevate database`:

```
$ elevate database add shop --site shop.example.org --json
{"format":1,"database":"shop","role":"shop","password":"kT3x0Gv9uQ1mYwZb5nR2LcHd7SpJaEfA","host":"127.0.0.1","port":5432}
```

The role is `shop` too, unless `--owner` names another. `--site` records which
website the database is for, which `database list` shows beside it. The password
is 32 characters from 24 random bytes, and it is in this line and nowhere else:
PostgreSQL is told only a SCRAM-SHA-256 verifier made from it, which cannot be
turned back into the password, and it is in no argument, no file and no log
line. Keep it where the program that connects reads it. A new one comes from
`elevate database password shop --json`; the old one then signs no one in, and a
session that is open stays open.

A database and a role are named by one rule, stricter than PostgreSQL's: a
lowercase letter, then lowercase letters, digits and `_`, up to 63 characters,
and never a name that starts with `pg_` or is `postgres`, `template0` or
`template1`. A database or a role of that name that is there already ends the
command before anything is made ([DB-102](../../errors/DB-102.md)), and a
statement that fails after the role was made drops it again, so a second try
starts clean.

`--locale` says how the database sorts text: `en_US.UTF-8`, the default, which
sorts as English does, or `C.UTF-8`, which sorts by character code and is the
same on every machine. The first needs the system's language data, and
PostgreSQL asks for the indexes to be made again when an update changes it, as
"A role and a database for an application" says; `database list` shows it as
`collation_mismatch`.

### Three ways to connect

A database is reached in one of three ways, and `database add` sets up the first
two.

- **With a password, from this server.** The program connects to `127.0.0.1`,
  port 5432, as the role, with the password. The bundle's `pg_hba.conf` lets
  such a connection in with SCRAM-SHA-256 already, so no setting changes. A
  website's PHP connects this way, as "Connecting from PHP" shows.
- **As an account, by the socket.** `database add notes --peer notes --json` makes
  the role `notes` with no password, for the account of that name. A program that
  runs as that account signs in as the role through the socket, and `psql -d
  notes` does. The account has to be there ([DB-106](../../errors/DB-106.md)). A
  service of another bundle cannot reach the socket yet, so PHP-FPM's pages use a
  password. Without `--json`, `database add` refuses a role that has a password,
  since the password is shown only in JSON.
- **With a password, from other machines.** `database allow`, below.

### Letting other machines connect

`database allow` lets networks sign in to one database, over TLS, as its owner,
with the password:

```
$ elevate database allow shop --from 192.168.1.0/24 --progress
step 1 of 8: checking the database, the networks and the certificate
step 2 of 8: naming the certificate in PostgreSQL's service file
step 3 of 8: writing the lines of pg_hba.conf
step 4 of 8: setting what PostgreSQL listens with
step 5 of 8: having PostgreSQL check its files
step 6 of 8: opening port 5432 to these networks only
step 7 of 8: restarting PostgreSQL
step 8 of 8: checking that PostgreSQL answers over TLS
192.168.1.0/24 may connect to shop over TLS, as shop, with the password
PostgreSQL restarted, and its open sessions ended
```

A network is an address and the length of its prefix, `192.168.1.0/24` or
`2001:db8::/32`, and `--from` may be given up to 20 times. `--certificate` names
one of `cert list`, and is `this-machine` unless you say; certd has to keep it
([DB-105](../../errors/DB-105.md)). It changes four places, and every other line
of each stays as it was:

- `pg_hba.conf` gains a `hostssl` line with `scram-sha-256` for each network, in
  one block at the end of the file, from `# database(8): who connects from other
  machines (ADR 0569)` to `# database(8): end`. A line outside the block is yours:
  `database` reads it, lists it as written by hand and never changes it.
- `postgresql.auto.conf`, which `ALTER SYSTEM` writes, gets `listen_addresses = '*'`,
  `ssl = on` and the paths of the certificate's copy, each only when it differs.
- PostgreSQL's service file gets the certificate in `certificates`, in its
  override, which keeps every item and comment that is in it.
- The firewall's rules, `/system/settings/firewall`, get a block that opens port
  5432 to the networks of every database and to no other:

  ```
  # database(8): who reaches PostgreSQL from other machines (ADR 0570)
  allow tcp 5432 from 192.168.1.0/24
  deny tcp 5432
  # database(8): end
  ```

  It has an `allow` for each network that a line of the block of `pg_hba.conf`
  names, whichever database the line is for, since 5432 is one port for all of
  them, and a `deny` that ends it.

Before anything takes effect, PostgreSQL checks the files it will read, and a
mistake it finds ends the command with everything put back ([DB-104](../../errors/DB-104.md)).
A new `listen_addresses` or certificate needs a restart: the sessions that are
open end, and the command waits for the new server and then connects over TLS to
check that the certificate loads. A change that needs none, such as a second
network for a database that lets others in already, is a reload that ends
nothing, and `allow` with the same networks again changes nothing.

PostgreSQL then listens on the network, and the block opens port 5432 to the
networks of the databases and to no other address: a connection from another one
is dropped before PostgreSQL sees it, and for those that are let through the
`hostssl` lines decide who may sign in. `elevate firewall list` shows the
block's rules, numbered as the others are. The block goes at the end of the
file, after your own rules, and the first rule that matches decides (see [The
firewall](networking.md#the-firewall)), so a rule of your own before the block
that covers port 5432, `allow tcp` or `allow tcp 5432` say, decides first for the
connections it names: `database` does not move it, `allow` says so, and the
console's **Databases** page shows it as a warning. If you move the block,
`database` writes it where you put it. It is not a trial of the firewall
(`firewall allow --try`), since Spark's trials hold files under
`/system/settings` and `pg_hba.conf` is not one. While a trial of the rules
runs, `allow`, `deny` and `remove` change nothing ([DB-107](../../errors/DB-107.md)),
and neither do they when the block's two comment lines do not make one block or
the file would hold more rules than the firewall holds
([DB-108](../../errors/DB-108.md)). A client checks the server with
`sslmode=verify-full`, and for the machine's own certificate needs the file `cert
show this-machine --pem` prints, as its `sslrootcert` (see
[Certificates](certificates.md#postgresql)).

`database deny shop` takes the lines of that database out of the block, or only
those of the networks named with `--from`. The firewall's block is made again from
the lines that are left, so a network that no database names any more is shut out
of port 5432, and when no line is left, the block goes, PostgreSQL listens on this
machine only again, which restarts it, and the settings for TLS stay.

### Taking a database away

`database remove shop` drops the database with its data, takes its lines out of
the block and shuts port 5432 in the firewall to the networks that no database
names any more. It stops with the programs that have a session open to it, by name, and
does nothing ([DB-103](../../errors/DB-103.md)); `--force` ends the sessions and
drops it. `--role` drops its owner too, only when `database` made the role and it
owns and may use nothing else; otherwise the command says that the role stays and
why. It is the data that goes: a snapshot of `/shared` taken before holds it until
the snapshots are pruned, and so do the copies of it that backups made (see
"Putting one database back").

### Putting one database back

Before each backup the system copies every database into a file of its own (see
"Backups"), and `database copies` lists the copies of one, the newest first:

```
$ elevate database copies shop
2026-11-16 02:00  12M  live  /shared/services/org.postgresql.postgresql/postgres/dumps/shop.dump
2026-11-15 02:00  12M  2026-11-15-0200  /snapshots/shared/2026-11-15-0200/services/org.postgresql.postgresql/postgres/dumps/shop.dump
```

`live` is the copy in use. The others are the copies that the snapshots of
`/shared` hold, each named by its snapshot, and a copy that several snapshots hold
is listed once. A database that has gone has copies in the snapshots made before
it went. `database restore` puts one back, from the path the list gives, and
always into a database that is made for it:

```
$ elevate database restore shop --from /shared/services/org.postgresql.postgresql/postgres/dumps/shop.dump
put the copy into the new database shop_restored_20261116; shop was not changed
```

By default the copy goes into a new database called `shop_restored_` and the day
(`shop_restored_20261116_2` when that is taken), with the owner, the encoding and
the sorting of `shop`, and nobody else may connect to it. `--as NAME` chooses
the name. `shop` is not touched, so the two can be compared, and the program is
pointed at the new one once it is right.

To put the copy where the database is, add `--over`:

```
$ elevate database restore shop --from /snapshots/shared/2026-11-15-0200/services/org.postgresql.postgresql/postgres/dumps/shop.dump --over --force
put the copy into shop; the database as it was is kept as shop_before_20261116_0200
`database remove shop_before_20261116_0200` removes it when it is not needed
```

The database as it is now is kept, renamed `shop_before_` with the day and the
minute, and a new `shop` takes its name and comment. Its owner cannot connect
until the copy is in, so that no program finds it empty. Programs that have a
session open stop it, by name ([DB-103](../../errors/DB-103.md)); `--force` ends
the sessions and lets no other in until the database is renamed. Nothing removes
the kept database: it holds the rows that the copy may lack, and the room it takes,
until `database remove` drops it.

When a step fails after a database was made, it is taken back: the new database is
dropped and, with `--over`, the old one has its name back and takes connections
again. The error then gives what `pg_restore` said first, and how the taking back
went ([DB-110](../../errors/DB-110.md)). A file that is no copy made with
`pg_dump -Fc` is refused before anything is made
([DB-109](../../errors/DB-109.md)). `--progress` tells each step, and `--json`
says what was done. The web console's **Restore…** on a database's page offers the
copies that `database copies` lists, as a long task with steps.

What a restore keeps in mind:

- **A short-lived role called `root`.** A copy holds what the database's owner
  wrote, and putting it back runs some of it, so it is put back with the owner's
  rights and no more, never the superuser's. `pg_restore` signs in over the
  server's socket as a role called `root`, which `database restore` makes for the
  restore and drops when it ends, whether it worked or not. It may become the
  database's owner, as `pg_restore --role` does at once, and inherits none of its
  rights; it may connect to the new database alone, and is no superuser. Do not
  make a role called `root`: the restore stops before it makes anything when
  there is one. A restore that a power cut stopped may leave one: `elevate psql
  -U postgres -c 'DROP ROLE root'` removes it.
- **The owner's rights only.** A copy that creates an extension that PostgreSQL
  does not trust the database's owner with, or that needs a superuser for
  anything else, fails ([DB-110](../../errors/DB-110.md) says how to put such a
  copy in by hand), and a database that a superuser owns is not restored here.
- **Roles are not in a copy.** A grant to a role that is not on this machine fails
  the restore: make the role first (`elevate createuser -U postgres NAME`).
- **No subscriptions.** A subscription in the copy would connect to another
  server, so none is made, and a database that had subscriptions comes back
  without them.
- **The first restore asks.** `pg_restore` is the bundle's program, run as root,
  so Dukovo asks once whether the bundle may run with root's access, as for
  `elevate psql` (see "Connecting as an administrator"). A restore that nobody
  answers for fails ([DB-110](../../errors/DB-110.md)): run `elevate pg_restore
  --version` at a terminal once and answer `A`, before the web console restores.

### From the web console

The web console's **Databases** page runs these commands, with the checks first
and the password shown once in a box that copies and saves it; see
[Databases](web-console.md#databases) in the chapter on the console. The password
that **Add a database** shows is this line's. The page also says when each
database was copied, and its **Restore…** puts a copy back.

## A role and a database for an application

A program, such as a web application, connects over the network of the
computer itself, `127.0.0.1`, with a password, and has a role and a database
of its own. `database add` makes both (see "Databases for programs and
websites"). With PostgreSQL's own programs the administrator makes them, and the
password is asked for as it is set:

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
`scram-sha-256`; and restarts the service. `database allow` does each of these
steps for one database, with the checks before and after (see "Letting other
machines connect"). Done by hand, they leave port 5432 open to every address,
since the firewall lets other computers reach what a service listens on;
`database allow` also writes the block of firewall rules that opens it only to
the networks named.

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
restored from it starts with a short recovery.

In the same step each database is copied with `pg_dump -Fc`, compressed with
zstd, into `dumps/NAME.dump` in the data folder of the service,
`/shared/services/org.postgresql.postgresql/postgres/dumps`, which the snapshot
and so the backup holds. A copy is written beside the last one and moved over it
once it is whole, so one that fails leaves the last good one, and the copies of
databases that are gone are removed. It is made before each round of backups,
the nightly one and `elevate backup run`, and never on its own: a computer with
no backup place set up has no copies. The step has up to an hour, says a line for
each database in the service's log (`elevate log postgresql`), and when a copy
fails the backup goes on and says so. A copy holds all that its database does,
and is readable by the account postgres and root alone. `database copies` lists
the copies and `database restore` puts one back (see "Putting one database
back").

A dump is also for another version of PostgreSQL, or another computer. `pg_dump`
writes a database as SQL, the redirection puts it in a file of yours, and `psql`
reads it into a database made the same way:

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
