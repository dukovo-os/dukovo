<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0570. PostgreSQL's port is open only to the networks a database is opened to, a certificate that could not be had is tried again, each database is copied before a backup, and a website's size and visits are counted

- Status: Accepted
- Date: 2026-11-16

## Context

The web console adds websites, databases and certificates through
website(8), database(8) and cert(8) (ADRs 0513 and 0569). Four things
those commands left open:

- **PostgreSQL's port.** The firewall lets every address reach the port of
  a service that listens, while it runs (ADR 0334). Once `database allow`
  has PostgreSQL listen on the network, port 5432 is open to everyone, and
  only `pg_hba.conf` keeps the others out, after the TCP and TLS
  handshakes.
- **A certificate that could not be had.** certd renews the certificates it
  has. An order for a new name that fails, because the name does not lead
  here yet or port 80 is closed, leaves a `failed` line in the history and
  nothing else: a person makes the order again, after they have mended the
  cause and remembered to.
- **One database.** A backup holds `/shared` after a checkpoint of
  PostgreSQL (ADR 0522). The whole cluster can be put back from it, but no
  database can be put back on its own, and the other databases would go
  back in time with it.
- **A site's size and visits.** nginx writes every request to the system's
  log in the `combined` format, which does not say which site was asked, and
  holds the visitor's address and agent. Nothing counts, and only the data
  folders of apps are measured (ADR 0565).

## Decision

### PostgreSQL's port is open to the networks a database is opened to, and to no others

`database allow`, `deny` and `remove` keep a block of rules in
`/system/settings/firewall`, between two comment lines, as they keep a
block in `pg_hba.conf` (ADR 0569):

```
# database(8): who reaches PostgreSQL from other machines (ADR 0570)
allow tcp 5432 from 192.168.1.0/24
allow tcp 5432 from 10.0.0.0/8
deny tcp 5432
# database(8): end
```

The block has an `allow` for each network that a line of the block of
`pg_hba.conf` names, whichever database the line is for, since 5432 is one
port for all of them, each network once, in the order they are first seen,
and a `deny` that ends it. `deny` and `remove` make the block again from
the lines that are left, and take it away when none is. A block that is
already as it should be is not written again. A new block goes at the end of
the file, after a blank line, and a block that is there is written where it
is, so that an administrator who moved it keeps it where they put it. Every
other line is kept byte for byte.

The change is not a trial of the firewall (ADR 0518). A trial holds files
under `/system/settings` alone, and `pg_hba.conf` and the settings that
`ALTER SYSTEM` writes are not there; and nothing here can cut the console
off, which has a port of its own. The firewall's file takes its place with
the other files of the change: it is written after PostgreSQL has checked
the files it will read and before it restarts or reads them again, the
firewall is asked to load it, and a later step that fails puts it back with
the rest and has the firewall load it again. `remove` shuts the port to the
networks that no database names any more once the database is dropped, so a
failure there is told, and the database stays dropped.

Two refusals come before anything is changed. While a trial of the
firewall's rules runs, Spark would put the file back as it was when the trial
ends, and the block with it (DB-107). A file whose two comment lines do not
make one block, or that would hold more rules than the firewall holds, 128,
cannot take it (DB-108); the words say which line, and an administrator
mends the file by hand. A rule of the administrator's before the block,
which decides for some connections to the port first (`allow tcp`, or
`allow tcp 5432`, say), is not moved: `allow` says so in a line of its
output, and `database list --json` has it as `server.firewall.earlier`,
beside the `networks` the block opens the port to. `firewall list` shows the
block's rules as any others, and `firewall remove` of one of them works; the
next `allow` writes the block again.

*Alternatives.* The block at the top of the file would decide before an
administrator's own rule, and so would override a deny of a network that an
administrator wrote; at the end, their rules decide first and the warning
tells when that opens the port wider than the databases do. A block for each
database would repeat a network that two of them share, and 5432 is one port
for all of them: one block, made from the lines of `pg_hba.conf`, says what is
open to whom.

### A certificate that could not be had is kept, and certd tries it again

