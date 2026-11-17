<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0569. Websites, databases and apps are added from the web console through website(8), database(8) and the catalog's sheet

- Status: Accepted
- Date: 2026-11-16

## Context

nginx, PHP-FPM and PostgreSQL are service bundles of the catalog (ADRs
0491, 0520 and 0521). At a terminal an administrator installs them with
`bundle install`, edits nginx's `nginx.conf` or adds a file it
includes, runs `createuser` and `createdb` as postgres, and edits
`pg_hba.conf` and `postgresql.conf` to let another machine in. Each
step is one a person can get wrong in a way that takes a running site
down: a typo in a server block stops nginx's next reload, a line of
`pg_hba.conf` in the wrong place locks a client out, and a password
typed into `psql` lands in its history and in the server's log.

The web console's Add an app, Websites and Databases pages do these
things for an administrator, often from a phone. A page cannot type at
a terminal, so it needs commands that do each step whole, check it
before it takes effect, take it back when it fails, and say what
happened in words a page can show. Three facts of the system shaped
them:

- The console runs commands, as the person or through authd as root
  (ADRs 0514 and 0518); it holds no right of its own to change nginx or
  PostgreSQL.
- A process cannot take another account's name: `setuid` to another
  account fails, and only Spark and authd, which hold the names
  capability (ADR 0218), start a process as an account. nginx's own
  check of its settings, `nginx -t`, has to run as the account nginx,
  since nginx's files are that account's alone.
- A bundle's program never runs as root (ADR 0385): its services run as
  accounts of their own, and its commands in a sandbox for a person.

## Decision

### A website is a file that nginx includes

Each website is one file, `/system/settings/websites/NAME.conf`, root's
and 0644, which the bundle's `nginx.conf` includes at the end of its
`http` block. Its files are in `/shared/www/sites/NAME`, its owner's;
the default server answers 404 under `/sites/`, so that a site's files
are served only under its own names, by its own rules. A file of the
administrator's own in that folder is served too, and listed as theirs.
A `nginx.conf` that was changed by hand keeps its text: `website` does
not edit it, and refuses with WEB-102, showing the two lines to add and
where.

`website add`, `https` and `remove` hold one lock, check everything
they can before anything changes, and write the new set of site files
to a copy that nginx checks before any file is live: a staged folder
and a copy of `nginx.conf` that includes it, in nginx's own `conf`
folder. Only a set that passes the check is renamed into place and
reloaded. After the reload, a file of random name is asked for over
127.0.0.1 under each of the site's names, over TLS with the name
checked for an HTTPS site; a site that does not answer takes every step
back (WEB-104). A change that adds a certificate or the HTTPS port to
nginx's service file restarts nginx, whose sockets Spark holds, so no
connection is refused (ADR 0477). Such a change restarts nginx once
more when the new sites are live, not a reload: as nginx starts it
closes the sockets Spark hands it that its settings do not use yet, as
port 443's before the first HTTPS site, and only a start takes them up
again. Spark says nginx runs as soon as it has started it, so after a
restart or a start `website` waits until nginx answers on port 80.

### A bundle's program runs as its service, through Spark

