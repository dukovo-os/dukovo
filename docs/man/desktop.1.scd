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

Release 1.1 draws a window as its program drew it, with no title bar,
and its programs place nothing themselves: a new window shows in the
middle of the screen the pointer is on, a step down and to the right of
the one before. A click on a window brings it to the front and gives it
the keyboard.

# KEYS

Three chords are the desktop's own and reach no program:

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

*windowd*(1), *consoled*(8), *inputd*(8), *keymap*(1), *radix-display*(2)
