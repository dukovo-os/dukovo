<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0702. One holder is in front, the front is lent and never taken, and the last picture stays

- Status: Accepted
- Date: 2026-11-24

## Context

Several programs draw on the same screens at different times: the
kernel's log, the text consoles, a desktop, a game that takes the
whole screen. Exactly one of them can decide what shows, and a person
has to be able to trust which.

On Linux a program becomes the one that sets modes by asking, and a
virtual-terminal switch takes that from it; each compositor then sets
every mode again, so a switch blanks the screens for a moment even
when nothing about them differs. When a compositor ends, the console
draws over its last frame at once: a desktop that restarts shows a
black screen and a console's text in between. Keys typed in that gap
go to whatever console is underneath.

Under ADR 0097 the screen had one writer, taken with TAKE and given
back with GIVE_BACK or by ending, and the kernel's log drew again the
moment it was given back.

## Decision

- **Each description of `/devices/display/control` is a holder,** with
  its own pictures, values and the state it last asked for. The layer
  keeps its holders in a stack, up to eight deep, with the kernel's
  log at the bottom. The one on top is in front, and only it changes
  what the screens show.
- **The front is passed, never taken.** Its holder lends it with LEND
  to a holder whose description it has, and gets it back when the
  borrower sends RETURN or ends. No request puts a holder in front of
  another that did not lend to it. The kernel's log is the one
  exception on the way up: it gives way to the first holder that makes
  a change while no holder is in the stack, which is how consoled
  takes the screens at start.
- **A holder that is not in front keeps what it has.** Its pictures
  and its state stay, and it can make pictures and ask what a change
  would do (CHECK_ONLY), so it is ready when it comes back. Its
  CHANGE, PUT and MOVE_POINTER fail with busy.
- **Coming back is one step, made by the layer.** A holder that gets
  the front back, by RETURN or by being lent it again, has its state
  made again at once, every frame drawn whole from its pictures.
  Where that state has the modes that show, no mode is set. If a
  screen of that state went meanwhile nothing is made: the holder was
  told with EVENT_SCREEN and makes a new change.
- **The last picture stays.** What shows changes only when the holder
  in front changes it. A holder that ends in front leaves its last
  picture showing; its lender is told EVENT_BORROWER_ENDED and
  EVENT_FRONT, and nothing is drawn for it until it asks for a change.
  One with no entries is enough, and makes all of its state that
  differs from what shows.
- **consoled waits 5 seconds** after a borrower ended before it draws,
  and lends again inside the wait straight from the old picture, so a
  desktop that restarts shows its old frame and then its new one, with
  no console between. A holder that ends with nobody under it leaves
  its picture for 3 seconds before the kernel's log draws, long enough
  for Spark to start consoled again. A message of the log at its error
  level ends that wait at once, and so does anything that moves the
  log: its card removed, or a screen of its card that comes, goes or
  changes.
- **consoled lends on a socket,** `/system/live/consoled/display`,
  root's, mode 0600. Connecting is the request. consoled opens a new
  description of the control file, lends the front to it and passes
  it over the socket, or answers that the front is lent already.
  Which other programs may connect, and how a desktop says which
  console it is on, are decided with the window server.
- **The kernel takes the front itself only when it stops,** for its
  report. The secure key (ADR 0251) is to be the second case: the
  file `/devices/display/attention` is made now, with its rule that
  one description holds it, and the key that brings its holder to the
  front comes with the input devices.

## Alternatives

- **The front is asked for,** as Linux's is. Then which program draws
  depends on who asked last, and a program that should not have the
  screens only needs the right to open the file. Lending makes the
  order a chain of programs that each chose the next.
- **The kernel's log draws the moment nobody is in front,** as under
  ADR 0097. Every restart of a desktop or of consoled would flash the
  log.
- **The lender's state is made again when its borrower ends,** as it
  is for RETURN. The lender would draw over the picture that is meant
  to stay, before it could decide to wait.

## Consequences

- A switch between holders that use the same modes takes one beat
  and sets no mode. The `display-handover` boot test passes the
  screens between three holders and reads the kernel's count of mode
  sets before and after.
- A person never sees a console they did not switch to because a
  desktop restarted, and never types into one: while the front is
  lent, and for the 5 seconds after its borrower ended, consoled
  reads keys and drops them. Until the kernel sees the secure key
  itself, that includes Ctrl+Alt+Delete.
- A holder that stops answering while in front keeps the screens.
  The way out is the secure key, and a desktop's own watchdog over
  what it lent to; ending the process returns the front.
- A program can hold the control file and never be lent the front. It
  costs its pictures and under 4 KiB of the kernel's memory, and can
  change nothing a person sees.
