<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0580. Intel's chipset Ethernet reaches the I217, the I218 and the I219s of Cannon Lake to Nova Lake

- Status: Accepted
- Date: 2026-11-16

## Context

ADR 0361 gave `radix_intel_net::pch` two generations: the 82579
(FreeBSD's `pch2lan`) and the I219 of Sunrise Point and the chipsets
that have it as Sunrise Point does (`pch_spt`), 13 device IDs. It left
out the I217 and I218 of Lynx Point and Wildcat Point (2013 to 2015),
and every I219 after Comet Lake H: Cannon Lake, Ice Lake, Comet Lake,
Tiger Lake, Alder Lake, Raptor Lake, Meteor Lake, Lunar Lake, Arrow
Lake, Panther Lake and Nova Lake. The I219 is in most Intel desktops
and laptops sold since 2015, so `e1000(4)` named a large part of
current hardware as not driven. This record amends 0361.

Every generation-specific step in the driver was written as
`mac == Mac::Spt` or `mac == Mac::Pch2`. FreeBSD's `e1000_ich8lan.c`
drives all the later chips with the same flow and a handful of
generation tests (`mac.type >= e1000_pch_lpt`, `>= e1000_pch_spt`,
`== e1000_pch_spt`, `>= e1000_pch_tgp`, `>= e1000_pch_mtp`,
`>= e1000_pch_ptp`).

## Decision

- **`pch::Mac` is an ordered list of generations, as FreeBSD's
  `e1000_mac_type` orders them**: `Pch2 < Lpt < LptLp < Spt < Cnp < Tgp
  < Adp < Mtp < Ptp < Nvp`. A step for a generation and the ones after
  it reads `mac >= Mac::X`. FreeBSD's one `pch_lpt` is two variants,
  split by the device-ID test FreeBSD makes twice (leaving ULP, and the
  I218 K1 workaround): `Lpt` is the I217 and the I218-LM2 and -V2,
  `LptLp` the four I218s of the laptop chipsets. Each existing
  generation test is rewritten as FreeBSD's, and `identify` lists
  FreeBSD's `e1000_set_mac_type` IDs for these chips: 52 more.
- **The I217 and the I218 keep the 82579's NVM**, in a flash BAR of
  their own, as FreeBSD attaches them (`< pch_spt`): `Mac::flash_bar`
  is true for `Pch2`, `Lpt` and `LptLp`, which the kernel already maps
  by that test. From Sunrise Point on the NVM is in BAR 0.
- **The later chips take five steps more**, each from `e1000_ich8lan.c`:
  the I218 K1 workaround on `LptLp` (K1 off while the PLL clock is
  requested at 1000 Mb/s, and the in-band transmit timeouts at 10 Mb/s
  and at 100 Mb/s half duplex on PHY revisions up to 5); the K1 exit timeout
  from Meteor Lake on, set before the PHY is probed, after its reset and
  after the MAC's; MDIC retries from Meteor Lake on (a failed
  transaction is tried twice more, 10 ms apart, except while the driver
  probes for the PHY or changes its interface); the DMA clock ungated
  in FFLT_DBG from Tiger Lake on; and dynamic power gating off from
  Arrow Lake on.
- **Where FreeBSD and Linux differ, FreeBSD is followed.** Linux's
  e1000e files Arrow Lake's 0x57a0 and 0x57a1 under its Meteor Lake
  board and Nova Lake's IDs under Arrow Lake's, and has no 0x57b5 and
  0x57b6; FreeBSD's tables are used as they are. Linux's e1000e was read
  only to cross-check; nothing here comes from it.
- **Still left out:** the 82577 and 82578 (Ibex Peak, FreeBSD's
  `pchlan`), whose PHYs are of another kind and need workarounds of
  their own; and Linux's list of two Meteor Lake laptops that need K1
  kept off, which keys on the machine's DMI names, which the driver does
  not see. Energy Efficient Ethernet, wake on LAN and jumbo frames stay
  as 0361 left them.

## Consequences

- The I217, the I218 and every I219 to Nova Lake start as the OptiPlex
  7040's I219 does, and `e1000(4)` says only the 82577 and 82578 are not
  driven.
- No lab machine has an I217, an I218 or an I219 later than Comet Lake
  H, and QEMU emulates none of them. The driver's tests check
  FreeBSD's sequences against a simulated chip of each generation; the
  first real test of these chips is a person's machine. A used
  OptiPlex 7070 or 7080 would put a Cannon Lake or Comet Lake I219 in
  the lab.
- The OptiPlex 7040's I219-LM (0x15b7) and the Precision T1650's 82579
  take the same paths as before; the shared code was rewritten to the
  ordered tests, so the lab's boot of each is the check that they did
  not change.
- A Meteor Lake machine that Linux keeps K1 off on gets K1 as FreeBSD
  has it; if one loses packets, that list is where to look.