An order that `cert get` makes by HTTP and that fails after it began is kept
as a record, `/system/state/certd/wanted/NAME.toml`, root's, in a folder of
0700, written while the store's lock is held. It holds the names, the
authority, the type of key and the profile, the file of the account at the
authority, the contact address and whether the terms were agreed to, the
times of the first, the last and the next try, how many failed, and why the
last did. `wanted.json` beside it, which anyone reads, holds the same without
the account, for `cert wanted`. An order made with `--dry-run`, `--check`,
`--staging` or `--dns` is not kept: the first three are for a person who
watches, and a DNS order needs a person to add the TXT records, so a try by
itself could never work. An order that failed before the authority was asked
has no account yet; it is kept when `--agree-terms` was given, and certd makes
the account at its first try with the contact address the order had.

certd's look gains a phase after the certificates it keeps: each record whose
next try is due is ordered as `cert get` orders it, with port 80 held by
certd, or the answers served by nginx, as for a renewal. A try that works
installs the certificate, tells Spark, takes the record away and writes `got`
into the history; one that fails writes the reason into the record and a
`failed` line into the history. certd looks again when the soonest record is
due, if that is before its next look, and `cert get` asks Spark to have certd
look after it changes a record, so that the first retry does not wait for the
next look.

The first try is when the order fails. The next tries come every hour for the
day after it, then every 6 hours, until 14 days after the first, when the
record is given up: it is kept, so that `cert wanted` shows why, and tried no
more, until `cert forget` or a `cert get` of the name takes it away. An
authority that asks to be left alone for a time, in a `Retry-After`, as a rate
limit does, is left alone until then, and a time past the 14 days gives the
record up. A record is given up at once when the account at the authority was
taken away and the order did not agree to the terms for a new one, or when a
certificate of that name was added by hand since. Let's Encrypt allows five
failed validations an hour for an account and a name, so an hourly try stays
below it. A lab build of certd reads `retry_s` from
`/system/settings/certd-lab.toml` to shorten the hour for a test; a release
has no such setting.

A certificate that has failed for 24 hours raises the alert `certificate:NAME`,
"The certificate NAME could not be had for a day: WHY", a warning that is
about something stopped (CRT-123); when the certificate also runs out, the
alert for that is shown in its place. A given-up record raises
`certificate-given-up:NAME` (CRT-124), a warning too, since alertd has no
lesser kind. Both clear when the record goes: a try worked, `cert forget`
took it, or `cert remove` took it with the certificate's name.

`cert wanted [--json]` lists the records and `cert forget NAME` removes one.
A `cert get` of a name that has a record replaces it, and one that works
removes it. The words of a `cert get` that failed and was kept end with
`certd tries again every hour; cert forget NAME stops it`.

*Alternatives.* Fourteen days, hourly for the first, is long enough to cover
a name whose DNS record or router forward is mended later in the week, and
short enough that a name nobody wants is not asked about for ever; after the
first day the authority is asked four times a day. Trying every hour for the
whole time would add nothing, since a cause that a day of tries has not
cleared is mended by a person. A record that is never given up would hide that
nothing has worked for months, which the alert and the give-up say instead.

### Each database is copied before a backup, and a copy is put back as a new database

The PostgreSQL bundle has a program, `postgresql-backup`, a shell script like
`postgresql-service`, which is the `before` hook of `backup` with an hour to
run. It writes a checkpoint, then for each database that is not a template and
takes connections it runs `pg_dump -Fc -Z zstd:3` into `dumps/NAME.dump.new`
in the data folder of the account postgres, and renames the file over
`dumps/NAME.dump` once it is whole. The folder is in the snapshot of
`/shared`, so it is in every backup. Copies of databases that are gone, and a
file a stopped hook left half written, are removed. The hook says a line for
each database and one at the end, and a copy that fails leaves the last good
one as it was and ends the hook with status 1, which `backup` tells and goes
on after. It runs before each round of backups, so each night and at
`backup run`, and never on its own: a machine with no backup has no copies. A
copy is 0600, readable by postgres and root.

