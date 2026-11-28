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
restored or resized, and the frame changes when the program has drawn
what was asked. While a window is resized, one size is asked for at a
time: the next is the newest the pointer wants once the program has
drawn the one before, so a slow program draws fewer sizes and is never
behind. A window is never made smaller or larger than its program says
it may be, nor narrower than the title bar's own parts.

A program may ask for what the frame does: to be maximized, restored
or minimized, and, with a button it was told of still held, to be moved
or resized by the pointer or shown its menu. A request to fill the
whole screen is answered with the window as it is.

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

The time a key or pointer event carries is in milliseconds. A program
that makes a timestamps object for its keyboard or its pointer
(*zwp_input_timestamps_v1*) is sent the whole time just before each
event: the time the kernel's driver read the event at, on the monotonic
clock, in nanoseconds (ADR 0723). Only the program an event goes to is
told its time.

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

_/system/resources/xkb_
	The XKB layouts, of xkeyboard-config.

# SEE ALSO

*desktop*(1), *windowd.json*(5), *consoled*(8), *inputd*(8),
*keymap*(1), *radix-display*(2), *radix-input*(2)
