<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0520. PHP-FPM runs PHP pages as a service of the PHP bundle

- Status: Accepted
- Date: 2026-11-13

## Context

The PHP bundle (ADR 0499) has the `php` command. The web applications the
catalog will carry are PHP programs that a web server runs page by page,
and the catalog's web server, nginx (ADR 0491), does not run PHP itself:
it passes a page to a FastCGI server and sends back what it answers. PHP's
own FastCGI server is PHP-FPM, its process manager: a master that keeps a
pool of workers, each running one page at a time, and starts, ends and
replaces them as the load changes.

A service bundle runs as an account of its own, in a data folder of its
own, started by Spark once an administrator enables it (ADRs 0385 and
0386). The PHP bundle already has everything PHP-FPM is made of: the same
build of PHP, its extensions and its `php.ini`; a second bundle for it
would carry them twice.

PHP-FPM asks the system for things Radix does not have, and its `configure`
takes them when it finds a header, as it does on Linux:

- **epoll.** The kernel has no epoll. PHP-FPM takes it when
  `<sys/epoll.h>` compiles, and the C library has no such header, so it
  does not today; with the header, PHP-FPM would take epoll and not start.
- **`ptrace(2)` and `/proc/PID/mem`**, which its slow log reads a worker's
  stack with.
- **TCP_INFO**, which PHP-FPM reads on each pass of its process manager to
  count the connections waiting for a pool's listening socket.
- **A signal.** PHP-FPM reloads when it gets SIGUSR2; `service reload`
  sends SIGHUP (service(8)), which PHP-FPM treats as an end, as it does a
  terminal's hangup.
- **A `php.ini` of its own.** PHP-FPM, like the command, looks for its
  `php.ini` in `PHPRC` and next to its program; that is the bundle's, which
  is sealed, and a service must not change.

## Decision

- **PHP-FPM is in the PHP bundle,** built with `--enable-fpm` into the same
  tree as the command: a static program, `bin/php-fpm`, with the
  extensions of ADR 0499 built in. `php-fpm` is a command of the bundle (so
  `elevate php-fpm -t` checks its settings, as `elevate nginx -t` does),
  and its manual page, `php-fpm(8)`, is in the bundle with the paths of
  Dukovo in it.
- **`configure` is told what the system has.** `php_cv_have_epoll=no`
  keeps PHP-FPM waiting with `poll(2)`, as nginx and Valkey do, should the
  C library gain the header, and the
  settings say `events.mechanism = poll`, so that its log names what it
  uses. `php_cv_func_ptrace=no` and `php_cv_file_proc_mem=` (no) leave out
  the slow log, and `php_cv_have_TCP_INFO=no` leaves out the listening
  queue's length, which its status page would show as 0.
- **A service, `php-fpm`,** as the account `php`, with `restart =
  "on-failure"`, `start = "when-enabled"` and a memory limit of 1 GiB. Its
  program, `php-fpm-service`, makes the data folder's folders, copies the
  settings the bundle brings as nginx's service does (a file the
  administrator changed stays, and the bundle's new version is written
  beside it as `NAME.default`), and runs PHP-FPM in the foreground
  (`--nodaemonize`) with its log, `error_log = /dev/stderr`, in the
  system's: `log php-fpm`. It names no `ready` or `after`: Spark counts it
  ready when its process runs (ADR 0470), and nginx, which does not wait for
  it, answers 502 until PHP-FPM listens, a moment after.
- **TCP on 127.0.0.1:9000.** A Unix socket would be reachable by nginx's
  account alone, but one bundle's services cannot reach another's folder
  (ADR 0385), so until sockets between bundles exist they talk over TCP on
  the computer itself, and the bundle asks for `network.listen` on port
  9000. `listen.allowed_clients = 127.0.0.1` leaves out other addresses,
  which the address already does.
- **`/shared/www`, read-write, narrowed by the files.** A permission to a
  shared folder is `read` or `read-write` for the whole folder (ADR 0385).
  Pages must write where a site lets them, so the bundle asks for
  `read-write`; what the account may change is then what the owners and
  modes of the files say, as for a person. What an administrator puts in
  `/shared/www` belongs to the administrator, and the pages, which run as
  `php`, cannot change it; a folder an administrator gives to `php` with
  `elevate chown -R php /shared/www/SITE/uploads` takes what pages upload.
- **The settings are in its data folder,** `php-fpm.conf` and a `php.ini`
  beside it, as nginx's are. PHP-FPM's default configuration path is its
  `sysconfdir`, which the port sets to the data folder's `conf`
  (`--sysconfdir`), so that `php-fpm -t`, run by an administrator with no
  options, reads the file the service reads. Only PHP-FPM's reading of the
  settings and the `PHP_SYSCONFDIR` constant use it; the command's search
  for `php.ini` does not.
