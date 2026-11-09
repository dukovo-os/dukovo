<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# Braille displays

A refreshable braille display shows the consoles in braille: the line
the cursor is on, with the cursor in dots 7 and 8, as much of it as the
display has cells for. Its keys move along the line and the screen, its
routing keys move the cursor to the cell pressed, and its braille
keyboard, if it has one, types. Dukovo drives displays with BRLTTY,
which knows nearly every display made. The full references are
braille(1), brailled(8), braille.toml(5), brltty(1), the *BRAILLE
DISPLAYS* section of consoled(8) and ADR 0326.

## Plugging one in

Plug the display in by USB. Within two seconds brailled finds it and
starts BRLTTY for it, and the display shows the console. Nothing needs
setting for the displays brailled knows by their USB IDs:

- Baum's, on a serial port by USB: Vario, SuperVario, VarioPro,
  EcoVario, VarioConnect, PocketVario and Refreshabraille;
- Hedo's MobilLine and ProfiLine, and Papenmeier's;
- HumanWare's Brailliant BI and B 80 and BrailleOne, APH's Chameleon 20
  and Mantis Q40, and the NLS eReader, in their serial mode;
- Eurobraille's b.note and b.book;
- the Canute 360.

`braille displays` lists them by their IDs. To see what brailled found:

```
# braille
Braille: on
Driver:  auto
Port:    auto
Table:   en-nabcc
Display: Baum SuperVario 40 on /devices/ttyUSB0, with BRLTTY's bm driver
```

Unplugging the display stops BRLTTY; plugging it in again starts it.

## A display on a serial port

For a display on a serial port, or one on a USB serial cable whose IDs
are the cable's rather than the display maker's, name its driver and
port:

```
# braille driver pm
# braille device /devices/ttyS0
```

`braille displays` lists the drivers' codes. `braille device auto` and
`braille driver auto` go back to finding displays by their IDs.

## Reading and typing

The display shows the console's rows and, under them, the system's own
line: elevate's password prompt, the Ctrl+Alt+Delete menu, notes and
notifications. While the system asks for something there, the display
shows that line, with the cursor where the answer goes, so a password
can be typed from the display's keyboard and the menu answered from it.

Which key does what depends on the display; BRLTTY's help for its
driver lists them, and BRLTTY's help command shows them on the display.
The screen reader, if it is on, speaks as usual alongside
([The screen reader](screen-reader.md)).

## The text table

Characters are shown in North American Braille Computer Code, eight dots
to a character, unless another table is named:

```
# braille table de
```

The tables are in `/system/resources/brltty/Text`.

## Turning it off

`braille off` stops BRLTTY and keeps brailled from starting it, until
`braille on`.

## What is not yet supported

Displays that are USB HID devices, such as Baum's SuperVario2 and Orbit
Reader, and HumanWare's in their HID mode, or that speak over USB of
their own, such as Freedom Scientific's Focus, HandyTech's and Alva's
newer displays, are not found yet, nor those on Bluetooth. Many of them
can be switched to a serial mode, or used on a serial port.
