<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0402. The install stick in the design's colours

- Status: Accepted
- Date: 2026-11-07

## Context

The installer's approved design (ADR 0310) draws every screen of the
install stick in sixteen colours of its own: a slate background, cream
text, tan for what has the focus, steel fields and a steel bar of key
hints, mint ticks. Its boot menu is the same: the name spaced out, the
entries in a box, the chosen one on tan. The first stick drew none of
that. The screens picked the nearest of the console's 256 colours, which
came out salmon, purple and lavender on black; the console's font was
its smallest, chosen for 100 by 40 cells, so the 100 by 30 screens sat
small in a corner; Spleen has no ✓, so the steps were ticked with √,
and in the 12x24 size with `?`; GRUB showed its plain white-on-black
menu; and between the menu and the first page the screen was black,
with a cursor, while the installer checked the release.

## Decision

- **The screens set the palette.** consoled reads the sequences xterm
  and Linux's console use to change a palette index's colour (OSC 4 and
  OSC 104, `ESC ] P` and `ESC ] R`), for each console alone, and paints
  the margins round the cells in index 0, the background. The screens
  send the design's sixteen as indices 0 to 15 when they start and give
  them back when they end; a new session starts with the colours as
  they were. They draw in those sixteen only, so a terminal that cannot
  change its palette shows the design in its own sixteen, the focus
  yellow and the fields blue.
- **Nothing in colour 0 is bold.** The console shows bold as the bright
  colour, as Linux does, and the focus's dark text would turn grey.
- **The installer's font.** Under the kernel flag `install`, consoled
  picks the largest font that gives 100 by 30 cells: 12x24 at 1280x800,
  16x32 at 1920x1080. The other consoles of that boot share it.
- **Glyphs of our own beside Spleen's.** The console's font crate draws
  ✓ in all three sizes, and ↑ and ↓ in 12x24, from bitmaps written as
  text in its source, where Spleen has none. The font files stay as
  Spleen ships them; the glyphs are the project's, under its licence.
- **A startup screen.** The installer draws the name, a bar and
  *Starting the installer* first, then fills the bar as it finds the
  release and checks its signatures, reads the machine's disks and
  cards, and lists the setup files, before the first page. consoled
  hides the cursor of a program it runs once until it draws. When the
  installer cannot start, the startup screen says why and waits for
  Enter: printed after the screens end, the reason would be cleared by
  the shell consoled starts next on tty1.
- **The boot menu themed.** `ob image` writes a GRUB theme beside the
  stick's and the ISO's menu: the layout in the design's colours, its
  box as tall as the menu's entries; the console's Spleen 12x24 and
  16x32 as GRUB's PF2 fonts, made from the same PSF files, with the
  glyphs above; and the box's pictures as PNG files, made by `ob`
  itself. GRUB's build has no `grub-mkfont`, which needs FreeType, and
  the theme needs nothing else. The images' GRUB holds `gfxmenu` and
  `png`. The menu keeps UEFI's screen mode; under a BIOS it takes
  1280x800, 1024x768 or 800x600, the first the card has. The serial
  port keeps GRUB's plain menu, and a screen GRUB cannot draw on shows
  the plain one too.

## Consequences

- The stick looks the same from the menu to the last page, on a
  console and over a serial terminal that has xterm's palette codes.
- The installed system's menu stays plain; the boot entries of Phase
  14's part 4b come with its own design.
- The theme's fonts are Spleen's glyphs, so the stick carries Spleen's
  notice, as every image does (ADR 0015).
- GRUB's images grow by the two modules, about 50 KiB before
  compression; `core.img` stays far inside its MiB.
- The boot tests read the screen by the font the console logs, so the
  larger font changes nothing for them; the theme shows only on the
  screen, and the boot tests' live menu does not wait.

## Alternatives considered

- **The nearest of the 256 colours.** What the first stick did: the
  cube's steps are too far apart for the design's greys and tans.
- **True colour cells in consoled.** Every cell would carry 24-bit
  colours where it carries two indices; the palette is how Linux's
  console and xterm let a program choose, and costs nothing per cell.
- **Editing Spleen's files.** A glyph added to a binary PSF file cannot
  be read in review; the glyphs in source can, and the files stay
  upstream's.
- **A GRUB theme with grub-mkfont's fonts.** It needs FreeType and an
  outline font, and would not match the console's cells.
