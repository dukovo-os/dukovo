<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0601. Caddy takes Spark's sockets by number, and reloads through its admin API on SIGHUP

- Status: Accepted
- Date: 2026-11-22

## Context

Caddy is a web server and reverse proxy written in Go, which the catalog
carries as a bundle built as ADR 0600 says. A service bundle of a web
server is expected to do what nginx's does (ADRs 0385, 0477, 0491 and
0512): run as an account of its own, serve the sites in `/shared/www`,
keep its port open across restarts and updates by taking Spark's sockets,
read the certificates certd keeps, and read them again on Spark's SIGHUP.

Caddy differs from nginx in three ways. It takes an inherited socket by
its descriptor's number, as a listener address `fd/N` (since Caddy 2.10),
not from an environment variable. It manages certificates itself by
default, from ACME authorities, which would have it open ports of its own
for their checks. And it ignores SIGHUP: its settings are read again
through its admin API, which `caddy reload` calls, and only when they
changed, unless `--force` says otherwise.

## Decision

- **The bundle `com.caddyserver.caddy`** has the command `caddy`, which
  runs with the access of whoever runs it, and the service `caddy`, which
  runs as the account `caddy` once an administrator enables it, with its
  data folder `/shared/services/com.caddyserver.caddy/caddy`. It reads
  `/shared/www` and may listen on ports 80 and 443; connecting out is
  optional, for a reverse proxy.
- **Spark holds port 80** for it, for IPv6 and IPv4 in one socket, passed
  as descriptor 3, which the Caddyfile it ships binds as `fd/3`. An
  administrator who turns HTTPS on adds port 443 as the service's second
  socket, descriptor 4, which the Caddyfile's HTTPS site binds. A port is
  not held before a site uses it: a held port that nothing accepts on
  leaves clients waiting rather than refused.
- **The Caddyfile** is copied into `conf/` at the first start and kept on
  updates as nginx's settings are (ADR 0386). It turns automatic HTTPS
  off, since Spark holds the ports and certd keeps the certificates
  (ADRs 0512 and 0513): the HTTPS site names
  `/system/live/certificates/caddy/this-machine/`, and the HTTP site
  serves certd's answers to an ACME authority's checks, as nginx's does.
  It serves HTTP/1.1 and HTTP/2; HTTP/3 needs a UDP socket, which Spark
  holds for no service yet. Its grace period, 20 seconds, is less than
  the 30 Spark waits for it to stop. The admin API listens on a socket in
  the service's live folder, never on a port.
- **`caddy-service` runs Caddy as its child.** On SIGHUP, which Spark
  sends when the service's certificates change and on `service caddy
  reload`, it runs `caddy reload --force` with the service's own
  settings, so that certificates are read again even when the settings
  did not change; settings that do not read leave Caddy as it was, and
  both say why in the log. It passes TERM, the service's stop signal and
  Caddy's graceful one, and INT on to Caddy, and ends with Caddy's
  status.
- **Caddy's data**, its certificate storage and the copy of its settings
  it keeps, is in the data folder's `data` and `config`
  (`XDG_DATA_HOME`, `XDG_CONFIG_HOME`).
- **Its manual pages** are the ones Caddy writes itself with `caddy
  manpage`, run on the build machine.

## Consequences

- nginx stays the server of the admin app's Websites and of
  `website(8)` (ADR 0491); Caddy is the administrator's to set up in its
  Caddyfile. A Caddyfile site of `website(8)`'s folders answers 404, as
  nginx's default server does.
- Caddy's own ACME client, its `caddy trust` and its local authority are
  not used; a certificate from an ACME authority comes from `cert get`.
- The boot test `caddy` installs and enables the service, serves HTTP
  and HTTPS, checks that broken settings leave it serving, reloads under
  load without losing a request, serves a renewed certificate without a
  restart, and restarts and updates under load without refusing a
  connection.
