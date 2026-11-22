<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0651. A TCP connection wakes one thread in accept

- Status: Accepted
- Date: 2026-11-22

## Context

PHP-FPM as shipped (`ports/php/php-fpm.conf`) listens on
`127.0.0.1:9000`, and its workers (dynamic, up to 8 children) all block in
`accept` on that one TCP socket. ADR 0650 stopped a Unix connection from
waking every acceptor, and left TCP listeners as they were (its
Consequences say they keep their own waits).

A listening TCP socket keeps a pool of smoltcp sockets for each interface
(`ListenPool`, ADR 0494), and each pool socket had the listener's wakers
(ADR 0550). So every step of every handshake (the SYN received, the
connection established) and every segment that arrived before `accept`
called `wake_all` on the listener's receive side: every thread blocked in
`accept` woke, one took the connection, and the others found nothing and
slept again. Every poller of the listener woke too.

Linux waits for a connection exclusively (`inet_csk_wait_for_connect`),
wakes one waiter when a connection joins the accept queue, and wakes
nobody for the steps before it.

## Decision

- **Pool sockets carry no wakers.** `ListenPool::top_up`, which the
  interface calls under the stack lock after every `stack.poll()` that
  reports a change (and after `take` and `give_back`), counts the pool's
  connections that were made (state neither Listen nor SynReceived) and
  wakes the listener's receive waker once for each new one since it last
  counted (`Members::made`). A connection `accept` took lowers the count
  and wakes nobody. A connection given back by a failed accept is counted
  again and wakes one.
- **The listener's receive side wakes one waiter per change.**
  `Waiters::accepts()`, called in `listen`, sets the `one` flag on the
  side (`kernel/radix/src/net/waiters.rs`), and a wake then goes through
  `wait::wake_one_lazily` (`kernel/radix/src/sched/wait.rs`).
  `Waiters::changed()` (close, shutdown, an interface's addresses or link
  changing) still wakes all.
- **A waiter that leaves at its timeout or for a signal looks once
  more.** A waiter on a wake-one side that leaves `wait()` at its deadline (`SO_RCVTIMEO`) or
  for a signal looks once more as it goes and returns a connection if one
  waits, so a wake it consumed never strands a connection while the other
  acceptors sleep. Linux's `inet_csk_wait_for_connect` checks the queue
  before the signal in the same way.
- **Pollers still hear of each connection.** The receive waker also calls
  `polled.changed()`, so a `poll` or `epoll` on the listener wakes for
  each connection made.

## Consequences

- A TCP connection wakes one of the processes blocked in `accept`. The
  `accepting` part of `userspace/posix/inet.c` checks it: four children
  block in `accept` on one listening socket (backlog 16) and 500
  connections over IPv4 loopback make their voluntary switches total at
  most 750; a failure names the total. Without the change the check
  failed in CI's QEMU, on both boots of the `posix` test.
- No wake is lost. Connections become made only inside `stack.poll()`:
  smoltcp's ingress returns `SocketStateChanged` for every packet it hands
  a socket, and the straight-across loopback path of ADR 0186 needs both
  ends Established and never changes a state. Both callers of
  `stack.poll()` in `kernel/radix/src/net/mod.rs` call `refill` under the
  same hold of the lock when it returns true, so each made connection is
  counted once, and so woken for once. The waiter protocol is ADR 0550's:
  it counts itself in, reads the changes, looks, and sleeps until the
  changes move, so a wake is either seen by a sleeping waiter or its
  change is seen by a waiter about to sleep. No new TLA+ model is needed:
  the count is taken under the stack lock after every poll that changed a
  socket, and the waiter protocol is the one `kernel/radix/tla/Poll.tla`
  already checks.
- The pool's smoltcp sockets no longer wake anyone for data that arrives
  before `accept`. The accepted socket gets its own wakers in `have`, and
  its readers look before they sleep, so data that came earlier is seen.
- A half-open connection that times out in SynReceived is closed without
  a packet, so it wakes no acceptor until the next poll that changes
  anything counts it. Linux never hands such a connection to `accept`.
- There is no ABI change.
