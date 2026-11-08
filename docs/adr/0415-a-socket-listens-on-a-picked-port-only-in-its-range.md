<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0415. A socket listens on a port the kernel picked only if its range holds it

- Status: Accepted; supersedes [0412](0412-a-socket-may-listen-on-a-port-the-kernel-picked.md)
- Date: 2026-11-08

## Context

ADR 0183 lets a holder of a network capability listen only on the ports
its range holds. ADR 0412 then let a socket bound to port 0 listen on the
port the kernel picked for it through any capability with the receive
right, whatever its range, to make `nc -l 0` work for a signed-in
session.

That did two things it should not have:

- **It widened what a narrow grant allows.** Spark gives a service only
  the ports its manifest asks for: the resolver port 53, wgd 51820,
  discoverd 5353, netd 68 and 546, often with the inbound right. Under
  0412, any of them could open a listener on any port from 49152 to
  65535 by binding to 0, and with the inbound right other machines could
  reach it. A service that is taken over could then offer whatever it
  liked on the network, which ADR 0183 exists to stop. 0412 said the
  boundary was "as strict as before"; it was not.
- **It did not fix `nc -l 0`.** A signed-in session holds two network
  capabilities: the send right on every port first, and the receive
  right from port 1024 up after it (systools' `give_network`). mlibc's
  `bind` tries the capabilities in order and takes the first one that
  allows the call. A bind to port 0 needs no right, so it went through
  the send-only capability, and the `listen` after it was refused under
  0412 just as before: that capability has no receive right at all.

The session's receive capability already holds every port the kernel
picks from (49152 to 65535), so the bind was simply going through the
wrong capability.

## Decision

- **Listening needs the receive right for the port the socket holds,
  whether the program named it or the kernel picked it** (ADR 0183, as it
  was before 0412). A capability whose range stops short of the
  ephemeral ports cannot listen on one.
- **mlibc binds a stream socket to port 0 through a capability with the
  receive right when the process has one,** preferring one whose range
  holds every port the kernel may pick, so the `listen` that usually
  follows is allowed. Only without one does it fall back to the first
  capability that allows the bind, as before, which still connects out.
  This is the same choice mlibc already made for a socket that listens
  without binding first.

## Consequences

- `nc -l 0`, servers that bind to 0 to learn their own address, and the
  POSIX benchmarks' TCP rows work in a signed-in session, as on Linux.
- A service granted only some ports listens on those and no others, port
  0 or not.
- The `sockets` test checks both sides in the kernel: a capability whose
  range stops short of the ephemeral ports cannot listen on a picked
  port, and one whose range holds them can. The POSIX `inet` test shapes
  a process's network as a session's and listens on a picked port through
  mlibc, and checks that a process allowed only ports 9000 to 9009 cannot.
- The lab's `network/any-port` test runs this end to end over SSH.
