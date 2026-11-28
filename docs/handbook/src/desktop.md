<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# The desktop

The desktop shows the windows of programs on the screens, in place of
a console's text. Each window has a title bar and a frame, and you
move, resize, maximize, minimize and close it with the mouse or the
keys; the console is back when the desktop's program ends. You snap
windows side by side, sort them into several desktops, and say where
your monitors stand. The full references are desktop(1), windowd(1),
windowd.json(5), consoled(8), inputd(8) and ADRs 0712 to 0729.

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
| Snap it to a half or a quarter of the screen | Drag it by its bar to an edge of the screen, or press Super with an arrow ([Side by side](#side-by-side)). |
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

A program may show a window full screen, as a film or a game does: it
fills its screen with no frame, in front of that screen's other
windows. Super+Down, or Restore in the window's menu (Alt+Space), has
it as it was, and every key of the desktop works meanwhile. A program
that holds the pointer, as a game does to turn its view, gives it up
when you go to another window by the keys.

## Side by side

A window snaps to the left or the right half of its screen, or to the
upper or the lower quarter of a half. A snapped window keeps its frame,
and has the place and the size it had before when you drag it away by
its bar.

| To | Do this |
| --- | --- |
| Snap the window you work in to a half | Press Super+Left or Super+Right. |
| Make a half a quarter, and back | Press Super+Up or Super+Down. Up from an upper quarter maximizes the window; down from a lower one has it as it was. |
| Snap with the mouse | Drag the window by its bar to the left or the right edge of the screen for that half, to an edge near a corner for that quarter, or to the top edge to maximize it. The desktop shades what the window will fill before you let go. |
| Pick from the layouts | Rest the pointer on the maximize button for half a second. Three small layouts show under it: click the part the window is to fill. |

When you have snapped a window and part of the screen is still free,
the desktop lists your other windows in the free part. Choose one with
the arrow keys and Enter, or click it, and it fills that part; Esc
puts the list away. Two windows side by side are two keys and Enter.

Drag the edge two snapped windows share and both follow: the one
grows as the other shrinks. Each desktop of each screen keeps its own
splits.

## Several desktops

Each screen has a row of desktops of its own, one of which shows. A
window is on one desktop and shows when that one does, and switching
on one monitor leaves the others as they are: the reference on the
second monitor stays while you go through your work on the first.

The keys act on the screen the pointer is on:

| Keys | What they do |
| --- | --- |
| Super+Ctrl+D | Makes a desktop after the one that shows, and shows it. |
| Super+Ctrl+Left, Super+Ctrl+Right | Show the desktop before or after it. With Shift held too, the window you work in goes along. |
| Super+Ctrl+F4 | Closes the desktop that shows. Its windows go to the one before it. The last desktop of a screen does not close. |

A row has at most 16 desktops. The list of windows, and Alt+Tab, have
the windows of the desktops that show.

## Its keys

These keys belong to the desktop, and no program in it reads them:

| Keys | What they do |
| --- | --- |
| Alt+Tab, Alt+Shift+Tab | Show the list of windows, the one you last worked in first, and step forwards or backwards through it while Alt is held. Let go of Alt for the window chosen; a minimized one comes back. |
| Super | Tapped by itself, shows the list of windows to stay. Choose with the arrow keys and Enter; Esc puts it away. |
| Alt+F4 | Asks the window you work in to close. |
| Alt+Space | Shows that window's menu. |
| Super with an arrow | Snaps the window to a half or a quarter of its screen ([Side by side](#side-by-side)). A window that is not snapped is maximized by Super+Up and minimized by Super+Down; a maximized or a full-screen one is as it was at Super+Down. |
| Super+Shift+Left, Super+Shift+Right | Carry the window to the screen before or after its own. |
| Super+Ctrl with D, Left, Right or F4 | Make, show and close desktops ([Several desktops](#several-desktops)). |
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

The desktop is on every screen that has a monitor. Until you say
otherwise each is in the mode its monitor prefers, side by side from
left to right in the order `hardware screens` lists them
([Screens](screens.md)). Move the pointer off the right edge of one
screen and it is on the next. Plug a monitor in and the desktop is on
it. Pull one out and its windows come to a screen that is left, as far
across and down as they were; plug it in again and they go back.
Super+Shift+Left and Right carry the window you work in from screen to
screen.

Where your monitors stand, and how each shows, are settings. A second
monitor that stands to the left of the first, and a dense panel drawn
half as large again:

```
{
  "screens": {
    "DEL U2720Q": { "scale": 1.5 },
    "virtio0/2": { "mode": "1920x1080@60", "turn": 90 }
  },
  "layouts": [
    { "screens": { "virtio0/2": [0, 0], "DEL U2720Q": [1080, 0] } }
  ]
}
```

A screen is named by its plug, as programs are told it, or by its
monitor's maker and model, which follows the monitor to any plug. Each
may have a `mode`, a `scale` from 1 to 4 in quarters, a `turn` of 90,
180 or 270 degrees for a monitor on its side or hung upside down, and
`"use": false` to leave it dark. A layout gives each screen of one set
its corner on the desktop, and is used when exactly those screens are
in use: your desk and a meeting room each keep their own. The screens
of a layout must touch and must not overlap. The desktop takes the
settings up when you tell it to read them again, with no window lost.

A program that names its windows to the desktop has them shown where
you left them the next time: on the same screen, at the same place and
size, maximized or snapped as they were.

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
