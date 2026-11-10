<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0437. SD card slots are drives whose cards come and go, and interrupt where they can

- Status: Accepted
- Date: 2026-11-10

## Context

ADR 0247 drives the SD cards in an SD host controller's slots at boot, a
polled queue for each, and names each card `mmcblk0` and on. A card put
in later was never seen, and one taken out left its disk behind. ADR
0430 made drives whose medium comes and goes for USB card readers: such
a drive keeps its name while empty, a device of no sectors, its driver
looks at it every two seconds, and volumed(8) mounts each medium put in
at its label and unmounts each taken out.

- An SD host controller has a card detect for each slot: Present State
  says whether a card is in it, once the controller has debounced it,
  and whether its write-protect switch is set; Normal Interrupt Status
  has a bit for a card put in and one for a card taken out, which stay
  set until cleared. QEMU's controller holds a card put in back from
  Present State until the removal before it is cleared.
- A slot's capabilities say whether it takes a card that comes and goes,
  or holds a device soldered to the board, an eMMC.
- A controller signals its status by an interrupt, MSI where it has the
  capability. The kernel routes MSI, and no PCI INTx: ADR 0247 left the
  interrupt off. QEMU's `sdhci-pci` has no MSI.
- Linux names a card's disk `mmcblk<n>` when the card is set up, and
  takes it away when the card is taken out; it waits 200 ms after a card
  is put in before powering it.

## Decision

- **A slot for cards that come and go is a drive whose medium comes and
  goes** (ADR 0430), named `mmcblk0` and on, a slot at a time, at boot,
  empty or not. While empty it is a device of no sectors. A thread for
  each controller looks at its slots' card detects every two seconds. A
  card put in is given 200 ms to settle, set up, and takes the slot's
  place with its partitions; one taken out, or swapped for another
  between two looks, is taken out of the kernel, and what was mounted
  from it is lost. `/kernel/block/<slot>/device` is the card's, and
  nothing while the slot is empty.
- **The card bits of Normal Interrupt Status are the card detect's**
  (`radix_sdhci::Detect`): a command clears every other bit before it
  goes and once it ends, and only the thread watching the slot, or the
  interrupt handler, clears those two, so that a card put in or taken
  out between two looks is seen.
- **eject(1) lets a card go:** an SD card cannot be pushed out, so the
  card leaves the kernel, its slot is stopped and its power turned off,
  and the card is left alone until it is taken out, as a floppy drive's
  disk is (ADR 0435).
- **A card's slot is set up by the watching thread alone, and never
  while the queue of the card before it may use the slot's registers.**
  A card's queue uses the registers only while its card is not gone,
  under a lock that the thread takes to say the card is gone; a reset
  of the slot after a request went unanswered ends before the thread
  goes on.
- **A controller with MSI interrupts** for its requests' ends, an error
  and a card put in or taken out: the handler hands each card's queue
  what finished, and wakes the thread for the card detect. A controller
  without MSI is polled, as before.
- A slot for a device soldered to the board is set up at boot only, as
  before; eMMC itself comes later in 11h.

## Consequences

- The boot test `sd-card-change` boots with an empty SD card reader,
  puts a card in with QEMU's monitor and finds it mounted at its label,
  lets it go with eject(1), puts it in again, swaps it for another, and
  pulls the second out while mounted. `storage-sd` runs the storage test
  on four cards in the readers at boot, as before.
- QEMU's SD card takes no read-only image, so a write-protect switch is
  checked by the unit tests against the simulated controller, which put
  a write-protected card in a slot after another. QEMU's controller has
  no MSI, so the interrupt is checked by the unit tests too, and by the
  lab on a reader that has it.
- A machine with an empty slot before a full one names the card after
  the empty slot, `mmcblk1`, where ADR 0247 named it `mmcblk0`: a slot
  keeps its name whatever card is in it.

## Alternatives considered

- **Naming each card as it is set up, as Linux does.** A slot then has
  no name while it is empty, and volumed(8) could not tell a card taken
  out from a reader unplugged; ADR 0430's drives keep their names for
  that.
- **Routing the controller's PCI INTx.** It needs the ACPI routing of
  every PCI slot's interrupt pins, which the kernel does not read yet;
  every controller since version 3.00 that a laptop has offers MSI.
- **Polling the card detect only, with no interrupt.** The look is one
  register's read, but a polled queue has its waiters poll each request
  to its end, which an interrupt spares them.