- **Two patches.** `0008-fpm-reload-on-sighup.patch` has SIGHUP reload
  PHP-FPM, as SIGUSR2 does: the master stops its workers when they have
  finished their pages, and runs itself again over the new settings, as the
  same process, so Spark's record of it and the listening socket, which
  the new master takes up from its environment, stay. The patch does not
  depend on Dukovo, and its header says why. `0009-fpm-php-ini-beside-its-settings.patch`
  has PHP-FPM, under Dukovo, take the `php.ini` of its settings' folder:
  without `-c` it behaves as if `-c` named that folder, which makes it the
  only place PHP looks, as with `-c` itself, so that nothing in the
  environment or in the sealed bundle changes what the pages run with;
  `-c` and `-n` still do what they do.
- **One pool, `www`, as `php`.** `user = php` and `group = php` are in the
  settings, though the service runs as `php` already and PHP-FPM then says
  it ignores them: `elevate php-fpm -t` runs as root, and PHP-FPM as root
  refuses a pool that names no account. The pool is `pm = dynamic`, from 2
  to 8 workers, each ending after 500 pages, with a memory limit of the
  service's 1 GiB for them all, and `catch_workers_output` so that what a
  page writes to its standard error output is in the log. Sessions and
  uploads go to the data folder's `sessions` and `temp` (`php_admin_value`),
  which only the account reads.
- **nginx's settings have a PHP location that is off.** The shipped
  `location ~ \.php$` returns 404, and the three lines that pass the page
  to PHP-FPM (`try_files $uri =404`, `fastcgi_pass 127.0.0.1:9000`,
  `include fastcgi.conf`) are comments, which the administrator turns on
  by commenting out the `return` and uncommenting them. The location is
  there, and not left out, because without it a `.php` file would be served
  by the location that serves any file: its code, with whatever passwords
  it holds, as text to whoever asks. `try_files $uri =404` comes first of
  the three so that nginx passes only a file that exists, which also keeps
  a URL such as `/uploads/photo.jpg/x.php` from running `photo.jpg`. The
  server's `index` now names `index.php` too, and the front page's
  location, `location = /`, only turns the 403 that the index module
  answers when `/shared/www` has no index file into the welcome page
  (`error_page 403 = @welcome`, status 200): a `try_files` that named
  `/index.php` would send it as text, where the index module hands it to
  the PHP location by an internal redirect.

## Consequences

- An administrator runs PHP pages with `elevate bundle install
  --for-everyone` of nginx and PHP, `elevate service` `enable` of both, and
  two lines of nginx's settings; the handbook's PHP chapter says how.
  The bundle grows by PHP-FPM's program, its manual page and its settings.
- Any program on the computer may connect to port 9000 and run a page as
  `php`: PHP-FPM serves whoever speaks FastCGI to it, with `/shared/www`'s
  files at hand. A Unix socket only nginx's account can reach replaces it
  when bundles can have sockets between each other (`docs/deferred.md`).
- Every pool runs as `php`, because a pool as another account needs PHP-FPM
  to start as root, which a bundle's service never does (ADR 0385). The
  slow log is not there. Both are rows in `docs/deferred.md`.
- A reload with settings that do not read stops PHP-FPM, as it does
  upstream: the master that runs itself again fails to read them and exits
  with status 78. Spark starts the service again (`restart = "on-failure"`)
  at once, and, if the settings still do not read, every 30 seconds, with
  the wrong line in the log each time; it does not give up, as the recipe
  has no `give_up_after`. Pages answer 502 from nginx meanwhile, and nginx
  keeps serving what it does itself. The settings' header and the handbook
  say to run `elevate php-fpm -t` first.
- The boot test `php-fpm` installs nginx and PHP for everyone, enables both,
  runs `elevate php-fpm -t`, turns nginx's PHP location on, and checks a
  site in `/shared/www`: its front page, a page that finds PHP-FPM's
  interface, its account, opcache, a session, a POST field, a file written
  into a folder of the account's and none into the site's; a missing page is
  404, three reloads while 150 requests come lose none and keep the
  process, a setting that does not read is named and stops a reload, a PHP
  error is in the log, and a stopped service is a 502. The `nginx` test now
  checks that a PHP page's URL is 404 with PHP off and its code never
  appears. The lab test `services/php-fpm` does the same on a real machine
  with 1,000 requests at a time of 50 during five reloads.
