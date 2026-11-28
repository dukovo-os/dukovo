<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0719. A program shares its window's title bar by a protocol of Dukovo's own

- Status: Accepted
- Date: 2026-11-28

## Context

windowd draws the frame of every window, title bar and all (ADR 0718).
Some programs put their own things where a title bar is: a browser its
tabs, an editor its search box. On the systems they come from they do
it by drawing the whole frame themselves and asking the compositor to
draw none (xdg-decoration's client-side mode). That gives up what ADR
0718 is for: the buttons are the program's to place and to leave out,
and a program that hangs cannot be moved or closed.

What such a program needs is less than the whole frame: a rectangle of
the bar to draw in and to hear the pointer in. Windows has this as a
custom title bar whose caption buttons stay the system's; macOS as a
toolbar that moves into the title bar while the three lights stay.
Wayland has no protocol for it.

Not everyone wants it. A person may want every window to have the same
bar, or one application's tabs out of it.

## Decision

- **A protocol of Dukovo's own, `dukovo_title_bar_v1`**
  (`userspace/programs/window-protocols/protocols/`). A program makes a
  title bar object for a toplevel and names a region of the bar with
  `set_region`. windowd says with the `bar` event whose the bar is,
  `full` or `shared`, with the bar's height and the columns at its two
  ends that stay windowd's.
- **Shared, the bar lies over the top rows of the window's own
  geometry**, which is the bar's height higher for it. windowd draws
  the bar over those rows as it does for every window, without the
  title, and leaves the program's region open: there the program's own
  pixels show and the pointer is the program's. The frame's box is the
  same as under a full bar, so a window does not move or change size
  on the screen when its bar changes hands.
- **What stays windowd's of a shared bar**: the frame's top line, the
  icon, the three buttons, and beside the buttons a strip 96 wide to
  move the window by, which no region reaches. A window can always be
  moved, minimized, maximized and closed by the same parts in the same
  places, whatever its program draws.
- **A change of hands follows the configure sequence.** The `bar` event
  comes before an `xdg_surface.configure`, with a size the bar's rows
  more or fewer where windowd gave the window its size. The frame is
  drawn for the mode the program last drew for: windowd keeps the
  modes it has told with their configures' serials, and takes each
  when the program commits an answer to it. A region is double-buffered
  with the window's surface in the same way.
- **The person decides which applications may share.** By default an
  application that asks does. The window menu of a window whose program
  asks has a line, *Full title bar for this app*, which gives the
  application windowd's whole bar whatever it asks, and gives it back;
  the choice is kept by the application's name in the person's settings
  (ADR 0721), where all applications can be given the full bar at once.
  Every window of the application is told when the choice changes.
- **xdg-decoration is offered and always answered `server_side`.** A
  toolkit that asks is told not to draw a frame of its own. One that
  draws one without asking has two, until it is ported or asks.
- **The protocol's tables are written by hand**, in the crate
  `window-protocols`, and its tests hold them to the XML. The Wayland
  crates' own tool writes `unsafe` code with its tables, for the C
  library that neither windowd nor a program here uses, and the
  desktop's programs have none (docs/coding-standard.md). windowd and
  the test program speak the protocol from the tables through the
  Wayland backend's own calls for an object's requests and events.

## Alternatives

- **Client-side decorations for programs that ask.** The frame is the
  program's, with what follows from that, above.
- **A surface of the program's laid over windowd's bar** (a subsurface
  or a layer). A second surface to place, to cut and to keep in step
  with the first at every resize; the region is the same thing with
  one surface and no step to keep.
- **The region as an `wl_region` of several rectangles.** One rectangle
  is what a row of tabs or a box is; windowd cuts it to what it keeps
  with four comparisons, and the frame's drawing leaves one hole.
- **The shared bar outside the window's geometry**, as extra rows only
  windowd knows of. The program would draw rows that are not part of
  its window as the protocols have it, and every toolkit's reckoning of
  its own size would be off by the bar.
- **No choice for the person.** The one bar on every window is the
  desktop's look; whether a program may change it is not the program's
  alone to say.
- **A generated binding with its `unsafe` allowed in one crate.** An
  exception to a rule that has none outside the kernel's core, for
  three messages.

## Consequences

- A program ported to Dukovo that wants its tabs in the title bar
  speaks one small protocol and draws 36 more rows; one that does not
  know the protocol has the full bar and nothing to do.
- Toolkits have the XML to write their own bindings from. The protocol
  is version 1 and Dukovo's alone; it is offered to no other
  compositor's users.
- The frame of a window whose program is slow to answer a change of
  hands is right the whole time, for the mode the program last drew.
- A program can draw anything in its region, a button that looks like
  windowd's among it. It cannot draw over the real ones, nor under the
  strip the window is moved by.
- The event carries the bar's height and what windowd keeps, so that
  a look with another bar can tell every program that shares.