Spark answers a new request on its control socket, root's alone: `run
SERVICE PROGRAM [ARGUMENT...]` (spark-api's `run`). It starts the
program once as a process of that service's own, the way it starts the
service's health checks: in its resource domain and its view, as its
account, with its environment and `/devices/null` as its input. It
kills the program after 30 seconds and answers with its code and the
first 16 KiB of what it wrote. A service with no account of its own,
or one that holds the names capability or the whole tree, is refused,
and one program runs at a time.

`bundles::as_service` is its caller's side: it checks the bundle as
`bundle run` does (installed for everyone, still sealed, its list the
one recorded and signed by the key recorded, its manifest the one the
list names, the program a sealed file the list names), checks that the
service file of that name is the one bundled wrote for that bundle,
with the account its manifest gives, then asks Spark. website(8) runs
`nginx -t` through it. The trusted prompt is not asked: the program
runs with what the bundle's service holds, not with the caller's access,
which is what the prompt guards (ADRs 0224 and 0493). In nginx's own
view, the check reads exactly what nginx will read, a certificate's
copy and the site's folder included.

### database(8) speaks PostgreSQL's protocol, and gives a role only its verifier

`database` signs in over PostgreSQL's socket as the role postgres, by
peer authentication through `pg_ident.conf`'s map `administrators`
(ADR 0522), and speaks PostgreSQL's protocol itself, version 3 and the
simple query protocol only: no `psql`, no bundle program, no trusted
prompt. A server that asks for a password, a message over 1 MiB, an
answer over 100,000 rows, 30 seconds of quiet, a copy or a binary
column ends the session. Every name in a statement has passed the name
rule and is still quoted as an identifier; the only literals are a
role's verifier, a locale from a fixed list and a comment, each quoted
as a string, and a statement holding a NUL is not sent.

A role's password is 24 random bytes, written as 32 characters of
base64url. PostgreSQL is given only the SCRAM-SHA-256 verifier that
`database` makes of it (PBKDF2 with 4096 rounds and a 16-byte salt,
RFC 5802 and 7677), so the password never reaches the server, its log
or a statement that fails. The command shows it once, in its JSON, and
keeps it nowhere.

The lines `database allow` adds to `pg_hba.conf` are in one block it
marks, `# database(8): who connects from other machines` to `#
database(8): end`; every other line is the administrator's, read and
listed as theirs, never changed. A change to `pg_hba.conf` or to
`ALTER SYSTEM`'s settings is checked through `pg_hba_file_rules` and
`pg_file_settings` before it takes effect, and taken back with
DB-104 and PostgreSQL's own line when either has an error. Other
machines connect over TLS only, with the certificate certd keeps for
PostgreSQL, and with a password.

### An app is installed from the sheet that was read

`bundle sheet ID` fetches an app's newest version from the catalog,
checks it as an install would, and shows its install sheet without
installing: the publisher and key, the licence, what it may use, its
services and their data. bundled keeps that one download for the
install that follows. `bundle install ID --sha256 HEX` installs only
the file whose hash the sheet showed: when the catalog changed between
the two, nothing is installed (APP-102), and the page offers to read the
sheet again. A sheet is read only when a person opens an app, never on
a timer, since it downloads the bundle.

### Errors

Three new areas of error IDs: APP (the catalogs' sheets and installs),
WEB (websites) and DB (databases).

## Consequences

- An administrator adds an app, a website and a database from a phone,
  and each change is checked before it is live and taken back when it
  fails. The same commands do it at a terminal.
- website(8) and database(8) are two more commands in
  `/system/commands`.
- Spark has one more request. What it runs holds no more than the
  service does; a root caller could already have written a service
  file with any program, so the request gives root nothing new.
- nginx's check runs in nginx's own view, so a site whose folder or
  certificate nginx cannot read fails its check, not its first request.
- Every PHP site is served by PHP-FPM's one pool, as one account; a
  pool for each site is a deferred row.
- Deferred, each in `docs/deferred.md`: a site that passes requests to
  another program or address, added from the console; a first
  certificate order that failed, tried again by itself; the catalog's
  summaries and categories in search; one database copied and restored
  on its own. The row "A certificate for each of nginx's sites" closes.

## Alternatives considered

- **Site files in nginx's own settings folder** (`conf/sites/`): that
  folder is the account nginx's, 0600, outside what the console reads
  and what the journal compares, so a hand edit there could not be
  shown, and a reader could not list the sites.
- **Site folders straight under `/shared/www`:** the default server
  serves that folder, so a site's `.env` or `.git` would be served
  under the machine's address, past the site's own rules.
- **Running `nginx -t` as root, or in a sandbox of root's:** a bundle's
  program never runs as root, and root's check would read files nginx
  cannot.
- **Starting the check as the account from `website` itself:** a
  process here cannot take another account's name.
- **authd starting the program as the account, as it starts people's
  jobs:** authd starts sessions for people; a service's account has no
  session, and authd knows nothing of a service's view.
- **`psql` as postgres, or a bundle's client library:** a password
  passed to `psql` is seen by the server and kept in its log when a
  statement fails, and a program of the bundle's would need the trusted
  prompt or root.
- **The password given to PostgreSQL to hash:** PostgreSQL would log it
  with a statement that fails, and `log_statement` would keep it.
- **Installing by ID without a hash:** the sheet a person agreed to
  could differ from what is installed when the catalog changes between
  the two.
