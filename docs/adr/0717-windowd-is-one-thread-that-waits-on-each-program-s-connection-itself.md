<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0717. windowd is one thread that waits on each program's connection itself

- Status: Accepted
- Date: 2026-11-27

## Context

A window server waits for many things: the display layer's events, each
keyboard and pointer, the programs that connect and each one that is
connected, the services it has its screens and devices from, a signal,
and the moment the next frame is due (ADR 0714). What it does between
two waits has to be short, because everything a person sees and types
goes through it.

Smithay's examples wait with calloop, an event loop that registers
every source with epoll on Linux and kqueue on the BSDs. wayland-backend
does the same for the programs' connections: it puts them all in one
epoll or kqueue of its own and gives the compositor that one descriptor
to wait on. Radix has poll(2) and a queue of its own, and neither of
those two.

A program can also hurt a compositor without breaking the protocol.
Its pixels are in memory it shares with the compositor, as a file; if
it makes the file shorter, the compositor's next read past the new end
is a fault in the compositor.

## Decision

- **windowd is one thread with one wait**: poll(2) on the pipe its
  signal handlers write to, the display layer's file, consoled's and
  inputd's connections, the socket programs connect to, each device's
  file and each program's connection, with the time to the next frame
  as its limit. Nothing between two waits waits itself: every
  descriptor is read without blocking, and a change of the screens
  does not wait for its beat.
- **windowd keeps a copy of each program's descriptor to wait on, and
  lets the program that wrote speak** by its id. wayland-backend is
  patched for Radix to register a connection with nothing, to dispatch
  one client or each in turn, and to give out a descriptor that is
  never ready where it gave its epoll's (ADR 0712). When a program is
  gone, windowd closes its copy, so that a program it disconnected
  reads the end of its connection.
- **At most 256 programs and 64 devices.** One more program is turned
  away, and one more device is left unread.
- **A program that cuts its pixels short is disconnected.** Radix sends
  SIGBUS, with the address, for a page past the end of a mapped file,
  which Smithay's guard for shared memory catches: it maps zeros there
  and marks the pool, and the program is sent the protocol's error. The
  frame that could not be drawn is drawn again without the program
  (ADR 0714).
- **The signals that end the desktop, and a child's end, write a byte
  to a pipe** that the wait reads. SIGTERM, SIGHUP and SIGINT end the
  desktop as Ctrl+Alt+Backspace does; SIGCHLD has windowd look whether
  its command ended.

## Alternatives

- **calloop with its poll backend**, and each source registered in it.
  A second list of what is waited on, callbacks that each need the
  whole state, and the same poll(2) underneath: the wait above is a
  hundred lines that say in one place what windowd waits for.
- **epoll for Radix**, in the C library over the kernel's queue. That
  is a port's worth of work with its own edge cases, for a list of a
  few dozen descriptors that poll(2) reads in microseconds. The
  kernel's queue stays for when a measurement asks for it
  (`docs/deferred.md`).
- **A thread for each program, or for input.** Locks around the windows
  and the seat, and no fewer waits: the compositor's state changes with
  every event, so the threads would take turns at one lock.
- **Copying a program's pixels at each commit**, so that nothing is
  read later. A copy of every window's every frame, to defend against
  what a signal handler defends against for nothing.

## Consequences

- What windowd waits for is one list in one function, and every source
  is handled in the order windowd chooses: signals, the display layer,
  the services, new programs, devices, programs.
- A wait builds its list anew: a few dozen entries for a desktop of a
  few dozen programs.
- A program that misbehaves, by the protocol or by its memory, loses
  its windows and nobody else's. windowd's tests and both desktop boot
  tests cut a window's pixels short under it.
- wayland-backend's patch has to be carried to each of its releases
  until Radix has what it asks for.
