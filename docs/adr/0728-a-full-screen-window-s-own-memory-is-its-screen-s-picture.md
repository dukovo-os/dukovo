<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0728. A full-screen window's own memory is its screen's picture

- Status: Accepted
- Date: 2026-11-28

## Context

A game, a film and a presentation fill the screen and want three
things a desktop of windows does not give by itself.

**Every copy counts.** A window's frame is drawn by its program into a
buffer, copied by windowd into the screen's picture, and on most cards
copied once more by the card. When the window is all the screen shows,
windowd's copy changes nothing: the screen's picture is the buffer.

**The frame's timing is the program's business.** It wants to know
when a frame showed, to pace the next and to keep sound with picture.
A game may prefer a frame shown late in a beat, torn, to one that
waits for the next.

**The pointer is the program's.** A view turned with the mouse has no
use for a pointer that stops at the screen's edge. It wants the
mouse's motion as the mouse made it, and the pointer out of the way.

The display layer has what the first two need: a picture can be made
of any memory file (ADR 0700), a change can be checked without being
made, a change can show at once, and each beat's event says when it
showed.

## Decision

- **A window is full screen when its program asks**
  (`xdg_toplevel.set_fullscreen`), on the screen it names or the one it
  is on. It fills the screen with no frame, in front of that screen's
  other windows, and is as it was before when the program asks, at
  Super+Down, or by Restore in the window's menu, which Alt+Space still
  shows. windowd's keys work throughout.
- **windowd draws no frame for a screen that shows one buffer.** When
  the front of a screen is a full-screen window whose content is one
  opaque buffer of the screen's mode, a pixel of it a pixel of the
  screen, with nothing of windowd's over it, on a screen that is not
  turned, windowd makes a picture of the buffer where it lies in the
  program's memory file and has the layer show that. A card that reads
  a picture's pages shows the program's pixels with no copy; any other
  copies them once. On a scaled screen that is a program that draws at
  the screen's scale.
- **The layer is asked first, once for each buffer** (`CHECK_ONLY`). A
  buffer it would not show, or whose file it does not take, is drawn
  from as any other from then on. Anything that breaks the conditions,
  a panel or a second surface, has the next frame drawn as always.
- **A buffer on the screen is not the program's to draw in.** It is
  given back only when the change after the one that showed it has
  shown, since the card may read it until then.
- **windowd keeps each pool's file.** `wl_shm` is served by Smithay's
  own code, which keeps the file to itself; windowd notes the file of
  each new pool and the pool of each new buffer as the requests pass.
- **A frame may show at once when its program says so**
  (`wp_tearing_control_v1`), and only where it costs no other program
  anything: while the buffer is all its screen shows. Composed frames
  always wait for the beat.
- **A program is told when its frame showed**
  (`wp_presentation`): the time of the beat, the time from beat to
  beat, the beat's number, whether it showed at a beat, and whether
  from the program's own memory. What was asked of a commit that never
  showed is answered as discarded.
- **A surface may hold the pointer** (`zwp_pointer_constraints_v1`),
  locked where it is or confined to the surface or a region of it,
  while three things hold: the surface has the pointer, its window is
  the one the person works in, and the window manager has no hold of
  the pointer. The person always has a way out: another window by the
  keys, or the list of windows. A constraint asked for once is gone
  when it stops holding; one asked for to last holds again when the
  person comes back. When a lock ends the pointer goes where the
  program said, if that is on the surface.
- **Every motion of a mouse is told twice**
  (`zwp_relative_pointer_v1`): as far as the pointer went for it,
  after the speed curve, and as the mouse's own counts.

## Alternatives

- **A plane over the desktop for the window's buffer.** It is how a
  card with overlay planes shows video in a window, and no card of this
  release has one. The main plane is what there is; overlays come with
  the graphics cards' drivers.
- **Copying into a picture of the card's and skipping only the
  composing.** It saves the renderer's work and keeps the copy, which
  is most of the cost.
- **Tearing for composed frames.** A torn desktop shows every window
  torn for the sake of one.
- **Letting go of the pointer only when the program does.** A program
  that hangs with the pointer locked takes the machine with it.
- **The program's own pixels shown on a turned screen, or scaled by
  the card.** The card would have to turn or scale them, and none of
  this release does.

## Consequences

- A full-screen program on the virtio card and on any later card that
  reads shared pages is shown with no copy by windowd, and its frames'
  cost is its own drawing and the card's.
- The screen's own pictures stay, unused, while a window is shown
  directly; the next composed frame draws the screen whole.
- A program that gives buffers of another size than the screen's mode,
  or with a viewport, is composed as always. Scaling by the card is for
  cards that have it.
- Frames that vary their rate with the program's (adaptive sync) and
  HDR are with the graphics cards' drivers, in later releases.
- Gamepads are not here; they are with the games' release.
