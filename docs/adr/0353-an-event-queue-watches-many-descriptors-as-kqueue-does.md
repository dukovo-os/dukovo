<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0353. An event queue watches many descriptors, as kqueue does

- Status: Accepted
- Date: 2026-11-20

## Context

Go's runtime waits for the descriptors its goroutines block on in one
place, its network poller: a goroutine that reads a socket with nothing
to read parks, and the poller wakes it when the socket becomes
readable. Radix's Go had no poller (ADR 0350): every file, pipe and
socket was used in blocking mode, a blocked read held a thread, and
deadlines on pipes did not work.

Radix's only wait on many descriptors was `poll` (ADR 0065). Each call
copies and looks at every descriptor it is given, and a change to any
object polled anywhere wakes every poller in the system. A server with
thousands of idle connections pays for all of them on every call.

Two interfaces do this on other systems: BSD's kqueue (FreeBSD,
OpenBSD, NetBSD and macOS; `kqueue(2)` and `kevent(2)`), and Linux's
epoll. Go has a poller for each. kqueue makes changes and waits in one
call, wakes a waiting thread with an event of the program's own (Go's
`netpollBreak` uses one), and has level-triggered, edge-triggered
(`clear`), oneshot and dispatch watches. epoll has the same model in
three calls, with a separate eventfd for wakes. Phase 18a brings
Linux's epoll, eventfd, timerfd, signalfd and pidfd, which need a
kernel object of this kind under them.

## Decision

- **A kernel object, the event queue** (object kind 25, interface
  `radix-event-queue`, `abi/event_queue.ridl`), which `process.create`
  makes. Its one method, `wait`, is kqueue's `kevent`: it makes a list
  of changes in order, then waits for events, with a timeout or none,
  and optionally with a signal mask, for epoll's `epoll_pwait`. Changes
  need the write right and events the read right.
- **Its rules are kqueue's**, as FreeBSD's and OpenBSD's manuals give
  them: the read, write and user filters; `add`, `delete`, `enable`,
  `disable`, `oneshot`, `clear`, `receipt` and `dispatch`; the user
  event's `trigger` and its and, or and copy of its own flags; and a
  failed change or a receipt reported as an event with `error`, which
  ends the call's changes when there is no room for it, as FreeBSD's
  `kevent` does. The rules are plain data in `radix_posix::watch`, with
  host tests; the kernel's `user::queue` carries them out.
- **The C library has `sys/event.h`**, with `kqueue`, `kevent` and
  `EV_SET`. Its `struct kevent` is the kernel's `QueueEvent`, byte for
  byte, so `kevent` passes the program's arrays to the kernel as they
  are, and turns the kernel's status in an error event into an errno.
- **A watch hangs off what it watches.** Each watch of a description
  counts itself in the `Polled`s that `poll` waits on for that
  description: a pipe's, a socket's, a terminal's. A change there puts
  the watch on its queue's ready ring, at most once, with spin locks
  only and no allocation, and wakes that queue's waiters. `wait` looks
  only at the watches on the ring, in the order they went on, and puts
  a level-triggered watch it reported back for the next call. A wait
  costs what changed, not what is watched, and a change wakes only the
  queues that watch the object.
- **No change is lost between a look and a sleep.** A look clears a
  watch's queued mark before it looks at the description; a change
  marks the watch after it changes the description; a full barrier
  separates each pair, so either the look sees the change or the change
  queues the watch again. A waiter counts itself among the queue's
  waiters before it looks at the ring, and a watch put on the ring wakes
  the wait table's key only if a waiter is counted, each with a full
  barrier between (`WaitCount`), so a change to a queue nobody waits on
  takes no lock of the wait table. A pipe counts its waiters the same
  way, as every read and write of it is a change. The TLA+ model
  `kernel/radix/tla/EventQueue.tla` (run by `ob test tla`) checks that no
  change is lost, that a deleted watch is never reported, that a watch is
  on the ring at most once, and that the ring's room is exactly what its
  watches may need.
- **Locks, outermost first:** the queue's registry (a mutex, held by
  every change and look, so a delete is never overtaken by a report of
  the watch it deletes), a watch's own state, a `Polled`'s list of
  watches, the ready ring, and the wait table. A change to a description
  takes only the last three, so it may run with any object's lock held.
