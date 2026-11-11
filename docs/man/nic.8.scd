nic(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

nic - show what a network card says of itself, as ethtool does

# SYNOPSIS

*nic* [_name_]

*nic* *-i* | *-g* | *-k* | *-S* | *-P* _name_

# DESCRIPTION

nic shows an interface's link, the speed and duplex its card
negotiated and whether its cable is in, and with an option its driver,
its rings, its offloads, its counts or its MAC address, each in the
words of Linux's ethtool, so that what reads ethtool's output reads
nic's (ADR 0341). It reads _/kernel/class/net_ (*radix-sysfs*(5)), which
any program may: nic needs no privilege.

```
$ nic net0
Settings for net0:
	Speed: 1000Mb/s
	Duplex: Full
	Link detected: yes
$ nic -i net0
driver: radix-intel-net
version: 0.14.1
firmware-version: 
expansion-rom-version: 
bus-info: 0000:00:03.0
supports-statistics: yes
supports-test: no
supports-eeprom-access: no
supports-register-dump: no
supports-priv-flags: no
model: Intel 82574L
interrupt: MSI vector 0x31
```

With no _name_, nic shows a line for each interface: whether its link is
up, its speed and duplex, its driver and its card's model.

A card that does not say its speed, as a virtio card, or one whose link
is down, shows *Speed: Unknown!* and *Duplex: Unknown! (255)*. A bridge,
a VLAN, a tunnel and _lo_ have no wire of their own, and show only
whether their link is up.

# OPTIONS

*-i*, *--driver*
	The driver, the kernel's release, the card's PCI address, its model
	and how it interrupts. A bridge's driver is *bridge*, a VLAN's
	*802.1Q VLAN Support*, a tunnel's *tun* and _lo_'s *loopback*, as
	Linux names them; a card a program drives says *program*, and a USB
	adapter *radix-usb*.

*-g*, *--show-ring*
	How many frames the card holds for its driver, and how many can wait
	to be sent. The rings are the size the driver gave them, which is
	also the most they take here.

*-k*, *--show-features*, *--show-offload*
	The offloads, each *off [fixed]*: the kernel sends and receives every
	frame whole, and checks and makes its checksums itself. _lo_'s
	*rx-checksumming* and *tx-checksumming* are *on [fixed]*: the loopback
	link needs none, as on Linux.

*-S*, *--statistics*
	What the interface sent and received, as in its _statistics_ folder,
	and for a card the kernel drives, *card_rx_dropped*, the frames the
	card dropped: damaged, too short, or with no slot for them.

*-P*, *--show-permaddr*
	The MAC address.

*-h*, *--help*
	Print what nic takes.

*-s* and *-K*, which on Linux set the speed and duplex and turn offloads
on and off, say why they cannot and change nothing: the drivers take the
link the card negotiates, and use no offloads.

Not yet: the link modes a card supports and advertises, auto-negotiation,
Wake-on-LAN, the cards' own counters, *-t* self-tests, register and
EEPROM dumps, and *--json*.

# EXIT STATUS

0 on success; 1 for an option or an argument it does not take, an
interface that is not there, or rings asked of one without a card.

# SEE ALSO

*e1000*(4), *ip*(8), *radix-sysfs*(5), *ss*(8), *systools*(1),
*vmxnet3*(4)
