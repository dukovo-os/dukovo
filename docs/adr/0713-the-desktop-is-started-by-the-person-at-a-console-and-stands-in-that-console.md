<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0713. The desktop is started by the person at a console and stands in that console

- Status: Accepted
- Date: 2026-11-27

## Context

The screens are consoled's, and a program that draws them borrows them
(ADR 0702). Until now the loan was root's alone: the socket was root's,
consoled closed the connection after its answer, and the only way back
to a console was for the borrower to give the screens back or end.

A desktop is a person's. The programs in it are the person's, they
connect to it, and what they send it is whatever they like. A window
server that ran as root would put a large program, written to read
other programs' requests, between every one of them and the whole
machine.

A person at the machine also expects the consoles to stay where they
are. Alt and a function key show another console; with a desktop on the
first, the same should hold.

## Decision

- **windowd runs as the person who typed `desktop`**, as a child of
  that person's shell, with no right of its own. The screens come from
  consoled and the devices from inputd, each as a descriptor passed
  over a socket.
- **consoled lends to the person at the console that shows.** Its
  socket is open to every account, and who asks is what the kernel says
  of the connection: the screens are lent to root's programs, and to a
  program whose session is the one whose terminal the console in front
  is. Any other is answered `refused`. A person signed in over SSH, or
  at a console that does not show, cannot take the screens from the
  person in front of them.
- **The borrower stands in a console.** consoled answers `lent N`, the
  console that showed, and a borrower that keeps the connection open
  stands in it. Before it gives the screens back for another console,
  it writes `console N`, and consoled shows that one. When the person
  chooses the borrower's console again, consoled lends the screens
  again to the same holder, a description of which it kept: the display
  layer puts back what the holder last showed (ADR 0702), and the
  borrower reads EVENT_FRONT.
- **In the desktop, Ctrl+Alt+F1 to F12 show a console**, and the
  desktop's own console shows the desktop again. They reach no program.
  Ctrl+Alt+Backspace ends the desktop.
- **inputd's socket is open to every account too.** What a program is
  handed there was always decided by the holder it sends (ADR 0707),
  and a holder of the screens is something only root, or a program that
  was passed one, has.
- **The programs' socket is in a folder that is the person's alone**:
  the one `XDG_RUNTIME_DIR` names, or `/tmp/runtime-N` for the person
  of number N, which windowd makes with mode 0700. A folder that is not
  the person's, or that others can open, is not used. The command the
  desktop starts is given `XDG_RUNTIME_DIR` and `WAYLAND_DISPLAY`.
- **`desktop COMMAND` ends when COMMAND does, with its exit status.**
  The console shows again, with what COMMAND wrote there.

## Alternatives

- **windowd as a service that root starts**, lending windows to the
  people who sign in. That is the desktop of a machine with a graphical
  sign-in, which is release 1.3's; it needs the session's protocol and
  the grants first, and a window server that runs as the person stays
  the one that draws that person's windows.
- **A seat service between the two**, as seatd and logind are on Linux:
  a third program that opens devices for the compositor and takes them
  away at a console switch. consoled has the consoles and the display
  layer has the front; the kernel already silences a holder that is not
  in front, and its input with it (ADR 0706).
- **The account of the caller alone**, without the session. Two
  sign-ins of one person, one at the screens and one over SSH, would be
  the same to consoled.
- **The borrower switching consoles itself**, by lending the front back
  with a number. Only consoled knows which console the person may be
  shown: one that waits for a password, one with a notice.

## Consequences

- A person signs in at a console, types `desktop` and a program, and
  works in its window; the console is back when the program ends.
- Every program on the machine can connect to consoled's and inputd's
  sockets. consoled answers one line and a refusal costs it a read of
  the caller's status; inputd hands nothing to a caller without a
  holder.
- The keys that switch consoles are windowd's while the desktop is in
  front, and consoled's at a console. Ctrl+Alt+Delete stays the
  kernel's (ADR 0708).
- A desktop started by Spark or over SSH by root stands in the console
  that showed when it asked.
- When consoled is started again under a desktop, the desktop no longer
  stands in a console of the new one (ADR 0716, `docs/deferred.md`).
