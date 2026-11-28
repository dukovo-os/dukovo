<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0727. A window snaps to a side of its screen or a part of one

- Status: Accepted
- Date: 2026-11-28

## Context

Two windows side by side is the arrangement people make most, and by
hand it takes four drags and never quite meets in the middle. The
systems people come from each have a way: a key that puts a window on
a half, a drag to an edge, and lately a small chart of layouts on the
maximize button. People know these by habit, and the roadmap promises
them.

What they leave rough is what happens next. The second window has to
be found and snapped too. And once two windows share the screen, the
place where they meet is the thing the person wants to move, not the
size of either.

## Decision

A screen has a left and a right side, and each side an upper and a
lower part. A window snaps to a side, a half of the screen, or to a
part of one, a quarter. Where the sides meet, and where each side's
parts meet, are the screen's splits.

- **A snapped window is a window with a frame.** It keeps its title
  bar and edges, is told which sides lie against others
  (`xdg_toplevel`'s tiled states) and the size to draw, and remembers
  where it was and how large. It is not maximized.
- **Super with an arrow snaps the window worked in.** Left and right
  put it on that side; a part of a side goes to the same part of the
  other. Up and down go through a side's parts: from a half to a
  quarter, and from a quarter back to the half. Up from the upper part
  maximizes, and down from the lower part has the window as it was. A
  window that is not snapped is maximized by up and minimized by down,
  as before (ADR 0721), and a maximized one snaps from there.
- **A window dragged by its bar to a screen's edge snaps when it is let
  go.** The top edge is the whole screen, a side edge that half, and a
  side edge within 32 pixels of a corner that quarter. While the
  pointer is there the desktop shows what the window will fill: the
  zone shaded, with a line around it. An edge that another screen lies
  beyond snaps nothing; the pointer goes on to that screen.
- **Dragged by its bar, a snapped window is as it was before**, under
  the pointer, as a maximized one is.
- **After a snap the other windows are offered.** If a zone of the
  screen is free and the desktop has windows that are not snapped,
  windowd lists them in the largest free zone. The arrows and Enter, or
  the pointer, choose one: it fills the zone and has the keyboard, and
  what is then left is offered again. Esc, any other key and a press
  beside the list put it away; a key of the window manager's own does
  what it does.
- **The pointer resting on the maximize button is shown the layouts.**
  After half a second a small chart shows under the button: two
  halves, a half beside two quarters, and four quarters. Each zone in
  it is a target; a click snaps the window there. A click on the button
  still maximizes.
- **An edge two snapped windows share moves them both.** The split
  moves with the pointer, never nearer than an eighth of the screen to
  an edge, and every window along it is asked for its new size, paced
  by its program as any resize is (ADR 0721). A window snapped later
  fills its zone up to where the split is now.
- **The splits are a desktop's**, kept for each desktop of each screen
  (ADR 0726), in the middle until an edge is dragged.
- **An edge that lies along the screen's own resizes that window
  alone**, which is then snapped no more.
- **A snapped window follows its screen.** When the screen's area
  changes it fills the same zone of the new area, and carried to
  another screen it fills that zone there (ADR 0725).
- **A window that fills a part of its screen is on that screen
  alone.** The line and the shadow around a frame fall outside it. For
  a snapped window whose zone ends at the screen's edge they would
  show as a stray line along the edge of the screen beside it, so that
  screen draws nothing of the window's frame. A window that fills
  nothing lies on the desktop wherever it is put, across screens too.

## Alternatives

- **A tiling layout**, where every window always has a zone. It is
  another kind of desktop; a stacking one with snapping is what the
  roadmap names, and tiling can be built on the zones later.
- **Thirds and other grids now.** The zones and their splits are one
  small module that can grow them; halves and quarters are what the
  keys people know reach.
- **Snapped as maximized with a smaller size.** The program draws no
  frame's sides and the person has no edge to drag, and the bar cannot
  tell a snapped window from a maximized one.
- **No offer.** The second window is then found with Alt+Tab and
  snapped by its own key: four more keys for the commonest case.
- **A preview that is the window itself, resized as it is dragged.**
  The program is asked for sizes it never shows, and a slow one lags
  the pointer.

## Consequences

- Two windows side by side are two keys and Enter.
- The splits end with windowd, and with their desktop when it closes.
  A named window keeps the zone it filled (ADR 0729), and fills it up
  to the middle when it shows again.
- A program that will not draw the size of its zone, as one with a
  least width, shows smaller or larger than the zone: its frame follows
  what it draws, as everywhere.
- The keys are fixed, as all of windowd's are (`docs/deferred.md`).
