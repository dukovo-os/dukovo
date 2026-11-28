windowd.json(5) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

windowd.json - how the desktop's title bars, pointer and keys are set

# SYNOPSIS

_/system/settings/windowd.json_

_~/.config/windowd.json_

# DESCRIPTION

The desktop (*desktop*(1), *windowd*(1)) reads these two files when it
starts: the system's, for everyone who uses the machine, and then the
person's own, which is laid over it key by key (ADR 0722). A key the
person's file does not have keeps what the system's says, and a key
neither has is as built. Neither file has to be there.

Each is one JSON object of three groups:

```
{
  "title_bars": {
    "apps": "decide",
    "full": ["org.example.editor"],
    "shared": []
  },
  "pointer": {
    "speed": 0,
    "double_click_ms": 500
  },
  "keyboard": {
    "repeat_delay_ms": 250,
    "repeat_rate": 30
  }
}
```

The example names every key with the value it has as built, but for
the one application in *full*.

# TITLE BARS

The desktop draws the title bar of every window. An application may ask
to draw a part of the bar itself, between the window's icon and its
buttons, as a browser does for its tabs; these keys say which
applications may. An application is named by the id it gives its
windows, as _org.dukovo.terminal_.

*title_bars.apps*
	What holds for an application with no entry of its own: *"decide"*,
	the application draws a part of the bar if it asks to, or *"full"*,
	every window has the desktop's whole bar whatever its application
	asks. *"decide"* as built.

*title_bars.full*
	A list of applications whose windows have the desktop's whole bar
	whatever they ask.

*title_bars.shared*
	A list of applications that decide, whatever *title_bars.apps*
	says.

An application in a list of the person's file has that entry whichever
list the system's file has it in. One that is in both lists of one file
has the full bar.

The desktop writes these two lists itself. The menu of a window whose
application asks for a part of the bar (*Alt+Space*, or the right
button on the title bar) has the line *Full title bar for this app*.
Choosing it puts the application in *title_bars.full* of the person's
file and takes it out of *title_bars.shared*; choosing it again does
the reverse. Every other key of the file stays as it was. A file the
desktop wrote into has its keys in the order of the alphabet.

# POINTER

*pointer.speed*
	How fast the pointer moves, from -1 to 1: every motion of a mouse is
	2 to this power times as far as built, so half as far at -1 and
	twice as far at 1. 0 as built. A fraction is allowed, as 0.5.

*pointer.double_click_ms*
	The longest time between two presses that are a double click, in
	milliseconds, from 100 to 2000. 500 as built.

# KEYBOARD

*keyboard.repeat_delay_ms*
	How long a key is held before it repeats, in milliseconds, from 100
	to 2000. 250 as built.

*keyboard.repeat_rate*
	How many times a second a held key repeats then, from 1 to 100. 30
	as built.

The desktop tells every program these two, and each program repeats the
keys it is given.

# CHANGING THE SETTINGS

The desktop reads both files again when it is sent SIGUSR1, and what
they say holds from then on, for the windows that are open as well:

```
pkill -USR1 desktop
```

# MISTAKES

A mistake never keeps the desktop from starting. A value of the wrong
kind or out of its range, and a key the desktop does not know, are said
on the desktop's standard error, by the file and the key, and the key
keeps what it had from before:

```
desktop: /users/ada/.config/windowd.json: `pointer.speed` is 3, not a number from -1 to 1
```

A file that is not JSON is said so and is as if it were not there. The
desktop does not write a choice from a window's menu into such a file:
it says that the choice is not kept, and the choice holds until the
desktop ends or reads its settings again.

# FILES

_/system/settings/windowd.json_
	The system's settings, an administrator's to write.

_~/.config/windowd.json_
	The person's own. The desktop makes it, and the folder, when the
	person first chooses from a window's menu.

# SEE ALSO

*desktop*(1), *windowd*(1)
