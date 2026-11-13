<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Running services: web, databases and certificates

Servers such as a web server come from the catalog as bundles that run
services. This chapter describes how they run, then each of them. The
full references are bundle(1), service(8), service.toml(5),
manifest.json(5) and log(1).

## How a bundle's services run

A bundle that runs services installs only for everyone, so an
administrator installs it through elevate. Its install sheet names each
service, the account it runs as and when it starts:

```
$ elevate bundle install --for-everyone org.nginx.nginx
```

Each service runs as an account of its own, which the install makes, with
no password and no shell. Its data folder is that account's home,
`/shared/services/ID/ACCOUNT`, which only the account reads: its
settings and what it keeps are there, and an administrator reaches them
through elevate. A service reaches only what its manifest asks for and
the install sheet showed: its own folders, the ports it listens on, the
folders of `/shared` it names, and the network if it connects out.

Most services wait until an administrator turns them on. `service`
starts, stops and reloads them as it does the system's own:

| Task | Command |
| --- | --- |
| Start it now and at each boot | `elevate service NAME enable` |
| Stop it, and no longer start it at boot | `elevate service NAME disable` |
| Start, stop or restart it now, and watch it happen | `elevate service NAME start` (`stop`, `restart`) |
| Read its settings again | `elevate service NAME reload` |
| List every service and how it runs | `service list` |
| See what Spark knows of it, and what it may reach | `service NAME status` |
| Read its log, with Spark's lines about it | `elevate service NAME logs` |

What a service writes goes to the system's log, under its name, so
`log NAME` reads it and logtrim keeps it within the log's room (see
[Logs, monitoring and troubleshooting](logs.md)).

An update stops the services, copies their data folders, installs the
new version and starts them again; if one fails its health check, the
bundle goes back to the version before, with its data. A settings file
an administrator changed stays as it is, and the new version's file is
written beside it as `NAME.default`, to compare and take over by hand.

`elevate bundle remove --for-everyone ID` stops and removes the services
and keeps their accounts and data; `--purge` removes those too.

## Services that wait for each other

A service can name the services it needs, say when it is ready to be
used, and have Spark check its health. A bundle's manifest says so for
its own services, and an administrator writes it into a service file of
their own with `service_version = 2` (service.toml(5) has every key):

```toml
service_version = 2
description = "Shop"
program = "/applications/commands/shop"
requires = ["postgres"]
after = ["netd"]
give_up_after = 5

[ready]
when = "listening"
port = 8080

[health]
command = ["/system/commands/curl", "-fsS", "-o", "/dev/null", "http://localhost:8080/health"]
every = 30
```

| Key | What it does |
| --- | --- |
| `requires` | Starts those services first, and this one only once they are ready; if one stops for good, this one stops too |
| `wants` | Starts those services first, and this one once they are ready or have stopped |
| `after` | Waits for those services when they are starting too, without starting them |
| `ready.when` | `started`, `listening` (on `ready.port`), `socket` (once `ready.path` is made), `healthy` (once the check passes) or `ended` (a setup step) |
| `ready.timeout` | Seconds it has to get ready, 90 unless set; past them its start has failed |
| `health.command` | A check run as the service runs, every `health.every` seconds; `health.failures` failures in a row restart it |
| `give_up_after` | Failed starts in a row after which Spark stops trying, until `service NAME start` |

`service NAME start` starts what the service requires and wants first,
and waits until the service is ready, showing each step as Spark takes
it:

```
$ elevate service shop start
started postgres (/applications/org.postgresql.postgresql/bin/postgres) as process 212
postgres is ready (listening on 5432) 2.1 s after its start
started shop (/applications/commands/shop) as process 220
shop is ready (listening on 8080) 0.4 s after its start
```

When one it requires cannot start, it stops and says why, with the last
line the failed program wrote:

