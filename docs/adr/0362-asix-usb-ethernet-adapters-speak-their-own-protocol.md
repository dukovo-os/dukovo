<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0362. ASIX's AX88179 USB Ethernet adapters speak their own protocol on the USB thread

- Status: Accepted
- Date: 2026-11-03

## Context

ASIX's AX88179 is among the most common gigabit USB Ethernet chips: it
is inside TP-Link's UE306, D-Link's DUB-1312, Lenovo's, Belkin's,
Samsung's and Sitecom's adapters and many USB-C docks. Unlike the
RTL8153, it has no CDC configuration to fall back on, so ADR 0199's
driver, which speaks CDC-ECM and CDC-NCM, leaves it out. Linux drives it
with `ax88179_178a`, FreeBSD with `axge(4)` and OpenBSD with `axen(4)`.

The chip has one interface, of the vendor's class, with a bulk endpoint
each way and an interrupt endpoint. Vendor requests read and write its
MAC's registers and, over MDIO, its PHY's. It sends received frames in
bursts: the frames, each on an 8-byte boundary, then a 4-byte header
for each, then a trailer that says how many headers there are and where
they start. How much a burst may hold is set in a register, and is more
than a page, while a transfer here carries at most a page (ADR 0199).
Each frame it is to send needs an 8-byte header before it.

The AX88179A, its successor, shares its ID and has a newer burst format
by default; FreeBSD puts it in the AX88179's. The AX88178A is the same
design on USB 2.

## Decision

- **`radix_usb::net::asix` knows the chips and their protocol**: their
  IDs, by release for the AX88179 and AX88179A as FreeBSD tells them
  apart; the vendor requests; the bring-up as steps; the PHY's link,
  resolved from its registers as IEEE 802.3's priority resolution has
  it; the medium and bulk-in settings for each link and USB speed, which
  are FreeBSD's; the frames of a burst; and a frame's header to send.
  It is tested on the host, the bursts against any bytes at all.
- **The USB thread drives an ASIX adapter as it drives a CDC one** (ADR
  0199), through the same link to its interface's stack, as a third
  `Protocol`. A device the CDC driver does not take, whose IDs are
  ASIX's, is given to it.
- **Bring-up follows FreeBSD's `axge_init`**: the PHY powered, the
  clocks on, an AX88179A put in the AX88179's mode, the MAC address read
  (a random local one written if it is not one a card can have), pause
  water levels set, checksum offload off both ways, the filter set to
  the adapter's own frames, broadcasts and every multicast frame, wake
  on LAN off, and the PHY reset and negotiating every speed to 1000 Mb/s
  without pause frames, as the crate's other drivers do.
- **The link is polled, as FreeBSD's and OpenBSD's drivers do:** each
  second the thread reads the PHY's status and the two ends' abilities
  and, when the link changes, sets the medium and how the chip gathers
  bursts. The interrupt endpoint is not read. The link is down until
  the PHY says it is up.
- **A burst is joined from transfers of a page until one comes short**,
  in a 64 KiB buffer of the adapter's own, FreeBSD's largest read; a
  longer burst is dropped whole. A frame that failed its CRC, or that
  the chip dropped, is passed over, as are headers of no length.
- **A frame is sent after its header, a transfer each;** a transfer that
  would end on a packet's boundary gets a second header asking for
  padding, as OpenBSD's does, rather than a zero-length packet.

## Consequences

- TP-Link's UE306, which the maintainer bought for the test lab, and the
  other AX88179 adapters get an interface as they are plugged in, named
  in the log as `ASIX AX88179`, with Linux's `ax88179_178a` as their
  interfaces' driver in /kernel/bus/usb.
- QEMU emulates none of these chips, so the driver is tested on the host
  only until an adapter is plugged into a lab box (roadmap 14f).
- A page a transfer, one at a time, is slower than the chip can go: a
  queue of larger reads is the next step when one is measured, as for
  ADR 0199's adapters.
- Checksum offload, VLAN tags, jumbo frames, wake on LAN, the
  AX88179A's newer burst format and ASIX's USB 2 chips of the AX88772
  family are left out.
