<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0445. SD host controllers that need more than the specification, and SDMA

- Status: Accepted
- Date: 2026-11-10

## Context

ADR 0247 drives an SD host controller as the SD Association's *SD Host
Controller Simplified Specification* describes it, with ADMA2
descriptors, and refuses a controller without ADMA2. Some controllers
in laptops sold over the last fifteen years either lack ADMA2 or need
more than the specification says. The BSDs, whose drivers are
BSD-licensed, record which ones and what they need: FreeBSD's
`sys/dev/sdhci/sdhci_pci.c`, OpenBSD's and NetBSD's
`sys/dev/pci/sdhc_pci.c`.

- **Ricoh's R5C822, R5U822 and R5U823** (PCI vendor 0x1180, devices
  0x0822, 0xe822 and 0xe823): their ADMA2 does not work, whatever their
  capabilities say (NetBSD). The R5U822 and R5U823 run their cards
  better in SD 2.0 mode with a base clock of 50 MHz than at their own
  200 MHz, set through a key in their PCI configuration space
  (OpenBSD, FreeBSD), and the R5U823 brings bus power up only when the
  voltage and the power bit are written together (NetBSD).
- **JMicron's JMB38x** (0x197b:0x2381) is kept stable by resetting its
  command and data lines after each request (FreeBSD).
- **O2 Micro's** controllers have no quirk in any of the BSDs.
- **SDMA**, the specification's first DMA, moves one buffer from an
  address below 4 GiB. It stops at each boundary of memory the Block
  Size register names, raising DMA Interrupt, and goes on when the
  address it stopped at is written back. Every controller has it; it is
  the only DMA of one without ADMA2.
- QEMU 8.2's `sdhci-pci` takes its capabilities as a property, so one
  without ADMA2 can be booted; it has none of the BSDs' controllers.

## Decision

- **A table in the driver, by PCI vendor and device,** says what a
  controller needs, from the BSDs' records. A controller not in it,
  O2 Micro's among them, is driven as the specification says.
- **A controller without ADMA2, or whose ADMA2 is broken, moves data by
  SDMA,** at a boundary of 512 KiB: a request's pages are handed over
  when they lie one after another below 4 GiB, and go through the
  slot's bounce buffer otherwise, as a 32-bit slot's do. A controller
  with neither is still refused.
- **The Ricoh mode is written to the R5U822's and R5U823's
  configuration space** when the controller is bound, before its slots
  are read; the R5U823's bus power is written once, with the voltage.
- **A JMB38x's command and data lines are reset after each request**,
  and the next request waits for the reset to end.
- The kernel's log names a controller that needs something, and what:
  `the SD card reader at 0000:03:00.1 is a Ricoh R5U823: SDMA, as its
  ADMA2 is broken, power on in one write, SD 2.0 mode at 50 MHz`; a slot
  that moves data by SDMA says so after its version.

## Consequences

- The storage boot test runs one of its four cards in a reader without
  ADMA2, so SDMA, its bounce buffer and its boundaries are checked on
  every change that touches them; the quirks themselves are checked by
  unit tests against the simulated controller, and the laptops that
  have these controllers go on the lab's list.
- SDMA is slower than ADMA2 for a request whose pages lie apart, as
  they are copied through the bounce buffer; the controllers that need
  it are ones that would otherwise not be driven at all.

## Alternatives considered

- **Linux's quirk list**, which is longer. Its driver is GPL-licensed,
  so it is not a source Dukovo takes from; a controller found to need
  more is added to the table from the lab, or from a BSD that records
  it.
- **PIO, the data port, for controllers without working DMA.** Every
  controller in the table has working SDMA, and PIO costs a CPU's time
  for each word of every request.
