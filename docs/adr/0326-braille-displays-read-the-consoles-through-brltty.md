<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0326. Braille displays read the consoles through BRLTTY

- Status: Accepted
- Date: 2026-11-09

## Context

A person who reads braille reads the console on a refreshable braille
display: a row of 14 to 80 cells of eight pins each, with keys to move
along the screen, routing keys over the cells, and often a braille
keyboard. Dozens of makers build them, each with its own protocol, over
a serial line, USB or Bluetooth. BRLTTY drives nearly all of them, and
every Linux distribution and installer uses it. It is under version 2.1
of the LGPL or later, its tables too, and is built from drivers of three
kinds: braille drivers, one for each family of displays; screen drivers,
which read the screen the system shows; and a system layer, for the
system's serial ports, USB, Bluetooth and HID devices. Two of its braille
drivers carry file transfer tools beside them, which only its API builds,
under the GPL.

On Dukovo the consoles are consoled's, not the kernel's (ADR 0100):
consoled holds each console's text and cursor, draws its own line at the
bottom for prompts no program draws (ADR 0251), and already tells the
screen reader what changed (ADR 0324). The kernel makes a terminal of a
USB serial port, an FTDI chip or a CDC-ACM device (ADR 0246), but has no
way for a program to drive a USB device itself, and no raw HID device.

Many displays are a serial port on USB: Baum's, Hedo's and Papenmeier's
are FTDI chips under IDs of their makers', and HumanWare's Brailliants,
Eurobraille's and the Canute are CDC-ACM devices. Newer ones are HID
devices (Baum's SuperVario2 and Orbit, HumanWare's in their HID mode,
APH's), and some speak their own protocol over USB bulk endpoints
(Freedom Scientific's Focus, HandyTech's newer ones, Alva's).

## Decision

- **BRLTTY is a port, with a screen driver of Dukovo's.** `ports/brltty`
  builds BRLTTY 6.9.1 from its git tree: configure is made with autoconf
  and the make dependencies with a short script of the project's
  (`gendeps.py`), since its git tree carries neither. It is built with
  every braille driver in it, as one static program, without BrlAPI,
  speech, X, its own USB, Bluetooth and HID access, and the GPL transfer
  tools are deleted before the build, so none of their code is compiled.
  Its text, contraction, attribute and key tables go in
  `/system/resources/brltty` (3.9 MiB, each under BRLTTY's LGPL). A patch
  tells its configure of Radix. The screen driver, `dk`, is the
  project's own (Apache-2.0 OR MIT), built into it.
- **consoled shows BRLTTY the consoles.** consoled listens on
  `/system/live/consoled/braille`, which only root can open, for one
  display at a time. Each time what the shown console shows changes it
  sends the screen whole: `screen NUMBER COLUMNS ROWS X Y`, then each row
  as text. The rows are the console's as the screen shows them, the
  scrollback while it is moved back, and one more, consoled's own line:
  its prompts, its Ctrl+Alt+Delete menu, its notes and the
  notifications, or blank. No program writes that row, so what is read
  there is consoled's, as the trusted sound marks it for speech; while
  consoled asks for something there, the cursor is on it. Only the
  newest screen waits for a slow reader.
- **The display types through consoled.** The screen driver sends back
  what BRLTTY types, the display's braille keyboard and the keys its
  commands press, such as those that route the cursor: `key NAME`, with
  `shift`, `ctrl` and `alt`, a character as `U+XXXX` or the name of a
  key, and `switch NUMBER`. consoled takes each as from its keyboard: its
  prompts and menu take keys first, so a person can answer elevate's
  password and reach the menu from the display, and Alt with a function
  key shows another console. A key no keyboard has is refused.
- **brailled starts BRLTTY when a display is plugged in.** The kernel
  shows a serial port's terminal in its interface's folder in
  `/kernel/bus/usb/devices`, `ttyUSB0`, or `tty/ttyACM0`, as Linux does.
  brailled, a service of systools, looks every two seconds for a display
  it knows by its USB IDs (34 of them, from BRLTTY's tables), and starts
  `brltty` for it with its braille driver, the display's terminal, the
  `dk` screen driver and the text table. It stops BRLTTY when the
  display is unplugged or another takes its place. braille.toml names a
  driver and port for a display on a serial port, or for one on a chip
  that other devices use too (FTDI's own IDs), which is not guessed at,
  and the text table, North American Braille Computer Code unless set.
  `braille(1)` sets them, and turns it off.
- **The kernel knows the FTDI displays.** Its FTDI driver takes Baum's,
  Hedo's and Papenmeier's product IDs, under FTDI's vendor ID and Baum's,
  as its own chips'.
- **HID and USB displays come later.** A display that is a HID device, or
  speaks over USB bulk endpoints, needs a way for BRLTTY to reach it: a
  raw HID device, or USB access for a program, in the kernel, with
  BRLTTY's system layer for them. Displays on CP210x, CH341 and Belkin
  chips need those chips' serial drivers. Until then such a display is
  not found, and `braille displays` lists those that are.

## Consequences

- The `braille` boot test plugs in QEMU's `usb-braille`, a SuperVario 40
  on an FTDI chip, whose serial line the test answers as the display, in
  Baum's protocol: brailled starts BRLTTY with its Baum driver, the cells
  show the cursor's line, a routing key moves the shell's cursor by the
  keys BRLTTY types through consoled, the display keys pan along a long
  line, consoled's menu shows on its own row, and unplugged, BRLTTY is
  stopped. The lab's `console/braille` test does the same with a real
  display, where one is plugged in.
- The screen reader and a braille display work together: each follows
  consoled, neither knows of the other.
- brailled reads a few files every two seconds; the kernel has no way yet
  to tell a program that a USB device was plugged in.
- BRLTTY's own settings, in its preferences file under
  `/system/state/brltty`, are kept as BRLTTY keeps them; its
  configuration file, `/system/settings/braille/brltty.conf`, is read if
  there is one.
