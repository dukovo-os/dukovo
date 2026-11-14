<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0477. Spark holds a service's sockets and keeps them across restarts

- Status: Accepted
- Date: 2026-11-14

## Context

A server that listens for itself refuses connections whenever it is not
running: while it restarts, while a bundle's new version replaces it, and
for as long as a failed start takes to be tried again. nginx did so for
the moments of each restart (ADR 0491), and a service that was needed only
now and then ran all day to be there when someone connected. Two bundles
that talk over a socket had to start in the right order, and a bundle had
no way to offer a socket to another (ADR 0385).

systemd answers these with a `.socket` unit beside the service, and a
program built against `sd_listen_fds(3)`. The files are two, nginx has no
support for it, and a socket in `/run` is reached by whatever its mode
allows.

## Decision

1. **A service file names its sockets.** `[[sockets]]`, at most 16, each
   a TCP socket (`port`), a UDP socket (`port` with `udp = true`) or a
   Unix stream socket (`path`, a plain name in the service's live
   folder). `address` narrows a port to one address: `"0.0.0.0"` is every
   IPv4 address, `"::"` every IPv6 address and no IPv4, and without it
   one socket takes both. `name` is a word for `LISTEN_FDNAMES` (the port
   or the path when left out) and `backlog` is 1 to 65535 (4096 when left
   out). A port must be one of the file's `network.listen` ports, which
   covers TCP and UDP alike, so the firewall's list of listeners and the
   permission prompt stay true. `service check` says where a socket is
   wrong, at its line and column.
2. **Spark makes them before the service runs** and holds them while the
   service is enabled or started: at boot for every enabled service,
   before any service starts, so services that connect to each other
   start in any order. A socket Spark cannot make, because the port is
   taken, fails that service's start with the reason; the others go on.
3. **They are passed as `sd_listen_fds(3)` passes them**: descriptors from
   3 in the file's order, with `LISTEN_FDS`, `LISTEN_PID` (the process's
   own number) and `LISTEN_FDNAMES`. The places a service's `files.mount`
   gives follow from `3 + LISTEN_FDS`. Programs that take
   `sd_listen_fds` work as they are, and a wrapper converts for the rest.
4. **`start = "on-connection"`.** An enabled service with it is not
   started at boot: Spark watches its sockets, and the first connection
   or datagram starts it, with that connection waiting in the backlog.
   `service status` says "waits for a connection on port 8080". Stopping
   it by request puts it back to waiting, and `service disable` closes
   its sockets. The file needs at least one socket for it.
5. **Restarts and updates keep the sockets.** `service restart`, a
   restart after a failure, and a bundle's update (which stops the
   service with its sockets kept and starts the new version) leave
   Spark's sockets open. While nothing accepts, connections wait in the
   backlog and are served once the new process runs, so a client waits a
   moment and is never refused. `service stop` of a service that is not
   `on-connection` closes them, so the port refuses, as an administrator
   expects of a stopped server.
6. **How a service stops is its file's `[stop]` table,** a key of
   service_version 2's: the signal that asks it to stop (`SIGTERM`
   unless it says otherwise, or `SIGINT`, `SIGQUIT`, `SIGUSR1`,
   `SIGUSR2` or `SIGWINCH`, those daemons take as a stop request; never
   `SIGHUP`, which `service reload` sends) and the seconds Spark waits
   for the service to end before it ends the domain (5 unless it says
   otherwise, at most 3600). Every stop of the service uses them: `service
   stop` and `restart`, a stop because a required service stopped, a
   health check's restart and shutdown, which sends each service of a
   stop group its own signal at once and waits for each to its own time.
   A bundle's service says the same in its manifest, and bundled waits
   for the longest of its services' times and 10 seconds more before it
   gives up on a stop.
