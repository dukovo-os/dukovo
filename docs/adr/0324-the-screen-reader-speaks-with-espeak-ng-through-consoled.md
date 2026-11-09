<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0324. The screen reader speaks with eSpeak NG through consoled

- Status: Accepted
- Date: 2026-11-09

## Context

A person who cannot see the screen must be able to install Dukovo and
run it from its consoles, from the install stick's first screen on. On
Linux that takes a speech synthesizer, a screen reader that follows the
console (Speakup in the kernel, or Fenrir and espeakup beside it) and a
boot menu that says how to start them. Those screen readers are GPL and
are not read. The speech synthesizer that every one of them can speak
with, and the only open one that is clear at a fast rate in more than a
hundred languages, is eSpeak NG, which is GPL-3.0-or-later too; its
voice data carries the Unicode Character Database, under Unicode's
license.

The sound core already gives a screen reader its own file,
`/devices/audio/speech`, whose first write takes the output from any
other player (ADR 0320). consoled holds every console's text, the cursor
and the keyboard, and draws the prompts no program draws: elevate's
password, bundled's permissions and its Ctrl+Alt+Delete menu (ADR 0251).

## Decision

- **eSpeak NG is a port, run as a program.** `ports/espeak-ng` builds
  release 1.52.0: the `espeak-ng` command, with its library built in and
  without MBROLA, libsonic, a sound output, the asynchronous mode or
  speechPlayer, and its compiled voice data for every language in
  `/system/resources/espeak-ng-data` (11 MiB, without Russian's extended
  word list). Nothing of Dukovo's links to it: readerd runs it for each
  thing it says, as `espeak-ng --stdout --stdin`, and reads the WAV it
  writes, so the GPL covers that program and its data alone. Its
  manual page is Dukovo's own.
- **readerd speaks; consoled decides what.** readerd, a service of
  systools, takes lines on `/system/live/readerd/socket` (root's alone):
  `say`, `now`, `trusted`, `stop`, `faster`, `slower`, `reload` and
  `status`. A speaker thread runs eSpeak NG for each and plays its
  samples on the speech file 20 ms at a time, so that a stop is heard at
  once, resampled to the output's rate. Past 200 lines waiting, the
  oldest plain ones go. consoled, the only program that knows what the
  console shows and which keys are typed, tells it what to say: each
  line written to the shown console as it ends, and the rest once the
  console has been quiet for 80 ms; what a key moved the cursor onto when
  the program wrote nothing; console switches; notifications; and its own
  prompts and menu.
- **The trusted sound.** Two short rising tones, 660 Hz then 990 Hz, come
  before consoled's own words alone (`trusted`), and only root can reach
  readerd. A person's program can draw a fake password prompt, but cannot
  have it said after the sound, as it cannot make the fake one take keys
  (ADR 0251). What is typed at consoled's prompts is never said.
- **The reader key is Caps Lock or Insert**, as in NVDA and Orca's laptop
  layout. Held, the keys pressed with it are the reader's and reach no
  program: Up and Down read the lines around a review cursor, Left and
  Right words, Comma and Period characters; L, A, T and H say the line,
  read on, name the console and list the keys; Page Up and Page Down
  change the rate, which readerd keeps. Pressed twice quickly it is
  itself. Ctrl alone, or any key pressed for a program, stops the
  speech. While readerd is not running, consoled leaves every key as it
  was.
- **It starts from the stick's first screen.** The install stick's menu
  has *Install Dukovo with speech*, on the hotkey S, which adds the
  kernel flag `speech`; GRUB plays a short rising tune on the PC speaker
  before the menu shows, so that a person who cannot see it knows when
  to press S. readerd runs when `speech` is given or `reader.toml` says
  `on = true`, and ends at once otherwise. `reader(1)` turns it on and
  off and sets the voice, rate and pitch. The flag `reader-log` logs what
  is said, for the boot tests; nothing is logged without it, since the
  screen may show what is private.

## Consequences

- The `screen-reader` boot test starts the system with `speech
  reader-log`, follows readerd's log through the first screen, a
  command's output, the reader keys and `reader say`, and finds the
  trusted tones and speech after them in what QEMU recorded of the
  virtio sound card. The lab's `sound/speech` test has the real machines
  speak through their own outputs.
- The installer's screens are read as any console's text is. Telling the
  installer's own focus and choices apart, larger text, and sticky and
  slow keys come next (Phase 14, PR 7b, ADR 0325), and braille displays
  through BRLTTY after (PR 7c, ADR 0326).
- consoled's text capture costs nothing while readerd is not running:
  only the shown console's text is kept, and only while readerd listens.
- A graphical desktop's screen reader is for when there is one; this one
  reads the consoles.
