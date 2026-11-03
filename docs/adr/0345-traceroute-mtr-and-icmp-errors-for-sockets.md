<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0345. traceroute, mtr and ICMP errors for sockets

- Status: Accepted
- Date: 2026-11-02

## Context

The plan's 12e also lists traceroute and mtr. Both send probes that go
one hop further each time and read what comes back: a router's ICMP
"time exceeded" when a probe's time to live runs out on the way, and
the far machine's "port unreachable" or echo reply when it gets there.
On Linux neither needs privilege: since 2.6 the kernel gives the ICMP
errors about what a UDP or ping socket sent to that socket, which asks
for them with IP_RECVERR and reads them with recvmsg's MSG_ERRQUEUE, a
`struct sock_extended_err` of `<linux/errqueue.h>` and the address of
the router that said it in a control message; and a connected socket
that did not ask learns of the hard errors as ECONNREFUSED. Dmitry
Butskoy's traceroute 2.1, the traceroute of Linux distributions
(GPL-2.0-or-later), sends its default UDP probes and `-I`'s echo
requests that way, and mtr 0.95 (GPL-2.0-only) falls back to it when
its mtr-packet cannot open raw sockets.

Radix's sockets heard none of these errors. smoltcp hands an ICMP error
only to an ICMP socket bound to the UDP port it quotes, and only when
the quote holds the whole datagram, though RFC 792's routers quote an
IP header and 8 bytes of what came after it, and many still do. Its UDP
and ICMP sockets also gave each datagram the socket's hop limit when it
went out rather than when it was sent, so that mtr's probes, a hop
limit each from one socket, went as far as the last one asked. And
mlibc's res_query failed every query, so that mtr could not ask DNS for
a router's AS number.

## Decision

