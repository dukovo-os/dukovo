<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0426. Quiet boot

- Status: Accepted
- Date: 2026-11-10

## Context

An installed system showed GRUB's menu for three seconds and then the
kernel's log, a few hundred lines of it, before the console service
took the screen with the sign-in. That is what a developer wants from
a test image, and not what a person who installed Dukovo wants each
morning: the menu is a choice they rarely make, and the log is noise
unless the boot goes wrong, when it is the first thing anyone needs.

The plan's Phase 14 asks for a quiet boot: no menu unless asked for,
a startup screen in place of the log, and the log a key away.

## Decision

- **GRUB's menu is hidden.** An installed system's menu
  (`image/grub/installed.cfg.in`) waits a second with the menu hidden,
  in which Esc or F4 shows it, as GRUB does for a hidden timeout. Shift
  held as the computer starts shows it at once, for `boot.timeout`
  seconds, which GRUB's `keystatus` reads (its module is in the image's
  GRUB). A computer with Windows shows the menu each time, for at least
  five seconds (ADR 0314), as the choice is then a real one. A
  `boot.timeout` of 0 skips the hidden second, as the test images do.
- **The kernel's `quiet` flag draws a startup screen.** The menu's
  Dukovo entry gives `quiet`; the startup trouble entry does not, as a
  person who chose it is looking for what goes wrong. With it, the
  framebuffer console draws the startup screen as soon as it starts, in
  the installer's colours: Dukovo's name, in the built-in font drawn
  several times its size, and a bar below it. The bar moves at four
  points of the kernel's boot to half way when Spark starts, and from
  there an eighth of the rest with each process made, so it is never
  full until the console service takes the screen.
- **The log is kept, and shown when it matters.** The framebuffer
  console keeps writing the log into its grid without drawing it, so
  nothing is lost. It is drawn in place of the startup screen when Esc
  is pressed (the keyboard thread), when an error is logged, when the
  kernel takes the screen over for a panic, or a minute
  after the boot began if no program has taken the screen, so a boot
  that stops shows why. A program that takes the screen
  (`/devices/fb0`) ends the startup screen, and gives back the log.
  The serial port, the crash log and dmesg(1) get every line either
  way.

## Consequences

- An installed system goes from the firmware's logo to the startup
  screen and then the sign-in, with no menu and no log shown, unless
  Shift or Esc is pressed or something goes wrong.
- The `quiet-boot` boot test checks the startup screen, its bar and Esc
  from a screenshot; the lab's `boot/quiet-boot` test checks Shift, Esc
  and F4 at GRUB, and the look, on real machines by hand.
- Showing the menu after a boot that failed needs the system to mark a
  boot as good once it has started, in GRUB's environment block. That
  belongs with the boot counting the system's updates need, and is left
  to that work.
- The startup screen ends when the console service starts, which then
  draws its own screen while the services finish starting; it does not
  show their progress.
