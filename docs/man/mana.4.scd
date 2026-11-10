mana(4) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

mana - Azure's network adapter, for accelerated networking

# DESCRIPTION

The Microsoft Azure Network Adapter (MANA), which Azure passes through
to a machine made with accelerated networking on. Its frames skip the
host's virtual switch, for lower latency and less of the host's time
than the synthetic network card Hyper-V gives every machine.

The adapter is not a card of its own to the machine. Azure offers it on
a virtual PCI bus of Hyper-V's, a VMBus channel, alongside the synthetic
network card, and its port has the synthetic card's MAC address. The
kernel sets the bus up once the synthetic devices are, gives the
adapter's registers addresses from the range the firmware sets aside
for VMBus, asks the host for its interrupt, and starts it: a hardware
channel in memory both share, through which it asks the adapter for its
port, then a queue each way for frames, copied through slots of 2048
bytes, with no offloads (ADR 0425).

The port joins the synthetic card with its MAC address: *net0* stays
one interface, with one address, whichever path its frames take. Once
the host says the synthetic card has a virtual function, the card sends
through the port and receives from both; the host still sends a few
frames through the synthetic card. The kernel log says when the port
joins:

```
Hyper-V: virtual PCI bus 0c5e7d1a-3c40-4b8f-7078-20d0f7e4a1b9 speaks vPCI 1.4; 1 function(s)
Hyper-V: Azure's network adapter at 7870:00:00.0 accelerates the synthetic network card 00:0d:3a:12:34:56
```

The host takes the adapter away before its maintenance, or before it
moves the machine, and gives it back after. The card's frames then go
through the synthetic card, the kernel stops the port and tells the
host, and a bus the host offers again is set up the same way. A port
that fails is not used again, and the card goes on through the
synthetic card until the host takes the adapter away.

Only the adapter's first port is used, so on a machine with several
network interfaces, only the first is accelerated.

# SEE ALSO

*ip*(8), *netd*(8), *vmxnet3*(4)
