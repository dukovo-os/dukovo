<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0815. A display holder's event wakes the polls of its own file

- Status: Accepted
- Date: 2026-11-28

## Context

Each holder of the display layer reads its events from its own file
and waits for them with `poll` (ADR 0700). The kernel kept one list of
pollers for all the holders' files, and woke the whole list after any
change a holder might wait for.

A desktop stands over the consoles' service, which holds the attention
file and lent it the screens (ADR 0716). The desktop asks to be told
when each of its frames shows (ADR 0714), so at every beat the layer
gave it an event, and the consoles' service was woken with it, looked,
found nothing to read and slept again. Counted on a Ryzen 9 9900X3D
under KVM, the service ran 1.01 times for each frame the desktop
showed: 5 µs of its own time, and a CPU brought out of its halt and
put back, about 21 µs of the host's.

## Decision

- **The layer says which holders were given an event**
  (`Layer::take_posted`), and the kernel wakes the polls of those
  holders' files and no others.
- **The holders' files share sixteen lists of pollers**, each file the
  one its description's address chooses. A poll finds its list without
  the layer's lock, and a list is there before the first holder and
  after the last. Two holders whose files share a list wake each other
  as all holders did before: the woken poll looks and sleeps again.

## Alternatives

- **A list of pollers in each holder.** Exact, where sixteen lists let
  two holders in sixteen share one. The layer is library code that
  runs in host tests, with its pictures and cards as type parameters:
  the holder would take a third for the kernel's list, and a poll
  would find the list under the layer's lock, a sleeping lock taken
  where a poll adds itself to what it waits on.
- **A list in every file description.** Every open file of the system
  would carry one for the few that are holders.
- **The consoles' service does not poll while it has lent the
  screens.** The event that the screens are back is one of those it
  polls for.
- **The card's thread, which keeps the beat, also hands on what the
  beat brought** (`sched::lazily_handing_on`, ADR 0710), so that the
  desktop it tells runs after it on its CPU and no idle CPU is woken
  for it. Tried and measured: the desktop wakes the card's thread as
  it asks for its next change, while it runs, so the card's thread
  goes to the other CPU and the desktop follows it there at the beat.
  The two changed CPUs three times a frame between them and the
  desktop ran 3.8 times a frame where it runs 3.4. The CPUs halted
  9.3 times a frame where they halt 9.5, and the frame's row was 1.30
  and 1.37 times Linux's where it is 1.19 and 1.27 without.

## Consequences

- The consoles' service sleeps through the frames of a desktop that
  stands over it: on the same machine it ran once in a hundred frames.
  The CPUs halt as often as before, 9.5 times a frame: the service ran
  on a CPU that the frame's own work had awake.
- A thread that reads a holder's file and waits in the read, and one
  that waits for its change to show, are still woken at every change
  and look again. The desktop and the consoles' service poll.
