<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0361. Intel's chipset Ethernet: the 82579 and the I219 get a driver of their own in `radix-intel-net`

- Status: Accepted
- Date: 2026-11-03

## Context

The test lab (roadmap item 14f) runs on a Dell OptiPlex 7040, whose
Ethernet is an I219-LM (8086:15B7), and a Dell Precision T1650, whose
Ethernet is an 82579LM (8086:1502). The lab reaches its boxes over SSH,
so without a driver for these two the boxes cannot be tested at all.
Most Intel desktops and laptops since 2011 have one of this family, the
LAN in Intel's chipset, which Linux drives with `e1000e` and FreeBSD
with `em(4)`'s `ich8lan` code. `radix-intel-net`'s `em` (ADR 0197) has
the discrete 8254x and 8257x cards, which share their descriptors and
most of their MAC registers with these chips, but not what is new in
them:

- The MAC is in the chipset (the PCH) and the PHY is a chip of its own
  beside it, joined to it by a link for frames and by SMBus for when the
  host sleeps. The PHY's registers are in pages, at two MDIO addresses.
- The chipset's management engine (ME), which runs Intel AMT on vPro
  machines such as both lab boxes, shares the PHY. A software flag in
  EXTCNF_CTRL says who has it, and the ME can block PHY resets while an
  AMT Serial-over-LAN or IDE-R session is open, which is when the lab
  uses AMT most.
- The PHY may be out of reach when the driver starts: on SMBus, or in
  the I219's ultra low power mode (ULP), from which the ME, or without
  one the driver, must bring it back.
- After each PHY reset, software may need to write the PHY a
  configuration from the NVM's extended region. The NVM is the GbE
  region of the chipset's SPI flash, read through flash registers: in a
  second BAR on the 82579, in BAR 0 at E000h on the I219, 32 bits at a
  time.
- The specification updates list workarounds the driver applies after
  a reset and each time the link changes: the 82579's slow MDIO, noise
  thresholds and K1 power state at 100 and 1000 Mb/s; the I219's
  interpacket gap, PLL clock gate, transmit pointer gap, beacon
  duration, K1-off and latency tolerance (LTR) settings.
- On the 82579 with the ME running, a register write the host makes
  while the ME holds the PCIm-to-PCI arbiter can be lost. FreeBSD waits
  for FWSM's PCIM2PCI bit to clear before every write, and checks each
  ring tail it writes.

QEMU emulates none of these chips, so no boot test can run the driver;
only unit tests against a simulated chip can, until the lab itself runs
it.

## Decision

A new driver, `radix_intel_net::pch`, drives the 82579LM and 82579V
(FreeBSD's `pch2lan`) and the I219-LM and I219-V of Sunrise Point,
Union Point, Lewisburg and Comet Lake H (FreeBSD's `pch_spt`), from
Intel's datasheets with FreeBSD's `e1000_ich8lan.c`, `e1000_phy.c`,
`if_em.c` and `e1000_osdep.c` as the reference for the sequences and
errata (credits/freebsd-network-drivers.md). It shares `em`'s legacy
descriptor rings and the crate's `Card`, so the kernel's network code
treats it as any other Intel card. `identify` knows them as
`Kind::Pch`.

It starts the chip as FreeBSD attaches and initialises one: it gates
the hardware's own PHY configuration, has the ME take an I219's PHY out
of ULP (or does so itself without an ME), reaches the PHY by forcing
SMBus or power-cycling it through LANPHYPC when it does not answer,
checks the PHY's ID, then resets the MAC and PHY together with the
software flag held and does what the PHY needs after: MDIO slow mode
and the noise thresholds on the 82579, the SMBus address, LEDs and the
NVM's extended configuration, and the OEM bits. It sets the errata bits
of `e1000_initialize_hw_bits_ich8lan`, advertises every speed to 1000
Mb/s full duplex with flow control off, as the crate's other drivers
do, turns the jumbo frame workaround off, and raises one MSI.

While the ME blocks PHY resets, the driver leaves the PHY and the link
as the ME set them up and resets the MAC alone.

`pch::start` takes a `Platform` beside the registers: the PCI function,
whose configuration space the reset touches (as FreeBSD does around the
reset, so that no register read lands in it) and the I219's latency
tolerance reads; the 82579's flash BAR, which the kernel maps; and a
clock. The crate's `start` refuses these chips, so `e1000d` (ADR 0198),
which drives only `Kind::Em`, is unchanged.

The running card looks at STATUS each time it has received everything
there is, and when the link changed it applies the link workarounds,
spinning on the clock where it must wait, as it runs under the network
stack's lock and cannot sleep.

On the 82579 with the ME running, every register write, including the
rings' setup and tails, goes through `rings::Interlock`, which waits up
to 100 ms for the arbiter. A tail that does not read back as written
turns its unit off and the card reports that it failed, as FreeBSD asks
for a reset there; the kernel then leaves it stopped and logs it.

Stopping turns interrupts, the receiver and the transmitter off, stops
the chip's DMA and gives it back to the ME (DRV_LOAD), without a reset:
an I219 reset with descriptors in its rings can hang it until the
machine resets, and the next driver resets it at its start, before it
loads a ring, as both Linux and FreeBSD do.

Left out for now: Energy Efficient Ethernet, wake on LAN, jumbo frames,
the I217 and I218 (Lynx Point), the I219s of Cannon Lake and later
(FreeBSD's `pch_cnp` and on), and the 82577 and 82578 (Ibex Peak).
Each is a small step from here when a machine needs it.

## Consequences

- The OptiPlex 7040 and the Precision T1650 have a network, so the lab
  can reach them over SSH once they boot Dukovo, and so do the many
  Intel machines of 2011 to 2020 with one of these chips.
- The driver is tested against a simulated chip (`tests/pch.rs`): the
  start with and without an ME, a PHY on SMBus or in ULP, a PHY that
  never answers, the ME blocking PHY resets, the NVM read from the flash
  BAR and from BAR 0 in its valid bank, the link workarounds and the
  latency tolerance at each speed, the 82579's write interlock and a
  lost tail, and frames both ways. It has not run on the chips
  themselves yet: the lab's first boot is its first real test, and the
  kernel log names the chip and says why when a start fails.
- `rings::Rings::interlocked` lets any driver have its ring registers
  written through an interlock; only the 82579's uses it.
- The flash is only read. Nothing here writes the NVM or changes its
  valid bank.
