<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0707. inputd hands each input device to the program that holds the screens

- Status: Accepted
- Date: 2026-11-27

## Context

Each input device is a file that only root opens (ADR 0706). A
program that draws the screens itself, a desktop, needs every
keyboard and pointer, and needs to hear of one that is plugged in
while it runs. It should not need to be root for that, and it should
not have to be trusted with more than the keys typed while its own
picture is in front.

Linux gives the job to a session service: the desktop asks it over a
bus for each device, the service opens the file and passes it, and
on a switch of session it takes the file away again. Which program
is allowed is decided by which session the service thinks is active,
kept in a state of its own beside the kernel's.

Here the kernel already knows who is in front, and already silences
a description of an input file that is tied to a holder behind
(INPUT_TIE, ADR 0706). What is left is small: open the files, tie
them, and pass them on.

## Decision

- **A service, inputd, hands the devices out.** It runs as root, as
  the `inputd` service, and listens on a stream socket,
  `/system/live/inputd/devices`, which root connects to.
- **A program asks with what it holds.** Its first record carries a
  copy of its description of the display layer's file (SCM_RIGHTS).
  inputd checks that the copy is a holder by tying a description of
  its own to it, which the kernel refuses for anything else. A
  program with no holder is refused and disconnected. inputd keeps no
  list of programs, users or sessions.
- **The copy can do nothing.** The program narrows it to the one
  right that lets it cross the socket. inputd cannot draw, change a
  screen or read the layer's events through it; it can name the
  holder, which is all INPUT_TIE needs.
- **Every device is tied before it is passed.** inputd opens
  `/devices/input/N`, ties that description to the program's holder,
  and sends it with a record of its number and what it is. The
  program reads the device only while its holder is in front, and
  nothing inputd or the program does later can change that: a tie is
  made once.
- **A device that comes or goes is told.** inputd reads
  `/devices/input/control`, sends each new device to every connected
  program as above, and a record for each that went.
- **inputd reads no event.** A key or a motion goes from the kernel
  to the description in the program that is in front, and wakes no
  other process. inputd waits on the control file and its socket and
  does not run between; its own memory is a table of the connected
  programs.
- **What a device is, is decided in one library.** The record says
  keyboard, keys, pointer, tablet, touch or switch, from the keys and
  axes the device has. inputd, `hardware inputs` and the programs
  that read a device use the same rules (`userspace/programs/input`),
  and the kernel has none.
- **The exchange is 16-byte records** of three numbers, with a
  version in the first, described whole in inputd(8). A program has
  two seconds to send its first record, 16 programs may be connected,
  and one that does not read what it is sent is disconnected.
- **The consoles do not use it.** consoled opens `/devices/kbd`
  itself and ties it to its own holder, so a text console takes keys
  with inputd stopped.

## Alternatives

- **The program opens the files itself.** It would have to be root,
  or the files would have to be open to a group, and then to every
  program in the group whether it is in front or not.
- **A list of allowed programs in inputd.** A second answer to "who
  may read the keyboard" that can disagree with the first. What a
  program holds cannot be out of date.
- **Take the files away on a switch, as Linux's session service
  does.** The service must then act at the right moment on every
  switch, and a program must cope with files that die. The tie makes
  the kernel do it in the call that changes the front.
- **inputd reads the events and passes them on.** Every key would
  cost two more wake-ups and a copy, inputd would see every password,
  and a stall in it would stall the pointer.
- **One device file passed on request, by name.** A desktop wants
  all of them; a list and a stream of changes is fewer messages and
  has no moment where a new keyboard is plugged in and nobody asked.

## Consequences

- A desktop needs no right over the input devices but a holder of
  the screens, which consoled lends it (ADR 0702).
- inputd is small enough to read whole, and holds nothing secret: it
  never has a key in its memory. The `inputd` boot test holds its
  memory under 2 MiB and counts that it does not run while keys are
  typed and the mouse moves.
- inputd's copy of a program's holder keeps that holder alive until
  inputd sees the connection close and drops it. A program that ends
  is gone from the stack of holders one wake-up of inputd later, not
  at its exit.
- If inputd stops, programs keep the devices they have and hear of
  no new one. Spark starts it again, and a program connects again
  for the list.
- A program that is handed a device can ask it for its serial number
  (INPUT_SERIAL); the public list of devices has none.
