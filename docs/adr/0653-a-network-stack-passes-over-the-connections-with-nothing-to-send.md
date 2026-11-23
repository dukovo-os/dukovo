<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0653. A network stack passes over the connections with nothing to send

- Status: Accepted
- Date: 2026-11-23

## Context

On the OptiPlex, with both systems held at one clock, nginx answering
on a new connection each time (`server_nginx_connect`, eight at a time
on the loopback interface) took 32.7 µs a request on Dukovo and 21.6 µs
on Linux, 1.52 times as long. Under KVM, on a faster processor, the two
were even; Dukovo's time grew with the slower clock and Linux's hardly
did, so the difference is work the processor does, not waiting.

A count of the instructions in QEMU (`-icount`) found 325,000 of them
in the kernel for each connection, against about 17,000 in nginx and
the client together, and most of the kernel's in two places:

- **Every poll of the stack worked out, for every connection, whether
  it had anything to send.** smoltcp's interface goes through all its
  sockets on each poll, and for a TCP connection that means its
  segment, acknowledgement, window, retransmission, keep-alive and
  zero-window checks; `poll_delay` then works most of them out again
  for the time of the next poll. The loopback interface polls in the
  thread of each call that may have given it something to do (ADR
  0185), and a connection's life takes four such polls or more, each
  looking at the sixteen connections of the eight in flight. A
  connection's life had a connection's checks worked out 195 times,
  about 80,000 instructions, nearly all for connections that had
  nothing to send.
- **Two of the four 64 KiB buffers a connection's sockets take were
  made from the heap and zeroed**, 128 KiB a connection. Eight buffers
  of sockets that closed are kept for new ones (ADR 0185), but eight
  connections end close together and take thirty-two buffers back; the
  rest went back to the heap, and the next connections made theirs
  again.

## Decision

- **smoltcp keeps, for each TCP connection, what its `poll_at` said
  when it last looked**, in the socket slot's `Meta`, and a poll passes
  over a connection whose answer was that it waits for a packet, or for
  a timer that is still to come. The answer is worked out again after
  each `dispatch` of the connection. `Interface::poll_at` takes the
  kept answer too.
- **Whatever may change a connection forgets the answer**: the
  socket set's `get_mut`, `try_get_mut` and `iter_mut`, through which
  the kernel reaches a socket for a program's call, and a packet the
  connection takes. A change to the interface's addresses, which can
  reset a connection, makes every answer kept before it stale. The
  kernel looks at a socket it does not change through `Stack::socket`,
  which keeps the answer: `poll` and `FIONREAD`, `accept` looking for a
  connection, `getpeername`, and the orphans' reaping.
- **Sixty-four buffers of sockets that closed are kept**, 4 MiB, from
  eight.

## Alternatives

- **A list of the connections that may have something to send.** It
  would spare even the look at each slot, but every place that changes
  a connection would have to add it to the list; the kept answer is
  forgotten in the few places that hand out a connection to change.
- **Smaller buffers, grown as a connection needs them**, as Linux's
  are. smoltcp's buffers are fixed slices lent for a socket's life
  (ADR 0182); growing one would mean copying a live ring. The spares
  make the same connections without it.
- **Zeroing buffers as they come back, while idle.** It moves the cost
  rather than removing it, and a server that is busy has no idle time.

## Consequences

- In QEMU, a new connection to nginx takes 154 µs, from 237 µs, and
  the kernel's instructions for it fall from 325,000 to 133,000: a
  connection's checks are worked out 20 times in its life rather than
  195, and no buffer is zeroed.
  Every TCP call on the loopback interface polls it, so the requests on
  connections that stay open (nginx's other rows, Valkey's, PHP-FPM's
  front end) look at fewer connections as well.
- A connection's next poll can be no later than its kept answer says:
  the answer comes from the same `poll_at` the interface's thread
  already waits by, so a timer is not missed for being kept.
- A socket the kernel changes without asking the set for it would keep
  an answer that is no longer so, and wait for its next packet or timer
  to send. Every change goes through `socket_mut`, which forgets it.
- Up to 4 MiB stays with the kernel after a burst of connections, until
  sockets are made again. This changes ADR 0185's eight spare buffers.
- The change to smoltcp is to be offered upstream with the others.
