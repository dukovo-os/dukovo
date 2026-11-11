<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0494. A listening socket makes room for connections up to its backlog

- Status: Accepted
- Date: 2026-11-11

## Context

A listening socket is a pool of smoltcp sockets on each interface it
listens on, each of which listens until a SYN comes and then holds that
connection until `accept` takes it (ADR 0183). The pool had four
sockets, whatever the backlog, and made another only when `accept` took
a connection. A fifth connection that came before the program took one
found no socket listening, and smoltcp answered its SYN with a reset:
the client was refused. On Linux a listening socket queues as many
connections as its backlog (up to `somaxconn`, 4,096), and a SYN that
comes when the queue is full is dropped, so the client sends it again a
second later and gets in once the server catches up.

The catalog's servers meet this at once. valkey-benchmark opens 50
connections together by default, and a refused one waits for ever
(it takes the refusal for a socket that cannot be written yet). pgbench
opens its clients' connections together too, and a browser opens six to
a web server at once. The Valkey boot test could only run with four
clients.

smoltcp cannot queue a connection without a socket for it: its sockets
are made with their buffers, 64 KiB each way (ADR 0182), and the SYN
needs a socket in LISTEN to be answered, with a window from the size of
the buffer it will have.

## Decision

- **A listening socket's pool on each interface is a `ListenPool`, which
  the interface tops up after every poll that changed something**, with
  the stack's lock held: it keeps four sockets listening, and makes
  another each time a connection takes one, until the connections being
  made or waiting for `accept` reach the backlog. `accept` takes the
  oldest waiting connection, and tops the pool up too, as it makes room.
  On the loopback interface each connection's handshake runs in the
  poll its `connect` starts, so a listener there is never without a
  socket to answer the next SYN, however many come before it takes one.
- **The backlog is the program's, up to 128 connections on each
  interface.** Each waiting connection holds its 128 KiB of buffers;
  Linux's queue costs far less per connection, and 128 is already more
  than most servers ask for on one interface. Calling `listen` again
  changes the backlog, as on Linux.
- **A SYN that comes when no socket of a pool listens is held, not
  refused** (the smoltcp patch, `set_pooled` and `take_held_syns`): the
  peer sends it again, a second later and then backing off, as it does
  when a Linux listener's queue is full. The pool counts the SYNs it
  held and makes as many more sockets listen, room allowing, so that
  the next tries get in; it lets those go again 15 seconds after the
  last SYN it held, past the third try a Linux peer makes. A SYN for a
  port no socket listens on is still answered with a reset.
- **A pool's sockets have a minute for their handshakes**, the time a
  connection being made has, and `accept` gives the connection the
  connected socket's timeout: a SYN whose peer never answers holds a
  socket of the pool for a minute, not the 15 minutes a connection
  waits for an answer.

## Consequences

- A burst of connections to a server on the loopback interface is never
  refused or delayed while the server keeps under its backlog. Over a
  card, SYNs that arrive together in one poll can find the four
  listening sockets taken; those wait for their peers' next try, about
  a second, where Linux would answer at once.
- A listener whose backlog is full drops new SYNs, as Linux does with
  SYN cookies off; a client sees a connection that is slow to be made,
  not a refusal. A SYN flood holds at most the backlog's sockets, each
  for a minute, on each interface it reaches; Linux's SYN cookies are
  not done.
- Each interface keeps a list of its listening sockets' pools, which it
  walks after every poll that received or sent a packet: a few sockets'
  states to read for each listening socket on it.
- The `sockets` boot test makes 40 connections to a listening socket
  that takes none, then takes them all, and has a connection past a
  backlog of two wait and be made once one is taken.
- ADR 0183's four handshakes at once on each interface, whatever the
  backlog, no longer hold.
