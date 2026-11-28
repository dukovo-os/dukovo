<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# The desktop

The desktop shows the windows of programs on the screens, in place of
a console's text. Each window has a title bar and a frame, and you
move, resize, maximize, minimize and close it with the mouse or the
keys; the console is back when the desktop's program ends. Release 1.1
builds the desktop in steps: several desktops, snapping a window to a
half of the screen and laying out the screens come with the next. The
full references are desktop(1), windowd(1), windowd.json(5),
consoled(8), inputd(8) and ADRs 0712 to 0723.

## Starting it

Sign in at a console of the machine, and type `desktop` and the program
to run in it:

```
$ desktop /users/ada/apps/editor notes.txt
```

The console's text gives way to the desktop, a dark background with the
pointer in the middle of the first screen, and the program's window
shows when the program has drawn it. A program that opens more windows,
or starts other programs, has them all on the same desktop. Each new
window shows in the middle of the screen the pointer is on, a little
down and to the right of the one before. Click a window to bring it to
the front and type into it.

When the program ends, so does the desktop. The console shows again,
with whatever the program wrote to it, and `desktop` exits as the
program did, so a script can tell how it went.

`desktop` alone shows an empty desktop, which stays until you end it
with its key.

## Windows

Every window has the same frame, which the desktop draws: a title bar
with the window's title and three buttons at its right end, and a thin
border.

| To | Do this |
| --- | --- |
| Move a window | Drag its title bar. Some of the bar always stays on a screen, so a window cannot be lost off the edge. |
| Resize it | Drag an edge or a corner. You need not hit the thin border exactly: the edges reach a little way out. |
| Maximize it, or have it back as it was | Click the middle button, or double click the title bar. Dragging a maximized window by its bar also has it back. |
| Minimize it | Click the left button of the three. The window is out of sight and still in the list of windows. |
| Close it | Click the right button, or press Alt+F4. The program is asked to close the window, and may ask you about unsaved work first. |
| See the window's menu | Click the title bar with the right button, or press Alt+Space. |

The menu's Move and Size hand the window to the arrow keys: each press
moves the window, or its right and bottom edges, by 16 pixels, and by
one with Ctrl held. Enter keeps the change, and Esc undoes it.

Some programs have better use for a title bar than a title, as a
browser has for its tabs, and ask the desktop for the part of the bar
between the icon and the buttons. The buttons stay the desktop's and
stay where they are in every window. If you would rather such a
program had the ordinary bar, choose "Full title bar for this app" in
its window's menu: it holds for all that program's windows, and is
remembered.

## Its keys

These keys belong to the desktop, and no program in it reads them:

| Keys | What they do |
| --- | --- |
| Alt+Tab, Alt+Shift+Tab | Show the list of windows, the one you last worked in first, and step forwards or backwards through it while Alt is held. Let go of Alt for the window chosen; a minimized one comes back. |
| Super | Tapped by itself, shows the list of windows to stay. Choose with the arrow keys and Enter; Esc puts it away. |
| Alt+F4 | Asks the window you work in to close. |
| Alt+Space | Shows that window's menu. |
| Super+Up | Maximizes the window. |
| Super+Down | Restores a maximized window, and minimizes another. |
| Ctrl+Alt+F1 to Ctrl+Alt+F12 | Show the console of that number. The desktop stays in the console you started it at: that console's key shows it again, as it was, and its programs go on meanwhile. |
| Ctrl+Alt+Backspace | Ends the desktop. Its program is asked to end. |
| Ctrl+Alt+Delete | The secure key, as everywhere: consoled's menu shows over the desktop, and Esc there shows the desktop again. |

At a console, Alt and a function key switch consoles; in the desktop
Ctrl is held too, so that Alt+F4 closes a window as people expect.

## The keyboard and the mouse

The keys type in the layout of the consoles: change it with `keymap`,
before you start the desktop. A layout you wrote yourself has no name
the desktop's programs know, and they type in English (US) with it.
The Caps Lock and Num Lock lights follow the desktop while it shows,
and the console when it shows.

Every keyboard and mouse of the machine works in the desktop, and one
you plug in while it runs works at once. A mouse moved slowly moves the
pointer pixel by pixel; moved fast, it carries the pointer up to two
and a half times as far, so that a wide screen is crossed in one
stroke.

## Settings

How fast the pointer moves, how quick a double click has to be, how a
held key repeats and which programs may share their title bars are
settings, in a file of your own, `~/.config/windowd.json`, laid over
the system's `/system/settings/windowd.json`. A pointer half as fast as
built, and keys that repeat more slowly:

```
{
  "pointer": { "speed": -1 },
  "keyboard": { "repeat_delay_ms": 400, "repeat_rate": 20 }
}
```

A running desktop reads the files again when you tell it to, with
`pkill -USR1 desktop`. A mistake in a file never keeps the desktop from
starting: it says which key is wrong and goes on with the rest.
windowd.json(5) lists every key.

## Screens

The desktop is on every screen that has a monitor, each in the mode its
monitor prefers, side by side from left to right in the order
`hardware screens` lists them ([Screens](screens.md)). Move the pointer
off the right edge of one screen and it is on the next. Plug a monitor
in and the desktop is on it; pull one out and the others move up. A
window that was on a screen that is gone keeps its place, and shows
again when a screen is there.

A desktop on which nothing moves draws nothing and uses no processor
time. When something changes, only the changed part of a screen is
drawn.

## Who can start it

The screens belong to the person in front of them. `desktop` asks
consoled for them, and consoled lends them to a program started from
the console that shows, and to root. Typed at a console that is not
showing, or over SSH as yourself, `desktop` says so and ends:

```
desktop: the desktop is started by root, or from the console that shows on the screens
```

Only one program has the screens at a time. While a desktop runs, a
second `desktop` is told:

```
desktop: another program has the screens, or no screen shows the consoles
```

The desktop runs as you, with your rights and no more. It is handed
the screens by consoled and the keyboards and mice by inputd, and both
are silent for it whenever it is not the one showing.

## Programs for the desktop

A program draws its windows as a Wayland client. `desktop` starts its
program with two variables set, which every Wayland library reads:
`WAYLAND_DISPLAY`, the name of the desktop's socket, and
`XDG_RUNTIME_DIR`, the folder it is in, which is yours alone
(`/tmp/runtime-N` for the account of number N, unless your environment
names another). A program you start later from another console finds
the desktop with the same two variables.

windowd(1) lists the protocols a program can use. A program that breaks
a rule of the protocol loses its connection and its windows, and the
desktop and the other programs go on.

## If consoled is restarted

The desktop does not depend on consoled once it has the screens. If
consoled ends, or an administrator restarts it, the desktop and its
windows stay, and the keys keep reaching them. The new consoled waits
under the desktop and shows its consoles when the desktop ends. Until
then Ctrl+Alt+F1 to F12 show no console: end the desktop to get to one.
