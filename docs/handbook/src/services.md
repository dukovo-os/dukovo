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
| Read its settings again | `elevate service NAME reload` |
| See what Spark knows of it | `service NAME status` |
| Read its log | `elevate log NAME` |

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

`/shared/www/index.html` is then the home page. Files are served as they
are; a new file needs no reload.

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

The HTTPS server in `nginx.conf` needs a certificate and its key beside
the settings, readable by the account nginx. Until certd gives
certificates, make one of its own with openssl, which browsers warn
about, for a test or a private network, or copy in one a certificate
authority gave you:

```
$ conf=/shared/services/org.nginx.nginx/nginx/conf
$ elevate openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 \
    -nodes -days 365 -subj /CN=$(hostname) \
    -keyout $conf/key.pem -out $conf/cert.pem
$ elevate chown nginx:nginx $conf/key.pem $conf/cert.pem
```

Then remove the `#` signs before the second `server` block of
`nginx.conf` and reload. It serves HTTP/1.1 and
HTTP/2 over TLS 1.2 and 1.3.

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
and key the account `valkey` reads, as for nginx's HTTPS.

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