- **The kernel reads ICMP and ICMPv6 errors itself, as each interface
  receives them, and gives them to sockets as Linux does.** radix-net's
  `reports` reads an error in a frame whatever it quotes, as long as
  that holds the header of the UDP datagram or echo request it is
  about, with its checksum checked where the interface checks them:
  RFC 792's "destination unreachable", "time exceeded" and "parameter
  problem", and RFC 4443's "destination unreachable", "packet too
  big", "time exceeded" and "parameter problem". Redirects and source
  quenches stay the stack's, as they never reach a UDP socket on Linux.
  The socket it goes to is the one whose protocol, port (an echo
  request's identifier for a ping socket), IP version, bound address,
  interface and, for UDP, connected peer match it best, as Linux's
  lookup scores them. Each error gives the errno Linux's
  `icmp_err_convert` and `icmpv6_err_convert` give, hard or soft as
  there: "port unreachable" ECONNREFUSED, hard; "time exceeded"
  EHOSTUNREACH, soft; and so on. With IP_RECVERR (IPV6_RECVERR) every
  error is kept, up to 32, with up to 128 bytes of what it quotes, and
  the socket's next call fails with its errno; without, only a hard
  error on a connected socket is told, by the next call. A recv or send
  takes the error first and fails with it once, SO_ERROR takes it, poll
  says POLLERR while one waits, and a read of the queue leaves the next
  one's errno, or none, as Linux's `sock_dequeue_err_skb` does; turning
  the option off throws the queue away.
- **radix-file(2) gains `receive_error` and the socket option
  `receive_errors`** (file interface version 11), and radix-network(2)
  the SocketError structure it fills: the error, its origin, type, code
  and information (the next hop's MTU of a "fragmentation needed" or
  "packet too big", the pointer of a "parameter problem"), the router's
  address and where the datagram went. The kernel's errors gain those
  Linux's mapping needs: host_unreachable, protocol_error, host_down,
  no_network, protocol_unreachable and prohibited. mlibc's Radix code
  turns them into Linux's: IP_RECVERR and IPV6_RECVERR, recvmsg and
  recvfrom with MSG_ERRQUEUE (what the error quotes of the datagram
  past its UDP header, or of the echo request from its ICMP header,
  MSG_TRUNC and MSG_CTRUNC as there), and `<linux/errqueue.h>` with
  `SO_EE_OFFENDER`. IP_TOS and IPV6_TCLASS take 0 alone, the type of
  service every packet has: mtr sets it before each probe.
- **A datagram keeps the hop limit it was sent with.** The smoltcp
  patch carries each UDP datagram's and echo request's own hop limit
  in its metadata, so that probes sent together with different limits
  go as far as each was meant to.
- **traceroute(8) and traceroute6(8) are systools', new Rust**, taking
  Butskoy's options and printing his lines: his probes, UDP to ports
  from 33434 each from a socket of its own, `-U`'s to one port, and
  `-I`'s echo requests through one ping socket (ADR 0336); up to 16 at
  once; his waits (MAX, HERE and NEAR, 5, 3 and 10); and his notes
  (`!H`, `!N`, `!X`, `!F-mtu`...). The methods that need raw sockets
  (`-T`, `-M`) are left out, as Linux leaves them to root. A new
  program, not a port, as dig and host are (ADR 0341): the part of
  Butskoy's that runs without privilege is small, and systools' own
  argument reading, name lookups and sockets are already there.
- **ping(8) says which router's error came back**, as iputils' does:
  "From 10.77.1.1 icmp_seq=1 Time to live exceeded".
- **mtr(8) and mtr-packet(8) are mtr 0.95, as a port**: a program of its
  own under version 2 of the GPL, as the roadmap has it, with the
  University of California's queue.h (BSD-3-Clause). It is built with
  ncurses, IPv6 and the AS lookups of `-y` and `-z`, without GTK, JSON
  output or capabilities; mtr-packet finds no raw socket to open and
  sends its probes through ICMP datagram sockets, reading the routers'
  answers from IP_RECVERR's queue, as it does for a user without
  privilege on Linux, so it is installed as any other command, without
  setuid. Its `-u` probes find no hop that way, on Linux either: mtr
  0.95 reads what the queue gives of a UDP probe as starting with the
  probe's UDP header, which the queue leaves out, here as on Linux; they
  need raw sockets, which Radix gives no program (EPERM).
- **mlibc's res_query asks DNS** (patch 0056): the nameservers of
  /etc/resolv.conf in turn, with the lookups' own queries (ADR 0193),
  for any type and class, without the search list, as glibc's does,
  giving the answer cut to the buffer with TC set or -1 with glibc's
  h_errno. res_init and its kin no longer say they are stubs. And
  gethostbyaddr, which mtr's report names the routers with, looks the
  address up in /etc/hosts and DNS as getnameinfo does (patch 0057),
  where it found nothing.

## Consequences

- traceroute, traceroute6 and mtr run for anyone with the network's send
  right, as on Linux, and print what Linux's print; mtr shows a
  router's name and AS number. The network-trace boot test plays four
  hops on a tunnel, a silent one and one that quotes only 8 bytes among
  them, with a name server, and checks the sockets' errors, the tools'
  lines and ping's.
- Any UDP program learns of a closed port: a connected socket's next
  call fails with ECONNREFUSED, as on Linux, so a client that sent to a
  server not yet up sees it rather than waiting out its timeout.
- TCP sockets do not hear ICMP errors yet: a connection to a machine a
  router says cannot be reached waits for its timeout rather than
  failing with EHOSTUNREACH. Path MTU discovery from "fragmentation
  needed" and "packet too big" is not done either; those errors reach
  the socket alone.
- A packet's type of service cannot be set; a program that sets one
  other than 0 is told ENOPROTOOPT.
- Each UDP and ping socket call looks for an error first, each frame
  is looked at for one, and each datagram keeps its hop limit: a UDP
  datagram there and back on loopback takes 17,794 instructions rather
  than 17,234 (its budget is 19,000), and a TCP connection made, taken
  and closed 57,660 rather than 56,324 (62,000). An error looked for
  and not there takes no locked instruction.
- mtr's security releases are taken as they come, as every port's are.
