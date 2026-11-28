<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0714. windowd draws a screen's frame where it changed, as late as its beat allows

- Status: Accepted
- Date: 2026-11-27

## Context

windowd draws on the processor (ADR 0712), so every pixel it draws is
time taken from the programs, and every frame it draws for nothing is
power. The display layer gives it what a careful compositor needs: a
change that names the rectangles that differ, an event when a change
shows, and with each beat the last moment at which a change still shows
at the beat after (ADR 0703). Some cards read a program's pages
whenever they like, and others copy them when a change says so
(ADR 0704).

What a person feels of a window server is how long a key or a motion
takes to show. A program draws its frame when it is told the last one
showed; if windowd draws right after the beat, the program's new frame
misses windowd's and waits a whole period. If windowd draws at the last
moment, the program's frame is in it.

## Decision

- **The desktop is on every screen with a monitor**, each in the mode
  its monitor prefers, side by side from left to right in the display
  layer's order. A monitor that comes or goes lays them out again.
- **A screen has pictures of its mode's size that pixman draws in**
  (ADR 0712): one on a card that copies from it when a change says what
  changed, and two that take turns on a card that reads the program's
  pages at any moment, so that such a card never reads a frame that is
  half drawn.
- **Only what changed is drawn.** Smithay's damage tracker gives the
  rectangles that differ from what the picture last held, which for one
  of two pictures is two frames back. The change that shows the frame
  names those rectangles, up to 32, or the one rectangle around them.
  A screen whose content did not change is not drawn.
- **A screen is still, due or waiting.** Still: nothing changed, no
  beat is asked for, and nothing wakes windowd for it. Due: its content
  changed, and its frame is drawn at the deadline the last beat's event
  gave, less the longest of the last four frames' drawing times and
  half a millisecond; at once, if no beat has given a deadline.
  Waiting: a change was asked for, and nothing more is drawn until the
  layer says it shows.
- **A change does not wait** (NO_WAIT). When the layer's event says the
  frame shows, windowd tells the programs with a window on that screen
  that a frame was shown, which is when they draw their next.
- **A frame that cannot be drawn because a program's pixels cannot be
  read is drawn again without that program** (ADR 0717). Three such
  frames in a row end the desktop with the renderer's error: something
  else is wrong.
- **When the desktop comes back to the front** the layer puts its
  pictures back (ADR 0702), and each screen is drawn anew once.
- **A frame drawn anew takes the place of one that still waits**
  (NEWEST_WINS, ADR 0703). windowd asks for no frame while the one
  before it waits, except when it lost sight of what a screen shows:
  the desktop came to the front, a change failed, or screens came or
  went. The screen is then drawn whole at once, and the frame that
  waited, which was of the screen as it was, gives way. So a change
  that fails with busy says one thing only, that the desktop is not in
  front, and windowd draws nothing until the layer says it is again.

## Alternatives

- **Drawing right after each beat.** The simplest loop, and every
  program's frame is a period late.
- **Drawing a fixed time before the deadline.** Too early on a machine
  that draws fast, too late on a slow one or a large screen; the last
  frames' own times follow the machine and the content.
- **Drawing the whole screen each beat.** 60 times the screen's pixels
  a second through the processor for a desktop where a cursor blinks.
- **One picture on every card.** A card that reads the pages at any
  moment would show the part of a frame that is drawn and the part that
  is not.
- **Three pictures**, so that drawing never waits for the beat. windowd
  draws one frame for each beat and no more, so a third picture would
  hold a frame nobody sees.

## Consequences

- A desktop on which nothing moves uses no processor time: windowd
  sleeps in its wait (ADR 0717) with no timer.
- A program that draws when it is told a frame showed has the period,
  less windowd's own drawing time, to be in the next frame.
- A frame that takes longer than the last four did misses its beat and
  shows at the next; the next frames are planned with its time.
- A screen's memory is one or two pictures of its mode, and the
  pointer's picture (ADR 0715). No other copy of a frame exists.
- Programs are told that a frame showed, and not when: presentation-time
  comes with the next step, as do the desktop's rows beside a Linux
  compositor (`docs/deferred.md`). The frame's way through the kernel is
  measured already (ADR 0709).
- Windows keep their places when a monitor is pulled out, and may then
  be where no screen shows (`docs/deferred.md`).
