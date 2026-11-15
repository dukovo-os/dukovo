<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0470. Services name what they wait for, say when they are ready, and check their health

- Status: Accepted
- Date: 2026-11-13

## Context

Spark started every enabled service at boot in one pass, sorted by
`order` (ADRs 0101, 0216). Nothing waited for anything: a lower `order`
only meant an earlier call. A service counted as working the moment its
process existed, and one that kept failing was started again forever,
0.1 s to 30 s apart. A web server that needs its database had no way to
say so, and nginx's bundle could not say that it looks up the names in
its `proxy_pass` lines once, as it starts, and so fails without the
resolver (ADR 0491).

The clients of Spark's control socket read its state from `status`, a
line per service of seven fields, which `service`, sysupdate's health
check and bundled each parsed on their own; and Spark wrote each answer
with one `write`, so an answer longer than the socket's buffer was cut
short.

A server's services need what systemd's units give: dependencies,
readiness, health checks, and a limit on restarts. They also need what
systemd leaves to the administrator to work out: why a service is not
running, in words.

## Decision

- **service_version 2** adds six keys to a service file
  (`service.toml(5)`): `requires`, `wants` and `after`, lists of
  services; `give_up_after`; a `[ready]` table (`when`, `port`, `path`,
  `timeout`); and a `[health]` table (`command`, `every`, `timeout`,
  `failures`). A version 1 file with one of them is refused, with the
  fix. The keys keep systemd's meanings where systemd has them:
  `requires` is Requires= with After=, `wants` Wants= with After=,
  `after` After=, and `ready.timeout` TimeoutStartSec=.
- **A service is wanted** when it is turned on at boot, when a request
  starts it, or when a wanted service requires or wants it, whether it
  is turned on or not. Rescue and a start with only the essentials still
  leave theirs off. A wanted service waits until each service it names
  is settled: ready, or a setup step that is done, lets it go; starting
  makes it wait; stopped, failed, left off or without a file stops it
  from starting if it requires that service, and lets it start if it
  only wants it or starts after it. Among the services that may start at
  once, `order` and then the name decide.
- **Readiness** is one of five: the process started (the default); a
  socket listening on a port of the service's own `network.listen`,
  read from `/processes/net/tcp` and `tcp6`; a socket file made since
  the start; the health check passing for the first time; or the
  process ending with status 0, for a setup step. Spark looks every
  100 ms while a service gets ready. A service not ready within its
  `ready.timeout` (90 s by default) is stopped, and its start has
  failed.
- **Health checks** run as the service runs: its account, view,
  environment, secrets and limits, in its domain. A check passes by
  exiting with 0 within its timeout. Spark keeps the last line a failed
  check wrote, logs the first failure and the recovery, and restarts the
  service after `failures` failed checks in a row.
- **Giving up.** Spark counts failed starts in a row: a program that
  cannot start, an end before ready, a readiness timeout, a restart for
  health, or a failing end within 10 s of getting ready. At
  `give_up_after` it stops starting the service, which is failed until a
  start request; 0, the default, keeps today's behaviour.
- **Stopping for good** (stopped by hand, ended and not to be started
  again, given up on, refused) stops the services that require the
  service, with the reason, recursively. A restart, by hand or by
  Spark, does not: what requires a service reconnects, which keeps
  restarts cheap, and a service that cannot reconnect is one its own
  health check finds.
- **Circles** of services that wait for each other never start; Spark
  logs each circle once and each service says so.
- **Shutdown** stops services by stop rank, highest first: a service's
  `order`, raised above the rank of each service it names, so that it
  stops before them.
- **Every refusal and stop says why**, as a clause that follows the
  service's name, in the log and in `service status`: "it requires db,
  which Spark gave up on", "it did not get ready in 90 s: nothing
  listened on port 8080", "it waits for itself, through a and b".
- **`show`**, a request of the control socket, answers a JSON document
  of every service, or of one, and of a pending shutdown. The
  `spark-api` crate defines it, and `service`, sysupdate and bundled
  read it through that crate in place of their own parsers; the web
  console reads the same. A client refuses a later `show_version`;
  fields are added without a new version, and clients pass over those
  they do not know. `status` stays for scripts. Spark writes any answer
  in as many writes as it takes, polling the connection until it is
  written or its time is up.
- **Bundles** take the same keys in their manifests' services
  (`manifest.json(5)`), naming their own services or the system's; a
  health check runs a program of the bundle's. bundled writes a version
  2 file only for a service that uses one of them, and `ob ports` gives
  such a bundle `requires_os` "1.0". nginx starts after `netd` and
  `resolved` and is ready once it listens.
- **sysupdate's health check** counts an essential service that runs but
  fails its health check as not running, and a failed one as failing at
  once.
- **The system's own service files stay version 1** for 1.0.0-alpha.1,
  as does `service new`'s template, which shows the new keys commented
  out: a release writes only what the release before it reads (ADR
  0454), and 0.15 reads version 1 alone. They move to version 2 in the
  release after alpha.1.

## Consequences

- A service that does not run says why in one line, with the service it
  waits for or the failure that stopped it, where systemd answers
  "Dependency failed" and leaves the chain to `systemctl
  list-dependencies`. A circle is refused with its members named, where
  systemd breaks it by dropping a job and logging which.
- A listening socket on the service's port counts whoever listens: the
  socket tables do not name their process. The port is one the
  service's own file lets it listen on, and two services granted one
  port is a mistake `service check` will name.
- A health check is a process every so often. Checks run only for
  services that have one, and only one at a time for each.
- Starting a requirement again does not start the services that were
  stopped with it; starting one of them starts it. This is systemd's
  behaviour for Requires= without PartOf=.
- Spark grows by about a quarter, to 1.2 MB as the release profile
  builds it, past the startup archive's MiB (ADR 0427). A release now
  builds Spark alone with the `spark` profile, for size and as one
  unit, and the archive is 804 KiB.
- The system's own services still start by `order` until their files
  move to version 2, in the release after alpha.1; the deferred list
  holds the move, and the check of `requires_os` at install.
- The `service-graph` boot test starts services that name each other
  and checks what `service status` says of each, and that shutting down
  stops a service before the one it requires; the lab tests
  `services/graph` and `services/health-giveup` do the same with nginx
  from the catalog and a service whose check fails, on the lab's
  machines.
