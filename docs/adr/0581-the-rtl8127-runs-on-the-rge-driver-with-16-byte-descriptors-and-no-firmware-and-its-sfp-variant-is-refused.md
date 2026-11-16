<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0581. The RTL8127 runs on the rge driver with 16-byte descriptors and no firmware, and its SFP+ variant is refused

- Status: Accepted
- Date: 2026-11-16

## Context

`radix-rge-net` drives Realtek's 2.5 and 5 Gb/s chips, the RTL8125 and
RTL8126, translated from OpenBSD's ISC-licensed `rge(4)` (ADR 0197) and
checked against OpenBSD's own functions by `tools/rge-openbsd-check`.
Realtek's 10GBASE-T chip, the RTL8127 (PCI 10ec:8127), was left out.
OpenBSD's `if_rge.c` revision 1.49 drives it with the same code as the
other chips and a handful of branches of its own: `RGE_TYPE_R27` and
`RGE_FLAG_DESCV2`. Revision 1.49
changes nothing for the six chips already driven: the harness's logs for
them are those of revision 1.48.

Three things need a decision. The RTL8127 takes another descriptor
format. Realtek ships microcode for its MAC and PHY processors, as for
the other chips. And a variant of the chip with an SFP+ cage for a
module, the RTL8127ATF, has the same revision in its transmit
configuration register and is told apart only by what the MAC's
register 0xd006 reads (0x0007).

## Decision

- **The RTL8127 is `Kind::R27` of `radix-rge-net`**, found by the
  revision 0x6c90_0000, with every branch OpenBSD makes for it:
  its PCI Express PHY's 36 settings written through the EPHY's
  extended address register (the page of each register in its top four
  bits), its PHY's configuration without the microcode part, 10 Gb/s
  advertised with the speeds below it, a link at 10,000 Mb/s when the
  PHY status register says so, and the MAC's setup of one interrupt
  vector. OpenBSD's several vectors and its segmentation offload are
  left out, as the offloads of the other chips are (ADR 0197).
- **Both rings use OpenBSD's second descriptor format**, 16 bytes each:
  a transmit descriptor holds its command, extended status and buffer
  address as the first format's does, and a receive descriptor its
  buffer's address in its first quadword, its extended status in the
  third word and its command and status in the fourth, where the chip
  writes the frame's hash over the address once the frame is in. The
  rings take a `Format` that holds the descriptor's size and the
  receive offsets and bits, so that one code path serves both formats.
- **No Realtek microcode is loaded**, as for the other rge chips: it is
  closed firmware under Realtek's own licence, which the base image does
  not carry (ADR 0007). The chip runs on the code in its ROMs. Linux's
  `r8169` also runs the RTL8127 when its `rtl8127a-1.fw` is missing.
  Realtek does not say what its patches fix.
- **The RTL8127ATF is refused** with `Error::Unsupported`, and the log
  says why. OpenBSD attaches it but sets up no SerDes for the module in
  the cage, and advertises only 2.5 Gb/s on it; Linux has a SerDes setup
  for it, under the GPL, which Dukovo cannot take. The driver reads the
  MAC's register 0xd006 before it sets anything up, and leaves a chip
  that reads 0x0007 as it found it.
- **The tests check the sequence as OpenBSD's code makes it**:
  `tools/rge-openbsd-check` takes `if_rge.c` 1.49 and gains the
  seventh chip, and the driver's register accesses on it hash to the
  harness's. The model of the chip takes the second descriptor format
  only when the receive configuration register and the MAC's register
  0xeb58 both select it, and writes its hash over each received
  buffer's address.

## Consequences

- A machine with an RTL8127 gets a network interface at up to 10 Gb/s,
  *net0* or the next in the order of the cards, as with the other rge
  chips.
- No lab machine has an RTL8127 and QEMU emulates none: the driver is
  checked against OpenBSD's sequence and a model, and the first real
  boot on a machine that has one is its real test. A link that is slow
  to come up or drops on it should be reported with the chip the kernel
  log names, as for the other chips.
- A machine with an RTL8127ATF has no network from this driver until a
  BSD-licensed description of the SerDes setup exists; the kernel log
  names the chip and the reason. `docs/deferred.md` lists it.
