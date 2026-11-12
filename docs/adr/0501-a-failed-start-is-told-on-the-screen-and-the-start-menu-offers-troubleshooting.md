<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0501. A failed start is told on the screen, and the start menu offers Troubleshooting

- Status: Accepted
- Date: 2026-11-12

## Context

ADR 0500 records every start and names what changed since the last good
one, but only for someone who already has a shell and knows to run
changes(1). After a failed update the computer falls back by itself and
signs in as usual, so the failure is easy to miss; after a start that
fails a check outside an update's try, it is only logged. Rescue (ADR
0460) repairs, but did not say why it had started. And the start menu
offered no way to start once otherwise, with the kernel's messages, with
fewer services or with the firmware before, short of editing GRUB's
command line by hand.

## Decision

1. **The failure is told where the owner looks.** When a start fails
   its health check and goes on, or follows an update that fell back,
   the health check writes a notice to
   `/system/live/sysupdate/failure`: which start failed and why, with
   its error ID; what Dukovo did (went back to the version before, or
   went on, as the start was no update's try); what changed since the
   last good start, as changes(1) says it; and the support page of the
   error ID, `https://dukovo.org/support/<version>/errors/<ID>.html`,
   which holds nothing of the machine. consoled(8) and serial-console(8)
   show it under the issue text at every sign-in until the next start,
   and notifyd(8) shows its first line on the screen's bottom line until
   a key is pressed. A failed try says nothing itself: the start after
   it does, once it has fallen back.

2. **Rescue says why it started.** After the administrator signs in,
   Rescue shows how many starts in a row were not seen through, as the
   boot menu counts them, and the newest start that failed or never
   finished, with what changed since the last good start and its
   support page. When the newest record is of a good start, the starts
   counted after it stopped before the health check began: in the
   kernel, mounting the root or starting the services. Choice 0 of
   Rescue's menu shows it again.

3. **Troubleshooting on the start menu.** Each slot's menu gains a
   Troubleshooting submenu of starts of the default slot, each chosen
   once, that change nothing that stays:
   - with all of the kernel's messages in place of the startup screen;
   - with only the essentials: the kernel's `essentials` has Spark
     start only the system's own services, those with a file among the
     defaults the system brings, and none an app or an administrator
     added, which stay off until started by hand;
   - with the previous firmware: the CPU microcode of the firmware set
     the previous version names, when it names another, with the kernel
     flag `firmware-from=<slot>` so that the start's record shows it;
   - the previous version, as the menu's own entry starts it.

   Each marks its start as any entry chosen by hand does, so that the
   menu shows again after one that fails. Rescue and the
   startup-trouble entry stay on the menu itself.

## Consequences

- A failure no longer passes unseen: the next sign-in, at the screen or
  the serial port, says what failed and what changed.
- A start that might fail because of something added, or because of new
  microcode, can be tried without it, once, from the menu, with no
  setting changed.
- Not yet: a QR code to the support page and single-key choices on the
  notice; SSH into Rescue when the system had SSH on, which needs
  sshd(8) to check Rescue's sign-in against the root's accounts and
  keys; a start without graphics, which waits for the window manager;
  a summary of the last starts on the EFI system partition for a root
  that will not mount; Repair startup on the install stick; and the
  memory test entry, which joins `hardware check` in 16b, as building
  memtest86+ from source belongs with the hardware tests. docs/deferred.md
  lists each.
