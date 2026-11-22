<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0546. The lab holds Dukovo's servers to Linux and keeps one up for a week

- Status: Accepted
- Date: 2026-11-22

## Context

The server acceptance run, which gates 1.0.0-alpha.1, asks two things the
lab could not answer. Does a server program run as fast on Dukovo as the
same program on Linux on the same machine? Does a small server stay up for a
week, through an update, a power cut and a pulled cable, and keep every
write it said it had made?

The performance tests held system calls, pipes and loopback to Linux, and no
test held nginx, PHP, PostgreSQL or Valkey to it. No test ran longer than a
day: the lab refused a test over 24 hours, a test that long would have run in
every round that names its category, and its log existed only when the test
had ended, so a run that was stopped on the fifth day left nothing to read.

## Decision

1. **A preset for the run.** `./ob lab run --preset server` runs every
   category but sound, and the tests of storage and hardware that a server
   has the devices for. Left out are the tests of audio CDs, disc burning,
   floppies, CompactFlash, memory card readers and the SD slot. The week's
   test is not in it.
2. **Server benchmarks.** `serverbench` (`userspace/posix/serverbench.c`)
   starts one server worker on the machine under test and drives it from the
   same program over the loopback or a Unix socket, with its data in
   `/tmp`, a tmpfs on both systems. Each row is one warm-up pass and three
   measured passes, and the lab keeps the fastest. The rows are
   `server_nginx_small` (a GET of 1 KiB on one of 8 keep-alive connections),
   `server_nginx_connect` (the same on a new connection each time),
   `server_nginx_large_mib`, `server_php_fpm` (a page through FastCGI on a
   Unix socket), `server_php_compute` (a loop of the program's own, timed
   inside PHP), `server_valkey_set`, `server_valkey_get`,
   `server_valkey_lpush`, `server_valkey_get_pipelined`,
   `server_postgresql_select` and `server_postgresql_tpcb` (pgbench's two
   scripts, 8 clients on 2 threads). The client runs on the machine so that
   the rows need no peer and read the same on QEMU and on a real machine, and
   they measure the system's work, not the network card's. One worker keeps
   the rows from depending on how many CPUs a machine has. Four lab tests,
   `performance/nginx`, `php`, `valkey` and `postgresql`, install the
   bundles, put the machine in the benchmark profile (ADR 0557), run
   `serverbench` and hold each row to parity.
3. **The same servers on Linux.** The Linux kit builds the same release and
   commit of each server that the port builds, with the host's compiler at
   `-O2`, as upstream builds it for Linux: without the port's patches, which
   adapt the program to Dukovo, and with only what the benchmarks use. Each is
   built with the prefix of the bundle (`/applications/org.nginx.nginx` and
   so on), so the paths in the benchmarks are the same on both systems. The
   programs are linked to the host's C library, which comes with them in the
   kit, as the POSIX benchmark's static program already does: the comparison
   is each system's C library and kernel under the same server code. Each
   server waits for events as it does on its own system, poll on Dukovo and
   epoll on Linux. The kit's builds are kept by commit, and the kit's own
   `time` in `kit.toml`, 15 minutes, is added to the deadline of a test that
   measures Linux, since the twin's benchmarks are the kit's time and not the
   test's.
4. **The week.** `stress/server-week` runs nginx, PHP-FPM, PostgreSQL and
   Valkey from the catalog as a small web shop, made with the system's own
   commands: a PHP website, a database for it, a firewall that lets in only
   ports 22, 80 and the web console's, and a backup of the machine's places
   to a folder of its disk, made again each hour. One page reads one of
   10,000 items through Valkey from PostgreSQL, lists the newest writes and
   writes, answering only after the commit. The lab PC sends it 20 requests a
   second over 4 connections, 70% reads, 20% lists and 10% writes, and every
   5 minutes signs in over SSH and to the web console's API, with a health
   read. Each day, the machine checks itself 24 times: the four services run,
   the items are as they were seeded (by their md5), the last backup is under
   two hours old once the day is two hours in, and the memory in use, the
   processes, the files open and the disk used are noted. A day fails at the
   first check that finds a service down, the items changed or the machine
   silent, and at its end if the memory in use grew by more than the soak's
   rule allows (ADR 0427: the last quarter's mean against the second's, the
   larger of 5% and 8 MiB), the processes grew by more than 2, or more than 2
   sign-ins in a row failed. Between days the machine is hurt: an update to
   a bridge release and its rollback, two power cuts and the cable out for
   two minutes. After each, the page answers within 120 seconds and every
   write that was acknowledged before is in PostgreSQL. The week is held when
   day 7's 99th percentile is within 20% of day 1's. Alerts by email are not
   part of it, as no mail sink stays up for a week.
5. **The latency is the page's own.** The lab PC may run other machines'
   tests during the week, and its load moves a round trip it measures. The
   page times itself from its first line to just before it prints and sends
   the microseconds as `X-Took-Us`, the load builds its histograms from that,
   and the verdict reads only those. The round trip as the lab PC measured it
   is kept beside them and shown, never judged. The histograms are buckets 5%
   wider than the one before, from 0.5 ms, so a minute's counts add up
   exactly into a day's. The test holds only its own machine: other machines
   stay free for other runs.
6. **The lab runs such a test.** In the lab's test files: a test may take up
   to eight days; `only_by_name = true` makes a test run only when a run names
   its id, not through `all`, a category or a preset that names the category;
   `optional = true` on a step that needs a control (the cable, the power)
   leaves the step out, saying so, on a machine that has none, where the test
   would otherwise be skipped; and a test's log is written after each step
   with a last line `Still running`, so that a run that ends early leaves
   what was done.

## Alternatives

- **TLS rows.** The handshake and the record costs are OpenSSL's arithmetic,
  the same code on both systems, and the kit's Linux servers are built
  without OpenSSL. A deferred row says so.
- **Servers over the network card.** The loopback needs no peer and gives the
  same rows on QEMU and on real machines; the card's path has its own rows.
- **Servers as the port builds them, on Linux too.** The port's patches adapt
  the programs to Dukovo; a Linux that ran them would run code written for
  somewhere else.
- **A week of a loop of commands.** The soak of ADR 0427 does that for the
  system. This test is what a server's owner sees: pages, a database and a
  cache under steady load, with the faults in the middle.
- **Judging the round trip.** It would make the verdict depend on whatever
  else the lab PC is doing in that week.

## Consequences

- The server benchmarks are in the Linux kit, which grows by the four
  servers, their libraries and the benchmark program, and builds in minutes
  more. When a server cannot be built for Linux, the kit says so and the
  server tests fail with the missing row's name.
- A run of `stress/server-week` takes a machine for a week and a PC for as
  long, and a restart of the lab PC ends it; the log it wrote as it went
  keeps what was done (docs/deferred.md).
- On a real machine, a person pulls the cable between days 4 and 5 when the
  run has `--with-person`; without one the step is left out and the log says
  so.
- The firewall of the week lets in three ports, so a person who runs the test
  by hand on a machine they use finds the others closed until the test ends.
