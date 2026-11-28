desktop(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

desktop - show the desktop, and a program's windows in it

# SYNOPSIS

*desktop* [_command_ [_argument_...]]

*desktop* *--help* | *--version*

# DESCRIPTION

desktop shows the desktop on every screen with a monitor, laid out as
the person's settings say (*windowd.json*(5)), in place of
the console it is typed at, and starts _command_ in it. The windows of
_command_, and of the programs it starts, show on the desktop and take
the keyboard and the pointer. When _command_ ends, the desktop ends and
the console shows again. Without a _command_ the desktop is empty, and
stays until its key ends it.

The desktop is the window server, *windowd*(1), which says what a
program in it is given and how its windows show. A program draws its
windows as a Wayland client: it finds the desktop through the
environment _command_ is started with (*ENVIRONMENT*).

desktop is typed at a console of the machine, by the person signed in
there: *consoled*(8) lends the screens to a program of the console that
shows, and to root's. Typed elsewhere, as over *ssh*(1), it says so and
ends. It runs as the person who typed it and needs no rights of its own:
the screens are lent by consoled and the keyboards and pointers handed
over by *inputd*(8).

# WINDOWS

Every window has a frame the desktop draws: a title bar with an icon,
the window's title and three buttons, and a border. A new window shows
in the middle of the screen the pointer is on, a step down and to the
right of the one before, in front and with the keyboard. A click on a
window brings it to the front and gives it the keyboard.

The pointer works a window by its frame:

- The title bar, held with the left button, moves the window. Some of
  the bar always stays on a screen. A double click on the bar maximizes
  the window, or restores a maximized one; dragging a maximized or a
  snapped window by its bar restores it under the pointer. A window
  let go at an edge of a screen is snapped there (*SNAPPING*).
- An edge or a corner, held with the left button, resizes the window.
  The edges reach a few pixels out from the frame, so that they are
  easy to catch.
- The buttons minimize the window, maximize or restore it, and ask it
  to close. A button acts when the left button is let go on it. The
  pointer resting on the maximize button is shown layouts to pick from
  (*SNAPPING*).
- The right button on the title bar shows the window's menu.

A minimized window shows nowhere and is still in the list of windows
(*KEYS*), which brings it back.

The window's menu has *Restore*, *Move*, *Size*, *Minimize*, *Maximize*
and *Close*. *Move* and *Size* give the arrow keys the window: each
press moves it, or its right and bottom edges, 16 pixels, or one pixel
with Ctrl; *Enter* keeps the change and *Esc* takes it back. A program
may draw a part of its windows' title bars itself, as a browser does
for its tabs; the menu of such a window also has *Full title bar for
this app*, which gives its windows the desktop's whole bar from then on
(*windowd.json*(5)).

A program may show its window full screen: it fills its screen with
no frame, in front of that screen's other windows. *Super+Down*, or
*Restore* in the window's menu, has it as it was.

A program that names its windows has them shown again where they
were: on the same screen, at the same place and size, maximized or
snapped as they were (*windowd*(1), *WINDOWS' PLACES*).

How the screens are laid out, how the title bars are shared out, how
fast the pointer moves, what a double click is and how a held key
repeats are settings (*windowd.json*(5)).

# SNAPPING

A window snaps to a side of its screen, the left or the right half, or
to the upper or the lower part of a side, a quarter (ADR 0727). A
snapped window keeps its frame and remembers the place and the size it
had.

- *Super* with an arrow snaps the window worked in (*KEYS*).
- A window dragged by its title bar snaps where it is let go: at the
  top edge of a screen it is maximized, at the left or the right edge
  it fills that half, and at one of those within 32 pixels of a corner
  that quarter. While the pointer is there the desktop shades what the
  window will fill. An edge another screen lies beyond snaps nothing.
- The pointer resting half a second on a window's maximize button is
  shown three small layouts: two halves, a half beside two quarters,
  and four quarters. A click on a part of one snaps the window there.

After a snap, if a part of the screen is still free, the desktop lists
the windows that are not snapped in the largest free part. The arrow
keys and *Enter*, or the pointer, choose one, which fills that part;
*Esc*, any other key or a click beside the list puts it away.

The edge two snapped windows share moves them both: dragged, it moves
where the two sides of the screen meet, or the two parts of a side,
and every window along it is resized. An edge of a snapped window that
lies along the screen's own resizes that window alone, which is then
snapped no more.

# SCREENS AND DESKTOPS

A window is on the screen most of it is on. *Super+Shift+Left* and
*Super+Shift+Right* carry the window worked in to the screen before or
after its own, to the same place there. When a monitor is pulled out
its windows come to a screen that is left, and go back when it is
plugged in again.

Each screen has a row of desktops of its own, of which one shows
(ADR 0726). A window is on one of them and shows when it does; the
list of windows has the windows that show. The keys act on the screen
the pointer is on (*KEYS*). A row has at most 16 desktops, and its
last one does not close.

# KEYS

These keys are the desktop's own and reach no program:

*Alt+Tab*, *Alt+Shift+Tab*
	Shows the list of windows, the one last worked in first, and goes
	to the next in it, or with Shift to the one before. Letting go of
	Alt brings the chosen window to the front, a minimized one too.

*Super*
	Pressed and let go by itself, shows the list of windows to stay:
	the arrow keys choose, *Enter* takes the choice, and *Esc* or
	Super again puts the list away.

*Alt+F4*
	Asks the window worked in to close.

*Alt+Space*
	Shows that window's menu. The arrow keys choose a line, *Enter*
	takes it, and *Esc* puts the menu away.

*Super+Left*, *Super+Right*
	Snaps the window worked in to that half of its screen, and a window
	in a quarter to the same quarter of the other side.

*Super+Up*, *Super+Down*
	Go through a side's parts: from a half to its upper or lower
	quarter, and from a quarter back to the half. Up from an upper
	quarter maximizes the window, and down from a lower one has it as it
	was before it was snapped. A window that is not snapped is maximized
	by Up and minimized by Down; a maximized or a full-screen one is as
	it was at Down.

*Super+Shift+Left*, *Super+Shift+Right*
	Carries the window worked in to the screen before or after its own.

*Super+Ctrl+D*
	Makes a desktop after the one that shows on the screen the pointer
	is on, and shows it.

*Super+Ctrl+Left*, *Super+Ctrl+Right*
	Shows the desktop before or after it. With Shift held too, the
	window worked in goes along.

*Super+Ctrl+F4*
	Closes the desktop that shows: its windows go to the one before it,
	or to the one after for the first.

*Ctrl+Alt+F1* to *Ctrl+Alt+F12*
	Shows the console of that number, as Alt and the key do at a console
	(*consoled*(8)). The desktop stands in the console it was started
	at, and that console's key shows the desktop again, as it was. The
	programs in it go on meanwhile and read no key.

*Ctrl+Alt+Backspace*
	Ends the desktop. _command_ is asked to end (SIGTERM).

*Ctrl+Alt+Delete*
	The secure attention key, which the kernel gives to consoled alone
	(*consoled*(8)): its menu shows over the desktop, and *Esc* there
	shows the desktop again.

The keys type in the layout of the consoles (*keymap*(1)).

# OPTIONS

*--*
	What follows is _command_, though it start with a dash.

*-h*, *--help*
	Says how desktop is used.

*--version*
	Says which release of Dukovo this desktop is of.

# ENVIRONMENT

desktop starts _command_ with its own environment and two variables
set:

*WAYLAND_DISPLAY*
	The name of the desktop's socket, _wayland-0_ for a person's first
	desktop.

*XDG_RUNTIME_DIR*
	The folder the socket is in. It is the folder desktop's own
	environment names with this variable, if it is the person's own and
	no one else can open it, and otherwise _/tmp/runtime-N_ for the
	person of number _N_, which desktop makes.

# EXIT STATUS

The exit status of _command_ if the desktop ended because it did, and
128 more than a signal's number if a signal ended it, as a shell says
it. 0 if Ctrl+Alt+Backspace or a signal (SIGTERM, SIGHUP, SIGINT) ended
the desktop. 1, with the reason, if the desktop could not start or go
on: the screens were not lent, _command_ could not be started, or no
screen is left.

# EXAMPLES

The desktop with one program in it, which ends when the program does:

```
desktop /users/ada/apps/editor notes.txt
```

# SEE ALSO

*windowd*(1), *windowd.json*(5), *consoled*(8), *inputd*(8), *keymap*(1),
*radix-display*(2)
