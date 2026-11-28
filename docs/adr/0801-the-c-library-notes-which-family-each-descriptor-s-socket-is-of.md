<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0801. The C library notes which family each descriptor's socket is of

- Status: Accepted
- Date: 2026-11-28

## Context

A network socket and a Unix socket are reached by different calls on
Radix: a network socket's `connect` is radix-file(2)'s
`connect_address`, a Unix socket's radix-directory(2)'s
`connect_socket`, and their names, options and datagrams differ too.
The C library's `connect`, `bind`, `listen`, `getsockname`,
`getpeername`, `getsockopt`, `setsockopt`, `accept`'s address and the
receives and sends that take an address asked the kernel first which
family the socket was of, with radix-file(2)'s `socket_option` for its
KIND, which fails for a Unix socket. Linux's calls take any socket, and
need no such question.

With the protections both systems run by default on the lab's Core
i5-6500 (ADR 0790), a call to the kernel costs about a microsecond. In
the lab's `server_php_fpm` row the question was three of the calls of
each request, all of them failing: one in PHP-FPM's `accept`, for the
connection's address, and two in nginx, in `connect` and in its
`getsockopt` of SO_ERROR (ADR 0800 counts the rest).

## Decision

- **The C library keeps a byte for each descriptor**: whether it holds a
  network stream socket, a network datagram socket, anything else (a
  Unix socket or no socket), or nothing known yet. It sits beside the
  descriptor table (ADR 0403) and the directory paths (ADR 0089), and
  as for them only atomic stores change it, so close and dup2 stay
  async-signal-safe.
- **What makes a socket notes its family**: `socket`, `socketpair`, and
  `accept`, whose connection is of the listening socket's family as far
  as the C library knows it. `dup`, `dup2`, `dup3` and `F_DUPFD` copy
  the note with the descriptor, as they copy its directory path.
- **Every new descriptor starts with nothing known**: one the C library
  chooses a slot for, one it closes, and one the kernel fills, as a
  descriptor received over a Unix socket. The first call that needs the
  family of a descriptor with none asks the kernel as before, and notes
  the answer. A descriptor from before `exec` is asked about once.
- **The note is as good as the descriptor table it sits beside.** A
  slot whose capability changes without the C library, which a program
  can do only through the kernel's calls themselves, leaves the note
  out of date as it leaves the table and the directory paths; as for
  them, a program that does so takes its own descriptors' notes in
  hand.

## Alternatives

- **Try the Unix call first and the network one when it fails.** One
  call for a Unix socket, but two for every network socket, whose
  calls are the more common on a server.
- **Have the kernel's calls take both families**, a `connect` that
  takes a path or an Address. That joins two interfaces whose rights
  differ (a network capability for one, a directory for the other)
  into calls that check one or the other, for what one byte in the C
  library saves.
- **Have `socket_state` say the family.** One call where there were two,
  where the note makes none.

## Consequences

- A connect, getsockopt or accept with an address on a socket the C
  library made or accepted asks the kernel nothing first: a PHP-FPM
  request through nginx makes three fewer calls.
- Each process has a byte for each descriptor it may have, in memory
  that takes a page only once a descriptor in its range is noted.
- `posix/inet` checks that a descriptor whose number a socket of the
  other family takes, a descriptor dup2 puts a socket of either family
  in, copies by `dup` and `F_DUPFD`, an accepted connection and sockets
  of both families received over a Unix socket each give the family and
  type they hold now, and that `connect` and SO_ERROR work on them.