7. **nginx takes Spark's sockets.** nginx keeps its listening sockets
   across its own binary upgrade through the `NGINX` variable (`3;4;`)
   and reuses an inherited socket whose address and port a `listen`
   names. The bundle's `nginx-service` wrapper sets `NGINX` from
   `LISTEN_FDS` before it runs nginx, and nginx's settings are
   unchanged. The bundle names two sockets, port 80 on `0.0.0.0` and on
   `::`, which its settings listen on, and stops with `SIGQUIT`, nginx's
   graceful stop, for up to 30 seconds. Its sockets are `non-blocking`,
   a socket's key as systemd's `NonBlocking=`: nginx takes an inherited
   socket as it is and polls it, and on a socket that waits, a worker
   that loses the race for a connection waits in accept(2) and never
   hears the stop. A socket no process takes leaves
   its clients waiting, so a file names only what its program listens on:
   an administrator who turns on HTTPS adds port 443 to nginx's service
   file, which keeps their change across updates (ADR 0386). The wrapper
   reads the sockets from that file, as a shell cannot ask a descriptor
   its address, and logs a warning with that fix when nginx's `listen`
   lines name an address and port no socket has.
8. **A bundle offers sockets to others.** The provider's manifest names a
   socket in `provides` and in its service's `sockets` (a `path`); Spark
   makes it in the provider's live folder, owned by root, and sets the
   folder to 0755 so that the socket can be reached by name. `to` says
   who may connect: `anyone` (mode 0666), `system` (0600, the system's
   own programs, which run as root), and `publisher` or `approved` (0660
   with a group of its own, `ACCOUNT-socket`). bundled writes it into the
   provider's service file as the socket's `to` or `group`, which a
   bundle's own `sockets` cannot say, and for the group has authd make
   it as the provider is installed (`SocketGroup`: a number from a
   service account's range, no members, recorded as the bundle's through
   the account it is named for, kept by a reinstall and removed with the
   account; a person's group of that name, or another bundle's, is
   refused). A consumer names the socket in `uses` as
   `{"socket": "PROVIDER_ID/NAME"}`. bundled gives it at install when `to`
   allows: `anyone`; `publisher` when both bundles are signed by the same
   key; `approved` when the person who installs approves it, which the
   sheet asks and `--yes` answers; `system` never, to a bundle. The
   provider must be installed for everyone. A required socket that is not
   given stops the install, with why and what to do; an optional one is
   left out and the install says so. A given socket's place is added to
   every service of the consumer as a `files.read-write` place, and its
   group, when it has one, to the service's `groups`. A consumer without
   the grant has neither: its view does not hold the socket, and the
   mode refuses it if it reached the socket another way. A person's own
   programs do not reach a socket offered to the approved yet
   (`docs/deferred.md`).
9. **A bundle's service says all of this in its manifest** (`sockets`,
   `start`, `stop`, `backup`, `uses.socket`), and bundled writes it into
   the service file, so the checks of the file and of the manifest are the
   same ones. A service of `start: "on-connection"` needs a socket of its
   own, and a socket a bundle provides must be a `path` socket of one of
   its services. A health check's socket (ADR 0386) is one the service
   makes itself, never one of these: Spark takes connections at the
   sockets it holds before the service runs, so a check through one
   would pass a version that never works.
10. **bundled waits for a stop as long as the services do.** Its wait for
    the services to stop is the longest `stop.timeout` among the services
    being stopped, 5 seconds where a file says nothing, and 10 seconds
    more for the stop and the start after it. A service that is given 30
    seconds is not cut off by a bundle's update at 15 (ADR 0386).

## What it is not

- No idle stop. A service that wants to end when idle ends itself, and the
  next connection starts it again.
- No second process started before the first ends. The domain empties
  first, so a program that cannot take an inherited socket still has a
  gap, which the connections wait out (`docs/deferred.md`).
- No process for each connection, as inetd's and systemd's `Accept=yes`
  make.
- sshd does not take Spark's socket yet (`docs/deferred.md`).

## Consequences

- A restart or an update of nginx loses no request: the lab test
  `services/sockets` asks for 20 seconds at 16 requests at a time through
  three restarts and an update, and the boot test `nginx` does the same
  with `connectloop`.
- A file's sockets are a few lines. The service needs no socket unit, and
  a program built for `sd_listen_fds` needs no change.
- A service that fails to start while its sockets are held leaves its
  clients waiting rather than refused, until it starts or an
  administrator stops it. `service status` shows the sockets held.