```
$ elevate service shop start
started postgres (/applications/org.postgresql.postgresql/bin/postgres) as process 212
postgres (process 212) exited with code 0x1
service: shop waits for postgres, which could not start: it exited with code 0x1 ("FATAL: data directory has wrong ownership"); Spark tries postgres again in 0.1 s
```

Ctrl-C stops the watching, not the start; `--no-wait` returns at once.
`service NAME status` says what it waits for, why it is not running,
what it needs and what started it:

```
$ service shop status
shop - Shop
   Active: activating (waiting for postgres)
    Needs: requires postgres; after netd
  Started: by a request
  Restart: on-failure, 0 restarts so far; gives up after 5 failed starts in a row
```

Stopping a service stops those that require it. When Spark gives up on
a service, or a service it requires stops, the reason is in `status`'s
`Why` line and in the log:

```
$ elevate service shop logs
Nov 13 10:40:12 web spark: stopping shop: it requires postgres, which Spark gave up on
```

Restarting a service leaves those that require it running: they
reconnect. Shutting down stops a service before those it needs.

The system's own services keep their start order by `order` in this
release; their files move to version 2 in the release after it.

## Seeing what a service does

`service list` shows every service, its state, whether it starts at
boot and since when, and names those that are not running as asked:

```
$ service list
SERVICE   STATE                    AT BOOT  SINCE     DESCRIPTION
nginx     running                  on       10:42:03  nginx web server
postgres  failed                   on       10:43:10  PostgreSQL
shop      stopped                  on       10:43:10  Shop

Not running as asked: postgres and shop. `service postgres explain` says why.
```

`service list --failed` lists those alone, and exits 1 when there are
some, for a script or a monitoring check. A service that is off as it
is meant to be is not among them: a network driver the kernel left no
cards to drive, as on most virtual machines, or a service Rescue or
essentials leave off.

`service NAME status` says what Spark knows of a service, what its
file lets it reach, which processes it runs and its last lines in the
log:

```
$ service nginx status
nginx - nginx web server
   Active: active (running) as process 57 since 10:42:03, 2 min ago; ready in 10.4 s
    Needs: after netd, resolved
  Started: by boot
    Holds: its own view, acting for org.nginx.nginx
           reads /applications/org.nginx.nginx, /shared/www
           changes /system/state/org.nginx.nginx
           listens on 80 (open), 443 (open); connects out
           memory up to 256M; CPU weight 100
Processes: 3 (57 nginx, 58 nginx, 59 nginx), 12.3M of memory, 1.2 s of CPU
  Restart: on-failure, 0 restarts so far

Nov 13 10:42:03 spark: started nginx (/applications/org.nginx.nginx/bin/nginx-service) as process 57
Nov 13 10:42:13 spark: nginx is ready (listening) 10.4 s after its start
```

`Holds` is the whole of what the service may touch beyond what every
service reads: a port marked `(open)` is one something listens on now.
A service stopped by hand says who stopped it and when (`Asked`), and
one that ended says the last line it wrote (`Last`).

### Why a service is not running

`service NAME explain` says why a service is in its state, in one
sentence, and what to do about it. It follows what the service waits for,
or what stopped it, to the service at the root, and says what each is
doing:

```
$ service shop explain
shop is stopped because it requires postgres, which was given up on: it failed to start 5 times in a row (the last: it exited with code 0x1) ("postgres: /shared/db: no such folder"). Fix what stops postgres (`service postgres logs` shows what it wrote), then `service shop start` starts both.
```

The service at the root, here postgres, is the one to look at; the
last words say what to do. `service NAME explain --json` gives the
sentence, the services it followed, the root and the fix as one line of
JSON, for a script. The command exits 0 for a service that is done or
runs well, and 3 for any other.

## Checking a change

`service edit NAME` opens a service's file in the editor, checks it when
the editor ends and puts it in place only once it reads. `service check`
checks the files you changed by other means, one service's or every
service's. A file that does not read is shown as a compiler shows a
mistake: the file, the line and the column, the line itself with carets
under the setting, and a fix where there is one:

