<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0708. The secure key is seen in the kernel and pauses every other reader

- Status: Accepted
- Date: 2026-11-27

## Context

The screen and the keyboard are where a password is typed. A person
who is asked for one needs a way to know that the system is asking,
and that only the system reads what they type next.

Ctrl+Alt+Delete has been that key since ADR 0251, and consoled saw
it: it read every key and showed its menu when the three were down.
That holds only while consoled reads the keyboard first. A program
that is lent the screens (ADR 0702) reads the keyboard itself, so it
would see the key before consoled, could keep it, and could draw
something that looks like the system's answer.

Windows sees its secure attention key in the kernel, below every
program. Linux has a key of that kind for the text console only, and
it ends the programs on the console; a graphical desktop on Linux or
macOS has none.

## Decision

- **The input hub sees the key** (ADR 0706): Delete, or the keypad's,
  going down while a Ctrl and an Alt are down, on any keyboards. It
  is looked for in every report, before any reader is given it.
- **It acts only if a program holds `/devices/display/attention`.**
  One description holds that file at a time. Its holder is the one
  the person can trust: consoled now, and the trusted prompt of the
  security release after it. With no such holder the three keys are
  ordinary keys.
- **The hold begins in the call that brought the key.** From the
  press of Delete, which reaches no reader at all, only descriptions
  tied to the attention holder are given events. Every other
  description of every input file is paused, tied to a holder or not:
  a device's file reads SYN_PAUSED, `/devices/kbd` reads each key
  that was down go up, and `/devices/mouse` its buttons. A key that
  was repeating stops.
- **Then the display layer raises the attention holder.** It is put
  over the whole stack of holders, whoever is in front and whoever
  lent to whom, and told EVENT_ATTENTION. Its state is made as a
  holder's is when it gets the front back, so its picture shows in
  one beat. The stack under it stays as it was, and nothing in it can
  change what shows.
- **The attention holder ends the hold** with ATTENTION_END, or by
  ending. The top of the stack has the front again and is told, and
  the paused readers go on: a device's file reads SYN_RESUMED and
  asks what is down. A holder that ends while another is raised over
  it is taken out of the stack as it would be otherwise.
- **No request does any of this.** No program can turn the key off,
  be given it first, raise itself, or pause another program's
  readers. The one way to the top of the stack that is not a loan is
  this key, and the one holder it can raise is the attention file's.
- **consoled is that holder.** Its display file is the attention
  file, and its reader of `/devices/kbd` is tied to it. Told
  EVENT_ATTENTION, it clears the password prompt on the shown
  console, or shows the menu of ADR 0251. It sends ATTENTION_END
  when nothing the key brought up is left: the menu is closed, and a
  prompt the key cleared is answered.
- **The event says whether the front passed.** Its count is 1 if the
  key put consoled in front of another holder, and 0 if consoled was
  in front already. Over another holder, Esc gives that holder the
  front back, and Enter keeps the consoles in front of it, for a
  sign-in or a shell, until the key is pressed again and its menu
  closed with Esc.
- **Lost events end a hold.** If consoled's queue of the layer's
  events overflowed, the key's event may be among the lost. consoled
  then sends ATTENTION_END, so that a hold never outlasts what it
  can show for it.
- **consoled still knows the three keys itself,** for two cases the
  hub cannot see: sticky keys, where Ctrl, Alt and Delete are pressed
  one after another and are never down together, and a braille
  display, whose keys do not come through the hub. Both reach
  consoled only while the consoles read the keyboard, and show the
  same menu.

## Alternatives

- **consoled goes on looking for the key.** It would have to read
  every keyboard all the time, also while a desktop has the screens,
  and nothing would stop the desktop from reading the key too.
- **The key asks the program in front to step aside.** A program that
  does not answer, or does not want to, keeps the screens.
- **The kernel draws the prompt itself.** A password prompt needs
  text, a keyboard layout and a way to check what was typed, none of
  which belongs in the kernel. Raising one trusted program gives the
  same assurance.
- **A key the person may choose.** A program that can change the
  setting can turn the key off.
- **The hold begins when the attention holder asks.** There would be
  a moment between the key and the hold in which another reader
  could read what follows. The hub begins it before it lets go of
  its lock.

## Consequences

- A person who presses Ctrl+Alt+Delete sees consoled's menu whatever
  had the screens, and what they type next is read by consoled only.
  The `secure-key` boot test presses it while another holder is in
  front and has a second program read every input file through it.
- A program that is in front and has stopped answering no longer
  keeps the machine: the key brings up the menu, which can restart
  it or switch to another console.
- No program but the attention holder ever reads the press of
  Delete in the three keys while consoled runs.
- If consoled is not running, the key does nothing special. With
  the consoles in front, Spark starts consoled again within seconds
  (ADR 0702), and the kernel's log shows meanwhile.
- The attention holder is as trusted as the kernel for what a
  person types into it, so the file is root's alone and only one
  description of it can exist.
- Ctrl, Alt and Delete typed one after another with sticky keys are
  the secure key only while the consoles are in front: sticky keys
  are consoled's, and consoled reads no key while a program has the
  screens. docs/deferred.md lists it for the release that gives the
  desktop sticky keys of its own.
- A consoled that fails while a program it lent the screens to has
  them cannot come back yet: the consoled Spark starts in its place
  finds the front taken, which no holder may take (ADR 0702), and
  ends. Until that program ends, the key raises nobody. The window
  server's step of release 1.1 lets the attention holder wait behind
  the front and be raised from there; docs/deferred.md lists it.
