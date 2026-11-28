<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0715. windowd reads the devices inputd hands it, and the pointer is the card's plane

- Status: Accepted
- Date: 2026-11-27

## Context

Each input device is a file of events in Linux's codes (ADR 0706), and
inputd hands a program that has the screens a description of each,
which reads only while that program's holder is in front (ADR 0707).
windowd has to turn those events into what Wayland's programs are told:
keys with a layout, a pointer that is somewhere, buttons, a wheel.

Three things have no place yet. A pointer has to move as far as a hand
expects, which is not as far as the mouse counted. The lock lights of a
keyboard were set by consoled through the merged keyboard file, which
is root's; windowd is not root, and the lights have to follow whoever
has the screens. And the pointer's picture has to move when the mouse
does, not when the next frame is drawn.

## Decision

- **windowd asks inputd once**, with a copy of its holder, and reads
  each device's file itself. A report's events are put together up to
  its end (SYN_REPORT) and told as one thing: the keys and buttons in
  their order, then the motion, the place, the wheels. When the desktop
  leaves the front (SYN_PAUSED) every key and button counts as let go,
  and the programs are told so; the same when events were lost
  (SYN_DROPPED).
- **There is one seat**, with every keyboard and pointer. The keyboard
  has the consoles' layout (ADR 0712), and a program repeats a held key
  30 times a second after 250 milliseconds, as the consoles do.
- **A reader of a keyboard that is heard sets the lock lights**:
  request INPUT_LIGHTS on a keyboard's own file lights what its argument
  names on every keyboard, as SET_LEDS does on the merged file. A
  description that is paused is refused with busy, so the lights are
  those of the program in front. windowd lights what the keymap says is
  locked, and consoled lights its own when the consoles are in front
  again.
- **The pointer is the pointer plane of the screen it is on.** windowd
  moves it with MOVE_POINTER as soon as a device says so, and no frame
  is drawn for it. Its picture is windowd's arrow, or the surface the
  program under the pointer set, drawn into the plane's picture when it
  changes; 64 by 64 pixels of it show. On the firmware's card the
  driver draws the plane into what the screen shows (ADR 0705).
- **A mouse's motion goes through one curve.** Up to half a count a
  millisecond a count is a pixel, so that a pixel can be pointed at.
  From three counts a millisecond a count is two and a half pixels.
  Between, the gain rises smoothly. The speed is one report's distance
  over the time since the report before, counted as no less than one
  millisecond and no more than 16. The curve is windowd's: it is the
  one program that moves a pointer.
- **A device that says where it points** points at that share of the
  first screen. A wheel's turn is told in 120ths of a notch and as 15
  pixels a notch.

## Alternatives

- **libinput as a port.** It finds devices through udev and reads them
  with evdev's ioctls, neither of which Radix has, and most of it is
  for devices release 1.1 does not have: touchpads and their gestures
  come with the laptop release, with their own decision.
- **The curve in the kernel or in inputd.** The kernel's events are
  what the device said, which a game wants as it is; inputd reads no
  event (ADR 0707).
- **The lights through the merged keyboard file**, opened for windowd
  by someone. A second descriptor to hand over and to silence when the
  desktop is not in front; a device's own file is silenced already.
- **The pointer drawn into each frame.** It would move at the frame's
  pace and make every motion a frame, where the plane moves in one
  request.
- **Linear motion, a count a pixel.** A 4K screen is then three lifts
  of the mouse wide.

## Consequences

- A key goes from the device's interrupt to the program in front
  through the kernel, windowd and the program's socket, and wakes no
  other process.
- The Caps Lock light is right after every switch between the desktop
  and a console, whichever keyboard is looked at.
- Moving the pointer over a still desktop draws no frame. On the
  firmware's card the kernel redraws the pointer's rectangle.
- The curve has no setting, nor has the repeat: both come with
  windowd's settings file (`docs/deferred.md`).
- A pointer picture larger than the plane is cut off, a program that
  asks for a pointer by name gets the arrow, and a card with no pointer
  plane and no driver that draws one would show no pointer
  (`docs/deferred.md`).
- `abi/input.ridl` has one more request; the others are as they were.
