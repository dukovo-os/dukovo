<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0700. The display layer is cards, screens, pipes, planes and pictures, changed in one checked step

- Status: Accepted
- Date: 2026-11-24

## Context

Since Phase 9 a program reached the screen through `/dev/fb0` (ADR
0097): the one framebuffer the firmware set up, with one writer, rows
written with `write`, and nothing else. It had no second screen, no
mode, no monitor's name, no pointer, and no moment at which a frame
starts, so nothing drawn through it could be free of tearing. That was
enough for the text consoles. Release 1.1 builds a window server on
it, release 1.10 brings drivers for graphics cards, and an interface
that has to change when they come would be written twice, with every
program that draws.

The shape that works on real hardware is known: Linux's mode setting
describes a card as connectors, controllers, planes and framebuffers,
and changes them in one step that is checked first. Its limits are
known too. A change stops at one card. A change that still waits for
its frame makes the next one fail as busy, and the compositor waits. A
compositor has to guess the last moment a frame can be handed over. A
compositor that gets the screens back sets every mode again. And only
buffers a card's driver made can go on a plane, so a window drawn in
ordinary memory is always copied once more.

## Decision

- **Five kinds of thing.** A _card_ is one graphics device and its
  driver. A _screen_ is a socket a monitor plugs into: plugged or not,
  the monitor's name, size and data, its modes. A _pipe_ makes one
  stream of frames in one mode and feeds one screen or, as a mirror,
  several. A _plane_ is one picture a card blends into a pipe's frames:
  the main plane, the pointer's, or an overlay. A _picture_ is pixels
  with a size and a format (ADR 0701). Cards, screens, pipes and planes
  have numbers that are good across the whole layer, not within one
  card, and are never used twice. Each property has a fixed number in
  the ABI (`abi/display.ridl`, radix-display(2)), as sound's requests
  have (ADR 0320).
- **One file.** `/devices/display/control` is root's, mode 0600; each
  description of it is a _holder_ (ADR 0702). Twelve requests go
  through it: LIST and GET say what the layer has; MAKE_BUFFER,
  MAKE_PICTURE, MAKE_BLOB and DROP make and drop a holder's pictures
  and its larger values (a mode, a colour table, HDR details); CHANGE
  is the one checked change; MOVE_POINTER and PUT are the two fast
  paths, for a pointer and for the rows of a picture a card keeps;
  ASK_BEAT asks for one beat's event (ADR 0703); LEND and RETURN pass
  the front. Events are read from the same file as whole records, and
  `poll` finds it readable while one waits, so a program sleeps in one
  place for its screens, sockets, timers and input.
- **One change.** CHANGE takes a list of entries, each a thing, one of
  its properties and a value. The layer builds the state the list asks
  for from the state the holder last made, checks it whole against
  every card it names, and only then makes it. If an entry fails
  nothing changes, and the answer names the entry and the reason. One
  change can span cards; each card makes its part at its own beat.
  Five flags say how: CHECK_ONLY asks whether it would work and makes
  nothing; MAY_BLANK lets it blank a screen, which a new mode does, so
  that no new picture sets a mode by accident; NO_WAIT returns once the
  check has passed and tells the result as an event; NOW shows at once
  and may tear; NEWEST_WINS lets it take the place of a change of the
  same pipe that still waits for the beat, which ends as replaced.
- **What changed is said.** A change names the rectangles of a picture
  that are new (PLANE_CHANGED), and a card copies or sends only those.
  Naming a new picture with no rectangle counts all of it.
- **A driver answers four calls:** what it has, whether a state is
  possible, make this state, and write these rows now. The last serves
  the kernel's log and the panic report. The calls are messages with
  answers and assume nothing of where the driver runs, so a driver
  that is a program can answer them when release 1.10 brings one.
- **The rules are a library.** `radix-display` holds the model, the
  check, the stack of holders, the beat's arithmetic, the events' ring
  and the monitor data's reader. It makes no system call and never
  waits, so its tests run on the build machine; the kernel
  (`radix::display`) holds the lock, the memory, the drivers and one
  thread a card.
- **Limits.** 4 cards; 32 screens a card; 64 pipes; 64 holders; 4,096
  pictures and 64 values a holder; 1,024 entries a change; 16,384
  pixels a side.
- **`/devices/fb0` is gone,** with radix-framebuffer(2). consoled is a
  holder as any other program is (ADR 0100), and the kernel's log and
  panic report reach a screen through the same cards.
- **What the layer has can be read without holding it:**
  `/kernel/class/display/state` (radix-sysfs(5)) is the kernel's own
  account, `hardware screens` prints it, and the web console's
  Hardware page shows it. None of them carries a monitor's serial
  number, since anyone may read them.

## Alternatives

- **More files like `fb0`, one a screen.** Enough for consoles on
  several screens, and nothing more: no mode, no monitor that comes
  and goes, no frame that starts at a known moment. The window server
  would have been written against it and again at release 1.10.
- **Linux's calls as they are.** Programs written for them would run,
  but every limit named above would come with them, with numbers that
  are good within one card only and the two generations of call Linux
  keeps side by side. Each of Linux's concepts has one here, so its
  calls can be offered over this layer if a program needs them. The
  reverse could not be done.
- **The whole layer in a program, with drivers as programs from the
  start.** The two cards of release 1.1 are the ones the kernel needs
  itself, for its log and for a panic, and neither has anything a
  program could do better. The driver's calls are kept as messages so
  that the choice stays open for the cards that do.

## Consequences

- The window server of release 1.1 is written once and runs on the
  firmware's framebuffer (ADR 0705) and on virtio-gpu (ADR 0704), and
  will run unchanged on the cards of release 1.10.
- A layout of several screens is tried with one call (CHECK_ONLY) and
  made with one more, across cards. For a card of sixteen screens the
  layer keeps 19,320 bytes, some 300 for each screen, pipe and plane,
  and checks a change of all sixteen, 80 entries, in about 25 µs on
  the machine that built it, against bars of 1 KiB and 100 µs
  (`kernel/lib/display/tests/kept.rs`).
- The interface names more than the two cards of release 1.1 can do:
  overlay planes, 10-bit and video formats, HDR details, variable
  refresh. The check refuses each with a reason of its own until a
  card offers it.
- A screen can be read by nobody who does not hold a picture of it:
  device memory is never read back, and there is no request that
  gives a screen's pixels.
- ADR 0097's `/dev/fb0` and its three requests are replaced by this
  record. Its keyboard file stays.
