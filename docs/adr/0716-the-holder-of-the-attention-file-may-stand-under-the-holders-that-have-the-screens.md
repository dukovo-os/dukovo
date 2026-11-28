<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0716. The holder of the attention file may stand under the holders that have the screens

- Status: Accepted
- Date: 2026-11-27

## Context

The display layer keeps the holders of the screens in a stack: the one
in front shows, a holder lends the front to another, and the front
comes back down when the borrower gives it back or ends (ADR 0702).
consoled is at the bottom: it took the screens from the kernel's log,
and it lends them to a desktop (ADR 0713). It is also the one holder of
the attention file, which Ctrl+Alt+Delete raises (ADR 0708).

consoled is a service, and a service can end: a fault, an update, an
administrator's `service consoled restart`. Its holder then leaves the
stack and the desktop over it goes on, as it should. The consoled that
Spark starts in its place found the front taken, could not show a
console, and ended; Spark tried again, and so on until the desktop was
closed. Meanwhile the attention file had no holder, so the secure key
raised nobody, and when the desktop ended the screens were nobody's.
`docs/deferred.md` kept this for the window server, which is the first
program that keeps the screens for hours.

## Decision

- **Request STAND_UNDER** of the display layer: the holder of the
  attention file takes the place under every holder that is stacked.
  Nothing that shows changes. When the holders over it have given the
  front back or ended, the front comes down to it and it reads
  EVENT_FRONT, as a lender does.
- **It is for the attention file's holder alone** (not_permitted for
  another), **and only where there is someone to stand under**: busy for
  a holder that is stacked already, and while no holder is stacked,
  when the front is the log's and the caller takes it with a change as
  before. too_many if the stack is as deep as it goes.
- **consoled asks it when its first change is refused with busy.** It
  then draws nothing, reads no key, logs that a program has the
  screens, and shows the first console when the front comes to it. If
  the program ended between the two requests, it takes the screens as
  on any start.
- **A raised holder may stand under too**: it stays in front for as
  long as it is raised, and is at the bottom when it lowers itself.

## Alternatives

- **consoled waiting and trying again** until the front is free. The
  attention file would have a holder, but one outside the stack: the
  front would go to nobody when the desktop ends, and consoled would
  find out by asking again and again.
- **The front given to the attention file's holder whenever the stack
  runs empty.** A rule in the kernel for one program's sake, where the
  stack already says who is under whom.
- **The desktop ending when consoled does.** A fault in the program
  that draws text would close every window on the machine.
- **consoled taking the front from the desktop.** The front is lent and
  never taken (ADR 0702); the secure key is the one exception, and it
  is the person's own act.

## Consequences

- A desktop outlives consoled. The `desktop-window` and
  `desktop-firmware` boot tests restart consoled under a window, which
  stays, keeps its keys and draws on; when its program ends the new
  consoled shows its consoles.
- Ctrl+Alt+Delete raises the new consoled over the desktop as it did
  the old one.
- The new consoled did not lend the screens, so the desktop does not
  stand in one of its consoles: Ctrl+Alt+F1 to F12 show none until the
  desktop ends, and another program that asks for the screens is told
  busy (`docs/deferred.md`).
- `abi/display.ridl` has one more request, which the layer's own tests
  and the stack's cover.
