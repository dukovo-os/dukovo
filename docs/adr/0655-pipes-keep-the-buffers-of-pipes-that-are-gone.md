<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0655. Pipes keep the buffers of pipes that are gone

- Status: Accepted
- Date: 2026-11-23

## Context

On the OptiPlex, with both systems held at one clock, PHP-FPM behind
nginx (`server_php_fpm`) takes 1.21 times Linux's time for a request,
where PHP's own work (`server_php_compute`) takes 1.03 times. nginx
passes each request to PHP-FPM on a new Unix connection, as it does by
default, and a connection between Unix sockets is two pipes, one each
way (ADR 0085).

A count of the instructions in QEMU (`-icount`) found that making
those two pipes was a tenth of the request, kernel and programs
together: each pipe had the heap make a buffer of 64 KiB and zeroed it
one 8-byte store at a time, some 33,000 instructions a pipe. On
hardware the 128 KiB of stores also push out half of the core's
second-level cache (256 KiB on the OptiPlex's i5-6500), whose data the
programs then read back from further away.

## Decision

- **The buffers of pipes that are gone are kept for new ones**, up to
  32 of them, 2 MiB: sixteen Unix connections' worth. A pipe gives its
  buffer back when it goes, and a pipe that `pipe_size` (ADR 0497) gives
  a new buffer gives back its old one; only buffers of the default size
  are kept. The spares are a leaf spin lock's, held only to take or put
  one, so a pipe can go wherever its last reference does.
- **A kept buffer is not zeroed again.** What it held for the pipe it
  came from stays out of reach: a ring reads only the bytes written to
  it since it was made (`radix_tty::Ring`), as smoltcp does with the
  socket buffers kept since ADR 0185.
- **A buffer made from the heap is zeroed with a string store**
  (`radix_mm::bytes::zeroed`), not a loop of pushes, as are the copies a
  Unix socket makes of what it sends with files or as a datagram.

## Alternatives

- **Pages on demand, as Linux's pipes are.** A pipe would hold a short
  list of pages, each made when a write needs it and given back when it
  is read; a request of a few kilobytes would take one or two. It means
  a second ring, of pages, under the byte ring every reader and writer
  uses now, and each page still has to come from somewhere: the spares
  make the same connections without it.
- **A small first buffer, grown as writes need room.** Growing a ring
  that holds bytes copies them, and the room a pipe reports
  (`pipe_size`, a write's wait, `poll`'s `POLLOUT`) would have to be
  told apart from the room it has.

## Consequences

- In QEMU a request to PHP-FPM behind nginx takes 566 µs, from 624: the
  65,000 instructions the two buffers took, and the heap's work for
  them, are gone. Shell pipelines, whose pipes are made and gone with
  each command, take their buffers from the spares as well.
- Up to 2 MiB stays with the kernel after a burst of Unix connections or
  pipes, until pipes are made again. The posix `datagram` test's check
  that sockets give their memory back measures after rounds that make
  what the kernel keeps, so the spares are made before it looks. The
  capability selftest, whose last step makes three pipes, makes and
  drops three before it counts the heap, for the same reason.
- `Socket::connected_with` is kept out of line. With this change the
  compiler began to inline it twice in `make_pair`, and the two sockets
  took that call's frame past the kernel's budget of 2 KiB (ADR 0058).
