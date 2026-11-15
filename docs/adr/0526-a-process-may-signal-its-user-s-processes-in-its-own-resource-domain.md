<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0526. A process may signal its user's processes in its own resource domain

- Status: Accepted
- Date: 2026-11-15

## Context

ADR 0061 let a process signal itself, its descendants and the processes
of its session, which stood in for users until Phase 13. ADR 0214 kept
the session and added that the process signalled act for the same user.
`prlimit` and `setpriority` reach the same processes (ADRs 0093, 0497).

PostgreSQL (ADRs 0521, 0522) is the first program ported whose processes
signal each other across sessions:

- Every process the postmaster starts calls `setsid`, so that the
  postmaster can signal it together with the scripts it runs, such as an
  `archive_command`, as one process group.
- They wake each other with signals: setting another process's latch
  sends it SIGURG, an event for a backend is SIGUSR1 (`procsignal`), and
  one for the postmaster is SIGUSR1 to it (`pmsignal`).
- Since PostgreSQL 18 a backend hands its reads to I/O worker processes
  (`io_method = worker`, the default) and sets a worker's latch.

Each of those signals failed with EPERM, so a backend that handed its
first read to a worker waited for it forever, and no connection was
answered. `pg_ctl` starts the server in a session of its own as well, and
asks it with signals whether it runs, to reload and to stop: `pg_ctl
status`, `reload` and `stop` failed from every other session, PostgreSQL's
own test suites' among them.

Linux lets a process signal any process that acts for its user. systemd
keeps a service's processes in a control group, which signals do not
look at. Spark starts each service in a resource domain of its own (ADR
0098), as systemd starts a unit in a control group, and its services
share Spark's session, so that a service's processes that keep that
session already reach each other and every other service acting for the
same user. authd starts the processes of a sign-in, and an elevated
command, in authd's domain (ADRs 0219, 0250), and a help screen in a
domain of its own below that one (ADR 0540).

## Decision

- **A process may also signal its user's processes in its own resource
  domain.** `kill`, and with it `prlimit`, `setpriority` and the
  scheduling calls that follow `kill`'s rule, reach the caller, its
  descendants, and the processes that act for the same user and share
  either its session or its resource domain
  (`radix_posix::tree::Tree::may_signal`, the kernel's `same_domain`).
- The domain is the one a process charges, which a child takes from its
  parent and which no process changes once started. Domains below it, or
  above it, do not count: a help screen's shell does not reach the
  sign-ins in authd's domain, nor they it.
- Tracing keeps ADR 0214's rule: a process traces only its descendants.

## Consequences

- PostgreSQL runs unpatched: its processes keep their sessions of their
  own, the postmaster still signals each one's process group, and a
  backend's first read through an I/O worker is answered. `pg_ctl status`,
  `reload` and `stop` work from any of the user's sign-ins, for a server
  started from any of them, as on Linux; a server run as a service is
  stopped and reloaded through Spark (`elevate service stop`), whose
  domain it is in.
- Processes of one service that leave Spark's session, as PostgreSQL's
  do, reach each other as they would on Linux, and still reach no other
  service's processes.
- A user's sign-ins reach each other's processes, as on Linux, where
  before they reached only their own session's. Another user's are still
  out of reach.
- An elevated command, which acts for root in authd's domain, reaches
  authd's own process, which it did not before. An administrator can
  already stop or restart authd through Spark.
- The `signal` boot test checks that a child in a session of its own
  signals its parent and a sibling in another session. The `services`
  boot test's service checks, from a session of its own, that it reaches
  its own processes and that neither `kill` nor `prlimit` reaches Spark,
  whose session and domain are others: the check of `prlimit` moves there
  from the `limits` boot test, whose program, as every program a boot
  test has Spark run, is in Spark's domain. The `principals` boot test's
  checks that alice reaches none of bob's or root's processes stand.
