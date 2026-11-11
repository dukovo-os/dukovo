<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0448. Realtek's card readers on PCI are driven from OpenBSD's rtsx(4), at High Speed

- Status: Accepted
- Date: 2026-11-10

## Context

Many laptops' SD card slots are Realtek's card readers on PCI Express:
the RTS5209, RTS5227, RTS5229, RTS522A, RTS5249 and RTS525A, and the
readers beside the RTL8402, RTL8411 and RTL8411B network chips. They are
not SD host controllers. Their PCI class is 0xff, not the SD host
controller's 0x08 0x05, and ADR 0437's driver does not see them. Each
is driven through registers of its own:

- A page of commands, each of which reads, writes or checks one of the
  chip's internal registers. The chip runs the page and says when it is
  done or has failed.
- A table of descriptors, each pointing at a piece of memory below
  4 GiB, through which the chip moves a card's data.
- An SD command is a few register writes in that page. The response is
  read back from registers in the same page.

The references:

- **OpenBSD's rtsx(4)** (`sys/dev/ic/rtsx.c`, ISC-licensed) drives nine
  chips: the six RTS chips and the three RTL chips above. It does SD
  cards at 25 and 50 MHz on a four-bit bus. NetBSD's driver is derived
  from it.
- **FreeBSD's rtsx(4)** drives one chip more, the RTS5260. Its source
  says it draws on Linux's `rtsx_pci` driver, which is GPL-licensed, so
  it is not a source Dukovo takes from.
- **Realtek publishes no datasheet** for these chips.
- **QEMU has no Realtek card reader.**
- **The Dell's reader is not one of these.** It is a Realtek USB card
  reader, which ADR 0430 drives as a USB disk.

## Decision

- **A crate of its own, `radix-rtsx`, written from OpenBSD's rtsx(4),**
  drives the nine chips OpenBSD drives. A function is matched by
  Realtek's vendor ID 0x10ec, its device ID and the class 0xff. A chip
  that says it is an SD host controller is left to that driver. A
  function whose configuration space says it is an FPGA, Realtek's
  stand-in for a chip under design, is refused, as OpenBSD refuses it.
- **SD, SDHC and SDXC cards,** set up as the SD specification says.
  They run at High Speed, 50 MHz, when the card has it, and at
  25 MHz when it does not, on a four-bit bus.
  - UHS-I's modes, at 1.8 V and tuned, are not driven. OpenBSD's driver
    does not set them up, and each chip's way of doing so is described
    only in Linux's GPL driver.
  - MultiMediaCards and Memory Sticks are not driven either. An MMC put
    in is said and not used.
- **A card's blocks move by the chip's descriptors,** each piece at most
  512 KiB.
  - Since the chip reaches only the first 4 GiB, a request whose pages
    lie above it goes through the slot's bounce buffer. A 32-bit SD
    host controller's request does the same (ADR 0247).
  - A request of several blocks ends with CMD12.
  - A write ends when CMD13 says the card has written its blocks and is
    ready for the next.
  - A transfer that fails stops the page, the DMA and the card's
    transfer. The next request first stops the card with CMD12, in
    case it is still sending.
- **The reader's one slot is a slot like an SD host controller's** (ADR
  0437). It is named `mmcblk0` and on at boot, and its watcher thread
  looks at it every two seconds. eject(1) lets its card go, and its
  card's queue is the same.
  - The kernel's reader code holds either kind of controller and either
    kind of card detect. Nothing above it knows which reader it drives.
  - The log names the chip: `a Realtek RTS5227 reader`.
- **MSI where the chip has it,** for a transfer's end and a card put in
  or taken out. Without MSI, the slot is polled with the chip's legacy
  interrupt masked, as an SD host controller without MSI is. Every
  change the chip reports is taken as the card taken out and whatever
  is in the slot put in. That costs a card put in again after a
  swap nothing it would not cost anyway.

## Consequences

- No QEMU boot test can reach the driver.
  - Unit tests run it against a simulated reader that runs its command
    pages and descriptors, with a simulated card. They check:
    - the set-up of each chip group: the RTS5209's power gates, the
      RTS5229's version C pull-ups and the RTS525A's regulator;
    - a card's identification, through the long response's bytes;
    - multi-block and single-block writes and reads;
    - a failed transfer and the stop after it;
    - the card detect.
  - The lab's `storage/sd-slot-data` test writes 64 MiB to a card,
    takes it out, puts it back and checks its sum, and reads the whole
    device. It runs on any machine with an SD card slot, so on a laptop
    with one of these readers it checks the driver. The lab's machines
    do not have one yet.
- A UHS-I card runs at High Speed in these readers, at about 22 MB/s,
  where Linux runs it at SDR104 and reads several times faster. Faster
  modes wait for a description of them that Dukovo can use: Realtek's
  documentation, or a BSD driver that does them.
- The RTS5260 is not driven, as only Linux's driver and FreeBSD's,
  which derives from it, describe its set-up. Nor are the RTS524A,
  RTS5228 and RTS5261, which only Linux drives. A laptop with one has
  no SD card slot under Dukovo until a usable description is found.

## Alternatives considered

- **Taking the chips' set-up from FreeBSD's driver.** It covers the
  RTS5260 too, but it says it draws on Linux's GPL driver. Code taken from it
  would carry that licence's terms into the kernel.
- **Treating the readers as SD host controllers.** They are not: no SD
  host controller register is where the specification puts it.
- **One driver for both kinds of reader, inside `radix-sdhci`.** The
  two share the SD protocol's types (`Card`, `Op`, `Error`, the CID and
  CSD), which `radix-rtsx` uses from `radix-sdhci`. Their registers,
  their requests and their set-up have nothing in common, so the
  Realtek driver is a crate of its own, as OpenBSD's is a driver of its
  own.
