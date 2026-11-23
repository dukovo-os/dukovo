<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0612. epoll is the event queue's poll filter, with Linux's rules

- Status: Accepted
- Date: 2026-11-23

## Context

Linux programs wait on many descriptors with epoll: event loops, servers,
language runtimes. Radix's event queue (ADR 0353) already watches many
descriptors with kqueue's rules, and its watches are told of a change by
the same `Polled` lists that `poll` uses (ADR 0354), so the machinery
that tells a waiter which descriptors are ready exists. What epoll adds is
its own view of a watch: one registration per descriptor and open file
description for any set of `poll(2)` events, edge or level triggered,
optionally one-shot, and `EPOLLEXCLUSIVE`, which wakes one of several
queues watching the same description instead of all of them. ADR 0610's
consequences left epoll answering ENOSYS until this step.

## Decision

- **An epoll instance is an event queue.** `epoll_create1` is the
  process's `event_queue` method in the lowest free descriptor; nothing
  new is an object of its own, and the queue's capability, rights and
  limits are those of any queue.
- **A new filter, `poll`** (-32, a number kqueue does not use;
  radix-event-queue(2) version 2), watches a descriptor for the
  `PollEvents` in the change's `fflags` and reports those found, with
  error and hang-up always, as epoll does. Unlike a read or write watch,
  every change that names a `poll` watch gives it its events and its
  `clear` (`EPOLLET`), `dispatch` (`EPOLLONESHOT`: disabled once reported
  until the next change) and `oneshot` anew, as `EPOLL_CTL_MOD` does.
- **A registration is found by descriptor and description.** A `poll`
  watch remembers the description it was added for; a change or delete
  through a descriptor that now names another description finds none
  (ENOENT), and the queue flag `create` makes an add fail with `exists`
  (EEXIST) when the queue holds a watch of that description, as
  `EPOLL_CTL_ADD` does. A watch holds its description weakly, so it keeps
  nothing open and goes when the description does, as Linux's does when
  the last descriptor of the file closes.
- **`exclusive`** (`EPOLLEXCLUSIVE`) is fixed when the watch is added: a
  change to the description queues every non-exclusive watch of it and
  exclusive ones only until one of them is queued where a thread waits,
  so one waiter wakes. It is invalid with a change of a held watch, and
  with events other than readable, writable, error and hang-up.
- **`POLLRDHUP`** (`PollEvents::read_hang_up`, radix-process(2)) is new: a
  stream socket whose reading side has been shut by its peer's
  `shutdown(SHUT_WR)` or close, or by its own `SHUT_RD`. Unix and TCP
  sockets report it; `poll` reports it only when asked, as on Linux.
- **The Linux table** (`user/linux/events.rs`) makes one change per
  `epoll_ctl` and waits through `queue::change_given` and
  `queue::wait_given`, which take the records in kernel memory and make
  the same checks as the queue's `wait`. The errors come in Linux's order:
  the record (EFAULT), both descriptors (EBADF), whether the first is a
  queue and not the second (EINVAL), then the queue's answer (ENOENT,
  EEXIST, and EPERM for a file or directory, which is always ready). A
  signal ends `epoll_wait` with EINTR, never a restart, as Linux's does.
  `EPOLLWAKEUP` is dropped, as for a program without `CAP_BLOCK_SUSPEND`.
  The flags and `struct epoll_event` (12 bytes, packed) are
  `radix_linux::epoll`'s, with host tests.
- **Dukovo's C library** has epoll too (mlibc's `linux-epoll` option over
  `sysdeps/radix/generic/event.cpp`), on the same filter.
- `kernel/radix/tla/EventQueueExclusive.tla` checks the exclusive rule
  with two queues and one change: no change is lost to a queue that is
  woken, and at most one exclusive watch is queued per change.

## Consequences

- One mechanism serves kqueue, epoll and `poll`; a fix to how a change
  reaches a waiter reaches all three.
- An epoll instance cannot watch another epoll instance, which Linux
  allows: EINVAL, as the queue refuses to watch a queue
  (docs/deferred.md).
- Ports' configure scripts find epoll now; each is told it is missing
  until a boot test runs its path over it (`ports/*/build.sh`).
- The `posix` boot test runs `epoll.c` natively, and `linux-programs` runs
  it as a static glibc and musl program; it passes on Linux first.
