<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0438. SD cards run at the fastest speed both the card and the reader have

- Status: Accepted
- Date: 2026-11-10

## Context

ADR 0247 sets every SD card up at the default speed, 25 MHz on its
four-bit bus, about 12 MB/s. Nearly every card sold since 2010 does
High Speed, 50 MHz at 3.3 V, and most since 2014 do UHS-I, at 1.8 V:
SDR50 at 100 MHz and SDR104 at 208 MHz, about 104 MB/s.

- A card says which version of the Physical Layer specification it
  follows in its SCR, read with ACMD51; from version 1.10 it answers
  CMD6, which in its check mode says which functions of each group it
  has (group 1 its bus speed modes, group 4 its current limits) and in
  its switch mode switches to them.
- A controller says in its capabilities whether it has High Speed,
  1.8 V and each of SDR50, SDR104 and DDR50, and whether SDR50 needs
  tuning; Maximum Current says the most current it gives at each
  voltage. From version 3.00, Host Control 2 holds the UHS-I mode, the
  1.8 V signaling and the tuning.
- A card asked for 1.8 V in ACMD41 (S18R) says whether it will switch
  (S18A); CMD11 then switches it, the host stopping the clock while the
  card holds DAT0 to DAT3 low, and the card lets them go high once the
  clock runs at 1.8 V (the Host Controller specification, 3.6.1). A
  card that agreed and did not switch has to be powered off before it
  can be set up again.
- At SDR104, and SDR50 where the controller asks, the sampling point is
  tuned: the host sends CMD19 and the controller tries points on the
  pattern the card sends back, 40 times at most, until it settles on
  one. A point that drifts, with heat or voltage, shows as data CRC or
  end-bit errors.
- QEMU 8.2's `sd-card` follows version 2.00, has High Speed and answers
  CMD6, but has no CMD11; its `sdhci-pci` is of version 2.00 with a
  base clock of 52 MHz.

## Decision

- **A card of version 1.10 or later is asked, with CMD6, which bus
  speed modes it has**, and is switched to the fastest that both it and
  the slot have. At 3.3 V that is High Speed, with Host Control 1's
  High Speed bit, up to 50 MHz.
- **A controller of version 3.00 or later with 1.8 V and a UHS-I mode
  asks the card for 1.8 V**; a card that agrees is switched with CMD11
  and then to the first of SDR104, DDR50, SDR50 and SDR25 that both
  have, with the highest current limit of 400, 600 and 800 mA that the
  card takes and the slot gives at 3.3 V.
- **SDR104, and SDR50 where the controller asks, is tuned before the
  card is used.** A mode that does not tune gives way to the next.
- **A card that agreed to 1.8 V and did not switch is powered off and
  set up again at 3.3 V**, without asking.
- **A data CRC or end-bit error in a tuned mode has the slot tuned
  again before the next transfer**; a card that no longer tunes is set
  back to SDR25, which is not tuned, and the transfer that failed is
  retried as any other.
- The kernel's log names the mode with the clock:
  `QEMU!, High Speed at 26 MHz on a 4-bit bus`.

## Consequences

- In QEMU, a card runs at High Speed, 26 MHz from the 52 MHz base, where
  it ran at 13 MHz: the boot tests `sd-card-change` and `storage-sd`
  check the log's line, and `ob bench`'s SD rows run at that speed.
- QEMU has no UHS-I card or controller, so the voltage switch, the
  modes, the current limit, tuning, a failed tuning, a failed switch
  and tuning again after garbled data are checked by the unit tests
  against the simulated controller and card. A real UHS-I card in a
  laptop's reader goes on the lab's list.
- DDR50 is tried before SDR50 only when the slot lacks SDR104: a card
  with SDR104 that does not tune gives way to DDR50, which needs no
  tuning and moves as much as SDR50 does at half the clock.

## Alternatives considered

- **Retuning on a timer**, every so many seconds, as the controller's
  re-tuning timer offers. A transfer that came back garbled is retried
  after tuning anyway, and the timer's mode 1 needs the controller's
  interrupt, which a polled reader does not have.
- **UHS-II.** Its second row of pins and its own protocol are another
  driver; UHS-I cards and readers are far more common.
