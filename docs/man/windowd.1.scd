windowd(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

windowd - the window server

# SYNOPSIS

*windowd* [_command_ [_argument_...]]

# DESCRIPTION

windowd is the desktop: it shows the windows of programs on the
screens, and gives the keyboard and the pointer to the window the person
works in (ADR 0712). People start it by its other name, *desktop*(1),
which this page's synopsis, keys, environment and exit status are
those of. This page says what a program in the desktop is given.

A program speaks Wayland to windowd, on a socket (*FILES*). windowd is
a compositor built from Smithay, and draws with pixman on the
processor, straight into the pictures the display layer shows
(*radix-display*(2)): a frame is copied nowhere on its way to a screen.

windowd is one process of one thread, which runs as the person and
holds no right of its own. It has the screens by a loan of
*consoled*(8), and the keyboards and pointers by descriptors *inputd*(8)
hands it, which read only while the desktop is in front.

# PROTOCOLS

windowd offers these Wayland globals:

[[ *Global*
:[ *What a program has of it*
|  *wl_compositor*, *wl_subcompositor*
:  Surfaces, their regions and their subsurfaces.
|  *wl_shm*
:  Pixels in memory the program shares, in the formats ARGB8888 and
   XRGB8888.
|  *xdg_wm_base*
:  Windows (toplevels) and popups.
|  *wl_seat*
:  One seat, *seat0*, with every keyboard and every pointer of the
   machine.
|  *wl_output*, *zxdg_output_manager_v1*
:  A global for each screen in use: its mode, its scale as the whole
   number at or above it, its turn, its place on the desktop and its
   name, which is the display layer's, as _virtio0/0_.
|  *wp_fractional_scale_manager_v1*, *wp_viewporter*
:  The exact scale of the screen a surface is on, and a surface of
   one size drawn from a buffer of another (*SCREENS AND FRAMES*).
|  *wp_presentation*
:  When a frame of the program's showed (*SCREENS AND FRAMES*).
|  *wp_tearing_control_manager_v1*
:  A full-screen window's frames shown at once, not at the screen's
   next beat (*FULL SCREEN*).
|  *zwp_pointer_constraints_v1*, *zwp_relative_pointer_manager_v1*
:  The pointer held to a window, and a mouse's own motion (*KEYBOARD
   AND POINTER*).
|  *dukovo_session_manager_v1*
:  A name for a window, by which it is shown again where it was
   (*WINDOWS' PLACES*).
|  *zxdg_decoration_manager_v1*
:  Who draws a window's frame: windowd always answers that it does.
|  *dukovo_title_bar_manager_v1*
:  A part of the window's title bar for the program to draw (*TITLE
   BARS*).
|  *zwp_input_timestamps_manager_v1*
:  The time of each key and pointer event to the nanosecond (*KEYBOARD
   AND POINTER*).

A window shows when its program has drawn it for the first time, at
the size the program drew, in a frame windowd draws around it
(*desktop*(1), *WINDOWS*). The window's own surface is what is inside
the frame: a program draws no title bar and no shadow, and one that
asks to (*zxdg_decoration_manager_v1*) is answered that the frame is
windowd's.

windowd asks a program for a size when the window is maximized,
snapped, restored or resized, and the frame changes when the program
has drawn what was asked. A snapped window is told which of its sides
lie against others (the tiled states of *xdg_toplevel*). A size of 0
by 0 leaves the size to the program: the one its window had before. While a window is resized, one size is asked for at a
time: the next is the newest the pointer wants once the program has
drawn the one before, so a slow program draws fewer sizes and is never
behind. A window is never made smaller or larger than its program says
it may be, nor narrower than the title bar's own parts.

A program may ask for what the frame does: to be maximized, restored
or minimized, and, with a button it was told of still held, to be moved
or resized by the pointer or shown its menu, and to fill a whole
screen (*FULL SCREEN*).

A popup shows where its program asked. One that asks for the keys and
the pointer, as a menu does, has them until a button is pressed
outside its program's windows, another window is given the keyboard,
or a key of the desktop's own is pressed; the press that ended it then
does what it would have done.

A program that breaks a protocol's rule is disconnected, as the
protocol says. One that makes the memory of a picture it gave shorter
than the picture is disconnected when windowd reads past its end, and
the frame is drawn again without it (ADR 0717). At most 256 programs
are connected; one more is turned away.

# TITLE BARS

A program that has something better to put in its title bar than its
title, as tabs, asks for the bar (*dukovo_title_bar_v1*, ADR 0720). If
the person's settings allow it (*windowd.json*(5)), windowd answers
that the bar is shared, how high it is, and how many columns at its
left and right ends stay windowd's: the icon's, and the buttons' with
a strip to move the window by. The program then draws its window the
bar's rows higher and names the rectangle of the bar it draws. That
rectangle shows the program's pixels and hears the pointer as the rest
of the window does; everywhere else the bar is windowd's, and holds the
window's title no more. A program that is answered that the bar is
full draws as any other.

# SCREENS AND FRAMES

The desktop is on every screen that has a monitor, each in the mode the
monitor prefers, side by side from left to right in the display layer's
order, unless the person's settings say otherwise: a screen's mode, its
scale, its turn, whether it is used, and where the screens of a set lie
(*windowd.json*(5), ADR 0725). When a monitor is plugged in or pulled
out, or the settings are read again, the screens are laid out anew.
Where no window is, the desktop is one dark colour.

A screen's area of the desktop is its mode's size, turned, divided by
its scale. A program is told the scale of the screen its surface is
mostly on, as a whole number (*wl_surface.preferred_buffer_scale*) and
exactly (*wp_fractional_scale_v1*), and the turn. One that draws at
that scale is sharp; what is drawn smaller is magnified by windowd, as
windowd's own frames and panels are: pixel for pixel at a whole scale,
smoothed at any other.

A window is on the screen most of it is on. When its screen goes, it
is carried to the nearest screen that is left, as far across and down
it as it was, and goes back when its screen is there again, unless the
person has moved it since. Each screen has desktops of its own
(*desktop*(1), *DESKTOPS*), and a window shows when its desktop does.

windowd draws a screen's frame only when what the screen shows changed,
only the parts that changed, and at most once for each beat of the
screen (ADR 0714). It draws as late as the beat allows, so that a
program has nearly the whole time between two beats to draw its own
frame and see it in windowd's next. A program is told that a frame
showed (*wl_surface.frame*) when the display layer says it did. A
desktop on which nothing changes draws nothing and is woken by nothing.

A program that asks when a commit of its showed (*wp_presentation*) is
told the time of the screen's beat it showed at, the time from one beat
to the next, the beat's number, that it showed at a beat (*vsync*) and
whether the screen showed the program's own memory (*zero_copy*). A
commit that never showed, as one that the next took the place of, is
told as discarded.

# FULL SCREEN

A program asks for its window to fill a screen
(*xdg_toplevel.set_fullscreen*), the one it names or the one the window
is on. The window is then told so and the size to draw, and shows with
no frame in front of that screen's other windows, until the program
asks for it as it was, the person presses Super+Down, or chooses
Restore from the window's menu (Alt+Space). The desktop's keys work as
always.

When a full-screen window is all its screen shows, drawn as one opaque
buffer with a pixel for each of the screen's, and the screen is not
turned, windowd draws no frame: the display layer shows the buffer
from the program's own memory (ADR 0728). The program is given a buffer
back only once the screen shows another. Anything over the window, as
a menu, has the screen drawn as always for as long as it shows.

Such a window may say that its frames need not wait for the screen's
beat (*wp_tearing_control_v1*, the hint *async*): each then shows at
once, and the screen may show two frames in parts for one beat. For
every other window the hint changes nothing.

# KEYBOARD AND POINTER

A program is given the layout of the consoles (_/system/settings/keyboard_,
*keymap*(1)) as the XKB keymap of that name: *us-intl* is XKB's layout
*us* in its variant *intl*. A layout of a person's own, which has no XKB
name, gives the programs *us*. A held key repeats in the program, as
often and after as long as the settings say (*windowd.json*(5)): 30
times a second after 250 milliseconds as built. The lock lights of
every keyboard show what the keymap says is locked.

The pointer is the display layer's pointer plane, and moves when the
mouse does without a frame being drawn (ADR 0715). A mouse moved slowly
moves the pointer a pixel for each count, to point at a pixel; moved
fast, up to two and a half times as far; the settings make every
motion longer or shorter. A device that says where it
points, as a tablet does, points at that place of the first screen. A
wheel scrolls the window under the pointer. The pointer shows windowd's
arrow, or the surface the program under it set (*wl_pointer.set_cursor*),
of which 64 by 64 pixels show. On a card with no pointer plane no
pointer shows.

Every motion of a mouse is also told to a program that asks
(*zwp_relative_pointer_v1*), to the surface with the pointer: as far
as the pointer went for it, and as the mouse's own counts. A program
may ask for the pointer to stay where it is on a surface of its own,
or to stay on the surface or a region of it
(*zwp_pointer_constraints_v1*). windowd says when that holds: while the
surface has the pointer, its window is the one the person works in,
and the desktop itself has no hold of the pointer, as while a window
is dragged or the list of windows shows. One asked for once is gone
when it stops holding; one asked for to last holds again when the
person comes back to the window. When a lock ends, the pointer is
where the program last said it would have it, if that is on the
surface.

The time a key or pointer event carries is in milliseconds. A program
that makes a timestamps object for its keyboard or its pointer
(*zwp_input_timestamps_v1*) is sent the whole time just before each
event: the time the kernel's driver read the event at, on the monotonic
clock, in nanoseconds (ADR 0723). Only the program an event goes to is
told its time.

# WINDOWS' PLACES

A program may give a window a name of its own that lasts longer than
the window does, before the window first shows
(*dukovo_session_manager_v1.name_window*, ADR 0729): *main*, or the
name of a terminal's profile. With the window's application id the
name is how windowd knows the window again. A name is one window's at
a time.

windowd keeps where each named window was: its screen, the place and
the size of its frame, and whether it was maximized or snapped and to
what. A window that shows with a name windowd knows is asked for that
size in its first configure and shows there, on the desktop that
shows, and never minimized. If its screen is not there it shows on the
screen the pointer is on, and goes to its own when that is back.

The places are in a file of the person's (*FILES*), written a second
after the windows last changed and when the desktop ends. It holds the
last 256 windows that changed, and outlives the desktop.

# SIGNALS

*SIGUSR1*
	windowd reads its settings again (*windowd.json*(5)).

*SIGTERM*, *SIGHUP*, *SIGINT*
	End the desktop (*desktop*(1), *EXIT STATUS*).

# FILES

_$XDG_RUNTIME_DIR/wayland-N_
	The socket programs connect to, in a folder that is the person's
	alone (*desktop*(1), *ENVIRONMENT*). Whoever can open the folder can
	draw windows and read the keys typed into them.

_/system/live/consoled/display_
	Where windowd asks for the screens (*consoled*(8)).

_/system/live/inputd/devices_
	Where windowd asks for the keyboards and pointers (*inputd*(8)).

_/system/settings/keyboard_
	The layout of the keyboard.

_/system/settings/windowd.json_, _~/.config/windowd.json_
	The desktop's settings, the system's and the person's own
	(*windowd.json*(5)).

_~/.config/windowd-session.json_
	Where the named windows were (*WINDOWS' PLACES*). The file is
	windowd's to write and the person's alone to read; taking it away
	has every window placed as new.

_/system/resources/xkb_
	The XKB layouts, of xkeyboard-config.

# SEE ALSO

*desktop*(1), *windowd.json*(5), *consoled*(8), *inputd*(8),
*keymap*(1), *radix-display*(2), *radix-input*(2)
