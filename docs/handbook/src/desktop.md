<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# The desktop

The desktop shows the windows of programs on the screens, in place of
a console's text. Release 1.1 builds it in steps, and this chapter
describes the first: a program's window shows as the program drew it,
takes the keyboard and the mouse, and the console is back when the
program ends. Title bars, and moving and resizing windows, come with
the next step. The full references are desktop(1), windowd(1),
consoled(8), inputd(8) and ADRs 0712 to 0717.

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

## Its keys

Three chords belong to the desktop, and no program in it reads them:

| Keys | What they do |
| --- | --- |
| Ctrl+Alt+F1 to Ctrl+Alt+F12 | Show the console of that number. The desktop stays in the console you started it at: that console's key shows the desktop again, as you left it. Its programs keep running meanwhile, and read nothing you type elsewhere. |
| Ctrl+Alt+Backspace | Ends the desktop. Its program is asked to end. |
| Ctrl+Alt+Delete | The secure key, as everywhere: consoled's menu shows over the desktop, and Esc there shows the desktop again. |

At a console, Alt and a function key switch consoles; in the desktop
Ctrl is held too, so that Alt+F4 and its like stay the programs'.

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
