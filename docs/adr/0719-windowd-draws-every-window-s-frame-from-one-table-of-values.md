<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0719. windowd draws every window's frame, from one table of values

- Status: Accepted
- Date: 2026-11-28

## Context

Since ADR 0712 a program's window shows on the desktop as the program
drew it, with nothing around it: no title, nothing to move it by, no
way to close it that the program did not draw itself.

Wayland leaves the frame to whoever wants it. Most toolkits draw their
own, each its own way, and ask the compositor to stay out
(xdg-decoration). A desktop then has as many kinds of title bar as it
has toolkits, the buttons are where each toolkit puts them, and a
program that hangs cannot be moved or closed, because the bar that
would do it is the program's.

The desktop's look is one design: a title bar with the window's icon
and title at the left and three buttons at the right, a frame with
round corners, a hard shadow under it. The first look is named Basic;
others follow in a later step of release 1.1 and read their values from
a theme's file.

The boot tests compare what a screen shows with what a test program
says it shows, pixel for pixel (ADR 0700). A frame around the window is
part of what the screen shows.

## Decision

- **Every window has windowd's frame**, whatever its program draws or
  asks for: the title bar above the window's own pixels, 5 pixels of
  frame at the left, the right and the bottom, and outside the box a
  line and the shadow. A maximized window has the title bar alone. A
  program that draws a title bar of its own has both, until it uses the
  title bar's region (ADR 0720).
- **The frame is a library, `window-frame`**, with no unsafe code and
  nothing of Smithay or the display layer in it:
  - `Theme` is the one table: every size and every colour of the frame,
    the buttons and the panels. `Theme::basic()` is the Basic look.
    Nothing else in the library or in windowd has a size or a colour of
    the frame in it.
  - `Layout::of` says, for a window's own size and shape, where the
    frame's box, the window's surface, the icon, the title and the
    buttons are, and `Layout::part_at` which part a point is in: the
    title bar, a button, an edge or a corner (which reach 8 pixels
    outside the box, so that a 5 pixel frame is easy to take hold of),
    or the window's own content.
  - `draw::frame` draws the frame for a `Look` (the title, whether the
    window is the one the person works in, the button under the
    pointer, the button held down) into a `Canvas`, which is any
    rectangle of the frame's plane with memory for its pixels.
  - `panel` places and draws the boxes windowd shows over the windows:
    the window menu and the list of windows (ADR 0721).
- **The frame is drawn in strips**, the rectangles `Layout::strips`
  names: the title bar's, the two sides', and the bottom's with the
  shadow. windowd keeps each as a picture of the renderer's and draws
  a strip again only when what it shows changed. A change of the title
  or of a button's look draws the title bar's strip alone.
- **The window's own surface shows cut to the inside of the frame**,
  behind the strips, which are see-through there. Nothing a program
  draws reaches over the frame.
- **The frame is whole pixels and plain arithmetic.** Fills change from
  row to row by a table of stops; a round corner and a stroke are
  reckoned by how much of each pixel they cover, sampled four times
  across and down. The same call gives the same bytes on every
  machine.
- **The title is in the console's font**, Spleen 8x16 (ADR 0015), until
  the desktop has a text stack. A title that does not fit ends in
  three dots.
- **The boot tests draw the frame with the same library.** The test
  program says where a window's own pixels are, its title and its
  state (`frame`, in the lines of ADR 0700), and the build tool draws
  the frame around them over what the screen showed, as the renderer
  lays one pixel over another (`shown_over`). windowd's own tests
  compare the same way.

## Alternatives

- **Each program draws its own frame**, as GNOME has it. No program can
  be moved or closed when it hangs, the desktop has no one look, and
  every toolkit has to be taught the design.
- **A frame only for programs that ask for one** (xdg-decoration as
  written). Two kinds of window on one desktop, and which a program is
  depends on its toolkit's default.
- **The frame as surfaces of a helper program**, as some compositors
  theme theirs. Another process between a press on a title bar and the
  window's moving, and a frame that can fall behind its window.
- **Vector drawing with a library** (tiny-skia, cairo). A frame is
  boxes, four round corners and three marks; a general rasterizer would
  be most of windowd's size, and its output would be its own to change
  between versions, which pixel-for-pixel tests cannot follow.
- **One picture for the whole frame**, the size of the window. Most of
  it would be the see-through middle, drawn and compared at every
  change; the strips are a few thousand pixels.

## Consequences

- A window can always be moved, resized and closed by the same parts
  in the same places.
- A program's own title bar shows under windowd's until the program
  uses the region (ADR 0720). Programs ported to Dukovo are built
  without one.
- Another look is another `Theme`: values, and no code.
- The build tool depends on `window-frame`, so that a change of the
  frame's look changes what the boot tests expect with it.
- Text in the frame is one size of one font, with the characters that
  font has, until the text stack comes (`docs/deferred.md`).
