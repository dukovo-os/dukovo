<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0417. Neighbours outlive address changes, and replies wait for them

- Status: Accepted
- Date: 2026-11-09

## Context

The `userspace-e1000` boot test failed now and then in CI: after nics
kills e1000d and Spark starts it again, DHCP works, but net1's TCP echo
stops and nics gives up after 30 seconds without progress. Captures of
the wire under a busy host showed why:

- The kill loses the guest's ACKs for the last few segments, so the
  other end (QEMU's slirp) sends the oldest one again, waiting longer
  each time: after about 1.4, 4.5, 11 and 23 seconds.
- Each time the guest must answer with a duplicate ACK. smoltcp makes
  that answer at once, as the segment comes, and sends it with the token
  the card handed over with the segment. Its next hop was not in the
  neighbour cache, so smoltcp sent an ARP request with the token instead,
  and the ACK was lost. A socket sends again what it did not get out
  once the neighbour answers; an answer made on receipt was never sent
  again.
- The neighbour was missing because smoltcp empties its neighbour cache
  whenever the interface's addresses change, and they change often after
  a link comes back: the DHCP address again, then each IPv6 address the
  stack makes and each router that advertises (ADR 0331). The slirp
  host's entry was gone again by its next resend.

So a resend found the guest unable to answer until one happened to come
while the cache still held the entry; under load that took one backoff
step more than nics allows. Linux has neither problem: it keeps its
neighbours when an address changes, forgetting them only when the link
goes down (`arp_evict_nocarrier`, `ndisc_evict_nocarrier`), and it holds
packets for a neighbour it is asking for and sends them when it answers
(`unres_qlen`).

## Decision

- **An address change forgets only the neighbours on no network the
  interface is still on.** smoltcp's `update_ip_addrs` keeps the others
  (the kernel's smoltcp patch). A neighbour on a network the interface
  left may be another machine when it comes back, so it goes.
- **A stack forgets every neighbour when its card's link goes down.** It
  asks the card whether its link is up once a second at most, as asking
  can cost a register read (`Link::carrier`, `Stack::poll`), and empties
  the cache when it was up and is not: a cable moved to another switch
  may bring the link back on another network, where an address the stack
  knew has another MAC address. A link with no wire of its own, as the
  loopback link, a bridge's, a VLAN's or a tunnel's, is always up.
- **A reply made on receipt waits for its neighbour.** When its next hop
  is not in the cache, smoltcp keeps a copy while it asks, and sends it
  once the answer comes (the kernel's smoltcp patch, `Held`). It keeps
  the last four, enough for the three duplicate ACKs that start the
  other end's fast retransmit, each up to 128 bytes, which an IPv6 ACK
  with SACK blocks or a ping's echo reply fits, for three seconds, as
  long as Linux asks before it gives up. A longer reply is dropped, as
  every one was before; four slots of 128 bytes keep the interface's
  memory small enough to be made in a kernel stack's frame.

Two more faults turned up on the way, and are fixed with these:

- **A TCP socket probes a shut window after its retransmission
  timeout.** A window-0 ACK that acknowledged nothing new while the
  retransmission timer ran started no zero-window probe; at the timeout
  smoltcp went back to sending nothing, and with the window update lost
  the connection waited for ever. It now probes at once, and on with
  the timeout backed off, as Linux's retransmission timer sends its
  probe when the window is shut.
- **e1000d gives the card its receive slots back after it receives,
  not only after it sends.** It wrote the receive ring's tail only when
  it had sent a frame; QEMU's e1000 holds what it receives until the
  driver gives it slots, so a driver that only received stopped
  receiving. `Nic::flush` says it tells the card of both.

## Consequences

- After a link comes back, a resend from the other end is answered at
  once: from the cache when the stack still knows the neighbour, else as
  soon as it answers the ARP request or neighbour solicitation.
- A ping to a machine that forgot this one is answered on the first try.
- Moving a cable to another network still means asking for every
  neighbour again, within a second of the link going down; a link down
  for less than a second may keep them, and their entries go when they
  expire (a minute) or another machine's answer replaces them.
- smoltcp's unit tests in the patch check each change, and fail without
  it: an address change on the neighbour's network keeps it and one off
  it does not, a held echo reply goes when the ARP reply or neighbour
  advertisement comes, only the last four are held, one not answered in
  three seconds is dropped, and a shut window is probed at the timeout.
  The kernel stack's `tests_neighbours` check the same through two
  stacks on a wire: no ARP request after an address change, one after
  the link went down, and a ping to a stack that forgot its peer
  answered.
- The lab's `network/stream-across-cable-pull` test streams from the lab
  PC to the machine across a three-second cable pull, which brings each
  of these into play on real hardware: every byte must arrive.
