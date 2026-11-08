<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0412. A socket may listen on a port the kernel picked for it

- Status: Accepted
- Date: 2026-11-08

## Context

ADR 0183 makes listening need a socket bound through a network
capability that allows its port: a holder cannot listen on a port
outside the range it was given, whatever library it uses.

Binding to port 0 asks the kernel for a free port, which it picks from
the ephemeral range (49152 to 65535). A program that binds to 0 and then
listens — a server that binds 0 to learn the address it will print, the
POSIX benchmarks' TCP rows, `nc -l 0` — gets a kernel-picked ephemeral
port. That port is almost never in the range a manifest's
`network.listen` grants, since those are the well-known server ports a
program means to offer, below the ephemeral range. So `listen` after a
bind to 0 failed with `missing_rights` (EACCES, "Permission denied") for
every holder whose receive range did not reach the ephemeral ports,
though on Linux anyone who may listen at all may listen on a port the
kernel picked. The lab's `network/any-port` test caught it, and it is
not what a program coming from Linux expects.

The kernel picking the port is the difference. ADR 0183 guards a program
from listening on a well-known port it *named* and was not granted; a
port the *kernel* chose, from its own ephemeral range, is not the
program reaching past its grant. The receive right is what it needs.

## Decision

- **A port the kernel picked for a socket — one bound to port 0 — may be
  listened on if the socket's capability has the receive right, whatever
  its range.** A port a program *names* must still be in the capability's
  range (ADR 0183); only a kernel-picked port is reached this way.
- **A socket with only the send right still cannot listen.** Binding to 0
  needs no right (connecting uses it), so a send-only socket may still
  bind to a kernel-picked port to connect from; a `listen` after it
  fails with `missing_rights`, as before. The boundary that a program
  cannot offer a service on a well-known port it was not granted is
  unchanged.

## Consequences

- `nc -l 0`, servers that bind 0 to learn their own address, and the
  POSIX benchmarks' TCP rows work for any holder with the receive right,
  as on Linux; the `network/any-port` lab finding is resolved.
- A holder with only the send right still cannot listen on any port, and
  no holder can listen on a well-known port it named outside its range,
  so the capability boundary is as strict as before for the ports a
  program chooses.
- The `sockets` test binds a socket to a kernel-picked port through a
  capability whose range stops short of the ephemeral ports and listens
  on it.
