<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0363. dukofetch shows the system at a glance, beside Duke

- Status: Accepted
- Date: 2026-11-03

## Context

People who show off a system, or ask for help with one, run neofetch or
fastfetch: a logo beside a dozen lines on the release, kernel, uptime,
shell, processor, memory and disk. neofetch is archived and is a bash
script; under Nukesh it runs, but it reads Linux's `/proc`, `/sys` and
package managers, so on Dukovo it finds little and guesses the rest.
fastfetch reads the same sources in C. Radix has its own: the kernel's
version in `/kernel/version`, the uptime and memory in `/processes`,
the CPUs in `/kernel/cpus`, the PCI functions in `/kernel/bus/pci`, the
interfaces in `/kernel/class/net`, and the bundles bundled(8) keeps.

The logo is the brand's: Duke, Dukovo's mascot. The maintainer approved
a mock of the output first, as every change to what people see is.

## Decision

- **dukofetch is a systools command** (ADR 0096) that reads those
  sources through the helpers lscpu(1), lspci(8), ip(8), df(1) and
  bundle(1) already have, and shows `user@host`, then the release with
  the machine, the kernel, the uptime, the bundles, the shell, the
  terminal, the processor with its CPUs and clock, each display
  controller, the memory in use, the root filesystem's room and type,
  and each interface's IPv4 addresses. A line whose source is missing
  is left out rather than shown blank or guessed: there is no line yet
  for the features installed or for updates, whose sources do not exist.
- **The lines say things as neofetch says them**, so that they read as
  people expect: the processor's name without `(R)`, `(TM)`, `CPU` and
  its clock, with the CPU count and the highest clock CPUID leaf 0x16
  gives, or the one the name ends with; a display controller as its
  vendor's short name and the bracketed model the PCI ID database gives;
  memory in use without the page cache, as free(1)'s `used`; the disk's
  percentage as df(1)'s.
- **Duke is drawn in half blocks**, two pixels to a character cell:
  `▀` in the top pixel's colour on the bottom one's, or `█` where they
  are the same. He is 16 rows high on a terminal 80 columns wide or
  more, and 10 rows high, beside the eight lines that matter most, below
  that. The pictures are in the source as rows of letters, one for each
  of 26 colours: the brand kit's colour mark scaled down, with shades
  closer than 4 in 255 in each channel made one.
- **Colour follows the terminal**: 24-bit when COLORTERM is `truecolor`
  or `24bit`, as terminals that have it say, and otherwise the nearest
  of xterm's 6x6x6 cube and grey ramp, which every 256-colour terminal
  shares, as consoled's own mapping picks them. The labels are the
  brand's tan, the separators its steel, and the values the terminal's
  own colour, so that they read on any background.
- **Plain text when colour is unwanted**: with NO_COLOR set and not
  empty, TERM `dumb`, or the output not a terminal, the lines are
  `label: value` without Duke, which `--no-logo` also gives on a
  terminal. `--json` gives scripts the facts in bytes and seconds, with
  a key left out where its source is missing.
- **The consoles draw the half blocks themselves when their font lacks
  them.** Spleen's 8x16, which consoled and the kernel's console pick on
  a screen narrower than 1200 pixels or shorter than 960, has no block
  elements or box-drawing lines, so Duke and the rule under the name
  came out as `?`; its 12x24 and 16x32 have the halves and lines but not
  the eighths. radix-fbcon now draws the glyphs that are one band of
  whole rows (`▀`, `▁` to `█`, `▔`, `─` and `━`) when the font has none
  of its own, as the terminals foot and kitty draw block elements
  themselves.

## Consequences

- `dukofetch` is in `/system/commands`, with a man page, and the posix
  boot test runs it into a pipe, as JSON, and on script(1)'s terminal in
  256 colours, in 24-bit colour and with NO_COLOR.
- On a terminal with 256 colours, Duke's steel blue is nearest a grey
  of xterm's ramp, so he is grey and tan there; true colour keeps his
  blue.
- The terminal line names the program on the other side only where the
  environment says it: sshd's SSH_CONNECTION, an emulator's
  TERM_PROGRAM, or consoled's TERM `linux`. A console session's
  terminal is the pseudo-terminal consoled opened, `pts/N`, not the
  console's number.
- Lines for the features installed and for updates come when those
  have sources, and the bundles line can then say how many updates wait.
