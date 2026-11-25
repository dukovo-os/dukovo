<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0703. A pipe's beat is the kernel's clock where a card has none, and its event carries the deadline

- Status: Accepted
- Date: 2026-11-24

## Context

A window server draws once for each frame a screen shows, and has to
know when the next one starts: too early and what it shows is stale,
too late and the frame is missed. Hardware that scans a picture out
tells its driver of each frame. The two cards of release 1.1 tell
nothing: the firmware's framebuffer (ADR 0705) is memory, and a
virtio graphics card (ADR 0704) shows what it is sent when it is
sent.

On Linux a card like these gets a pretended frame from a timer or
none at all, and each compositor works out for itself how long before
a frame it must hand its picture over, from the times of earlier
frames. A change that arrives while another waits fails as busy.

A display that wakes the processor sixty times a second to show a
picture that does not change is the largest single cost of an idle
desktop.

## Decision

- **Each pipe has a beat:** the moment a new frame starts. Beats are
  numbered, and a pipe's numbers only grow. A card whose hardware
  tells of each frame keeps its own. For a card that has none the
  kernel's clock keeps it, at the rate of the pipe's mode: 60 a second
  on the firmware's framebuffer, whose monitor's rate nobody knows,
  and the mode's own on virtio.
- **A beat's time is worked out from its number and the rate,** never
  added up from a rounded period, since few rates divide a second
  evenly: at 59.94 Hz a frame lasts 16,683,350.02 ns. A pipe's beat
  does not drift from the clock however long it runs. A new mode at
  the same rate keeps the beat; a new rate starts one at the next
  number.
- **A change shows at its pipes' next beat.** EVENT_SHOWN then carries
  the beat's time in nanoseconds, its number, and the number the
  caller gave the change. The time is the monotonic clock's, the one
  that stamps input events, so the delay from a key to a picture is
  one subtraction.
- **The event carries the deadline:** the last moment a change can be
  asked for and still show at the beat after this one. A card says
  how long before a beat it needs a change, and the layer takes that
  off; a compositor reads the answer and does not guess.
- **ASK_BEAT asks for one beat's event** with no change, for a
  compositor that paces windows which redraw themselves.
- **The beat runs only while something waits for it.** Each card has
  one kernel thread, which sleeps until the earliest beat anything
  waits for and otherwise until it is woken. With nothing to show and
  nobody asking, a screen has no timer, no interrupt and no wake-up.
- **While a change waits, another of the same pipe fails with busy,**
  unless it carries NEWEST_WINS: then it takes the waiting one's
  place, which ends with EVENT_REPLACED, and only the newest shows.
  NOW does not wait for the beat at all: the picture shows at once
  and may tear, which is what a game that wants the least delay asks
  for.
- **A change that waits comes back at the beat,** or after one second
  with timed_out if its card never showed it. One with NO_WAIT comes
  back when the check has passed.
- **The pointer does not wait.** MOVE_POINTER is carried out when it
  is asked, whatever waits for the beat, so the pointer keeps moving
  when a frame is slow.

## Alternatives

- **A timer that runs with the pipe,** as a card's own frame
  interrupt does. Sixty wake-ups a second for each screen, for as
  long as the machine is on, to tell nobody anything.
- **No beat on cards that have none:** every change shows at once.
  The window server would have nothing to pace itself or its windows
  by on exactly the cards release 1.1 runs on, and would be written
  again around a beat for release 1.10.
- **The compositor works the deadline out,** as on Linux. The layer
  knows the beat exactly and the card's need exactly; a compositor
  that measures them gets them late and with noise.

## Consequences

- An idle desktop is woken by its display not at all. The
  `display-beat` boot test counts the wake-ups of both cards' threads
  over two seconds of a still screen and takes none; it also holds
  forty beats in a row to their times to the nanosecond.
- On these two cards the beat is the kernel's clock and not the
  monitor's. It paces what draws, and bounds the work at one frame a
  beat, but a frame copied to a firmware framebuffer can still be
  caught half-way by the monitor. The cards of release 1.10 tell the
  layer of each real frame, and nothing above the layer changes.
- A program that counts frames sees each number once, across modes.
- A change that is replaced is told so, and its program can let go of
  whatever it kept for it.
