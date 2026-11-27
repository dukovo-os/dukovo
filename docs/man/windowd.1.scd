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
:  A global for each screen with a monitor: its mode, its place on the
   desktop and its name, which is the display layer's, as _virtio0/0_.

A window shows when its program has drawn it for the first time, at
the size the program drew. Release 1.1 asks no size of a window, and
answers a request to move, resize, maximize or minimize a window, or to
show it on the whole screen, with nothing. A popup shows where its
program asked, and takes no grab: a click outside it reaches what is
there.

A program that breaks a protocol's rule is disconnected, as the
protocol says. One that makes the memory of a picture it gave shorter
than the picture is disconnected when windowd reads past its end, and
the frame is drawn again without it (ADR 0717). At most 256 programs
are connected; one more is turned away.

# SCREENS AND FRAMES

The desktop is on every screen that has a monitor, each in the mode the
monitor prefers, side by side from left to right in the display layer's
order. When a monitor is plugged in or pulled out the screens are laid
out again in that order, and the windows keep their places on the
desktop. Where no window is, the desktop is one dark colour.

windowd draws a screen's frame only when what the screen shows changed,
only the parts that changed, and at most once for each beat of the
screen (ADR 0714). It draws as late as the beat allows, so that a
program has nearly the whole time between two beats to draw its own
frame and see it in windowd's next. A program is told that a frame
showed (*wl_surface.frame*) when the display layer says it did. A
desktop on which nothing changes draws nothing and is woken by nothing.

# KEYBOARD AND POINTER

A program is given the layout of the consoles (_/system/settings/keyboard_,
*keymap*(1)) as the XKB keymap of that name: *us-intl* is XKB's layout
*us* in its variant *intl*. A layout of a person's own, which has no XKB
name, gives the programs *us*. A held key repeats in the program, 30
times a second after 250 milliseconds. The lock lights of every
keyboard show what the keymap says is locked.

The pointer is the display layer's pointer plane, and moves when the
mouse does without a frame being drawn (ADR 0715). A mouse moved slowly
moves the pointer a pixel for each count, to point at a pixel; moved
fast, up to two and a half times as far. A device that says where it
points, as a tablet does, points at that place of the first screen. A
wheel scrolls the window under the pointer. The pointer shows windowd's
arrow, or the surface the program under it set (*wl_pointer.set_cursor*),
of which 64 by 64 pixels show. On a card with no pointer plane no
pointer shows.

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

_/system/resources/xkb_
	The XKB layouts, of xkeyboard-config.

# SEE ALSO

*desktop*(1), *consoled*(8), *inputd*(8), *keymap*(1),
*radix-display*(2), *radix-input*(2)