`database copies NAME [--json]` lists the copies of a database, the one in use
and those in the snapshots of `/shared`, the newest first; a copy that several
snapshots hold is listed once. `database list --json` gives each database
`copy`, the time and size of the copy in use or `null`, and the server
`copies`, true when the hook of the service is the one that copies. It is the
service's file that says, so it is told while the server is off.

`database restore NAME --from FILE [--as NEW | --over [--force]]`, run by root,
puts a copy into a database made for it, and writes over nothing. The file is
opened once, never through a link, and must be a regular file of at most 64 GiB
that starts as `pg_dump -Fc` writes (DB-109); the open file is `pg_restore`'s
input, so what was checked is what is read. With `--as`, the default being
`NAME_restored_` and the day, the copy goes into a new database with the same
owner, encoding and sorting, which nobody else may connect to. With `--over`,
the database is renamed `NAME_before_` and the day and minute, a new database
takes its name and comment, and its owner cannot connect while the copy goes in,
so that no program finds it empty. Open sessions stop the command (DB-103)
unless `--force` ends them and lets no other in. The old database stays until a
person removes it with `database remove`. Each step that was made is taken back
when a later one fails, last first; DB-110 gives the first 16 KiB of what
`pg_restore` said and how the taking back went, and the old database is never
dropped.

`pg_restore` is the bundle's own program, run as root with `--no-owner
--role=OWNER --exit-on-error --no-subscriptions`, over PostgreSQL's socket. It
is the one thing database(8) does with a program of the bundle. It is found and
checked as `bundle run` checks a bundle installed for everyone (its folder
sealed, its list the one recorded and signed, the program one the list names)
and run as a program of the system's, which no person is asked about: the
trusted question that `bundle run` asks before a command runs with a person's
access is asked only at a console, which a task of the web console does not
have and a machine with no screen never shows. An administrator installed the
bundle for everyone and asked for the restore, as Spark runs a service of such
a bundle with no question. A subscription of the copy would connect to another
server, so none is made, and a restore over a database with subscriptions does
not bring them back.

A copy holds what the database's owner wrote, and putting it back runs some of
it: a check or a default that calls a function of the owner's runs as each row
goes in, and such a function can step out of its role with `RESET ROLE` into the
rights of the session. So `pg_restore` does not sign in as the superuser. It
signs in as a role called `root`, which the server's peer rule, `local all all
peer`, lets in by the socket as the account root. `restore` makes it for the
restore, `LOGIN NOINHERIT`, a member of the database's owner with `SET` and
without `INHERIT`, with the right to connect to the new database alone, and no
superuser. `--role` takes the owner's rights at once, so what the copy does is
done as the owner, and a way out of the owner's role lands in a role that can do
nothing. The role is dropped when the restore ends, whether it worked or not. A
role called `root` that is there already stops the restore before it makes
anything, and so does a database that a superuser owns. A copy that needs a
superuser, as an extension PostgreSQL does not trust its owner with does, fails
with DB-110, which says how to put it back by hand.

*Alternatives.* Over the database as the default would change a database that
programs use before a person has looked at the copy. As a new database is the
default, nothing is written over, and `--over`, the second choice, keeps the old
database under another name, so a restore never loses data. A copy on a
schedule of its own, apart from backups, would be a second clock beside the
backups'; it is a deferred row.

### A website's size and visits are counted from a log that holds no address

The nginx bundle's `nginx.conf` defines `log_format site '$msec $status
$body_bytes_sent';`: the time, the status and the bytes of the answer's body,
as `1763175600.123 200 5120`, with no address, no path and no agent. The file
of each site that `website add` writes logs each request to the system's log as
before, and also, in that format, to `logs/sites/NAME.log` in nginx's data
folder. A `nginx.conf` that was changed by hand needs the format's line too;
`website check` says so, with the line to add (WEB-102).

The service file of nginx has `logs = { files = ["logs/*.log",
"logs/sites/*.log"], keep = 2, reopen = "SIGUSR1" }`, so logturn turns the sites'
files over with the others, and a request is kept for about two days. A
service has one `logs` entry, with one number to keep, so the files of nginx's
own that `logs/*.log` names keep 2 turned copies as well as the sites', where
the usual number is 14.

