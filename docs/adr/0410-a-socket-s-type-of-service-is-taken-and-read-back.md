<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0410. a socket's type of service is taken and read back

- Status: Accepted
- Date: 2026-11-07

## Context

ADR 0345 decided that IP_TOS and IPV6_TCLASS take 0 alone: the type of
service every packet goes out with. Its consequence was that "a packet's
type of service cannot be set; a program that sets one other than 0 is
told ENOPROTOOPT." mlibc's `inet_setsockopt` enforced it, returning
ENOPROTOOPT for any nonzero value (and 0, or -1 on IPV6_TCLASS, as a
no-op).

A program that sets a type of service and carries on regardless now
pays for that with noise. OpenSSH sets IP_TOS on every connection (0x48
for an interactive session, 0xB8 — 184 — before it), and when the call
fails it warns and keeps going. So every `ssh` to or from a Dukovo
machine prints, three times, `setsockopt socket 3 IP_TOS 184: Protocol
not available`, which Linux never shows because there the call succeeds.
Other programs that shape their own traffic (lftp, some mail clients)
warn the same way. The connection works; the warning does not, and it is
not what a user coming from Linux expects. The lab's `ssh/keys` test
caught it as a finding.

The reason ADR 0345 gave for a fixed type of service still holds: the
kernel chooses what is on the wire, and a stream socket's smoltcp part
carries no per-socket DSCP, so honouring the value would mean patching
the stack. What does not hold is rejecting the call: Linux takes the
value and lets a program read it back, whether or not the kernel acts on
it, and a program that only sets it (as ssh does) never knows the
difference.

## Decision

- **setsockopt for IP_TOS and IPV6_TCLASS is taken for any value and read
  back by getsockopt, as on Linux.** IPV6_TCLASS takes -1 for the default
  (read back as 0) and 0 to 255; IP_TOS takes any int and keeps its low
  byte. The value is kept per socket in a new socket option,
  `type_of_service` (radix-file(2), SocketOption 24), 0 to begin with.
- **Every packet still goes out with a type of service of 0.** The stored
  value is not put on the wire: ADR 0345's choice that the kernel chooses
  what is on the wire, and its reasons for a fixed 0, stand. Honouring a
  socket's type of service on the wire is left for if a program ever needs
  it, and would be its own change to the smoltcp patch.
- **This amends ADR 0345's consequence** "a packet's type of service
  cannot be set; a program that sets one other than 0 is told
  ENOPROTOOPT": it can now be set and read back, though not put on the
  wire.

## Consequences

- ssh, and other programs that set a type of service and carry on, no
  longer warn on each connection; as far as they can see, Dukovo behaves
  as Linux does. The `ssh/keys` lab finding is resolved.
- A program that reads back its own setting with getsockopt sees the
  value it set, as on Linux.
- The wire is unchanged: every packet still carries a type of service of
  0, so DSCP-based routing and queueing downstream see what they saw
  before, and the privacy property of a fixed type of service is kept.
- The `sockets` test sets a socket's type of service and reads it back.
