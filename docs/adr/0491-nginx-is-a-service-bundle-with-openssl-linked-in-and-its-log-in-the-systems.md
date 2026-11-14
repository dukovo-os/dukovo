<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0491. nginx is a service bundle, with OpenSSL linked in and its log in the system's

- Status: Accepted; `nginx -t` runs through elevate since [0493](0493-a-command-elevated-in-a-pane-is-asked-about-in-that-pane.md)
- Date: 2026-11-11

## Context

nginx is the first of the 1.0 server services to be ported, and the
first service bundle of the catalog: until now a bundle's service (ADRs
0385, 0386) ran only test programs. nginx needs a TLS library, and it
supports OpenSSL and its forks, not rustls, which is the only TLS library
the image's programs use (ADR 0192); the image carries OpenSSL's command
alone (ADR 0342). Its build for Linux uses epoll, sendfile, eventfd and
thread pools, which Radix does not have, and its configure script runs
the programs it compiles, which a build for Radix on Linux cannot.

A bundle's service runs as an account of its own, in a view of what its
manifest asks for, and its standard output and error go to the system's
log. Linux distributions start nginx with its settings in `/etc/nginx`,
its logs in `/var/log/nginx` turned over by logrotate, and its sites in
`/var/www`.

## Decision

- **A port of the catalog.** `ports/nginx` builds nginx's stable series
  with PCRE2 10.49, built into it from its sources without JIT, zlib,
  and OpenSSL 3.5. It is built as for a generic POSIX system
  (`--crossbuild=Radix::x86_64`), without `select`, so it waits with
  `poll` and copies files through its buffers. A patch lets configure
  build for another system: a feature whose test links counts as there,
  a test for a bug or a value finds none, and the compiler checks the
  sizes and the byte order. Two more fit its workers' channels to
  Radix: the master's end of a channel goes without signal-driven I/O
  (`FIOASYNC`), which Radix does not have and the master never needs, as
  it does not read that end; and a worker is not given the other
  workers' channels, as Radix passes no socket in a message (no cycle
  of sockets can then keep itself open), which nginx's own modules never
  use. It has the HTTP/2, SSL, real IP, sub, gunzip, gzip static, auth
  request, secure link, slice and stub status modules, and the stream
  module with SSL, real IP and SSL preread.
- **OpenSSL's libraries in the bundles that need them.** The openssl
  port keeps its static libraries and headers in its own folder for
  the catalog's bundles, out of the sysroot, so the image's programs
  still use rustls alone. A bundle that links them, and any other port,
  names them in its recipe's `links`, and ob adds those ports, and the
  compiler's runtime, to the bundle's SBOM and licenses. A fix to
  OpenSSL reaches nginx as a new version of its bundle, which the
  catalog's advisories name (ADR 0389). PostgreSQL and PHP link it the
  same way.
- **ob packs service bundles.** A recipe's `[bundle]` names its
  programs besides its commands, the ports it links, and its services,
  permissions and data folder, which ob writes into the manifest as
  they are; manifest.json(5) checks them as it reads the manifest.
- **The service.** nginx's service runs as the account nginx once an
  administrator enables it, is started again if it fails, and may use
  512 MiB. It listens on ports 80 and 443, may connect out as a reverse
  proxy, and reads `/shared/www`. bundled makes a folder of `/shared`
  that a bundle's services read and none may change if it is not
  there, the root's and everyone's to read, as Spark refuses to start a
  service a place it reads is missing from. `nginx-service` keeps nginx
  in the foreground (`daemon off`), as Spark's services are.
- **Its settings are in its data folder.** nginx is built to read
  `/shared/services/org.nginx.nginx/nginx/conf/nginx.conf`, write its
  process number in its live folder and keep its temporary files in its
  data folder, and the log files of its own that its settings may name
  in its `logs` folder. The first start copies the bundle's settings
  into its `conf` folder, as files of the account's, which it may
  change. After an update, a file the administrator has not changed
  takes the bundle's new version, and one they changed stays, with the
  bundle's version beside it as `NAME.default`, as bundled does for
  service files (ADR 0386). An administrator applies them with `elevate service nginx
  reload`, whose SIGHUP has nginx start new workers while the old ones
  finish their requests; settings that do not read leave it as it was,
  and its log names the wrong line. `nginx -t` does not run through
  elevate: the bundle's command runs with the access of whoever runs it,
  which the person allows at a console's trusted prompt the first time,
  and elevate gives the command a terminal of its own, which is no
  console's.
- **Its log is the system's.** nginx writes its errors to its standard
  error output (`error_log stderr`), and, with a patch, its access log
  too (`access_log stderr`), so `log nginx` reads both and logtrim(8)
  keeps them within the log's room; nothing has to turn log files over.
- **Sites in `/shared/www`.** The default server answers on port 80
  for any name, from `/shared/www`, and shows nginx's welcome page from
  the bundle until the folder has an index file, `index.html` or, once
  PHP is set up (ADR 0520), `index.php`. Until then its settings answer
  every `.php` URL with 404, so a PHP page's code is never sent as text.
  Its HTTPS server is in the settings, turned off, with the commands
  that make a certificate with openssl until certd gives them. Each
  worker raises its open files to 4096, the hard limit, as Spark does
  not set a service's `open_files` yet.

## Consequences

- An administrator runs a web server with `elevate bundle install
  --for-everyone org.nginx.nginx` and `elevate service nginx enable`.
  The bundle is about 3.5 MB.
- The boot test `nginx` installs the bundle from its `.dkv` file,
  enables it, and checks its welcome page, a page in `/shared/www`, the
  404 of a PHP page, a reload with settings that do not read, which
  leaves it serving and logs the wrong line, a restart, HTTPS with a
  certificate of its own, three reloads during 300 requests that lose
  none, its log, the account that owns its files, and requests logged to
  a file of its data folder's `logs` folder instead. The lab test `services/nginx`
  serves the lab PC over the machine's own card, with five reloads
  during 20 seconds of requests 16 at a time; `./ob lab` packs the
  catalog's bundles for it.
- nginx's `auth_basic` takes its own password schemes (`{PLAIN}`,
  `{SHA}`, `{SSHA}` and `$apr1$`, which `openssl passwd -apr1` makes),
  but not `crypt(3)`'s, which the C library lacks.
- **Not yet** (docs/deferred.md):
  - A certificate from certd for nginx's sites and its lab test.
  - The catalog's commands through elevate, `nginx -t` among them.
  - The services a site needs started first, and readiness, once
    manifests have dependency lines.
  - Spark setting `open_files`.
  - Log files of a service's own, turned over.
  - nginx on an event queue, with sendfile and thread pools.
  - HTTP/3.
  - `crypt(3)`.
