<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0339. Tunnels are IP links whose packets a program carries

- Status: Accepted
- Date: 2026-11-02

## Context

The plan's 12d asks for WireGuard. Linux carries it in the kernel, and
wireguard-go, boringtun and the BSDs' userspace ports carry it through a
tun device: an interface whose IP packets go to a program, which seals
them and sends them by UDP, and whose packets come from it. The kernel's
interfaces were cards, the loopback, bridges and VLANs (ADR 0333), all
Ethernet but the loopback; radix-net's stacks take either medium, as
smoltcp does. Putting WireGuard's cryptography in the kernel would add
its keys and handshakes to what the kernel must get right; a program can
be restarted, confined, and built from a crate the WireGuard world
already runs.

## Decision

- **A tunnel is a new kind of link, `tunnel`**, made as a bridge is,
  through the network capability's control right (radix-network(2)'s
  `add_link`, version 6), and taken away by `remove_link`. Its stack is
  an IP one (smoltcp's `Medium::Ip`): no Ethernet header, MAC address,
  ARP or neighbour discovery, its MTU WireGuard's 1420, which leaves room
  in a 1500-byte frame for WireGuard's 32 bytes, UDP's 8 and IPv6's 40.
- **`open_tunnel` (method 9) puts the program's end of it in a slot**,
  as Linux's TUNSETIFF gives /dev/net/tun's file an interface: a read
  takes the next IP packet its stack sent, waiting unless the file is
  non-blocking; a write gives its stack one IPv4 or IPv6 packet of at
  most 1518 bytes, and never waits. Each way is a queue of 64 packets
  under a lock of its own, which a stack's lock may be held under; a
  packet that finds its queue full is lost and counted, as a card's
  frame is. A poll finds the file readable while a packet waits and
  always writable.
- **One program at a time has a tunnel open**; a second gets EBUSY.
  While none has, its link is down and what its stack sends is lost, as
  on a wire with nothing at the other end; the tunnel stays with its
  addresses, so that its program, restarted, carries on. Once taken
  away, its file is readable and in error, and reads and writes fail
  with ENXIO.
- **The firewall's hooks see a tunnel's packets** (ADR 0334): radix-net
  runs the admit hook and flow tracking on every link with hooks, and
  finds the IP header at the packet's start on an IP link. The Ethernet
  sift runs only on Ethernet links.
- **An IP link's IPv6 addresses are its own at once**: no link-local
  address and no duplicate address detection, as Linux gives a NOARP
  interface such as WireGuard's.
- **/kernel/class/net shows a tunnel as Linux shows a tun**: type 65534
  (ARPHRD_NONE), flags POINTOPOINT and NOARP, no address, operstate
  `unknown` while open and `down` while not, and `tunnel` in its link
  file; ip(8) lists it as `link/none` with those flags.

## Consequences

- wgd(8) (ADR 0340) carries WireGuard over tunnels; any other userspace
  VPN could, with the control right. netd(8) leaves tunnels alone.
- A tunnel's packets cost two copies each way through the queues, which
  a program on Linux's tun pays too. Offloads (GSO, multi-queue tun)
  are later work, should the network bench show the need.
- The kernel still routes by subnet, the first interface whose subnet
  holds the destination or else the first with a router: packets for a
  tunnel's peers beyond its subnet (AllowedIPs of 0.0.0.0/0) go by the
  default route until the routing table comes.
- ip(8)'s `ip link add type wireguard` is not wg-quick's: it says to
  write a file for wgd, which makes the interface.
