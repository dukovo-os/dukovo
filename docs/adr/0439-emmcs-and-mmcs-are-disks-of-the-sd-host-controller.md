<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0439. eMMCs and MMCs are disks of the SD host controller, at HS200 where they can

- Status: Accepted
- Date: 2026-11-10

## Context

Many low-cost laptops, tablets and single-board computers boot from an
eMMC: a MultiMediaCard soldered to the board, on an SD host controller
whose slot is marked as holding an embedded device. ADR 0247 and ADR
0437 drive SD cards only: a card that does not answer ACMD41 was
refused as "an MMC card, which is not driven yet", so an eMMC was not a
disk at all.

- JEDEC's *Embedded MultiMediaCard (e•MMC) Electrical Standard 5.1*
  (JESD84-B51) sets an MMC up with CMD1 rather than ACMD41; the host
  gives it its address with CMD3. A device over 2 GB is addressed in
  sectors and gives its capacity in the 512-byte EXT_CSD, read with
  CMD8, which every device of version 4 or later has. Its CID is laid
  out differently from an SD card's, with a six-character name.
- CMD6 writes a byte of the EXT_CSD: BUS_WIDTH sets a bus of one, four or
  eight bits, HS_TIMING High Speed (52 MHz) or HS200 (200 MHz, with the
  I/O at 1.8 V). CMD13's status says whether the device took it.
- A slot can offer eight data lines (the capabilities' 8-bit bit), and a
  board need not wire all eight: a read at a width whose lines are not
  there comes back garbled.
- HS200 is tuned as SDR104 is, with CMD21's blocks: 128 bytes on eight
  bits, 64 on four. The controller times it as SDR104.
- QEMU 8.2 has no eMMC and no MMC card.

## Decision

- **A card that does not answer ACMD41 is set up as an MMC**, with CMD1
  asking for sector addressing and both voltage ranges, and address 1.
  In a slot for a device soldered to the board it is an eMMC; in one
  for cards that come and go, an MMC card. Both are named `mmcblk0` and
  on, as SD cards are.
- **Its capacity is the EXT_CSD's SEC_COUNT** for a device addressed in
  sectors, and its CSD's for one addressed in bytes, which is given
  512-byte blocks with CMD16. A device before version 4 runs on one bit
  at up to 20 MHz.
- **Its bus is made as wide as both can**: eight bits where the slot has
  them, then four, the EXT_CSD read again at each width and compared
  with the one read on one bit; a width whose read does not match is
  given up for the next.
- **It runs at HS200** where the device has HS200 at 1.8 V, the
  controller is of version 3.00 or later with 1.8 V and SDR104, and the
  bus is four or eight bits wide: the I/O is switched to 1.8 V, HS_TIMING
  to 2, the slot to SDR104's timing, and tuned with CMD21. Otherwise, or
  where HS200 is refused or does not tune, it runs at High Speed, at up
  to 52 MHz. A device with HS200 at 1.8 V takes 1.8 V at every speed, so
  the I/O stays at 1.8 V after a fall back.
- **Garbled data at HS200 tunes again**, as at SDR104 (ADR 0438); a
  device that no longer tunes falls back to High Speed.
- **Its cache is left off**, as it is at power on, so a write is done
  when it ends and a flush needs nothing; **only its user area is a
  disk**, its boot partitions and RPMB left alone. An MMC has no
  write-protect switch, so it is never read-only.
- The kernel's log names an eMMC as such: `mmcblk0 is an eMMC of 58 GiB
  on the SD host controller at 0000:00:1a.0, slot 1: EMMC01, HS200 at
  200 MHz on an 8-bit bus`.

## Consequences

- QEMU 8.2 has no eMMC, so set-up, the widths, a slot with four of
  eight lines wired, High Speed, HS200, a refused or untuned HS200,
  tuning again, devices addressed in bytes and devices before version 4
  are checked by unit tests against the simulated controller, which now
  answers as an MMC too. A laptop with an eMMC goes on the lab's list.
- Turning the cache on, with a flush that writes it out, would speed
  writes up; it is left for the lab, where a power cut can be tried.

## Alternatives considered

- **HS400**, eMMC 5.0's 200 MHz on both edges with a data strobe. It
  needs the controller's enhanced strobe or vendor registers, which
  differ by chipset; HS200 reaches most of its speed on reads.
- **Boot partitions as disks of their own**, as Linux's `mmcblk0boot0`.
  Nothing Dukovo does reads them, and writing one can brick a board that
  boots from it.
