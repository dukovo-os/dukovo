<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0333. Bridges and VLANs are kernel links, joined through each stack's hooks

- Status: Accepted
- Date: 2026-11-02

## Context

The roadmap's 12c asks for bridging, VLANs and several interfaces, with
a QEMU test of "two NICs, VLAN tags", done when a bridged pair works.
A server joins virtual machines' and containers' networks to its card
with a bridge, and reaches a tagged network on a trunk port through an
802.1Q VLAN interface. On Linux both are links made through netlink
(`ip link add br0 type bridge`, `ip link add link eth0 name eth0.7 type
vlan id 7`), a port joins a bridge with `ip link set eth0 master br0`,
and a port has no addresses of its own while it is one.

Each of Radix's interfaces has its own smoltcp stack on a thread of its
own (ADR 0180), whose lock is held while it polls its card.

## Decision

- **A bridge or VLAN is an interface with a stack of its own**, made by
  radix-network(2) version 4's `add_link` through the control right,
  joined to a bridge by `set_master` and taken away by `remove_link`.
  ip(8) makes them as iproute2 does, with `link add`, `link set
  master`/`nomaster` and `link del`, and shows them with `-d`;
  `/kernel/class/net/NAME/link` says what an interface is, and a VLAN's
  `iflink` its parent.
- **Frames pass between them through each stack's hooks**: the frames a
  link receives for a VLAN on it or for its bridge are taken out of
  what its stack sees into those interfaces' queues of 64 frames, and
  theirs to send come from its own queue. The queues are taken under one
  lock that a stack's lock may be held under but that never holds one,
  so the interfaces never wait for each other's stacks; an interface
  given a frame is woken once the poll that gave it has let go of its
  stack.
- **A bridge learns** where each machine was heard, 256 machines for 5
  minutes, sends a frame on to that port or floods it to every other,
  and up to its own stack when it is for the bridge or every machine.
  It takes its first port's MAC address. A VLAN tags what it sends in
  software, takes the frames tagged for it untagged, and has its
  parent's MAC address.
- **netd sets bridges and VLANs up as cards, by their names**
  (network(5)), and leaves a bridge's ports alone.
- **Left out: the spanning tree protocol**, VLAN filtering on a
  bridge's ports, and bonding. A removed interface's place is not used
  again until the machine restarts.

## Consequences

- A machine can join virtual machines to its network and reach tagged
  networks, with Linux's commands.
- The `network-pair` boot test bridges a machine's card and a cable to
  a second machine, which gets its addresses through the bridge, and
  checks the frames tagged for VLAN 7 on the cable.
- A loop of bridges floods until it is broken by hand, as on Linux with
  STP off, its default.
