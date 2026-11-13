<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0496. A close with a linger of 0 resets, and a cut-short datagram says so

- Status: Accepted
- Date: 2026-11-13

## Context

CPython's test suite, which the lab runs whole (ADR 0495), found
network sockets that behaved differently from Linux's in ways that
programs rely on:

- `SO_OOBINLINE`, `SO_LINGER` and `TCP_QUICKACK` failed with
  ENOPROTOOPT. asyncore-style servers set `SO_OOBINLINE` on each
  connection they accept and treat a failure as a closed connection, so
  the suite's FTP server never answered its client. A TLS server that
  wants a client gone sets `SO_LINGER` to 0 seconds, so that its close
  sends RST rather than FIN.
- `getpeername` on a socket whose non-blocking `connect` was still
  waiting failed with ENOTCONN even after the other end had accepted
  the connection. Python's `socketpair` over TCP, which asyncio uses
  where there are no Unix sockets, connects that way and checks both
  ends' names at once. And `getpeername` still found a peer once the
  connection had been reset: Python's TLS client asks it to tell a
  server that wrote and reset before the handshake, whose bytes it must
  not take for TLS, from a connection it may wrap.
- A `recvmsg` with no address, one buffer and no control buffer on a
  network socket failed with EOPNOTSUPP: the C library took it for a
  Unix socket's receive, which may carry descriptors.
- A datagram larger than the buffer was cut short silently: `recvmsg`
  never set `MSG_TRUNC` in `msg_flags`, and `MSG_TRUNC` as a flag, which
  asks for the datagram's whole size, failed with EOPNOTSUPP.

## Decision

- **radix-file(2) has three more socket options**: `linger` (0 for off,
  1 for a linger of 0 seconds, 1 plus the seconds otherwise),
  `quick_ack` and `out_of_band_inline`. A stream socket closed with a
  linger of 0 seconds has its connection reset, as Linux's is: RST goes
  and what was not sent is thrown away. A longer linger is kept and read
  back, but `close` still returns at once and the connection ends in the
  background, as without one; no program the catalog ships waits on its
  close. `quick_ack` turns smoltcp's delayed acknowledgement (10 ms) off
  for the connection, and stays set, where Linux's may turn itself off.
  `out_of_band_inline` is kept and read back: smoltcp gives urgent data
  inline whatever it is, which is what `SO_OOBINLINE` asks for.
- **`getpeername` settles a connection being made** before it answers,
  as `send` and `recv` already do: once the handshake is done, the
  socket is connected. **A stream socket whose connection has ended**,
  reset or closed both ways, **has no peer** (ENOTCONN), as on Linux,
  while what came before the end is still received.
- **A datagram's receive can return its whole size**: the
  `whole_size` socket flag of `receive_from` and `receive_message`. The
  C library always asks for it, sets `MSG_TRUNC` in `msg_flags` when the
  datagram was larger than the buffers, and returns the whole size when
  the program passed `MSG_TRUNC`, as on Linux.
- **`recvmsg` that fails on a network socket the way a Unix socket's
  receive of descriptors does is received as a network socket's**: the
  C library asks the socket's family only then, so that a plain
  `recvmsg` on a Unix socket costs no more than it did.

## Consequences

- The FTP, TLS and socket tests these gaps held back run in the lab's
  CPython suite again, and their lines leave
  `lab/tests/services/python-skips.txt`.
- A linger past 0 seconds does not make `close` wait. A program that
  needs to know its data arrived must still `shutdown` and read to the
  end, which is the portable way on Linux too.
- RFC 3542's IPv6 ancillary data (`IPV6_RECVHOPLIMIT`,
  `IPV6_RECVTCLASS` and their control messages to send) still fails
  with ENOPROTOOPT: smoltcp does not give a datagram's hop limit or
  traffic class to its socket, and it stays in deferred work. So does
  `send` with `MSG_OOB` (EOPNOTSUPP): smoltcp sends no urgent pointer,
  and sending the bytes as ordinary data would be another meaning.
- The `posix` boot test's `inet` program checks each of these: the
  options read back, a reset seen by the other end as ENOTCONN from
  `getpeername` and ECONNRESET after the bytes that came before it,
  `getpeername` after a non-blocking connect, a plain `recvmsg`, and
  `MSG_TRUNC` both ways.
