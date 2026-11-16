<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0550. A network socket wakes only the threads that wait on it

- Status: Accepted
- Date: 2026-11-16

## Context

Until now the kernel's network sockets (ADR 0183) had one count of
changes for all of networking. A socket call that could not go on waited
for that count to move, and every poll of a stack that changed anything
moved it, woke every thread blocked in any socket call on the machine,
and every poller of any network socket. Each of them looked at its own
socket again, found nothing, and slept.

`ob bench` boots a system where only the benchmarks use the network, so
its BENCHCOUNT lines showed two thread switches per UDP round trip on one
CPU and no IPIs. The lab's run on a Dell OptiPlex 7040, an installed
system whose services wait on their sockets, showed 18 switches and 9
IPIs per round trip, and the other three CPUs busy the whole time. Against Linux on
the same machine, UDP round trips took 3.7 to 3.9 times as long, TCP round
trips 2.2 to 2.6 times, a mebibyte over TCP 2.65 times and a TCP
connection 1.23 times. Every server with more than one service would pay
the same.

Linux keeps a wait queue on each socket, and a packet wakes the threads
on that socket's queue alone.

## Decision

- **Each network socket has its own waiters** (`net::waiters::Waiters`):
  a count of changes that its blocked calls wait on, how many threads
  wait, and its own pollers (`Polled`), in place of networking's one
  count and one list of pollers.
- **smoltcp says which socket changed.** Each smoltcp socket a network
  socket makes, for UDP, TCP, ping and each of a listening socket's pool
  (ADR 0494), has the socket's waker, for receiving and sending, set once
  when it is made, and an orphan (ADR 0185) has it taken away when its
  interface adopts it. smoltcp wakes it whenever what the socket can receive
  or send may have changed: data came, room came free, its state changed.
  A connection `accept` takes gets its new socket's waker; one given back
  gets the listening socket's again.
- **The kernel's smoltcp patch keeps a waker registered after it wakes**
  (smoltcp wakes and forgets it, for tasks that register again each time
  they wait) and never drops it: drop glue on a socket costs every frame
  that holds one by value another copy of it, over ADR 0058's budget for
  `connect` and a listening pool's refill. The kernel's waker holds no
  reference, so there is nothing to drop; the socket's smoltcp sockets
  leave their stacks, or lose the waker, before the socket goes. The
  calls that pass TCP bytes straight across (ADR
  0186) wake the receiving end, and the sending end when a read tells it
  the window, as the segments they stand in for would.
- **A wake costs an atomic add and two loads while nobody waits**: it
  moves the socket's count, wakes the waiters only if some wait, and its
  pollers only if some poll. The waiter counts itself in before it reads
  the count and looks, the wake moves the count before it reads how many
  wait, each sequentially consistent, as `poll`'s pollers and changes are
  ordered (`kernel/radix/tla/Poll.tla`), so no wake-up is lost.
- **What no socket sees wakes every socket**: a change to an interface's
  addresses or link, a bridge or VLAN taken away, a tunnel going up or
  down, calls `net::changed`, which wakes each socket of each stack. These
  are rare.
- An ICMP error a datagram or ping socket hears (ADR 0345) wakes that
  socket.
- **The benchmarks keep it so**: the POSIX network benchmarks run beside
  a process whose threads wait on sockets of their own (two in `recv`,
  one in `accept`, one in `poll`), on Radix and on Linux alike, and the
  BENCHCOUNT lines show any thread they wake.

## Consequences

- A round trip between two programs wakes those two, whatever else waits
  on the network.
- `poll` still has one count for everything polled (docs/design/posix.md):
  a change to a socket that someone polls wakes every poller on the
  machine. A program that polls many sockets while others poll theirs
  pays for that until `poll` waits on each object it looks at; that is
  a separate change.
- The kernel's wait table is one list, which `wake_all` goes through:
  waking a socket's waiters costs a walk of every thread waiting on
  anything. A wake walks it only when a thread waits on that socket.
- The lab's slow watchdog pets while nginx served the OptiPlex
  (docs/deferred.md) came while every request woke every service; the
  lab's next run of that test shows whether this was why.
