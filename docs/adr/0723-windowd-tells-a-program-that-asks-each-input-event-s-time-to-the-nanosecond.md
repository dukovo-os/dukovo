<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0723. windowd tells a program that asks each input event's time to the nanosecond

- Status: Accepted
- Date: 2026-11-28

## Context

Every event of an input device has the time the kernel's driver read it
at, in nanoseconds on the monotonic clock (ADR 0706). windowd hands a
key or a motion on to the program whose window has it, and the protocol
those events are sent in has room for milliseconds alone: the driver's
time, cut short.

A millisecond is too coarse for two kinds of program. One reckons how
fast the pointer moves from the times of its motions, as a drawing
program or a game does: a mouse reports up to eight thousand times a
second, and motions an eighth of a millisecond apart carry the same
time or times a whole millisecond apart. The other measures how long an
event was on its way to it, which is what the desktop's own key rows do
(ADR 0724): a key takes some tens of microseconds from the driver to
the program, which no count of milliseconds shows.

Wayland has a protocol for exactly this, `zwp_input_timestamps_v1`: a
program makes a timestamps object for its keyboard, its pointer or its
touch screen, and is then sent an event with the whole time, in
seconds and nanoseconds, just before each event of that device. The
library windowd is built on does not serve it.

## Decision

windowd offers `zwp_input_timestamps_manager_v1`, version 1, and serves
its objects itself (`stamps.rs`).

- **The time is the driver's.** It is the stamp the event came from the
  kernel with, not when windowd read it or sent it on: the event's own
  milliseconds are that time cut short, so the two always agree, and
  a program that measures from it measures windowd's share too.
- **It goes to the program the event may go to, and to no other.**
  Before a key, that is the program with the keys; before a motion, a
  button or the wheel, the program the pointer is on, and the one it is
  about to come onto. When another program's keys went down is not a
  program's to know.
- **A time too many is allowed, a time too few is not.** The protocol
  has a time belong to the next event of its kind, so a time before an
  event that windowd then keeps for itself does no harm, and none is
  sent before a chord of windowd's own that no program is told of.
- **An event windowd makes itself has windowd's time.** The pointer
  coming onto a window that moved under it has no driver's stamp: it
  carries the monotonic clock's time when windowd made it.
- **A program that does not ask is sent nothing.** The objects are per
  program, and a program with none costs one look in an empty list for
  each event.
- **Touch objects are made and are silent.** The desktop has no touch
  screens yet; a program that asks for a touch screen's times is not
  refused, and hears none.

A held key repeats in the program, not in windowd (ADR 0715), so a
repeat has no event and no time.

## Alternatives

- **Milliseconds alone, as before.** The desktop's key row would then
  have to be measured from outside the machine, where nothing knows
  when the driver read the key.
- **A protocol of windowd's own.** One more thing for a program to
  learn that an existing protocol already says, and no Linux toolkit
  would speak it; this one, toolkits and games ask for where it is
  offered.
- **The time windowd read the event.** Simpler, since it needs nothing
  from the driver, and wrong by exactly what a program asks the
  protocol for: the wait before windowd ran.
- **Every time to every program's objects.** The protocol allows a
  time that is followed by no event, and it would save finding the
  program. It would also tell every program when the person types into
  another's window, to the nanosecond.

## Consequences

- A program can time an event's way to it, and `window-test cost` does
  for the desktop's key rows: on Linux, where the compositor measured
  beside does not offer the protocol, the same program reads the same
  stamp from the keyboard's event device instead.
- A program that asks is sent one more small event for each key,
  motion, button and wheel step.
- The protocol is still marked unstable upstream, which is what the
  `z` its names start with says: if a later version changes it, that
  version is offered beside this one.