- **Radix differs from kqueue where its descriptors do:**
  - A watch belongs to a description, not to a descriptor number. It
    ends when the description goes (its last descriptor closes, in any
    process), when it is deleted, or when its `ident` is added again for
    another description. Closing one of several descriptors of a
    description leaves it, where the BSDs end it, and `delete` works
    whether or not the descriptor is still open.
  - `fork` shares a queue, as Linux shares an epoll descriptor; the
    BSDs give the child none.
  - Files and directories, which are always ready, cannot be watched
    (EPERM, as epoll says), nor can another event queue (EINVAL).
  - Only the read, write and user filters exist, and `data` of a read
    or write event is 0, where the BSDs give the bytes to read or the
    room to write.
  - A user event with `clear` forgets its flags and value when it is
    reported, as it forgets that it was triggered, so each report
    carries only what came since the one before.
  - `kevent` on a descriptor that is not a queue fails with EBADF.
- **Go's poller on Radix is its kqueue poller**, `netpoll_kqueue.go`,
  on the C library's `kqueue` and `kevent`, called as Go calls every
  C library function on Radix. A watch's `udata` carries the poll
  descriptor tagged with its sequence number, so an event of a watch
  whose description outlived Go's close of it is passed over, as on the
  BSDs. Go's `os` package offers regular files to the poller, as it
  does on Linux; the queue refuses them and `os` uses them in blocking
  mode. Go's `syscall` package gets no `Kqueue` or `Kevent`: no package
  of the standard library needs them, and programs that do take them
  from `golang.org/x/sys/unix`.

## Consequences

- A Go program's goroutines block on pipes, sockets and terminals
  without holding threads, and deadlines on them work: the five `os`
  tests `userspace/go/skips.txt` left out for the poller run again, and
  Go's `net` package can follow.
- The posix boot test's `events` program checks each rule above on the
  running kernel, among them a thousand watched pipes, two hundred TCP
  connections, a listener watched before it listened, many threads
  waiting on one queue, and a queue shared by `fork`. `ob bench` runs a
  wait for one ready pipe among a thousand idle ones, on Radix's event
  queue and on Linux's epoll, next to `poll` on the same pipes.
- The `poll` row on those 1,001 pipes is reported but not gated until
  Radix first reaches parity on it, as ADR 0294 has the `git status`
  row: `poll` looks each descriptor up past the 16 entries of the CPU's
  capability cache, at about nine times Linux's time, which this change
  does not touch. A look-up of many descriptors at once is to follow as
  its own change; the row is then gated like the rest.
- A port whose build finds `kqueue` uses it unless its kqueue code needs
  more than Radix's queue gives: PHP-FPM waits on the queue, and nginx
  (which reads `data`), PostgreSQL, libevent and Python (which name the
  timer, signal or process filters) are built without kqueue and wait
  with `poll`, each as its build script says.
- A watch is charged to the account of the process that adds it. The
  ring grows when a watch is added, so a change to a description never
  allocates; a queue holds a ring of a word or two for each of its
  watches.
- epoll, eventfd, timerfd, signalfd and pidfd (Phase 18a) are to be
  built on this object: epoll's `EPOLLET` is `clear` and `EPOLLONESHOT`
  is `dispatch`, and the timer, signal and process filters they need
  are new filters of the same queue. `docs/deferred.md` lists the rest
  left out: the `data` of read and write events, which nginx's kqueue
  module reads, the BSDs' end of a watch at the close of one
  descriptor, and a queue watching another.

## Alternatives considered

- **epoll's interface first.** Linux programs would use it at once,
  but Go's epoll poller needs eventfd as well, and epoll's three calls
  would be three methods where kqueue's one call is one. Every option
  of epoll's is one of kqueue's, so the C library can give epoll on
  this object, where kqueue on an epoll object would need another
  object for its user events and a call for each change.
- **Go's poller on `poll`.** Go's runtime has none; one would cost
  every wait the whole set of descriptors, and wake on every change in
  the system, the cost this object removes.
- **A watch per descriptor number, as on the BSDs.** Radix's
  descriptors are capability slots, and a description is shared by
  slots and processes the kernel does not tie together; a watch of the
  description needs no hook in every close, and a re-add for another
  description replaces it, which is what Go's poller and the BSDs'
  programs do with a reused number.
- **Notifying a queue through a callback in each object**, as the BSDs'
  `knote` filters do. Each kind of object would carry code of the queue;
  `poll`'s `Polled` already says when any watchable object changes, so
  the queue hangs off it, and every object `poll` waits on can be
  watched.
