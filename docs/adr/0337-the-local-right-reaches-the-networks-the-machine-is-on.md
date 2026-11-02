<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0337. The local right reaches the networks the machine is on

- Status: Accepted
- Date: 2026-11-02

## Context

A bundle's manifest may ask for `network.local`, "Reach devices on your
local network and see what discovery finds", apart from
`network.outbound`, "Connect to the internet and to services on this
computer" (ADR 0221). Until now a network capability's send right
reached every address alike, so network.local could not be given: it
was among bundle(1)'s permissions not yet given. What is on the local
networks is what a program should not see without asking: printers,
cameras, routers' pages, other people's computers, and what discovery
says of them.

## Decision

- **A right of its own**, `local` (bit 14 of radix-capability(2)'s
  rights, which are 15 now). The kernel tells how far an address is
  (`net::reach`): this machine (a loopback address, or one of its own),
  the networks it is on (an address on one of its interfaces' networks,
  a broadcast, link-local or multicast address), or beyond them,
  through a router. The send right reaches beyond; the local right
  reaches the networks the machine is on; either reaches the machine
  itself. Connecting, sending and joining a group need the right the
  address needs (missing_rights, EACCES in the C library). Listening
  is the receive right's, as it was.
- **Services keep reaching everything**: Spark gives a service whose
  file says `network.outbound = true` both rights, as service.toml(5)
  says the permission is for any address; and a session's network
  capability, which signin gives, has both.
- **Bundles get what they ask for**: bundle(1) gives the send right for
  network.outbound and the local right for network.local, which it asks
  about on first use, from the session's capability; with network.local
  a bundle also sees discoverd's folder, whose socket says what is on
  the local networks (ADR 0338). A bundle that asked for
  network.outbound alone reaches the internet, and this machine, but not
  the printer beside it.

## Consequences

- network.local is given; only processes.debug is not yet.
- A program that derives a capability with the send right alone, as the
  sockets boot test's does, reaches no machine on its own networks; the
  test now derives one with the local right for QEMU's 10.0.2.100.
- The posix test's `inet` checks that the send right alone joins and
  sends to no group and that the local right alone reaches this machine
  and joins groups; netgrant checks what Spark and bundle(1) give; the
  network-discovery boot test checks that a ping socket reaches the
  other machine with the local right and not the send right alone.
- Which addresses are local follows the interfaces: an address becomes
  local when the machine joins its network, and stops being so when it
  leaves.
