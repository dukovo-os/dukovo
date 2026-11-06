<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0310. The installer's screens

- Status: Accepted
- Date: 2026-11-06

## Context

Phase 14a, the fourth step of its third part. `dukovo-install --answers`
installs the signed release from an answers file (ADR 0309); the plan's
installer also has screens, which the approved design draws: a step bar,
a page a step, a review that is the only step that changes the
computer, the install's progress with its log a key away, and help on
every page. The stick's boot menu starts the live system with a shell
on every console (ADR 0308) and has no entry that installs.

## Decision

- **The screens ask, the engine installs.** `dukovo-install` with no
  arguments shows the screens. They build the same `Answers` an answers
  file holds and check each page's answers with the same
  `Answers::check` when Next leaves it, so a page refuses exactly what a
  file would. The review's Install hands the answers to the engine of
  ADR 0309 in a thread of its own; the screens draw its progress from
  the install log's events (each step, each line, the bytes copied), so
  the log the screens show is the log the installed system keeps.
- **Five steps now:** Welcome (language, keyboard layout, a box to try
  it, step by step or a setup file), Network and name (cards, DHCP or a
  fixed address, the computer's name, the time zone), Disk (the target,
  the guided layout and the swap's size), You (the administrator and
  other people) and Review (everything the install does, and the disk's
  name typed to confirm it is erased). The Software step comes with the
  profiles and features of the plan's fifth part, the accessibility
  switches and *Install with speech* with its seventh, and the free-space
  and manual layouts with the next step of this part.
- **Setup files on the stick.** A setup file is an answers file called
  `install.json` at the top of the stick's EFI system partition, FAT,
  which any computer can write. The Welcome page's *From a setup file*
  lists it with what is wrong with it, if anything; *Load* goes straight
  to the review, where the disk's name is still typed to confirm. The
  Done page saves the answers there for the next computer, without the
  passwords' hashes unless they were asked to be kept. Spark leaves EFI
  system partitions unmounted, so the installer mounts it only to read
  or write the file.
- **Ratatui, through crossterm.** The screens draw with Ratatui 0.30
  (MIT) on crossterm 0.29, which Nukesh already uses and `ob` already
  patches for Radix: no terminal code of our own, and the same code runs
  in a terminal on Linux for its tests, which draw every page on
  Ratatui's test terminal at the design's 100 by 31 and at 80 by 24.
- **Every message in Fluent.** What the screens say is in
  `messages/en-US.ftl`, in Project Fluent's syntax (fluent-rs 0.16,
  Apache-2.0 OR MIT), built into the program. A translation is a copy of
  the file named for its language; Fluent's plurals and variables keep
  word order a translator's to choose. A message missing from the file
  shows as its id in brackets, which the tests look for. The engine's
  own errors and the answers check's stay in English for now, as the
  log keeps them.
- **The console's glyphs only.** The console's Spleen font lacks ✓, ⚠
  and ▼, so the screens use √, ! and `… more`, which it has, as it has
  the box lines, `‹ ›`, `•`, `·` and the bar's `█░`.
- **Keys as the design gives them.** Tab and Shift+Tab or Up and Down
  move, Space chooses, Enter goes on, Esc goes back, F1 shows the page's
  help, L shows the log during and after the install, and Alt+F2 is
  consoled's own: a shell on tty2 throughout. Letters typed on a list,
  the time zone's or the keyboard's, find the first name that starts
  with them, as a list box does, so `america/den` finds America/Denver
  among some 300 zones; a pause of a second starts again. The keyboard
  layout chosen takes effect at once through keymap(1), so the passwords
  are typed as they will be at the sign-in.
- **The stick's menu installs by default.** `image/grub/live.cfg.in`
  gains *Install Dukovo*, first and the default, which adds the kernel
  flag `install`, read by consoled: tty1's first session runs
  `dukovo-install` as root in the shell's place, and the shell once it
  ends, by *Live console* on the first page or after the install. The
  other consoles have root's shell, as *Try Dukovo* gives every console.
  The entries have ids (`install`, `try`, `first-disk`) and the menu's
  default is a placeholder `ob image` fills: `install` on a release's
  stick and ISO, `try` in the boot tests' image, whose cases add
  `install` to their own command line when they test the screens.
- **The boot test `install-screens`** boots the stick with `install`
  beside a blank virtio disk and a network card, and answers every page
  by keys as a person would: F1's help, *Live console* and the shell on
  tty1, `dukovo-install` from there, a computer name, a time zone typed
  by name, the disk, alice with a password typed twice, the disk's name
  at the review; then the progress to Done, L's log, the setup file
  saved on the stick and read back from tty2's shell. The host checks
  the disk as for `install-uefi`, and the second boot, from that disk
  alone, signs alice in with the password typed in the screens.

## Consequences

- An install by the screens and one by an answers file run the same
  checks and the same steps, and the Done page's setup file repeats the
  install on the next computer.
- The installer grows by Ratatui and Fluent with the crates they use,
  all under MIT, Apache-2.0, Zlib, BSL-1.0 or Unicode-3.0 licenses
  (credits/ratatui.md, credits/fluent-rs.md).
- The screens test caught a Btrfs race: a sync could return while
  write-back's own sync was still writing pages it had taken, so a
  snapshot taken right after a copy, as the slot is made, held the last
  files without their data. Syncs of a Btrfs mount now take turns (a
  lock held through each), and a host test that fails without it keeps
  it so.
- The screens are English only until the first translation; the
  language list shows that more come in a later release.
- *Install with serial console*, a setup file from HTTPS, and the
  Software step come with the plan's fifth part; *Boot from first disk*
  is tested with the next step of this part.
