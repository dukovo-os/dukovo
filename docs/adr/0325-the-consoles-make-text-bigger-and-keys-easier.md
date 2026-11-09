<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0325. The consoles make text bigger and keys easier

- Status: Accepted
- Date: 2026-11-09

## Context

The screen reader (ADR 0324) serves those who cannot see the screen.
Others see it but need bigger text than the font the screen's size
chooses, or cannot hold Shift with a letter, or brush keys on the way to
another. On Linux the console's font is changed with setfont and its keys
filtered by the X server or a desktop's settings, neither of which runs
before a desktop does; on Windows the same help is a few keys away from
the sign-in screen: Shift five times for sticky keys, holding Right
Shift for filter keys.

The screen reader also reads a full-screen program, such as the
installer's screens, by what changes on the screen. A program that
redraws a list to move a highlight changes a marker and a colour, and
the reader cannot tell which item now has the focus. The program knows.

consoled holds the screen, the fonts, the keys and the screen reader's
connection, so all of this belongs there.

## Decision

- **Text size.** Ctrl+Alt+Plus and Ctrl+Alt+Minus step the text through
  three sizes: *normal*, the font the screen's size chooses (radix-fbcon's
  choice, as before), then *larger* and *largest*, one and two steps up a
  ladder of the built-in 8x16, 12x24 and 16x32 Spleen fonts and the two
  larger ones drawn at twice their size, pixel by pixel. A step is taken
  only while the screen keeps at least 40 by 12 cells. Each console is
  resized to the new grid: rows that no longer fit above the cursor go to
  the scrollback, and its program is told by the terminal's window size,
  which sends it SIGWINCH. The keys are those that type `+` and `-` in
  the layout, or the keypad's, so that Ctrl+Alt+= works on a US keyboard
  and Ctrl+Alt with the `+` key on a German one.
- **Sticky keys.** Shift pressed five times, with no other key between,
  turns them on or off, as on Windows, with a note on the bottom line.
  With them on, Shift, Ctrl and Alt pressed and let go alone hold for the
  next key; pressed twice, until pressed again. A modifier held down with
  a key is as always.
- **Slow keys.** A key counts only once it has been held for a delay, 300
  ms unless set otherwise, and then repeats as usual; let go before, it
  is as if never pressed. Every key waits, modifiers too. They are turned
  on in the settings file, not by a key: turned on by accident, they
  would leave a keyboard that seems broken to whoever did not mean to.
- **The settings** are `/system/settings/console/accessibility.toml`
  (accessibility.toml(5)), beside the welcome banner's: `text-size`,
  `sticky-keys`, `slow-keys` and `slow-keys-delay`. consoled reads it as
  it starts and on SIGHUP, and writes back the text size and sticky keys
  as they are changed from the keyboard, keeping the rest of the file. A
  person's own settings.toml cannot hold them: the consoles' text and
  keys are the same for everyone, and are needed before anyone signs in.
- **A program names its focus.** `ESC ] 7700 ; text ST` has the screen
  reader say `text` at once, over what it was saying. From then on, what
  that program draws is not read by its changes, until it leaves the
  alternate screen (`ESC [ ? 1049 l`), which every full-screen program
  sends as it ends, a crashed one's panic hook included, or the terminal
  is reset. Any program may use it, and its words never come after the
  trusted sound (ADR 0324). While the reader is not listening, the
  sequence does nothing, and other terminals ignore an OSC they do not
  know. 7700 is Dukovo's own number, clear of those xterm, VTE, kitty,
  iTerm2 and Windows Terminal document.
- The screen reader says each change: *Text larger*, *shift*, *shift
  locked*, *Sticky keys on*.

## Consequences

- The `console-access` boot test steps the text size and reads the
  shell's new size with `stty size`, has a program name its focus and
  checks that its drawing is not read meanwhile, turns sticky keys on
  with five Shift presses and types a capital with Shift alone, and
  types with slow keys, where keys tapped type nothing and keys held do.
  The boot tests read the screen in the doubled sizes too.
- The installer's screens, which follow the console's size, name their
  focus with this sequence and offer these settings next (Phase 14, PR
  7b's second part).
- consoled's grid is no longer fixed for its life: anything that keeps
  its size, such as the boot tests' screen reading, takes it from the
  last geometry line consoled logs.
