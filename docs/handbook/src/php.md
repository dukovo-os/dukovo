<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# PHP

PHP comes from the catalog as a bundle: the `php` command, the language's
command-line program, and `php-fpm`, which runs a website's pages, with the
extensions built in, a `php.ini` and its test suite. Its service,
PHP-FPM, makes it a bundle that installs only for everyone, so an
administrator installs it:

```
$ elevate bundle install --for-everyone net.php.php
```

The first time you run `php`, Dukovo asks whether it may run with your
access, as for any command of the catalog's that does; answer `A` to
allow it always.

```
$ php -v
PHP 8.5.12 (cli) ...
$ php -r 'echo 6 * 7, "\n";'
42
$ php script.php
```

`PHP_OS` is `Dukovo`, the name of the system, and `PHP_OS_FAMILY` is
`Unknown`: a program that checks for `Linux` to choose what to do takes
another path on Dukovo. It runs as it does on Linux where the system has
what it asks for (see below).

The bundle also has PHP-FPM, which runs the pages of a website for a web
server such as nginx; see [Websites with PHP](#websites-with-php).

## Websites with PHP

PHP-FPM is a service of the bundle: it runs the pages of a website as the
account `php`, for nginx, which passes each `.php` URL to it on port 9000 of
the computer itself. Install both bundles for everyone and turn both
services on:

```
$ elevate bundle install --for-everyone org.nginx.nginx
$ elevate bundle install --for-everyone net.php.php
$ elevate service nginx enable
$ elevate service php-fpm enable
```

nginx's settings come with PHP turned off: its `.php` location answers 404,
so that no page's code is ever sent as text. To turn PHP on, edit
`/shared/services/org.nginx.nginx/nginx/conf/nginx.conf`: put a `#` before
`return 404;`, and remove the `#` signs before the three lines after it.
The location is then

```
location ~ \.php$ {
    #return 404;
    try_files $uri =404;
    fastcgi_pass 127.0.0.1:9000;
    include fastcgi.conf;
}
```

`try_files` keeps nginx from passing a URL whose file is not there to PHP.
The same lines are in the commented HTTPS server. Check the settings and
have nginx read them again:

```
$ elevate nginx -t
$ elevate service nginx reload
```

A site is a folder of `/shared/www`, as for any nginx site (see [The web
server: nginx](services.md#the-web-server-nginx)). `index.php` is the front
page of a folder that has no `index.html`:

```
$ elevate cp -R mysite/. /shared/www/
$ curl http://localhost/
```

The pages run as the account `php`, which, like nginx, reads `/shared/www`
and cannot change it: only administrators do. A page that stores what a
visitor uploads, or a file of its own, needs a folder of the account's:

```
$ elevate chown -R php /shared/www/mysite/uploads
```

PHP-FPM keeps the sessions of the pages, and the files visitors upload
until a page moves them, in its own data folder.

### Its settings

PHP-FPM's settings are in its data folder,
`/shared/services/net.php.php/php/conf`: `php-fpm.conf`, with the
service's settings and those of its one pool, `www`, and `php.ini`, which
PHP-FPM reads instead of the bundle's own in `etc`, as the pages it runs
read it. Both are copies of the bundle's files, which an update leaves as
you changed them, writing its own version beside them with `.default` added
to the name. Edit them through elevate, check them, have PHP-FPM read them
again, and see what it said:

```
$ elevate nano /shared/services/net.php.php/php/conf/php-fpm.conf
$ elevate php-fpm -t
$ elevate service php-fpm reload
$ elevate log php-fpm | tail -n 3
```

The first time you run `elevate php-fpm -t`, Dukovo asks in your terminal
pane, with a warning, whether PHP-FPM may run with root's access, as for any
command of the catalog's that runs with its user's; answer `A` to allow it
always.

A reload loses no request: PHP-FPM lets each worker finish the page it is
running, then starts again over the new settings, as the same process.
Run `-t` first. Unlike nginx, PHP-FPM that cannot read its settings as it
starts again stops, with the wrong line in its log, and Spark starts it
again, every 30 seconds after the first try, for as long as the line is
wrong (`log php-fpm` shows it, and nginx answers 502 for the pages in the
meantime); fix the line, and the next start takes it.

PHP-FPM's documentation, at
<https://www.php.net/manual/en/install.fpm.configuration.php>, describes
every setting. What differs on Dukovo:

| On a Linux distribution | On Dukovo |
| --- | --- |
| `/etc/php/8.5/fpm/php-fpm.conf` and `pool.d/www.conf` | `/shared/services/net.php.php/php/conf/php-fpm.conf`, the pool in it |
| `/etc/php/8.5/fpm/php.ini` | `php.ini` beside it |
| `/var/log/php8.5-fpm.log` | The system's log: `elevate log php-fpm` |
| `systemctl reload php8.5-fpm` | `elevate service php-fpm reload` |
| `php-fpm8.5 -t` | `elevate php-fpm -t` |
| Runs as `www-data` | Runs as the account `php` |
| A Unix socket, `/run/php/php8.5-fpm.sock` | TCP, `127.0.0.1:9000` |

Errors the pages raise are in the log too, with what they print to their
standard error output.

### Sizing

PHP-FPM starts two workers, adds them as pages come, up to `pm.max_children`
(8), and ends each after 500 pages. A worker may use up to `memory_limit` of
`php.ini`, 128M, so the service may use 1 GiB, and the service's limit is 1
GiB. When you raise `pm.max_children` or `memory_limit`, raise the limit with
an override file of the service, and restart it:

```
$ elevate nano /system/settings/spark/services/settings.d/php-fpm.toml
```

```
[limits]
memory = "4G"
```

### Limits

Any program on the computer can connect to port 9000 and run a page as the
account `php`, as with any FastCGI server on TCP: a Unix socket only nginx's
account could reach will replace it when bundles can have sockets between
each other. PHP-FPM's slow log (`request_slowlog_timeout`) is not offered,
as it reads a worker's stack with `ptrace`, which Dukovo does not have, and
every pool runs as `php`: a bundle's service never runs as root, and
PHP-FPM must to give a pool another account. Like nginx, PHP-FPM waits for
connections with `poll`, where on Linux it uses epoll.

## Its php.ini

The bundle brings a `php.ini`: PHP's `php.ini-production`, naming the
system's root certificates for `openssl.cafile` and `curl.cainfo`
(dukovo-certificates(7)). PHP finds it beside its program, in the
bundle's `etc` folder, `/applications/net.php.php/etc/php.ini`. `php -i`
says which it read:

```
$ php -i | grep 'Loaded Configuration'
Loaded Configuration File => /applications/net.php.php/etc/php.ini
```

The bundle's folder is sealed, so to change a setting for yourself, copy
the file, change the copy and tell PHP where it is with `PHPRC`, the
folder it is in, or with `-c` for one run:

```
$ mkdir ~/php && cp /applications/net.php.php/etc/php.ini ~/php/
$ export PHPRC=~/php
```

An administrator does the same with a folder of the computer's, and
`PHPRC` in the environment of whatever runs PHP. For a single setting,
`-d` is quickest: `php -d memory_limit=1G script.php`.

`date.timezone` is left unset, so PHP uses UTC. Set it in your copy, as
`date.timezone = "Europe/Paris"`, or for a script, with
`date_default_timezone_set()`.

## What it has and what it lacks

These extensions are built in, so there is nothing to load: Core, bcmath,
bz2, calendar, ctype, curl, date, dom, exif, fileinfo, filter, ftp, hash,
json, lexbor, libxml, mbstring (without its regular expressions, `mb_ereg`),
mysqli, mysqlnd, openssl, pcntl, pcre, PDO, pdo_mysql, Phar, posix,
random, Reflection, session, shmop, SimpleXML, sockets, SPL, standard,
sysvsem, sysvshm, tokenizer, uri, xml, xmlreader, XMLWriter and zlib,
and Zend OPcache. `php -m` lists them. TLS in the `openssl` extension
and in `curl` checks certificates against the system's root certificates.

These are missing, each as Dukovo does not have what it needs yet: `gd`
(libgd), `intl` (ICU), `zip` (libzip), `sodium` (libsodium), `gmp`
(GMP), `iconv` (the C library has no `iconv`), `sqlite3` and
`pdo_sqlite` (SQLite), `pgsql` and `pdo_pgsql` (PostgreSQL's library),
and `sysvmsg` (System V message queues). PCRE and opcache run without
their JIT compilers, which map memory writable and executable at once, as
Dukovo never does. Without `dl()`, an extension from PECL cannot be
loaded: Dukovo links every program as a whole, so what PHP has is built
into `php` itself.

Some functions fall short of Linux's where the system does not have
what they use yet: `dns_get_record`, `dns_check_record` (`checkdnsrr`) and
`dns_get_mx` (`getmxrr`) are not there, as the C library has no
`res_search`; `getprotobyname` and `getprotobynumber` return false; an
`AF_UNIX` socket is a stream only, so `socket_create` of a datagram one
fails, and with it passing a descriptor or credentials in a message; and
`mail()` fails, as it does anywhere without `sendmail`.

## Its test suite

The bundle carries PHP's own test suite, in its `tests` folder, the way
the tree has it, with `run-tests.php`. The bundle's folder is sealed and
`run-tests.php` writes beside the tests it runs, so run it on a copy:

```
$ cp -R /applications/net.php.php/tests /tmp/php-tests
$ chmod -R u+w /tmp/php-tests
$ cd /tmp/php-tests
$ TEST_PHP_EXECUTABLE=/applications/net.php.php/bin/php \
    php run-tests.php -j4 -q --offline Zend/tests ext/standard/tests/array
```

Some tests fail on Dukovo for reasons that are known: the lab's test of
PHP runs the whole suite and lists those it leaves out, each with its
reason, in `lab/tests/services/php-skips.txt` of Dukovo's repository.
