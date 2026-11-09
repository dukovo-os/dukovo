<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0440. ls shows titles, octal modes, icons and colours that mean something

- Status: Accepted
- Date: 2026-11-09

## Context

Chase asked which of [eza](https://github.com/eza-community/eza)'s
features our `ls` should have, and approved mockups of them on
2026-11-09. Our `ls` is uutils' (ADR 0080), which prints what GNU's
prints, and scripts read that output. eza is under the EUPL-1.2, so we
take its ideas, not its code.

This first part covers how the long listing looks: column titles, the
mode in octal, colours for the permission letters, sizes and times, and
icons before names. eza does each of these; GNU's `ls` does none. Later
parts add trees, git status, folder sizes and Dukovo's own columns, in
ADRs of their own.

## Decision

Four additions to `ls`, in a patch of the port
(`ports/uutils-coreutils/patches/0009`, code in `fields.rs` and
`icons.rs`). None changes what `ls` prints unless it is asked for, and
none is on in a pipe by default.

- **`--header`** writes a line of titles over the long listing:
  Permissions, Links, Owner, Group, Size, Modified (Accessed, Changed or
  Created with `--time`) and Name, and Inode, Blocks, Octal, Context and
  Author for those columns. Numbers' titles are aligned right, words'
  left; a column widens to its title. Each listing of `-R` has its own.
  `--dired` leaves it out, since Emacs counts the lines.
- **`--octal`** writes each mode as four octal digits, as chmod takes
  them (`0755`, `4755`), in a column before the letters.
- **`--field-colors[=WHEN]`** colours the long listing's fields by what
  they say. The permission letters: `r` yellow, `w` red, `x` green, `s`
  and `t` magenta, `-` faint, the type letter in its kind's colour.
  Sizes under a KiB faint, of a MiB or more yellow, of a GiB or more
  bold red. Times in the last day bold, over a week ago faint. Titles
  underlined. `auto`, the default, is when the names are coloured
  (`--color`) and standard output is a terminal, so `--color=always`
  into a pipe or file prints what GNU's prints; `always` and `never`
  say otherwise. The colours come from `LS_FIELD_COLORS`, in
  `LS_COLORS`' form with words for keys (`read=33:write=31:...`); a key
  it leaves out keeps its default, one it does not know is skipped, and
  the variable set empty turns the colours off. After each coloured
  field comes `LS_COLORS`' normal colour (`no=`) again.
- **`--icons[=WHEN]`** writes a Nerd Font character before each name:
  by type (folder, link, device, pipe), then by whole name (`.git`,
  `Makefile`, `Cargo.lock`), then by ending, then a program or a plain
  file. The default is `never`, since a terminal without such a font
  shows boxes; `--icons` alone means `auto`, on a terminal. The icon
  takes the name's colour.

Nukesh (ADR 0260) gives `ls` its field colours from colors.toml's `[ls]`:
twelve new keys (`read`, `write`, `run`, `special-bit`, `denied`,
`octal`, `small-file`, `big-file`, `huge-file`, `today`, `old`,
`header`), with the type letter taking `directory`, `link` and `device`.
It exports them as `LS_FIELD_COLORS` beside `LS_COLORS`. The themes
change them as they change the rest: high-contrast fades nothing, light
uses no yellow, colour-blind uses no red against green. `[ls]`'s
`icons = true` adds `--icons` to the alias; it is `false` as shipped.

## Consequences

- `ls -l` on a terminal looks different from GNU's, in colour only;
  its text is the same. `ls -l | ...`, `$(ls -l)` and `ls -l > file`
  are as before, `--color=always` included.
- The icons need a Nerd Font in the terminal. Dukovo's console has none,
  so they are for SSH sessions and terminals of other systems for now.
- The lab's `commands/ls-views` test and the `ls-views` boot test check
  each flag, and that a pipe gets GNU's output.

## Alternatives considered

- **Taking eza as `ls`.** Its options differ from GNU's (`-l` and `-t`
  mean other things), so scripts would break. eza stays a catalog app.
- **Field colours whenever names are coloured.** uutils' own GNU
  comparison tests failed: `--color=always` into a pipe must be GNU's.
- **New keys in `LS_COLORS`.** GNU's `ls` refuses keys it does not know
  and then colours nothing, which would break a GNU `ls` run with
  Dukovo's environment, as in a container.
- **Icons on by default.** Without a Nerd Font each name would start
  with a box.
