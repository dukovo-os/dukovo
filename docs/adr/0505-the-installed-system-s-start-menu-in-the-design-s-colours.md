<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0505. The installed system's start menu in the design's colours

- Status: Accepted
- Date: 2026-11-12

## Context

The install stick's boot menu is drawn in Dukovo's colours and the
console's font, by a GRUB theme `ob image` writes beside it (ADR 0402).
That ADR left the installed system's menu plain, for later. Since then
the installed menu has become the one people meet when something goes
wrong: it shows by itself after a start that did not finish, with
Troubleshooting and Rescue on it (ADRs 0460 and 0501), and Esc, F4 or
Shift show it at any start (ADR 0426). Shown in GRUB's own white on
black, it looks like another system's.

The installed menu on the EFI system partition stays as the install
wrote it, and hands over to the menu of the slot that starts (ADR 0451),
so that an update changes the menu with the rest of the system.

## Decision

1. **The theme comes with the slot.** `ob image` writes the stick's
   theme, made for an installed system, into each slot's `startup/theme`,
   beside the slot's menu: the same layout, colours, Spleen fonts and
   box pictures, with "start menu" under the name in place of
   "installer on USB". It is part of the release, signed and updated
   with it.
2. **The box fits the most entries the menu has.** Dukovo, the version
   without its update during a try, the previous version, Windows,
   startup trouble, Troubleshooting, Rescue and the firmware's settings:
   eight. A menu of fewer leaves the box's foot empty, as the box cannot
   follow the entries a start offers.
3. **The installed menu loads it when the menu can show.** After every
   entry is made, if the menu shows or has its hidden second, the menu
   loads the fonts and the theme from the default slot and draws on the
   screen with GRUB's graphical terminal, keeping UEFI's screen mode,
   and under a BIOS the first of 1280x800, 1024x768 and 800x600 the card
   has, as the stick does. A start with no hidden second (a timeout of
   0) reads nothing more.
4. **Where the theme cannot be used, the menu is plain.** The serial
   port keeps GRUB's plain menu. A screen GRUB cannot draw on, or a
   slot from before this release, which has no theme, shows the plain
   menu on the screen too.

## Consequences

- The start menu looks like the stick's and the installer's screens, on
  every installed system, the cloud's and the lab's included, and an
  update brings the theme of its own release.
- Each start that may show the menu reads about 100 KiB of fonts from
  the root, and the screen is cleared at GRUB's hidden second rather
  than when the kernel starts.
- Tests read the menu's text from the serial port, as the screen's font
  is not the console's: the boot test `start-menu` checks the theme's
  colours on the screen after a start cut short, and the lab keeps a
  screenshot of the menu for a person to look at.

## Alternatives considered

- **The theme on the EFI system partition.** The installer, Repair
  startup and `ob image` would each write it there, and an update could
  not change it: the partition's files stay as the install wrote them.
- **Loading the theme only when the menu shows at once.** Esc and F4 in
  the hidden second would then show the plain menu, a different look
  for the same menu.
- **A box as tall as the entries.** GRUB's theme is fixed when it is
  loaded; it cannot count the entries the menu makes after.
