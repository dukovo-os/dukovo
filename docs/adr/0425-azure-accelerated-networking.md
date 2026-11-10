<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0425. Azure's accelerated networking

- Status: Accepted
- Date: 2026-11-10

## Context

Azure gives a machine made with accelerated networking a function of a
real network card besides Hyper-V's synthetic one (ADR 0423): on most
of its sizes now, Microsoft's Azure Network Adapter (MANA). The
function's frames skip the host's virtual switch, which gives lower
latency and costs the host less. Accelerated networking is on by
default for many sizes, and some sizes require it, so a guest that
cannot drive MANA either loses that speed or cannot run on them.

The function does not stand alone. The host passes it through on a
virtual PCI bus of its own, offered as a VMBus channel (vPCI), with no
PCI configuration mechanism the firmware describes: the guest reads and
writes the function's configuration space through a window of
addresses it picks and names to the host, gives its BARs addresses from
the range the firmware sets aside for VMBus, and asks the host for each
interrupt it uses (CREATE_INTERRUPT), whose answer is the message to
write in the MSI-X table. The function's port has the MAC address of
one of the synthetic cards, and the host tells that card when it has a
virtual function, and can take the function away at any time: before
its maintenance, or before it moves the machine to another host.

FreeBSD's drivers are BSD-2-Clause and describe all of it: vmbus_pcib
the bus, hn the synthetic card's side of the bond, and mana(4) the
adapter, its GDMA hardware channel and its queues. Microsoft's public
documentation describes accelerated networking and the RSS key.

## Decision

- **The vPCI protocol is part of `radix-hyperv`** (`pci`), versions 1.4
  and 1.1: the bus's functions and their resource requirements, the bus
  powered on with its configuration window, each function's resources
  assigned, interrupts created and deleted, ejections answered. The
  synthetic card (`network`) speaks NVSP 5's SR-IOV messages: it says it
  can take a virtual function, keeps the serial number the host
  associates with it, and switches the path its frames take
  (SET_DATAPATH).
- **MANA is a library with no unsafe code,** `radix-mana`, tested
  against a simulated adapter. It brings the hardware channel up through
  the adapter's shared memory, asks for its version, resources, device
  and port, creates an event queue for its interrupt, a send queue and a
  receive queue of 256 entries each with their completion queues, and
  steers every frame to the receive queue through the port's RSS table
  with the 64 entries Microsoft's example key fills. Frames are copied
  through slots of 2048 bytes, with no offloads, as the other network
  cards' are (ADR 0197). Only the adapter's first port is used.
- **A bond in the stack's place of the synthetic card** (`bond`): each
  synthetic card is a `Bond` of the card and the virtual function it may
  have, one interface with the synthetic card's MAC address and link.
  The bond takes the function once the host associates one, asks for its
  path, sends through it while it works and receives from both cards in
  turn, as the host still sends some frames through the synthetic card.
- **The `vmbus` thread sets the function up and hands it over.** Once
  the synthetic devices are set up (the boot does not wait for the bus),
  the thread waits for uACPI's namespace, takes VMBus's ranges from the
  `_CRS` of its ACPI device or the nearest device it is in, preferring
  those above 4 GiB and keeping clear of the framebuffer, and gives the
  window and the BARs addresses from them. The function's MSI-X entry
  for its queues gets the message the host translated; the entry for
  the hardware channel stays masked, as the channel is polled. The port
  is offered to the synthetic card with its MAC address through a slot
  both threads reach, and its interrupt wakes that card's thread.
- **An ejection is answered once the port is back.** The thread asks
  the bond for the port, which turns the path back to the synthetic card
  and gives the port back running; the thread stops it (its queues
  destroyed, its memory deregistered, the hardware channel closed),
  takes its interrupt and addresses back, and says the ejection is
  complete. A bus offered while the machine runs, as after the host's
  maintenance, is set up as the boot's are; a bus taken away has its
  functions stopped first.

## Consequences

- An Azure machine made with accelerated networking sends and receives
  through MANA, with one interface and one address whatever the path,
  and goes on through the synthetic card while the host takes the
  adapter away. The handbook no longer asks for machines without it.
- The `hyperv` boot test covers the code that sets the bus up only as
  far as QEMU's VMBus goes, which offers no vPCI bus; the protocols are
  tested by the libraries' unit tests (the bus, the synthetic card's
  messages, the bond, the adapter) and on Azure, in the lab
  (`hardware/azure-accelerated`).
- Hyper-V's own SR-IOV, with a host's Mellanox or Intel card, offers its
  virtual functions on the same bus, which the kernel sets up and leaves
  unused: it has no driver for them yet.
- Checksum and segmentation offloads, several queues across the CPUs,
  the adapter's other ports, and the host's link speed are not given;
  the stack does each of them, or does without.
