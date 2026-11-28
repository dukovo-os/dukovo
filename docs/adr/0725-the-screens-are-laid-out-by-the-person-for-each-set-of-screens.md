<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0725. The screens are laid out by the person, for each set of screens

- Status: Accepted
- Date: 2026-11-28

## Context

Until now the desktop showed on every screen in the mode its monitor
prefers, side by side from left to right, one pixel of a program's to
one of the screen's (ADR 0714). That is right for one monitor and wrong
for most desks with two: the second stands to the left or above, a
small dense panel wants its content larger, a monitor stands on its
side.

Four things have to be settled together, because each changes what the
others mean: which mode a screen runs in, how large the desktop is
drawn on it, which way up, and where its area lies among the others.
And screens come and go while the desktop runs. A laptop that is taken
off its dock and put back should find its windows where they were.

The display layer offers planes that scale and turn (ADR 0700), and no
card of release 1.1 has one: the firmware's framebuffer and the virtio
card show a picture as it is.

## Decision

The person sets the screens in `windowd.json` (ADR 0722), under two
keys, and windowd does the rest in software.

- **`screens` has an entry for a screen**, with `mode` (`"1920x1080"`
  or `"1920x1080@60"`), `scale` (1 to 4 in quarters), `turn` (0, 90,
  180 or 270 degrees) and `use` (false leaves the screen dark). The
  entry's key is the screen's name as programs are told it
  (`virtio0/1`) or its monitor's maker and model (`DEL U2720Q`). A
  screen with both has the entry by its name laid over the one by its
  monitor, key by key: a setting follows the monitor to whatever plug
  it is in, unless the plug says otherwise.
- **`layouts` places the screens of one set.** Each layout names
  screens by the same keys and gives each a corner on the desktop. The
  layout whose keys are exactly the screens in use places them; a desk
  and a meeting room each keep their own. With none, the screens are
  side by side in the layer's order, their tops level.
- **A layout must leave the desktop in one piece.** One whose screens
  overlap, or that leaves a screen touching no other, is not used and
  is said on the standard error: the pointer could not reach a screen,
  or a window would show twice.
- **A screen's area is its mode's size, turned, divided by its scale.**
  Programs are told the whole scale at or above it (`wl_output`), the
  exact one (`wp_fractional_scale_v1`) and the turn, and may give a
  buffer of any size for a surface of any other (`wp_viewporter`).
- **windowd scales and turns as it draws.** The renderer draws the
  frame turned and at the screen's scale into the screen's picture, and
  the rectangles that changed are named in the picture's own rows and
  columns. The pointer's picture is turned and scaled with it. Frames,
  panels and the pointer are drawn at one size and magnified: pixel for
  pixel at a whole scale, smoothed at a fractional one.
- **One checked change sets everything.** A reload (SIGUSR1) and a
  monitor plugged in or pulled out both lay the screens out anew, and
  ask the layer for all of it in one change that it checks first.
- **A mistake leaves a screen that shows.** A mode the screen does not
  have is said on the standard error, and the screen shows in the one
  its monitor prefers. Settings that leave no screen in use are said
  too, and the desktop shows on every one: the person is not left with
  no screen to set one right on.
- **A window is on the screen most of it is on, and has a home.** When
  its screen goes, it is carried to the nearest screen that is left, as
  far across and down as it was, filling what it filled, and remembers
  where it came from. When that screen is there again it goes back.
  Moving it by hand forgets the home. A screen whose area changes
  carries its windows the same way.
- **Super+Shift+Left and Right carry the window worked in** to the
  screen before or after its own, from left to right: as far across
  and down, and maximized, snapped or full screen there if it was.

## Alternatives

- **Positions without sets.** One place for each screen, whatever else
  is plugged in. The laptop's panel is then right of a monitor that is
  not there, or the desk's layout is lost at every meeting.
- **Screens keyed by the plug alone.** The same monitor on another
  port, or a dock that numbers its ports anew, is a stranger.
- **A window that goes with its screen and does not come back.** Every
  undocking is then an afternoon of putting windows where they were.
- **Scaling by the card's plane.** It is what the layer's interface
  has, and where a card offers it windowd will ask for it; no card of
  this release does.
- **Whole scales only.** A 27-inch monitor of 2560 by 1440 is too small
  at 1 and too large at 2.

## Consequences

- A person with two monitors says once where they stand, and each set
  of screens they work at keeps its own layout.
- Nothing lists the screens' names and modes for the person yet: they
  are in the layer's report, and a command for it is in
  `docs/deferred.md`. So is a settings program.
- At a fractional scale windowd's own pixels are smoothed, not drawn at
  the scale's own detail (`docs/deferred.md`). Programs that follow the
  fractional scale are sharp.
- A turned screen costs the renderer's work for every frame: a
  full-screen window is shown from its own memory only on a screen that
  is not turned, and from a buffer drawn at the screen's scale
  (ADR 0728).
