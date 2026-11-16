<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0525. Unix datagram sockets, and sockets passed under a rule that keeps out cycles

- Status: Accepted
- Date: 2026-11-16

## Context

Radix's Unix sockets are stream sockets (ADRs 0085, 0091). Programs
ported to Dukovo use three things those lack:

- **Datagram sockets** (`AF_UNIX`, `SOCK_DGRAM`): a service's
  notification socket, a log socket, and a server that answers each
  request in one message.
- **Unix sockets passed with `SCM_RIGHTS`**: a server handing a
  connection to a worker, or a listening socket to its next version.
  ADR 0091 refused them, so that no socket on its way could hold itself
  open and the kernel needed no collector, and left a rule that keeps
  cycles out for when a port needed one.
- **Credentials as control messages** (`SO_PASSCRED`,
  `SCM_CREDENTIALS`).

PHP's test suite skipped nine tests for want of them (ADR 0499).

Linux lets any descriptor ride in a socket's queue. A socket on its way
can then hold its own connection open, or two can hold each other's,
which no process can close any more, and Linux runs a garbage collector
over the sockets on their way to find such cycles and close them.

## Decision

### Datagram sockets

- radix-process(2)'s `datagram_socket` and `datagram_pair` make datagram
  sockets, which `socket` and `socketpair` give for `SOCK_DGRAM`. A
  socket is named as a stream socket is (`bind_socket`).
  radix-directory(2)'s `send_datagram` sends to a name, and
  `connect_socket` sets where a datagram socket's sends go and the only
  socket it takes datagrams from; radix-file(2)'s `disconnect` undoes it,
  as `connect` with `AF_UNSPEC` does. Each send is one datagram, of up to
  212,960 bytes, which a receive takes whole; what the buffer has no room
  for is lost, and `MSG_TRUNC` says so.
- They behave as Linux's (`net/unix/af_unix.c`), error for error:
  `ENOTCONN`, `EMSGSIZE`, `EPIPE` without SIGPIPE, `ECONNREFUSED` for a
  name whose socket has gone, the new `EPROTOTYPE` for a name of a
  socket of the other type, either way, and `EPERM` for a receiver
  connected to another socket. A socket connected to one that has gone
  gets `ECONNREFUSED` once, and is then not connected. Connecting to
  another socket, or disconnecting, throws away the datagrams waiting to
  be received.
- **Each socket's queue is bounded where it is received**: at most 512
  datagrams (the `net.unix.max_dgram_qlen` Linux systems run with) and
  212,992 bytes of them (Linux's default `SO_RCVBUF`), a datagram always
  going into an empty queue. A send to a full queue waits, or fails with
  `EAGAIN`. Linux bounds what each sender has on its way instead, and
  lets a socket's own peer past the queue's length; bounding the
  receiver keeps each socket's memory fixed where it lies, whoever sends.
- A sender finds a queue through the socket's name, or through the
  socket it is connected to, by weak references: no socket keeps another
  alive, and a queue goes with its socket.

### Unix sockets passed

Any Unix socket, of either type, listening or not, may ride among the
files a socket sends (`send_files`, `send_datagram`), unless it could
come to hold itself open. A socket *keeps* the queues it receives from:
a connected stream socket the pipe it reads, a listening one the pipes
of the connections that wait for `accept`, a datagram socket its own
queue. A send fails with `ETOOMANYREFS`, Linux's error for a send that
would put too many descriptors on their way, when for a Unix socket T
among its files

- (a) a queue T keeps holds a Unix socket on its way, or
- (b) the queue the files are going into is one T keeps.

Say that S holds T while T rides in a queue S keeps. A send of T adds
only "the keeper of the target queue holds T". By (a), T holds nothing
then, and by (b), that link does not start at T, so no cycle closes
through T. Taking files out of a queue only takes links away. A queue
changes keeper only when `accept` gives a waiting connection a socket
of its own, which nothing can send until it has it and which therefore
nothing holds, so the move adds no cycle. One lock serializes every
check with the insertion that follows it, so two sends never each pass
against the state before the other; it is taken inside the target
queue's lock and held only while the kernel looks at its own state,
never across a wait or a copy from a program's memory. A connection
that `accept` took but could not give a socket goes back to the front
of the queue, unless a Unix socket rides in it: while it was out, no
socket kept its pipe, so the sockets sent meanwhile were checked without
it. The kernel needs no collector.

A socket's last reference can be the one a queue holds, and its own
queues can hold more sockets: dropping them in place would go as deep as
such a chain is long, on a small kernel stack. Each goes instead to the
kernel's list of released files, which is dropped one after another
until it stays empty.

### Credentials

`SO_PASSCRED` is an option of either type. radix-file(2)'s
`receive_unix` receives as `receive_files` does and says where the bytes
came from: the sending socket's name, and the process, user and group
that sent a datagram, as they were at the send, or a stream's peer, as
`SO_PEERCRED` gives it; and whether `SO_PASSCRED` is set, for the C
library to give them as `SCM_CREDENTIALS`. A process may send
`SCM_CREDENTIALS` naming itself, with its real or effective user and
group; the receiver gets the kernel's record of the sender either way.

## Consequences

- PHP's nine tests run, and the posix boot test's `datagram` part checks
  datagram sockets, sockets passed both ways and refused, and
  credentials.
- A typical passing works: a server hands a fresh connection to a
  worker, or its listening socket to its next version. A socket sent
  over its own connection goes, as it can be closed from the other end.
  Refused, where Linux would collect a cycle later: a socket whose queue
  holds a socket not yet received, and the other end of a connection
  sent into the pipe that end reads.
- Datagram sockets differ from Linux's where noted: the queue's bound
  is the receiver's, a connected socket's peer is not told when a
  reconnect throws away what waited for it (Linux sets `ECONNRESET` on
  it), and a socket without a name sends with an empty address, as
  Linux does without autobind, which Radix does not do.
- A stream's credentials are its peer's, where Linux gives those of the
  process that wrote each piece; only a stream passed to another process
  between writes tells them apart.
- `SOCK_SEQPACKET`, abstract names and autobind, and credentials naming
  another process, which Linux lets a privileged process send, are still
  to come (`docs/deferred.md`).
- The process interface goes to version 11, the directory interface to
  12, and the file interface to 13.
