<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0553. A loopback datagram goes straight to the socket it is for

- Status: Accepted
- Date: 2026-11-20

## Context

On a Dell OptiPlex 7040 (bare metal), `ob bench`'s
`posix_udp_roundtrip_same_cpu` took 7,228 ns against Linux's 6,217 ns,
1.16 times as long. The TCP round trip on one CPU took 4,067 ns, 0.56
times Linux's, because the bytes of a TCP connection on `lo` already go
straight across (ADR 0186). Under CI's instruction count the UDP round
trip took 21,291 instructions, where the TCP one took 9,274.

The difference is the packet. A datagram to a socket on the same machine
went through smoltcp's egress, which made its IP and UDP headers and
checksums, into `lo`'s ring, and back out through ingress, which took
them apart, checked them, and searched the sockets for the one it was
for, though that socket's buffer was in the same kernel all along.

## Decision

- **A UDP datagram to another socket of the loopback interface is
  copied straight into that socket's receive buffer.** The sending call
  finds the socket a packet would reach, the first that accepts the
  destination address and port in the order of the stack's sockets, as
  `process_udp` does, and puts the datagram in its buffer with the
  addresses and ports the packet would have carried. The reader sees the
  same payload, sender and local address as before. The sender may be the
  socket itself.
- **The kernel's patch to smoltcp (ADR 0182) adds two calls to its UDP
  socket.** `takes_across` says whether the socket would accept a
  datagram to an address and port, with the same rules for its bound
  address and its version of IP as a packet has, and `receive_across`
  puts the datagram in the buffer, waking the reader.
- **The interface counts it as a packet sent and received**, of the
  length the packet would have had: 20 bytes of IPv4 header or 40 of
  IPv6, 8 of UDP, and the payload.
- **Only a datagram that would be taken goes across, and only when
  nothing can overtake it.** The destination is one of the interface's
  own addresses, never a group's, a broadcast address or one the
  interface does not have. No packet is in `lo`'s ring, the sender has no
  datagram queued, and no capture watches the interface (ADR 0346).
  Otherwise the datagram goes in a packet, as before.

## Alternatives

- **Make the packet path cheaper.** Each step of egress and ingress could
  be trimmed, but the packet would still be made and taken apart for a
  buffer in the same machine, and the copy of the headers, the checksums
  and the search would remain. TCP's answer, to skip the packet, costs
  less than any version of this.
- **Leave it.** The UDP round trip stays 1.16 times Linux's on bare
  metal, and 2.3 times the instructions of TCP's under CI.

## Consequences

- A datagram on `lo` costs the copy into the receiver's buffer, and not
  the packet. The next run on the OptiPlex and CI's instruction count
  show how much of the gap this closes.
- The interface's counters are as before: one packet sent and one
  received for each datagram, with the bytes it would have had.
- A capture on `lo` still sees packets: a datagram goes in one while a
  capture watches the interface, as TCP's bytes do (ADR 0186).
- No ICMP error comes back for a datagram that goes across, because it
  goes across only when a socket takes it. A datagram for a port no
  socket has goes as a packet, and its sender gets the port-unreachable
  error as before.
- A datagram for a full receive buffer is dropped, as a packet would be,
  and the sender is not told.
- Order is kept. A datagram goes across only when no packet is in the
  ring and the sender has none queued, so that nothing sent earlier can
  arrive after it.
- radix-net's tests check a datagram across, to the sender's own port,
  over IPv6, and to the first of two sockets on one port; and that
  one goes in a packet for no socket, a capture, a queued datagram, a
  group, a broadcast or foreign address, or a socket bound to another
  address or version of IP, and is dropped for a full buffer.
