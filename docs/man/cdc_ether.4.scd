cdc_ether(4) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

cdc_ether, cdc_ncm, ax88179_178a - Ethernet adapters on USB

# DESCRIPTION

USB Ethernet adapters of the USB communications class's Ethernet Control
Model (CDC-ECM, Linux's *cdc_ether*) and Network Control Model (CDC-NCM,
Linux's *cdc_ncm*), on an xHCI controller: most USB-C docks and
adapters, Realtek's RTL8153 and RTL8156 in their class configuration,
phones and boards that share their network over USB, and QEMU's
*usb-net*. An adapter whose first configuration is its maker's own, as
the RTL8153's is, or Microsoft's RNDIS, as QEMU's is, is set up in its
CDC configuration instead, as Linux sets it up without the maker's
driver. ASIX's AX88179, AX88179A and AX88178A, which have only their
maker's protocol, are driven in it (see ASIX ADAPTERS); any other
adapter with only its maker's protocol, or only RNDIS, is not driven.

The kernel's USB thread sets the adapter up as it is plugged in: it
reads its MAC address from its string, asks an NCM adapter for the
rules of its transfer blocks and has it send blocks of at most a page,
picks its data interface's setting with the bulk endpoints, and has it
pass on the frames to its own address, broadcasts and multicast frames.
The adapter gets an interface, *net0*, *net1*... after the PCI cards,
which *netd*(8) sets up by DHCP or as its settings say, and *ip*(8)
shows. The log names it:

```
Net: net0 is a USB CDC-ECM network adapter on 1-6, MAC 40:54:00:12:34:80
```

Its frames go between the thread and the interface's stack through the
same shared slots as a driver program's (ADR 0198, ADR 0199), one
transfer at a time each way: a frame (ECM) or a block of frames (NCM)
each. The link is taken to be up, at 100 Mb/s, until the adapter says
otherwise on its interrupt endpoint.

An adapter unplugged takes its interface down, which keeps its
addresses; plugged in again, in any port, the adapter gets the same
interface back, by its MAC address, and netd asks for its address again:

```
Net: net0 is down: its USB adapter is gone
Net: net0 is back: the USB CDC-ECM network adapter on 1-6, MAC 40:54:00:12:34:80
```

QEMU's *usb-net* is tested in its CDC-ECM configuration; the NCM blocks
are tested on the host.

# ASIX ADAPTERS

TP-Link's UE306, D-Link's DUB-1312, Lenovo's, Belkin's, Samsung's and
Sitecom's gigabit adapters and many USB-C docks are built on ASIX's
AX88179 (Linux's *ax88179_178a*), which speaks no CDC protocol. The USB
thread knows them by their IDs and brings them up with ASIX's vendor
requests, as FreeBSD's axge driver does: their PHY powered, their MAC
address read from the chip, their filter set to their own frames,
broadcasts and multicast frames, without checksum offload, and their
PHY negotiating every speed to 1000 Mb/s (ADR 0362). The log names the
chip:

```
Net: net0 is a USB ASIX AX88179 network adapter on 1-3, MAC 50:91:e3:12:34:56
```

The link is the PHY's, asked for each second; it is down until the PHY
says otherwise. The chip sends what it receives in bursts of many
frames, which the thread reads a page at a time and joins until a read
comes short; each frame it sends goes after a header of the chip's. The
AX88179A is put in the AX88179's mode, and its newer format is not used.
QEMU emulates none of these chips: the bursts and headers are tested on
the host.

# SEE ALSO

*re*(4), *ip*(8), *lsusb*(8), *netd*(8), *nic*(8)
