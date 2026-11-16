database(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

database - make, change and remove the databases of PostgreSQL, and let other machines connect to them

# SYNOPSIS

*database list* [*--json*]

*database add* _name_ [*--owner* _role_] [*--peer* _account_] [*--locale* _sorting_] [*--site* _site_] [*--json*]

*database password* _name_ *--json*

*database allow* _name_ *--from* _network_ [*--from* _network_ ...] [*--certificate* _name_] [*--progress*]

*database deny* _name_ [*--from* _network_ ...] [*--progress*]

*database remove* _name_ [*--role*] [*--force*]

*database copies* _name_ [*--json*]

*database restore* _name_ *--from* _file_ [*--as* _new_ | *--over* [*--force*]] [*--progress*] [*--json*]

# DESCRIPTION

*database* makes the databases of PostgreSQL, the bundle
*org.postgresql.postgresql*, and the roles that own them, and lets
other machines connect to them. The web console's Databases page runs
these commands, and a person at a terminal may too. A website that
keeps its pages in a database is the common case: *database add*
makes the database and a password for it, and *--site* records which
website it is for.

*Signing in.* PostgreSQL lets root in by its socket,
_/system/live/services/postgres/.s.PGSQL.5432_, as the role *postgres*,
without a password: the server's *pg\_hba.conf* maps the *administrators*
to it. *database* speaks PostgreSQL's protocol over that socket itself.
It runs no *psql* and no shell, and it knows no password of *postgres*.
Every command therefore needs root: an administrator types *elevate
database*; anyone else is told *only root runs database; try elevate
database*. If PostgreSQL is not installed, is off or is not running,
or does not let root in, the command says so and which service command
helps (DB-101).

*The password, once.* A database made by *add* is owned by a role that
signs in with a password, which *database* makes: 32 characters from 24
random bytes. PostgreSQL is told only the SCRAM-SHA-256 verifier of it,
which cannot be turned back into the password. The password is in one
place: the JSON line that *add --json* and *password --json* print. It is
in no argument, no file, no log line and no error message, and nothing
can read it again; a new one is made with *password*. A role made with
*--peer* has none: it is an account of the machine, and signs in as
itself by the socket, as the role of the same name.

*Letting other machines in.* Until it is told otherwise, PostgreSQL
listens on this machine only. *allow* lets networks connect to one
database, and only over TLS, with the password of its role. It does that
in three places, each changed so that every other line stays as it was
written:

- *pg\_hba.conf* gains one line for each network, *hostssl* with the
  method *scram-sha-256*, inside a block that *database* keeps at the
  end of the file, which begins with the comment *# database(8): who
  connects from other machines (ADR 0569)* and ends with *# database(8):
  end*. A line outside the block is the administrator's, and *database
  list* shows it as written by hand. A line inside the block that
  *database* did not write is kept too.
- PostgreSQL's settings, which *ALTER SYSTEM* keeps in
  _postgresql.auto.conf_: *listen\_addresses* is *\**, *ssl* is *on*, and
  *ssl\_cert\_file* and *ssl\_key\_file* name the certificate that certd
  keeps. Only what differs is set. *deny* resets *listen\_addresses* when
  no line of the block is left, so that the server listens on this
  machine only again; the settings for TLS stay, since the service's
  next start reads the certificate.
- The service file's override,
  _/system/settings/spark/services/settings.d/postgresql.toml_, names the
  certificate in the service's *certificates*, which Spark copies to the
  folder PostgreSQL reads at each start. The override keeps every item
  that is in it, and every comment.

Before anything takes effect, PostgreSQL is asked to check the files it
would read (*pg\_hba\_file\_rules* and *pg\_file\_settings*). A mistake it finds
is the end of the command, and everything it changed is put back (DB-104).
Then the server is told. A change of *listen\_addresses* or of the
certificate needs a restart, which *database* has Spark do with
*service postgresql restart*: the sessions that are open end, and the
command waits until the new server answers. Any other change is a reload,
which ends nothing. Last, the command connects over TLS to this machine's
address with the certificate it set, and checks the certificate against
the names it holds; if that fails, the change is taken back and the
server told again.

A change takes the lock _/system/state/databases/lock_ for as long as it
runs, so that two do not mix; the second waits up to a minute.

*Copies, and putting one back.* PostgreSQL's backup hook, the bundle's
program *postgresql-backup*, writes a copy of each database before every
backup round: *backup run* and the nightly backup, and never on its own.
A copy is the file _NAME.dump_ in _dumps_ in the folder of the account
*postgres*, made by *pg\_dump -Fc*, so it is in every snapshot of
_/shared_ and in the backup, and it is written beside the last one and
moved over it once whole. A machine that has no backup set up has no
copies. *copies* lists them: the one in use, and those in the snapshots
of _/shared_ that hold another, the newest first. *restore* puts one
back, always into a database that is made for it, and writes over
nothing; see *restore* below.

*Names.* A database and a role are named by one rule, stricter than
PostgreSQL's: a lowercase letter, then lowercase letters, digits and *\_*,
up to 63 characters, and never a name that starts with *pg\_* or is
*postgres*, *template0* or *template1*. Each name that goes into a
statement is quoted as an identifier, and each value as a string.

*Networks.* A network is an address and the length of its prefix, as
*192.168.1.0/24* or *2001:db8::/32*. The bits after the prefix are
set to zero. *allow* takes up to 20 at a time.

# COMMANDS

*list* [*--json*]
	Shows the server, the databases except *postgres* and the templates,
	the roles except those whose names start with *pg\_*, who may connect (the lines of
	*pg\_hba.conf*, as the server read them) and the mistakes the server
	found in that file. One line each:

	```
	# database list
	PostgreSQL 19.0, running, listens on this machine only
	database shop: owned by shop, 8192 bytes, 0 open session(s), for shop.example.org
	pg_hba.conf line 5: host all all 127.0.0.1/32 scram-sha-256 (by hand)
	```

	When PostgreSQL is not installed or is not running, it says so and
	exits with 0: that is an answer, and the JSON has empty lists.

*add* _name_ [*--owner* _role_] [*--peer* _account_] [*--locale* _sorting_] [*--site* _site_] [*--json*]
	Makes the database _name_ and the role that owns it, which is _role_,
	or _name_ when *--owner* is not given. The role may sign in, and has
	a new password, shown once in the JSON that *--json* prints. With
	*--peer* the role is the account _account_ and has no password;
	*--owner* may then be left out, or name the same role. _sorting_ is
	*en\_US.UTF-8*, the default, or *C.UTF-8*, which sorts by character
	code and is the same on every machine. _site_ is the name of the
	website the database is for, which *list* shows beside it. Nobody
	else may connect to the new database: *PUBLIC* has no rights on it.

	The statements are *CREATE ROLE*, *COMMENT ON ROLE* (the role is
	then known as made by *database*), *CREATE DATABASE* from *template0*
	with the encoding *UTF8*, *COMMENT ON DATABASE* for the site, and
	*REVOKE ALL ON DATABASE ... FROM PUBLIC*. If one fails, the database
	and the role are dropped again, so that a second try starts clean. A
	database or role of that name that is there already ends the command
	before anything is made (DB-102), and a *--peer* account that does
	not exist as well (DB-106).

	*add* without *--json* is a mistake of the command line, unless
	*--peer* is given: the password is shown only in JSON.

*password* _name_ *--json*
	Makes a new password for the role that owns the database _name_, and
	prints it in JSON. The old one signs nobody in again; a session that is
	open stays open. It is for a
	role that has a password: one made with *--peer* has none, and
	*password* says so.

*allow* _name_ *--from* _network_ [*--from* _network_ ...] [*--certificate* _name_] [*--progress*]
	Lets each network connect to the database _name_ from other machines,
	over TLS, as its owner, with its password. The certificate is
	_name_, as *cert list* shows it, and *this-machine* when
	*--certificate* is not given; a name that certd does not keep is
	DB-105. The change is as *Letting other machines in* says. With
	*--progress* the command tells each of its seven steps as it starts
	it, as *step 3 of 7: writing the lines of pg\_hba.conf*; the step
	that restarts PostgreSQL says so. A line that is there already is
	not written twice, and a setting that is as it should be is not set
	again, so a second *allow* with the same networks changes nothing.

*deny* _name_ [*--from* _network_ ...] [*--progress*]
	Takes the lines of the block out of *pg\_hba.conf*: those of the
	networks named, or every line of the database _name_ when *--from*
	is not given. When no line is left in the block, the block goes
	too, with the blank line before it, and PostgreSQL listens on this
	machine only again, which restarts it. *--progress* tells five
	steps. A line outside the block is never touched.

*remove* _name_ [*--role*] [*--force*]
	Drops the database _name_ with its data, and takes its lines out of
	the block of *pg\_hba.conf*. It stops with the programs that have a
	session open to it, by name, and does nothing (DB-103); *--force*
	ends the sessions and drops it. With *--role*, the role that owned
	it is dropped too, but only if *database* made it and it owns and may
	use nothing else; otherwise the command says that the role stays and
	why. It is the data that goes: nothing puts it back.

*copies* _name_ [*--json*]
	Lists the copies of the database _name_ that backups made, the
	newest first, one line each: when it was written, its size, where it
	is (*live* for the folder in use, or the name of the snapshot of
	_/shared_ that holds it) and its path, which *restore --from* takes.
	A copy that more than one snapshot holds is listed once, with the
	newest. It reads files only, so it needs no running PostgreSQL, and a
	database that is gone has copies in the snapshots that were made
	before it went. With none, it says *there is no copy of* _name_ and
	exits with 0.

	```
	# database copies shop
	2026-11-16 02:00  12M  live  /shared/services/org.postgresql.postgresql/postgres/dumps/shop.dump
	2026-11-15 02:00  12M  2026-11-15-0200  /snapshots/shared/2026-11-15-0200/services/org.postgresql.postgresql/postgres/dumps/shop.dump
	```

*restore* _name_ *--from* _file_ [*--as* _new_ | *--over* [*--force*]] [*--progress*] [*--json*]
	Puts the copy _file_, a path that *copies* gives or any file that
	*pg\_dump -Fc* wrote, into a database, which is _name_'s: the same
	owner, encoding and sorting, and nobody else may connect to it. The
	database _name_ must be there.

	With *--as* the copy goes into a new database called _new_, which
	must not be there (DB-102), and _name_ is not touched. Without
	*--as* and *--over*, _new_ is _name_, *\_restored\_* and the local
	day, as in *shop\_restored\_20261116*, and with *\_2*, *\_3* and on
	when that is taken. The new database has no site and no comment.

	With *--over* the copy goes into _name_ itself. The database as it is
	is kept, renamed to _name_, *\_before\_* and the local day and
	minute, as in *shop\_before\_20261116\_0200*, and a new database
	called _name_ is made, with the comment the old one had, which its
	owner cannot connect to while the copy is put into it, so that no
	program finds it empty. It stops with the programs that have
	a session open to _name_, by name, and does nothing (DB-103);
	*--force* ends those sessions, and lets no other come in until
	_name_ is renamed. The old database stays until it is removed with
	*database remove*. A name that would be longer than 63 characters, or
	that starts with *pg\_*, is no name for the kept database or the new
	one: the command says so before it makes anything, and *--as* names
	one.

	The copy is opened once, never through a link, and must be a regular
	file of at most 64 GiB that starts as a copy made with *pg\_dump -Fc*
	does (DB-109). What was checked is what is read: the open file is the
	standard input of the bundle's *pg\_restore*, which *restore* runs as
	root, as *--no-owner --role=* the owner *--exit-on-error*
	*--no-subscriptions -h /system/live/services/postgres -U root -d* the
	new database: a subscription of the copy, which would connect to
	another server, is not made.

	A copy holds what the owner of the database wrote, and putting it
	back runs some of it, as a check that calls a function of the
	owner's. So the restore is made with the owner's rights and no more.
	*pg\_restore* does not sign in as the superuser: it signs in as the
	role *root*, which *restore* makes for it and PostgreSQL lets in by
	the socket as the account root. That role holds no right of its own,
	may connect to the new database alone, and may become the owner and
	no one else, which *--role* does at once; a copy that steps out of
	the owner's role finds a role that can do nothing. The role is
	removed when the restore ends, whether it worked or not. A role
	called *root* that is there already stops the restore before it makes
	anything, as one left by a restore that was stopped does, and so does
	a database that a role with every right owns. The objects of the copy
	are the owner's; a copy that needs a superuser, as an extension that
	PostgreSQL does not trust its owner with does, fails. The first time,
	Dukovo asks whether the bundle may run with root's access.

	When anything fails after the new database is made, it is taken
	back: the new database is dropped, and with *--over* the old one has
	its name back, and is open again if *--force* had shut it. Then the
	failure is DB-110, with what *pg\_restore* said, the first 16 KiB of
	it, before it. A restore changes only what it made. *--progress*
	tells its steps, three for *--as* and five for *--over*, as *step 2
	of 5: keeping shop as shop\_before\_20261116\_0200*. *--json* prints
	what was done, below.

	While it runs it holds the lock of *database*, so that another change
	waits.

# JSON

*--json* prints one line of JSON, with *"format":1* first; the documents
below are shown on several lines. The members
of a document are only added to, never changed or taken away, as long as
*format* is 1.

*list --json*

```
{"format": 1,
 "server": {"installed": true, "enabled": true, "running": true,
            "version": "19.0", "listen": "*", "ssl": true,
            "certificate": "this-machine", "copies": true},
 "databases": [{"name": "shop", "owner": "shop", "encoding": "UTF8",
                "provider": "libc", "locale": "en_US.UTF-8", "bytes": 8192,
                "sessions": 1, "site": "shop.example.org",
                "made_by_database": true, "collation_mismatch": false,
                "copy": {"at_ms": 1794794400000, "bytes": 12345678}}],
 "roles": [{"name": "shop", "login": true, "password": true,
            "made_by_database": true}],
 "access": [{"line": 9, "type": "hostssl", "databases": ["shop"],
             "users": ["shop"], "address": "192.0.2.0/24",
             "method": "scram-sha-256", "by_hand": false}],
 "problems": []}
```

*server* says what is installed, turned on and running. *version* is
what PostgreSQL calls itself, *listen* is its *listen\_addresses*
(*localhost*, or *\** for every address), *ssl* whether it takes TLS and
*certificate* the name of the certificate it serves when *database* set
it, else *null*. While PostgreSQL does not run, *version* and *listen*
are *null*, and the lists are empty. *copies* is true when the backup
hook of the service, as its file and the override say it, is the one that
copies each database, and false for a hook that only writes a checkpoint,
as the bundle's before it did; it is the file that says, so it is told
while the server is off too.

A database's *provider* is *builtin*, *libc* or *icu*. *locale* may be
*null*. *sessions* counts the programs signed in to it and *bytes* is its
size. *copy* is the copy the backup hook last made of it, when it was
written in milliseconds since 1970 and how long it is in bytes, or *null*
before the first. *site* is the website it was made for, or *null*, and
*made\_by\_database* says that *database add* made it. *collation\_mismatch*
is true when the sorting data of the system is not the version the
database was made with, so that its indexes are to be rebuilt; it is
false when the provider does not say.

A role's *password* is whether it has one, never the verifier. The roles
include *postgres*. *made\_by\_database* is for a role that *add* made.

*access* has the lines of *pg\_hba.conf* as the server read them. A
line's *address* is the address with its prefix as *192.168.1.0/24*, and
*null* for a line of *local*; *databases* and *users* are lists, with
*all* as PostgreSQL writes it. *by\_hand* is false for the lines inside the
block of *database*, and true for every other. *problems* has the
mistakes the server found in the file: its *id* is *DB-104*, *words*
are PostgreSQL's, and *line* is the line of the file, or *null*.

*add --json* and *password --json*

```
{"format": 1, "database": "shop", "role": "shop",
 "password": "kT3x0Gv9uQ1mYwZb5nR2LcHd7SpJaEfA",
 "host": "127.0.0.1", "port": 5432}
```

This is the only output that holds a password. *password* is *null*, and
so is *host*, for a role made with *--peer*, which signs in by the
socket. *host* and *port* say where the password is for: this machine,
until *allow* lets others connect. *password --json* has the same
members.

*copies --json*

```
{"format": 1, "database": "shop",
 "copies": [{"at_ms": 1794794400000, "bytes": 12345678, "from": "live",
             "path": "/shared/services/org.postgresql.postgresql/postgres/dumps/shop.dump"},
            {"at_ms": 1794708000000, "bytes": 12301122,
             "from": "2026-11-15-0200",
             "path": "/snapshots/shared/2026-11-15-0200/services/org.postgresql.postgresql/postgres/dumps/shop.dump"}]}
```

*copies* is the newest first, and empty when there is none. *from* is
*live* or the name of the snapshot of _/shared_ that holds the copy.
*path* is what *restore --from* takes.

*restore --json*

```
{"format": 1, "database": "shop", "from": "/shared/services/org.postgresql.postgresql/postgres/dumps/shop.dump",
 "restored_as": "shop_restored_20261116", "kept_as": null, "took_ms": 52113}
```

*restored\_as* is the database the copy was put into: _new_, or _name_
for *--over*, when *kept\_as* is the name the old database was kept
under, else *null*. With *--progress* the lines of the steps come before
the document.

*allow*, *deny* and *remove* tell what they did in plain words, one line
each, and with *--progress*, *allow* and *deny* tell each step first as
*step N of M: words*. They have no JSON.

# EXIT STATUS

0 when the command did what it was asked, and for *list* when PostgreSQL
is not there to ask. 1 when it could not; the last line of its error
output ends with the ID in parentheses when the error has one (*ERRORS*
below). 2 for a mistake in the command line: an option that is not one, a
name that does not follow the rule, a network that is not an address with
a prefix, *add* without *--json* for a role that has a password,
*--peer* and *--owner* that name two roles, *restore* without *--from* or
with a path that does not start with */*, with *--as* and *--over* both,
or with *--force* and no *--over*.

# ERRORS

An error that has an ID ends its last line with it, such as *(DB-103)*.
The web console shows the page for it.

*DB-101*
	PostgreSQL is not installed, is off or is not running, or does not
	let root in. The words say which, and the *service* command that
	helps.

*DB-102*
	A database or a role of that name is there already. *add* made
	nothing, and *restore --as* a name that is taken made none.

*DB-103*
	The database has open sessions, from the programs the words name.
	Stop them, or use *--force* to end them: *remove* and
	*restore --over* both say so.

*DB-104*
	PostgreSQL's settings did not pass its check, so nothing was changed:
	the words are PostgreSQL's, with the file and the line. It also ends
	an *allow* whose server did not come back or did not answer over TLS
	after the change; the words then say that the change was taken back,
	and whether PostgreSQL runs again.

*DB-105*
	There is no certificate of the name given to *--certificate*. *cert
	list* shows them.

*DB-106*
	There is no account of the name given to *--peer*. *user list* shows
	them.

*DB-109*
	The file after *restore --from* is not a copy that can be put back:
	it is not there, it is a link, it is not a file, it is over 64 GiB, or
	it does not start as a copy made with *pg\_dump -Fc* does. Nothing
	was made.

*DB-110*
	*pg\_restore* did not put the copy in. What it said comes first, and
	the last line says how what was made was taken back.

Other failures have no ID: a database that is not there, a role that
has no password to sign in with from other machines, a restore that
finds a role called *root* there already or a database owned by a role
with every right, or a statement PostgreSQL refused, which is told in
its words with its error code.

# EXAMPLES

A database for a website, and its password, which is shown once:

```
# database add shop --site shop.example.org --json
{"format":1,"database":"shop","role":"shop","password":"kT3x0Gv9uQ1mYwZb5nR2LcHd7SpJaEfA","host":"127.0.0.1","port":5432}
```

A database for a program that runs as an account of the machine, with
no password:

```
# database add notes --peer notes --json
{"format":1,"database":"notes","role":"notes","password":null,"host":null,"port":5432}
```

Letting the machines of a network connect, with this machine's
certificate, and then shutting them out again:

```
# database allow shop --from 192.168.1.0/24 --progress
step 1 of 7: checking the database, the networks and the certificate
step 2 of 7: naming the certificate in PostgreSQL's service file
step 3 of 7: writing the lines of pg_hba.conf
step 4 of 7: setting what PostgreSQL listens with
step 5 of 7: having PostgreSQL check its files
step 6 of 7: restarting PostgreSQL
step 7 of 7: checking that PostgreSQL answers over TLS
192.168.1.0/24 may connect to shop over TLS, as shop, with the password
PostgreSQL restarted, and its open sessions ended
# database deny shop
```

A copy put back as a new database, beside the one there, and then the
old one, kept, in the place of the database:

```
# database copies shop
2026-11-16 02:00  12M  live  /shared/services/org.postgresql.postgresql/postgres/dumps/shop.dump
# database restore shop --from /shared/services/org.postgresql.postgresql/postgres/dumps/shop.dump
put the copy into the new database shop_restored_20261116; shop was not changed
# database restore shop --from /snapshots/shared/2026-11-15-0200/services/org.postgresql.postgresql/postgres/dumps/shop.dump --over --force
put the copy into shop; the database as it was is kept as shop_before_20261116_0200
`database remove shop_before_20261116_0200` removes it when it is not needed
```

Removing a database that a program still has open, and its role:

```
# database remove shop --role
database: shop has 1 open session, from php: stop those programs, or use --force to end them (DB-103)
# database remove shop --role --force
removed the database shop
removed the role shop
```

# FILES

_/system/live/services/postgres/.s.PGSQL.5432_
	PostgreSQL's socket, which *database* signs in on.

_/system/state/databases/lock_
	The lock a change holds.

_/shared/services/org.postgresql.postgresql/postgres/dumps_
	The copies that PostgreSQL's backup hook writes, _NAME.dump_ for each
	database, which *copies* lists with those of the snapshots in
	_/snapshots/shared_.

_/system/settings/spark/services/settings.d/postgresql.toml_
	The service's override: the certificates it serves, which *allow*
	adds to and keeps the rest of.

# SEE ALSO

*website*(8), *cert*(8), *service*(8), *service.toml*(5), *elevate*(8),
*user*(8), *backup*(8), *snap*(1), *dukovo-certificates*(7)
