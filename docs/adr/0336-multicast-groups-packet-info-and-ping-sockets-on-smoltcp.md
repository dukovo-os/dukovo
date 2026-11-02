<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0336. Multicast groups, packet info and ping sockets, on smoltcp's

- Status: Accepted
- Date: 2026-11-02

## Context

Phase 12's discovery (12c.3) asks for `hostname.local` between two
machines, for what DNS-SD and WS-Discovery find, and for ping. mDNS and
DNS-SD send to and listen on multicast groups (224.0.0.251 and ff02::fb,
port 5353), and WS-Discovery on 239.255.255.250, port 3702; the crate
discoverd uses, mdns-sd, joins the groups on each interface with
IP_ADD_MEMBERSHIP and IPV6_JOIN_GROUP, sets IP_MULTICAST_IF and the
hop limits, shares port 5353 between an IPv4 and an IPv6 socket with
SO_REUSEPORT, as other responders on the machine would, and asks
IP_PKTINFO and IPV6_RECVPKTINFO which interface each datagram came by.
Its interfaces come from getifaddrs. ping must work for anyone, as on
Linux, where SOCK_DGRAM with IPPROTO_ICMP or IPPROTO_ICMPV6 is a ping
socket that needs no privilege.

smoltcp has multicast, behind its `multicast` feature: an interface
joins groups, reports them by IGMPv2 and MLDv2 and answers queries,
and takes datagrams to them. Its UDP sockets did not know of groups: a
datagram to a group came to the first socket on its port, as one to a
unicast address does.

## Decision

- **smoltcp's `multicast` feature**, with 64 groups an interface. radix-net
  counts each group's joins, so that an interface stays in a group while
  any socket has joined it; 48 groups a stack at most, and 20 a socket.
  The patch (kernel/patches/smoltcp-0.14.0.patch) leaves groups one at a
  time, answers MLD's general queries in reports of 8 records, and keeps
  the tables it gathered off the stack, so that the stack's frames stay
  within ADR 0058's budget.
- **BSD's semantics for groups**: a UDP socket takes a group's datagrams
  only if it joined the group on the interface they came by, and a
  datagram to a group or a broadcast address comes to each socket on its
  port that takes it, not only the first. A unicast datagram to a port
  that sockets share comes to one of them.
- **radix-file(2)'s `socket_group`** (method 31) joins or leaves a group
  on an interface, by number or, for 0, the one the group's routes go
  through: ENODEV, as on Linux, when there is no such interface or none
  is routed to. The C library's IP_ADD_MEMBERSHIP and IP_DROP_MEMBERSHIP
  take ip_mreqn, ip_mreq and its interface's address, as Linux does, and
  IPV6_ADD_MEMBERSHIP and IPV6_DROP_MEMBERSHIP their ipv6_mreq; an IPv6
  socket takes an IPv4 group as an IPv4-mapped one. A stream socket has
  no groups: EPROTO, as Linux's say; and neither has a ping socket.
- **The options**: IP_MULTICAST_IF, IP_MULTICAST_TTL (1 unless set; 0
  sends nothing), IP_MULTICAST_LOOP, IPV6_MULTICAST_IF,
  IPV6_MULTICAST_HOPS and IPV6_MULTICAST_LOOP, as ints or as bytes, as
  Linux takes them; SO_REUSEADDR and SO_REUSEPORT on datagram sockets,
  each of which shares a port with the others that set the same one, as
  on Linux, between IPv4 and IPv6 ones too, a port the kernel picks
  included (without Linux's check that SO_REUSEPORT's sockets are one
  user's, as SO_REUSEADDR's need none); and IP_PKTINFO and
  IPV6_RECVPKTINFO. A datagram sent to a group goes out of
  IP_MULTICAST_IF's interface, else the socket's device, else the default
  route's. Multicast loopback, a machine's datagrams to a group coming
  back to its own sockets, is not done: the option is kept, and lo, which
  carries the machine's own datagrams, does it.
- **radix-file(2)'s `receive_message`** (method 32) receives a datagram
  with the interface it came by and the address it was sent to, and says
  whether the socket asked for them; recvmsg gives them as IP_PKTINFO's
  in_pktinfo, whose ipi_spec_dst is the interface's address for a group's
  datagram, or IPV6_PKTINFO's in6_pktinfo, and MSG_CTRUNC without room.
- **Ping sockets**, SocketType `ping` (network.ridl version 5): an echo
  request through one goes out with the socket's identifier, which it
  holds as a datagram socket holds a port, and a checksum made as it
  goes; only echo requests go, and only echo replies come, without their
  IP header, as on Linux. Each is smoltcp's ICMP socket, one on each
  interface a request went out of.
- **getifaddrs** in the C library (patch 0054 takes its sysdeps out of
  mlibc's Linux option), from `/kernel/class/net` (radix-sysfs(5)): each
  interface's IPv4 addresses with their masks and broadcast addresses,
  and its IPv6 ones, a link-local one with its scope; no AF_PACKET
  entries. A card's `flags` say IFF_MULTICAST. The Rust libc crate has
  ifaddrs too, and the if-addrs crate is patched for Radix.

## Consequences

- mdns-sd, and any program that uses multicast as on Linux or the BSDs,
  builds and works unchanged; ping is a program anyone may run.
- The kernel's multicast and its fan-out are tested in radix-net
  (`tests_multicast.rs`), the options, ping sockets, packet info, shared
  ports and getifaddrs in the posix test's `inet` on lo, and multicast
  between machines in the network-discovery boot test.
- Raw sockets, and IGMPv3's and MLDv2's source filters (IP_ADD_SOURCE_MEMBERSHIP),
  are not there; nor is IP_RECVTTL, so ping shows no TTL.
