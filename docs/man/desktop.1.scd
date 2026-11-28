desktop(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

desktop - show the desktop, and a program's windows in it

# SYNOPSIS

*desktop* [_command_ [_argument_...]]

*desktop* *--help* | *--version*

# DESCRIPTION

desktop shows the desktop on every screen with a monitor, in place of
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
  the window, or restores a maximized one; dragging a maximized window
  by its bar restores it under the pointer.
- An edge or a corner, held with the left button, resizes the window.
  The edges reach a few pixels out from the frame, so that they are
  easy to catch.
- The buttons minimize the window, maximize or restore it, and ask it
  to close. A button acts when the left button is let go on it.
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

How the title bars are shared out, how fast the pointer moves, what a
double click is and how a held key repeats are settings
(*windowd.json*(5)).

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

*Super+Up*
	Maximizes the window.

*Super+Down*
	Restores a maximized window, and minimizes another.

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