`website count [--now] [--json]` is run by the system timer `website-count` ten
minutes after midnight UTC, up to five minutes later, and a machine that was off
then runs it soon after it starts. For each site it reads the live file and the
two turned copies, since logturn turns a file when a day has passed since it
last did and not at midnight, and counts the lines of yesterday by their own
times, in UTC: the visits, the requests answered with a status from 200 to 399;
the errors, 400 to 599; and the bytes sent. A line that is not a request of the
format is passed over. The files are nginx's, so they are opened beneath the
folder without following a link, and each is read as far as 2 GiB or 60 seconds
go, after which its day is marked `complete: false`. `--now` counts today so far
from the live file. The numbers go to `/system/state/websites/counts.json`, which
anyone reads, 400 days for a site, and for 30 days after the site is gone.

The same run measures each site's folder with the walk that measures an app's
data (ADR 0565): in a process of its own, with idle disk priority and nice 19,
and with its limits of 10 minutes and 5 million entries a site. `website list
--json` gives each site `size`, the bytes, the files, whether the walk was
complete and when, and `visits`, yesterday's, those of the last 7 and 30 days,
and the newest 30 days as `[day, visits, errors, bytes]`; `website list`
shows the size.

*Alternatives.* The system's log has the full lines of the `combined` format,
with the address and the agent, and logd keeps them for 90 days; counting from
it would read, every day, what a count has no need of. A file of each site, in a
format with no address, holds what a count needs and nothing more, and the
system's log keeps its lines as it did.

## Consequences

- A database that is opened to 192.168.1.0/24 can be reached on port 5432 from
  there only; the firewall drops the connections of every other address without
  an answer, before the TLS handshake. `firewall list` shows the rules, and
  `database remove` shuts the port again with the last network.
- A rule of an administrator's own that comes before the block can still open
  the port to everyone; `database list` and the Databases page say so.
- `database` has two more refusals before it changes anything, DB-107 and
  DB-108, and the console's journal compares `/system/settings/firewall` for
  `databases.allow` and `databases.deny`.
- A first order that fails is made again, by itself, for 14 days, and an
  administrator is told when it has not worked for a day. A name that will
  never work is asked about every hour for a day and every 6 hours after, until
  `cert forget` or the 14 days end it.
- A backup holds a copy of each database as well as the cluster's files, and
  the copies are in every snapshot of `/shared` until the snapshots are pruned,
  so a row deleted today is still in the copies of earlier days. The copy of a
  large database takes time before the snapshot is made, up to the hook's hour.
- `database restore` runs a program of the bundle, as root, which the other
  commands of database(8) do not, and asks no person first: the bundle's
  signature and an administrator's install for everyone are what it trusts,
  as for a service. It puts back only what a copy
  holds: subscriptions are not made, and a copy that needs a superuser does not
  go in.
- The web console's restore offers the copies that `database copies` lists and
  no other file; a person at a terminal may name any copy `pg_dump -Fc` wrote.
- Each site's requests are kept for two days in a file that holds no address,
  and the counts for 400 days, which dukovo-privacy(7) says. A machine that was
  off for more than a day counts the last day only.
- Deferred, each in `docs/deferred.md`: the counts of a machine that was off for
  more than a day; the retry of a wanted certificate tried against an authority
  that grants it, the lab's or a public one; a copy on a schedule of its own;
  subscriptions brought back by a restore; the check of a backup opening a
  database's copy; and a service's `logs` with an entry for each file pattern,
  so that nginx's own files keep their 14 turned copies. The rows "A first
  certificate order that failed, tried again by itself" and "One database
  copied and restored on its own" close, the second without its check of a
  backup, which stays as a row of its own.
- The boot tests `admin-websites` and `admin-certificates` cover the firewall's
  block, the copies and the restore, the counts and the retry, and the web
  console's lab tests the rows and dialogs that show them.
