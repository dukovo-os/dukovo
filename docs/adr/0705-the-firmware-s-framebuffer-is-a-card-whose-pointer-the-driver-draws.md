<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0705. The firmware's framebuffer is a card whose pointer the driver draws

- Status: Accepted
- Date: 2026-11-24

## Context

Every real machine Dukovo runs on shows its screen through the
framebuffer the firmware set up and the boot loader passed on: one
screen in one mode, as memory to write pixels into, in whatever
layout the firmware chose. Drivers for graphics cards come with
release 1.10, so for nine releases this is the card under the desktop
on every real machine.

It cannot do much. It has no second screen, cannot change its mode,
knows nothing of the monitor, has no moment at which a frame starts,
and is slow to read. On Linux a compositor on such a card draws the
pointer into its own frame, so the pointer moves at the frame rate
and stops when a frame is slow.

The same memory is where the kernel draws its log and, when it stops,
its report (ADR 0015), which nothing a program does may get in the
way of.

## Decision

- **The framebuffer is a card of the layer,** `firmware0`: one
  screen, one pipe, a main plane and a pointer's plane, and the one
  mode the boot loader left. It answers the whole interface (ADR
  0700); what it cannot do, the check refuses with a reason. A
  window server written for it runs on any other card.
- **A frame is a copy.** The main plane's picture is copied into the
  framebuffer: all of it when the picture is new, and otherwise the
  rectangles the change names. The card keeps no memory but the
  screen's own.
- **Any pixel layout.** Programs draw 32-bit colour. The copy
  converts when the firmware chose 24-bit or 16-bit colour or ten
  bits a channel, and takes a row straight from the picture's pages
  when the layouts are the same.
- **The driver draws the pointer.** The pointer's picture, up to 256
  pixels a side, is blended over the main plane's rows on their way
  to the screen, so the picture's own pixels are never altered and
  the framebuffer never read. MOVE_POINTER copies what was under the
  old place back from the picture and draws the new place, at once,
  whatever frame is waiting. The card holds the pointer's image only
  while a pointer shows.
- **A picture the card keeps has no pixels here.** PUT's rows go
  straight to the screen while the picture shows and are dropped
  while it does not (ADR 0701). With nothing to restore from under a
  pointer, a pointer over such a picture is refused.
- **Its beat is the kernel's clock at 60 a second** (ADR 0703): the
  firmware says nothing of the monitor's rate. Nothing waits for the
  monitor, and a change that asks for now is on the screen when the
  call returns.
- **The framebuffer stays the kernel console's.** Every write goes
  through the console, a row at a time under its lock, and writes
  nothing once the log or the kernel's report has the screen: the
  report shows whatever a holder was doing.
- **Only a card's memory is a framebuffer.** A firmware that draws
  its screen only through its own calls has no framebuffer to pass
  on, and a boot loader then names whatever address it left, often
  zero. A framebuffer that lies in memory the firmware's map names as
  other than reserved is not mapped and never drawn on: the machine
  has no firmware card, and its screen comes with a driver.
- **It steps aside.** When a driver takes the device whose memory the
  framebuffer is, this card goes, its screen is told gone as the new
  card's screens come, and what the firmware set up is that driver's
  to change. In release 1.1 that happens at start only, on a virtual
  machine whose VGA card is also a virtio card (ADR 0704).

## Alternatives

- **The pointer drawn by the window server into its frame,** as on
  Linux. It needs no driver work, and makes the one thing a person's
  hand moves directly the thing that waits for every slow frame, on
  every real machine until release 1.10.
- **A second copy of the screen in the kernel,** to restore from under
  the pointer and to redraw from. The picture already is that copy,
  in the program's memory, and the layer holds its pages.
- **No card: programs keep writing rows to a file.** The window
  server would have one interface for real machines and another for
  virtual ones.

## Consequences

- The pointer on a real machine moves when the mouse does, at the
  cost of two copies of the pointer's size for each move.
- A frame costs one copy of what changed. The copy is the kernel's,
  into memory that is usually write-combined and slow: the bar is a
  whole frame of 1920 by 1080 in under 3 ms on the lab's OptiPlex,
  which the lab measures.
- Nothing ties a frame to the monitor's own scan, so a change on this
  card can tear. Only a driver for the hardware can end that.
- A virtual machine under UEFI whose only card is a virtio one is
  such a machine without a framebuffer: its kernel log says so and
  shows on the virtio card's screen (ADR 0704).
- Several monitors, a change of mode and the monitor's name need a
  driver for the card. `hardware screens` and the Hardware page say
  of such a screen that it is in the mode the firmware set.
