<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0418. A stack waits rather than spins while its card has no room

- Status: Accepted
- Date: 2026-11-09

## Context

An interface's thread polls its stack, asks it how long until it has
work again (`Stack::poll_delay`), and sleeps that long or until the
card's interrupt or its driver wakes it. A delay of 0 has it poll again
at once.

The stack said 0 for as long as anything waited to be sent that the card
had no room for. smoltcp asks to be polled now while a socket has data
it may send, and keeps it when the card hands it no slot; the stack's
IPv6 asks to be polled now while an address's duplicate address test is
due, and keeps the test due when the card takes no solicitation. So
while a card's ring stayed full, its thread kept a CPU busy: a ring
whose driver is gone, or hung, or a card that holds what it has to send
while its link is down.

The `userspace-e1000` boot test showed it while nics kills e1000d and
Spark starts it again: net1's thread polled again at once until the new
driver attached, and IPv6 counted each solicitation the card could not
take as a frame lost, 351,661 of them in one run. Had the driver not
come back, the thread would have kept the CPU for good, and a UDP
datagram a program sent then would have done the same as the
solicitation.

Linux does neither. A full queue stops the device's transmit queue, and
the driver wakes it when it frees room; nothing polls in between. A
packet that waits in the queue is not counted as dropped.

## Decision

- **A stack whose card has no room polls again at once for a
  millisecond, then every 10 ms.** `Stack::poll_delay` notes when it
  first says 0 with the card's ring full, and after `FULL_NS` (1 ms)
  says `STUCK_NS` (10 ms) instead, until the card has room again. A card
  that sends frees a slot within a frame's time, a millisecond at
  10 Mb/s, so a ring that fills as a card sends at full speed is filled
  again as soon as before. One that freed no slot for a millisecond is
  not sending, and still has frames to send 10 ms later. The card's
  interrupt, or a driver that attaches it or puts frames on its link,
  wakes the thread sooner, as before; 10 ms is how long a card's thread
  sleeps when nothing is due.
- **A frame the card has no room for is not counted as lost.** IPv6 sends
  its solicitation at the next poll that has room, as smoltcp sends what
  it could not, and `tx_dropped` counts only what the card refused: no
  frame on a card whose frames are all of a frame's length.

## Consequences

- A card whose driver is gone, or that holds what it has to send while
  its link is down, costs its thread a wakeup every 10 ms rather than a
  CPU.
- A ring the card drains sends as fast as before: the stack still polls
  again at once while the card frees a slot each millisecond.
- What waited goes within 10 ms of the card having room, or at once when
  its interrupt or driver wakes the thread.
- The kernel stack's `tests_full` check it with a card whose ring can be
  full: a UDP datagram and an IPv6 address's test each have the stack
  poll at once for a millisecond, then every 10 ms, go as soon as the
  card has room, and are not counted as dropped. Both fail without the
  change.
- nics, in the `userspace-e1000` boot test, now checks that the card
  whose driver it killed lost no frame for want of room, as it checks
  the others.
- The lab's `network/send-with-cable-out` test sends from the machine
  to the lab PC as fast as TCP allows and pulls the cable: the machine's
  CPUs must be busy for under a second of the five it measures.
