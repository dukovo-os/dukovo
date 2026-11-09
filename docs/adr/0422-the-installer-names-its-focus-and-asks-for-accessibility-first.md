<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0422. The installer names its focus and asks for accessibility first

- Status: Accepted
- Date: 2026-11-09

## Context

The install stick's *Install Dukovo with speech* starts the screen
reader before the installer's first page (ADR 0324). The reader follows
what changes on the screen, which serves a shell well and the
installer's screens badly. They are drawn whole again for each key, and
show the focus by a tan background that moves from one label to the
next. The reader would hear two labels change, or none it could tell
apart. consoled now lets a full-screen program name what has its focus
with `ESC ] 7700 ; text ST`, and stops reading that program's drawing
until it leaves the alternate screen (ADR 0325).

The consoles' text size, sticky keys and slow keys are kept in
accessibility.toml(5), and the screen reader's state in reader.toml(5).
A person who needs them during the install needs them on the installed
system's first sign-in too. They should not have to find them again in
files they cannot yet read.

## Decision

- **The screens say what they show.** After each draw they compare what
  the reader was last told with what the screens now show, and send what
  changed in one `OSC 7700`:
  - a new page: its name and step, its text, and what has the focus;
  - a field reaching the focus: its label, its kind (choice, check box,
    edit, password, list or button), its place among the choices beside
    it ("2 of 2"), its state or value, and its note;
  - on the same field, what a key changed: the character typed or taken
    away, the list's new value, *selected* or *checked*;
  - a problem or a message as it shows, and each step of the install as
    it starts.
- The help, the log, the forms for another person and a partition, and
  the rdisk screen's partitions are said the same way. The startup
  screen says each thing it checks, and why the installer cannot start
  if it cannot. Nothing is sent to the serial console.
- **A password is never said.** A character typed into a password field
  is said as *star*, one taken away as *deleted*, and the field as so
  many characters.
- **The Welcome page asks for accessibility**, beside the language and
  the keyboard: *Screen reader*, *Text size*, *Sticky keys* and *Slow
  keys*. Each takes effect at once. The installer writes the live
  system's accessibility.toml(5) and runs `service consoled reload`, or
  runs `reader on` or `reader off`. It reads the file again before each
  key and on each resize, so Ctrl+Alt+Plus and Shift pressed five times
  show on the page too. The Screen reader box starts checked when the
  installer runs with speech. When it starts the reader, the page is said
  whole: consoled keeps the last text a program named while the reader
  did not listen, and says it after the console's number, in place of
  the program's drawing. This amends ADR 0325, under which the sequence
  did nothing while the reader was not listening.
- **The answers keep it.** The answers file gains `accessibility`:
  `screen_reader`, `text_size`, `sticky_keys` and `slow_keys`
  (install-answers(5)). A left-out `screen_reader` is as the installer
  runs: on with the kernel flag `speech`. The screens leave it out only
  while the reader is off and the installer runs without speech, so a
  setup file saved with the reader on keeps it on wherever it is used
  again. The install writes the consoles' file into
  `@settings` when any of its settings is not the default, and
  reader.toml with `on = true` into `@state` when the reader starts with
  the system. The review lists them with the settings. A setup file
  without the key takes the Welcome page's.
- **The focus shows without colour.** The field with the focus is also
  marked with `›` (`>` on the serial console) in the margin beside it,
  for a screen that shows few colours or a person who tells them apart
  poorly. The buttons keep the design's highlight.

## Consequences

- The `install-speech` boot test starts the stick with speech and reads
  the installer through readerd's log. It stops and starts the reader by
  the Screen reader box, sets larger text and sticky keys on the Welcome
  page and sees consoled take them. It installs by the screens, typing a
  password, and checks the password is never said. The installed system
  then starts its reader at the sign-in, with its text larger. The lab
  test `install/with-reader` starts the reader from the Welcome page of
  the stick's usual entry on the lab's machines, where it speaks on their
  speakers, and finds all three on the installed system.
- Every message said is in the installer's Fluent file, under its own
  heading, so a translation translates what is said with what is shown.
- A program that redraws its screen whole can follow the same pattern:
  the words come from its state, not from the screen.
