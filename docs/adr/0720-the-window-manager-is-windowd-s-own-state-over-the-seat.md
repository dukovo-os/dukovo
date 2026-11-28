<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0720. The window manager is windowd's own state over the seat

- Status: Accepted
- Date: 2026-11-28

## Context

With a frame on every window (ADR 0718) the pointer and the keys have
work that is windowd's own: moving a window by its title bar, resizing
it by an edge, its three buttons, the window menu, going through the
windows with Alt+Tab. All of it happens between two things Smithay
keeps: the seat, which says which surface hears the pointer and the
keys, and the windows' places.

Smithay's way to take the pointer from the programs is a grab: an
object that is given every motion and button in place of the surface
under the pointer, until it lets go. Its examples have one for a move,
one for each kind of resize, one for a menu. Each grab keeps its own
copy of what it needs, changes the windows from inside the seat's
calls, and has to say for every kind of event what the programs hear
meanwhile.

A resize has a second difficulty. windowd asks the program for a size
and the program draws it when it can. If the frame took the new size
at once, frame and content would differ on the screen until the
program caught up; if windowd asked for a size at every motion of the
pointer, a slow program would have a queue of sizes to draw that the
person has long left behind.

## Decision

- **windowd has one `Hold`**, in `manager.rs`: what it has the pointer
  or the keys for. Nothing, a frame's button held down, a move, a
  resize, the window menu, the list of windows, or a move or a resize
  by the arrow keys. The desktop tells the manager each press, release,
  motion and key first, and then tells Smithay's seat.
- **While a hold is on, no surface is under the pointer** as the
  programs are told, and over a frame none is either. For the menu, the
  list and the arrow keys, every key but a modifier is windowd's as
  well. Modifiers always reach the program that has the keyboard, so
  that it never holds one down that the person has let go.
- **No pointer grab of Smithay's is used for windowd's own work.** The
  seat's default grab goes on: it sends a button to the surface under
  the pointer, which is none.
- **A window is where its frame's box is.** The frame's shape follows
  what the program last drew: maximizing, restoring and resizing ask
  the program with a configure, and the frame changes when the program
  commits what was asked.
- **A resize asks for one size at a time** (`windows/resize.rs`). The
  size the pointer wants is kept; a configure goes out when none is
  out, and the next when the program has committed the one before,
  with the newest size wanted. The sides that are not dragged stay
  where they were, whatever size the program draws. A window is never
  narrower than its title bar's own parts, nor outside the smallest
  and largest sizes its program gave.
- **The order of the windows on the screen is Smithay's `Space`; the
  order of their use is windowd's**, a count in each window that the
  keyboard's coming renews. Alt+Tab goes by use, and so does the list.
  A minimized window is out of the `Space` and still in the list.
- **The keys**: Alt+Tab and Alt+Shift+Tab show the list while Alt is
  held and choose when it is let go; Super pressed and let go alone
  shows the list to stay; Alt+F4 asks the window to close; Alt+Space
  shows its menu; Super with the up arrow maximizes, with the down
  arrow restores a maximized window and minimizes another. They are
  fixed in this release.
- **A program may ask for what the frame does**: `set_maximized`,
  `unset_maximized`, `set_minimized`, and `move`, `resize` and
  `show_window_menu`. For a move or a resize the serial has to be that
  of a button's press the program was told of, with the button still
  down; the program's hold of the pointer ends there and the manager's
  begins. windowd says it can do all three (`wm_capabilities`), and
  answers a request to fill the screen with the window as it is, until
  the next step of release 1.1.
- **A popup that asks for the keys and the pointer has them** by
  Smithay's popup grab, which is the one grab windowd sets. A press
  outside the popup's program ends it, as Smithay has it; so does
  windowd's giving the keyboard to another window, and any key of the
  window manager's. The press that ended it does what it would have
  done: it gives the window under it the keyboard and reaches it.
- **The menu and the list are panels** windowd draws itself
  (`panel.rs`), not surfaces of a program. A list with more windows
  than its screen has room for shows those about the chosen one.

## Alternatives

- **A grab of Smithay's for each kind of hold.** Six objects that each
  borrow the state from inside the seat's calls, where one enum and one
  place that asks it serve. The tests would have to reach a hold
  through the seat.
- **The frame follows the pointer and the program catches up.** The
  frame and the content differ for as long as the program takes, on
  every motion: the window tears at its edge.
- **A configure for every motion.** Hundreds a second to a program that
  draws sixty frames; the last size the person asked for comes after
  all the others.
- **Menus and the list as programs' windows** (a shell program beside
  the compositor). A second program to start, to trust with the list of
  windows and to keep alive, for boxes of a few lines of text.
- **Keys from a settings file now.** The table of keys is small and
  the same as on the systems people come from; a file of keys is for
  when someone asks for another (`docs/deferred.md`).

## Consequences

- What the pointer and the keys do to windows is in one file, and
  tested without a seat: the tests press, move and read the screen.
- Frame and content are never of two sizes on the screen. A program
  that does not answer a configure keeps its window as it was, frame
  and all, and can still be moved, minimized and closed.
- A program that hangs can be moved and minimized, and asked to close;
  ending a program that does not answer is for the desktop's shell, in
  a later release.
- windowd's own keys do not repeat when held: an arrow in the list or
  in a move by the keys acts once for each press (`docs/deferred.md`).
- Windows on a monitor that is pulled, a maximized window's size when
  its screen's size changes, and windows' places across a restart of
  windowd come with the screens' layout in the next step.
