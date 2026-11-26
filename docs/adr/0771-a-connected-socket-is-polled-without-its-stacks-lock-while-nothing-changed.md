<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0771. A connected socket is polled without its stack's lock while nothing changed

- Status: Accepted
- Date: 2026-11-26

## Context

A `poll` or an event queue that looks at a TCP connection takes the
socket's state and then its interface's stack, the sleeping lock that
every packet and every socket call on that interface takes
(`connection_events`). A poll of many connections takes the stack once
for each, though most of them have not changed since the last poll.

The lab's nginx connect row on the OptiPlex 7040, where the benchmark's
client and nginx open and close connections over the loopback, 8 at a
time, each polling its sockets, ran at 1.09 times Linux's time per
connection. Its kernel profile put a third of the time that was not idle
in `mutex_wait`, spinning on a held lock, about evenly in the client and
in nginx, and its counters 7.2 contended locks for each connection; the
two programs share nothing else that sleeps. Linux's `tcp_poll` reads a
socket's state without its lock.

## Decision

- **A connected stream socket keeps what a look found it ready for**
  (`Waiters::keep`), with the sum of its two counts of changes (ADR
  0550) and a count of its own calls, read before the look. A look that
  reads the same sum takes what was kept, with neither the socket's
  state nor its stack locked (`InetSocket::events`).
- **Every change to what it is ready for moves the sum.** smoltcp wakes
  a side for each change it makes: data or a FIN came, room came free,
  the state changed, and the calls that pass bytes straight across (ADR
  0186) wake the other end as the segments they stand in for would. A
  change to an interface wakes every socket. What smoltcp does not wake
  for is what the socket's own calls change, as a read that empties its
  buffer or a shutdown: each call takes the state through a guard
  (`InetSocket::locked`) that moves the count of its calls before it lets
  the state go.
- **A look that races a change finds one of the two states**: the
  change's lock is held while it moves the count, so a look that read
  the old sum and then looked under the lock found the old state or the
  new one, as a look under the lock would; the next look reads the new
  sum and looks again. Two looks may keep what they found in either
  order: each is at least as new as the sum it read.
- **Only a connected stream socket keeps it.** A listening socket's, a
  connecting one's and a datagram socket's are looked at under the lock
  as before.

## Alternatives

- **nginx on its kqueue module**, built without it today because Radix's
  event queue reports no byte counts. It would look only at the
  connections that changed, but each look would still take the stack,
  and the benchmark's client, and every other program that polls, would
  gain nothing.
- **Readiness kept in the socket by the stack**, recomputed for each
  socket a poll of the interface touched. Every path that changes a
  socket, its own calls included, would have to recompute it, under the
  stack's lock that is the cost to begin with.
- **A lock for each socket in place of the stack's**, as Linux has.
  smoltcp keeps all of an interface's sockets in one set that its poll
  walks; splitting it is a rewrite of the stack.

## Consequences

- A poll of connections that did not change since the last look takes
  no lock and writes nothing another CPU reads, as one of pipes does.
- Each call of a network socket moves one more atomic count, on the
  socket's own waiters.
- The inet POSIX test checks that each change is found at once and that
  a look made twice finds the same: bytes coming and read, a send that
  fills the other end's room and the read that frees it, a shutdown by
  the socket's own call, and the other end's FIN.
- On OptiPlex 1 it took nginx's new connection from 1.09 times Linux's
  time to 1.00 (lab runs 20261126-005747-d6c9 and
  20261126-015125-9b1d).