```
$ service check postgres
/system/settings/spark/services/postgres.toml:7:1: `discription` is not a setting here; service.toml(5) lists them
  |
7 | discription = "PostgreSQL"
  | ^^^^^^^^^^^
  = fix: did you mean `description`?
```

A mistake in an override file, in `settings.d` beside it, names that
file and its line, and which file it is set over. `--brief` prints the
first line alone, for a script.

A file that reads says what putting it to use would do. For a service
that runs, that is the restart that applies the change and what the
restart touches; for one that does not, when the change applies and
whether the service would start with it:

```
$ elevate service check postgres
/system/settings/spark/services/postgres.toml: ok; postgres runs an older copy: health.every differs; `service postgres restart` applies the change: it restarts postgres (its last start took 3.1 s); shop, which requires postgres, keeps running
$ elevate service postgres restart
$ elevate service check postgres
/system/settings/spark/services/postgres.toml: ok
```

Only an administrator sees this second part, since Spark's copy of the
file, which it compares, is theirs to read; anyone else is told so. A
change to a service on a machine you reach over the network is better
tried than checked alone: the next section undoes it if it goes wrong.

## Trying a change with an undo

`service try NAME` changes a service's settings the safe way on a
machine you reach over the network. It opens the service's file in the
editor as `service edit` does, restarts the service with the change,
and asks whether to keep it:

```
$ service try sshd
/system/settings/spark/services/sshd.toml: trying the change for 60 s
stopping sshd
started sshd (/system/commands/sshd) as process 912
sshd is ready (listening on 22) 0.2 s after its start
Keep the change to sshd? (y)es keeps it, (n)o undoes it now; Spark undoes it by itself in 58 s, unless `service keep sshd` keeps it from another session: y
kept: sshd runs the change
```

Before you answer, open a second session. If it signs in, keep the
change from there with `service keep sshd`. If the change cut you
off, do nothing: Spark puts the old file back and restarts sshd as it
was once the time is up. A start that fails with the change is undone
at once, and so is a change still being tried when the machine starts
again, so even a power cut brings the old settings back. `--for 300`
gives you five minutes; `service sshd status` shows `Trying:` and the
time left, and `service undo sshd` undoes the change at once.

## The web server: nginx

nginx serves websites and passes requests on to other servers. Install
it, then turn its service on:

```
$ elevate bundle install --for-everyone org.nginx.nginx
$ elevate service nginx enable
$ curl http://localhost/
```

It answers on port 80 with its welcome page until you put a site of your
own in `/shared/www`, which every account reads and only administrators
change:

```
$ elevate cp -R mysite/. /shared/www/
```

