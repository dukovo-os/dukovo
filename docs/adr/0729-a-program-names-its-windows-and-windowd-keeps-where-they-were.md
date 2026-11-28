<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0729. A program names its windows, and windowd keeps where they were

- Status: Accepted
- Date: 2026-11-28

## Context

A person who puts the terminal on the left of the second monitor
expects it there the next time. On Wayland a program is told nothing
of where its window is and cannot ask for a place, so the program
cannot remember it; only the window server can. And the window server
cannot tell one window of a program from another: two terminals have
the same application id, and a title changes with what the window
shows.

Release 1.1 also promises that the desktop outlives its window server:
when windowd is started again, the programs connect again and their
windows should be where they were (steps 5 and 6 of the release's
plan). That needs the places kept outside windowd's process.

The Wayland project's own protocol for this, session management, is
young: Smithay does not serve it and few programs speak it. It also
asks more of a program than a name: a session with a life of its own
that the program keeps and restores.

## Decision

A program gives a window a name that lasts longer than the window does,
and windowd keeps a table of where each named window was.

- **`dukovo_session_v1` has one request that matters.**
  `dukovo_session_manager_v1.name_window(toplevel, name)` names a
  window before it first shows: `main`, the terminal of one profile, a
  document by its path's hash. The name is at most 255 bytes of UTF-8,
  and with the window's application id it is the window's key. The
  protocol is spoken from a table in `window-protocols`, written by
  hand and held to its XML by a test, as the title bar's is (ADR 0720).
- **A key is one window's at a time.** A second window that asks for a
  name a window of its application has is not named, and is placed as
  any new window.
- **windowd keeps a place for each key**: the screen's name and its
  size, the corner and the size of the window's frame there while it
  filled nothing, what of the screen it filled (nothing, all of it, or
  a zone), its desktop, and whether it was minimized. A window that is
  full screen, or away from its own screen while that is unplugged, is
  not noted: its place before stays.
- **The table is a file of the person's**,
  `$HOME/.config/windowd-session.json`, written whole beside itself and
  renamed over, a second after the windows last changed and when the
  desktop ends. It holds at most 256 windows; the one changed longest
  ago goes first. A file that cannot be read is said on the standard
  error and taken as empty.
- **A window that shows with a key the table has shows there**: on
  that screen, at that corner and size, filling what it filled. Its
  program is asked for the size and the shape in the window's first
  configure, so that the first frame it draws is the one that shows. If
  the screen is not there, the window shows as far across and down the
  screen the pointer is on, and goes to its own when that is back
  (ADR 0725).
- **It shows on the desktop that shows, and never minimized.** The
  person who starts a program looks for its window where they are.
  The desktop and the minimized state are in the table for the
  programs that connect again after windowd was started again, whose
  windows are to be exactly as they were: that is the session's, in
  step 5.
- **Rows of desktops and the splits are not kept.** A snapped window
  fills its zone up to the middle when it shows again.

## Alternatives

- **Places by application id and title.** Two windows of one program
  take each other's places, and a browser's title is the page's.
- **The program told its place, to keep it itself.** It is what X had
  and what Wayland left out on purpose: a program that knows where it
  is can cover what it likes.
- **The Wayland project's session management alone.** It is the
  protocol to offer once programs speak it, beside this one and from
  the same table.
- **The table in windowd's memory, handed to its successor.** It does
  not outlive a crash, which is when it is wanted.
- **A known window back on its desktop, and minimized if it was.** The
  window a person just asked for would open out of sight. It is right
  for a session put back as a whole, and wrong for one program started
  by hand.

## Consequences

- A program that names its windows has them where the person left
  them, across its own runs and windowd's.
- A program that names none is placed as new each time.
- The table has names that programs chose. It is the person's own
  file, written for nobody else to read, and a program can learn
  nothing from it through windowd: the protocol has no request that
  reads.
- Putting a whole session back, with every window on its desktop and
  minimized as it was, is in `docs/deferred.md` until the session
  outlives windowd.
