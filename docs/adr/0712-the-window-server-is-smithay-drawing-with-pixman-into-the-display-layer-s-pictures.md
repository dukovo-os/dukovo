<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0712. The window server is Smithay, drawing with pixman into the display layer's pictures

- Status: Accepted
- Date: 2026-11-27

## Context

Release 1.1 puts windows on the screens. The display layer shows
pictures on every card the kernel drives (ADR 0700), the input devices
are files of events (ADR 0706), inputd hands them to the program that
has the screens (ADR 0707), and consoled lends the screens (ADR 0702).
What is missing is the program in the middle: the one that takes the
windows of other programs, puts them together into a frame for each
screen, and gives each key and each motion of the pointer to the right
window.

The programs that will run on Dukovo's desktop draw their windows
through a toolkit, and every toolkit that is maintained speaks Wayland.
A window server that speaks Wayland runs them as they are. The
protocols are many and their rules are fine: which request is an error
in which state, when a surface's pending state applies, how a popup is
placed. Writing them a second time would give Dukovo years of other
people's bug reports to find again.

No card of release 1.1 draws: the virtio card and the firmware's
framebuffer show pixels a program drew in memory. So a frame is drawn
on the processor.

## Decision

- **windowd is a Wayland compositor built from Smithay 0.7**, the Rust
  library of the protocols' server side, with its features
  `wayland_frontend`, `desktop` and `renderer_pixman` and none of its
  Linux backends. Its other name, `desktop`, is the command people type
  (ADR 0713). windowd has no `unsafe` of its own.
- **It offers** wl_compositor and wl_subcompositor, wl_shm, xdg_wm_base,
  wl_seat with a keyboard and a pointer, wl_output and
  zxdg_output_manager_v1. A window shows as its program drew it, in the
  middle of the screen the pointer is on, a step from the one before; a
  click brings a window to the front and gives it the keyboard. Frames
  of windowd's own around a window, and moving and resizing, are the
  next step's.
- **A frame is drawn by pixman, on the processor, straight into a
  picture of the display layer.** A picture's pixels are the pages of a
  memory file (ADR 0701). windowd hands that file to Smithay as a
  buffer of one plane, which the pixman renderer maps once and draws
  in; the card shows the same pages. No frame is copied between windowd
  and the screen.
- **The C libraries are ports**: pixman 0.46.4 and libxkbcommon 1.13.1,
  static libraries that windowd links, and the keyboard layouts of
  xkeyboard-config 2.48 in _/system/resources/xkb_, about 4 MiB in 294
  files. The compose tables and the translations of the layouts' names
  are left out.
- **A program's keyboard has the consoles' layout.** The name in
  _/system/settings/keyboard_ is given to XKB as the layout of that
  name, with what follows a dash as its variant: `us-intl` is `us` in
  the variant `intl`.
- **Five crates are patched for Radix**, each in a few lines:
  wayland-backend, which waits on its clients with epoll or kqueue
  (ADR 0717); Smithay, for the file it gives a program the keymap in,
  and the address of a SIGBUS; calloop and rustix, for what Radix's C
  library does not have; and getrandom 0.3, as 0.4 was.
- **The licenses** are the MIT license for Smithay, pixman and the
  crates. libxkbcommon and xkeyboard-config add the X Consortium's and
  The Open Group's forms of it and the historical permission notices of
  the companies that wrote X's keyboard code (`X11`, `MIT-open-group`,
  `HPND`, `HPND-sell-variant`, Digital's, and Ivan Zinoviev's for his
  layouts). All are permissive, their texts are in the image, and the
  license manifest lists each port.

## Alternatives

- **wlroots**, the C library most compositors are built from. Its
  backends are for Linux's DRM, libinput and udev, none of which Radix
  has, and windowd would call C at every step, in `unsafe` code that
  the rule for programs outside the kernel's core forbids.
- **The protocols written anew** on wayland-server alone. The wire
  format is the small part; the state of surfaces, of xdg-shell and of
  the seat is what Smithay has had years of programs run against.
- **X11.** The toolkits keep their X11 backends for old systems, the
  protocol lets every program read every key, and a server for it is a
  larger program than a Wayland compositor.
- **A renderer on the graphics card.** No driver of release 1.1 draws.
  Smithay's renderer is a trait, and a card that draws comes with a
  second renderer behind it; what a window server does with the frames
  stays.
- **Layouts of Dukovo's own given to the programs** as an XKB keymap
  made from the consoles' layout files. The programs and their toolkits
  expect the layouts' usual names and symbols, which xkeyboard-config
  is the source of.

## Consequences

- A Wayland program built for Dukovo shows its window, is drawn where
  it changes, and has the keyboard and the pointer. The first one is
  `window-test`, the tests' own.
- Three ports and five patched crates are added to what a release
  builds and what the security scan follows.
- windowd's frames cost the processor what pixman's loops cost; a
  screen on which nothing changes costs nothing (ADR 0714).
- libxkbcommon stays at 1.13.1 while the build machines have Meson 1.3:
  later releases ask for 1.4 (`docs/deferred.md`).
- windowd is not told which account or process a connection is of:
  wayland-backend reads that with a socket option of Linux's. Nothing
  asks until the grants of step 5 (`docs/deferred.md`).
- A console layout of a person's own has no XKB name, and the programs
  get English (US) for it (`docs/deferred.md`).