`/shared/www/index.html` is then the home page, or `index.php` once PHP is
set up (see [Websites with PHP](php.md#websites-with-php)). Files are served
as they are; a new file needs no reload.

nginx starts after the network and the resolver, since it looks up the
names in its `proxy_pass` lines as it starts, and Spark counts it ready
once it listens, so a service of yours that requires it starts only
then:

```
$ elevate log | grep 'nginx is ready'
spark: nginx is ready (listening) 0.3 s after its start
```

### Its settings

nginx's settings are in its data folder,
`/shared/services/org.nginx.nginx/nginx/conf`. `nginx.conf` holds the
server on port 80 and, turned off, the same server over HTTPS; the other
files are nginx's own, which `nginx.conf` includes. Edit them through
elevate, check them, have nginx read them again, and see what it said:

```
$ elevate nano /shared/services/org.nginx.nginx/nginx/conf/nginx.conf
$ elevate nginx -t
$ elevate service nginx reload
$ elevate log nginx | tail -n 3
```

The first time you run `elevate nginx -t`, Dukovo asks in your terminal
pane, with a warning, whether nginx may run with root's access, as for
any command of the catalog's that runs with its user's; answer `A` to
allow it always.

A reload loses no request: nginx starts workers with the new settings
and lets the old ones finish what they are serving. Settings that do not
read leave the running nginx as it was, and its log names the line that
is wrong:

```
Nov 11 10:20:31 web nginx: 2026/11/11 10:20:31 [emerg] 41#41: unknown directive "sevrer" in /shared/services/org.nginx.nginx/nginx/conf/nginx.conf:30
```

`elevate nginx -t` names the same line before a reload does. Fix the
line and reload again.

nginx's own documentation, at <https://nginx.org/en/docs/>, describes
every setting. What differs on Dukovo:

| On a Linux distribution | On Dukovo |
| --- | --- |
| `/etc/nginx/nginx.conf` | `/shared/services/org.nginx.nginx/nginx/conf/nginx.conf` |
| `/var/www/html` | `/shared/www` |
| `/var/log/nginx/access.log` and `error.log` | The system's log: `elevate log nginx` |
| `systemctl reload nginx` | `elevate service nginx reload` |
| `sudo nginx -t` | `elevate nginx -t` |
| Runs as `www-data` or `nginx` | Runs as the account `nginx` |
| `sites-available` and `sites-enabled` | Servers in `nginx.conf`, or files of your own it includes |

nginx reads only what its service may: its bundle, its data folder,
`/shared/www` and the system's own folders. A site kept elsewhere goes in
`/shared/www`, or in a folder of its data folder.

### HTTPS

The HTTPS server in `nginx.conf` needs a certificate and its key. certd
keeps them and Spark gives nginx a copy: name the machine's own
certificate, `this-machine`, or one you added with `cert add`, in
nginx's service file, remove the `#` signs before the second `server`
block of `nginx.conf`, and restart nginx:

```
$ elevate service edit nginx          # add: certificates = ["this-machine"]
$ elevate nano /shared/services/org.nginx.nginx/nginx/conf/nginx.conf
$ elevate service nginx restart
```

It serves HTTP/1.1 and HTTP/2 over TLS 1.2 and 1.3. A renewed
certificate reaches it without a restart or a lost request. Browsers
warn of `this-machine` until told to trust its fingerprint; the
[Certificates](certificates.md) chapter has that, adding a certificate
from an authority, and what to do when something is wrong.

### A reverse proxy

nginx may connect to other servers, so it passes requests on to an
application that listens on a port of its own:

```
location /app/ {
    proxy_pass http://127.0.0.1:8080/;
    proxy_set_header Host $host;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
}
```

### Passwords for a site

`auth_basic` asks for a password, from a file of names and hashes in the
settings' folder. nginx checks the hashes it knows itself; make them with
`openssl passwd -apr1`:

```
$ printf 'alice:%s\n' "$(openssl passwd -apr1)" | elevate tee -a $conf/passwords
$ elevate chown nginx:nginx $conf/passwords
```

and in a `location`:

```
auth_basic "Private";
auth_basic_user_file passwords;
```

### Its log

nginx writes a line for each request, and its warnings and errors, to the
system's log:

```
$ elevate log nginx
Nov 11 10:12:04 web nginx: 2026/11/11 10:12:04 [notice] 41#41: start worker processes
Nov 11 10:12:09 web nginx: 192.0.2.10 - - [11/Nov/2026:10:12:09 +0000] "GET / HTTP/1.1" 200 896 "-" "curl/8.23.0"
```

A busy site can keep its requests out of the system's log with
`access_log off;`, or write them to a file of its data folder's `logs`
folder, by its full path:

```
access_log /shared/services/org.nginx.nginx/nginx/logs/access.log;
```

A path that does not start with `/` is in nginx's bundle, which it
cannot write to. Nothing turns such a file over yet, so watch its size.

### Limits

This first version of nginx for Dukovo waits for connections with `poll`
and copies files through its buffers, where nginx on Linux uses epoll
and sendfile, so a very busy site serves fewer requests a second on the
same machine. It has no HTTP/3. Each worker may have
4096 connections and files open, and the service may use 512 MiB.

## The key-value store: Valkey

Valkey keeps keys and their values in memory, for a cache, sessions,
queues or counters, and saves them to disk. It is the open-source
continuation of Redis, and speaks its protocol, so Redis clients and
libraries work with it. Install it, then turn its service on:

```
$ elevate bundle install --for-everyone io.valkey.valkey
$ elevate service valkey enable
$ valkey-cli ping
PONG
```

The first time you run `valkey-cli`, Dukovo asks whether it may run with
your access, as for any command of the catalog's that does; answer `A`
to allow it always. Valkey answers on port 6379 of the machine itself
only, for programs on the same machine.

```
$ valkey-cli set greeting 'hello'
OK
$ valkey-cli get greeting
"hello"
```

### Its settings

Valkey's settings are in
`/shared/services/io.valkey.valkey/valkey/conf/valkey.conf`, which
holds the settings that differ from Valkey's own, each with a comment;
every setting, with Valkey's words on each, is in
`/applications/io.valkey.valkey/doc/valkey.conf`. Valkey does not read
its settings again on a reload: edit them, then restart it, which saves
its data as it stops:

```
$ elevate nano /shared/services/io.valkey.valkey/valkey/conf/valkey.conf
$ elevate service valkey restart
$ elevate log valkey | tail -n 3
```

`valkey-cli config set` changes most settings while Valkey runs, and
`valkey-cli config rewrite` writes them into `valkey.conf`.

To serve other machines, give the default user a password, or set up
users of your own, then name the addresses to listen on:

```
bind * -::*
requirepass a-long-password
```

Valkey's protected mode refuses clients from elsewhere while the default
user has no password. Clients then give it with `AUTH`, or `valkey-cli
-a`. For TLS, `valkey.conf` has the lines to turn on, with a certificate
and key the account `valkey` reads, such as ones made with `openssl req`
and given to that account with `chown`. Valkey does not read its
certificates again on SIGHUP, which Spark sends when a certificate of
certd's changes, so it names none of them.

What differs from a Linux distribution:

| On a Linux distribution | On Dukovo |
| --- | --- |
| `/etc/valkey/valkey.conf` | `/shared/services/io.valkey.valkey/valkey/conf/valkey.conf` |
| `/var/lib/valkey` | `/shared/services/io.valkey.valkey/valkey/data` |
| `/var/log/valkey/valkey-server.log` | The system's log: `elevate log valkey` |
| `systemctl restart valkey-server` | `elevate service valkey restart` |
| `/run/valkey/valkey-server.sock` | None yet: clients connect over TCP to `127.0.0.1:6379` |
| Runs as `valkey` | Runs as the account `valkey` |

### Its data

Valkey saves a snapshot of all its keys, `dump.rdb`, in its data
folder: an hour after a key changed, five minutes after 100 did, a
minute after 10,000 did, and when it stops. A crash loses what changed
since the last snapshot. For data you keep, turn on the append-only
file, which logs every write and loses a second of writes at most:

```
$ valkey-cli config set appendonly yes
$ valkey-cli config rewrite
```

To keep a copy, have it save, then copy the snapshot out of its data
folder into a file of yours:

```
$ valkey-cli save
$ elevate cat /shared/services/io.valkey.valkey/valkey/data/dump.rdb > valkey-backup.rdb
```

`valkey-check-rdb` and `valkey-check-aof` check a snapshot and the
append-only files, read from a copy you can read, such as the one above:
`valkey-check-rdb valkey-backup.rdb`.
An update of the bundle copies the data folder before it starts the new
version, as for every service.

### Limits

This first version of Valkey for Dukovo waits for its clients with
`poll`, where Valkey on Linux uses epoll, and uses the C library's
memory allocator rather than jemalloc, so its active defragmentation is
off and `info memory` reports less about fragmentation. Its Lua scripts
work; modules from files, which `MODULE LOAD` would load, do not, as
its programs are static. It has no Unix socket yet. Without a
`maxmemory` it grows until the machine has no memory left: for a cache,
set one and `maxmemory-policy allkeys-lru`.
