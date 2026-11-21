<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0506. syslog(3) writes to the kernel log when nothing takes /dev/log

- Status: Accepted
- Date: 2026-11-21

## Context

ADR 0210 left syslog(3) without a reader: the C library sends each
message as a datagram to `/dev/log`, nothing listens there, and programs
that use it were to lose those lines. mlibc did not lose them quietly.
When it could not connect to `/dev/log`, it printed "mlibc: syslog:
connect returned an error, falling back to infoLogger" in red on the
program's standard error, then every message there after "mlibc:
syslog: ", and it left out `LOG_PERROR` and `LOG_CONS`. A service's
standard error reaches the log through Spark, so a service's messages
came with that line and its escape codes in front; a command's came out
on the terminal of whoever ran it. PHP's test suite found it: one test
expects nothing on standard error after syslog(3), and two ask for
`LOG_PERROR` and expect the message there in syslog's own form.

Unix datagram sockets have come since (ADR 0525), so logd could take
`/dev/log`. A datagram says nothing of who sent it, though, unless the
receiver asks for credentials, and logd would keep records that came two
ways with two kinds of trust. `/devices/kmsg` already takes a line from
any program, keeps the facility and level its `<N>` gives (ADR 0211) and
marks the record with the process and the user that wrote it, which a
program cannot fake (ADR 0252); logger(1) writes there.

## Decision

- syslog(3) sends each message to `/dev/log` first, as on Linux, so
  that a syslog daemon listening there takes them, even one that starts
  after the program does.
- A message `/dev/log` does not take goes to `/devices/kmsg` as one
  record: `<N>`, with the message's facility and level, then the
  message. The kernel puts the thread's name in front of the record and
  marks it with the process, so the record has no process number, and
  has the ident only when the ident is not the program's own name: PHP's
  `openlog("phpt", ...)` in `php` writes `phpt: ` before the message.
- A service sees `/devices/kmsg` only if its file names it
  (service.toml(5)). Where the program cannot open it, the same line
  goes to standard error, which Spark keeps in the log as the service's,
  at the level and facility its `<N>` gives (ADR 0211).
- Only when none of them takes it does `LOG_CONS` write the message to
  the console, as musl does when `/dev/log` fails.
- `LOG_PERROR` copies every message to standard error in syslog's form,
  `ident[pid]: message`, as glibc and musl do; the line with `<N>` then
  does not go there as well.
- The C library writes nothing of its own on standard error for
  syslog(3).

The change is mlibc's patch 0095, for Radix alone.

## Consequences

- Programs that log with syslog(3), PHP with `error_log = syslog` or
  `mail.log = syslog` among them, have their records in the system's log
  with the facility and level they gave; log(1) and `dmesg -x` show them
  and logforward(8) sends them on. They are no longer lost, as ADR 0210
  said they would be.
- A record has the kernel's time, as every record does; syslog's own
  timestamp is dropped. The kernel keeps the first 256 bytes of a line
  written to `/devices/kmsg` (logger(1)), so a longer message is cut
  there.
- A program outside a service that cannot see `/devices/kmsg` prints
  the line with its `<N>` on standard error, where glibc would drop the
  message. A login session sees the kernel log, as logger(1) needs, so
  only a program given a narrower view does this.
- Each message costs a failed send to `/dev/log`, a failed connect and
  an open, a write and a close of `/devices/kmsg`. That is nothing at
  syslog's usual rate; a program that logs a great deal should log to a
  file or to its standard error.
- If logd takes `/dev/log` one day, programs need no change: their
  messages go there first.
- The boot test `logging` checks that a syslog(3) message becomes one
  record with its level, facility, ident and process, that the ident
  that is the program's name is left out, and that `LOG_PERROR` writes
  the message alone to standard error.
