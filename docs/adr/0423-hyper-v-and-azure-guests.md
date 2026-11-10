<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0423. Hyper-V and Azure guests

- Status: Accepted
- Date: 2026-11-10

## Context

Under Microsoft's hypervisor, Hyper-V, a generation 2 virtual machine
has almost no PCI devices: its disks, its network card, its keyboard and
the services that let the host shut it down and keep its clock are
synthetic devices on VMBus, a bus of channels between the guest and the
host. Azure runs every machine this way. The cloud image (ADR 0328) has
been written as a VHDX since it was made, but a machine started from it
found no disk and stopped.

VMBus is described in two places Dukovo may follow. Microsoft's
Hypervisor Top Level Functional Specification (TLFS) gives the CPUID
leaves, MSRs, hypercalls and synthetic interrupt controller (SynIC)
through which a guest reaches the host. FreeBSD's Hyper-V drivers,
BSD-2-Clause, give the bus's messages, its ring buffers and the
protocols of each device. Linux's drivers, which are GPL-2.0, are not
read.

An Azure machine also has to say it started. Azure's agent normally
reports to Azure's wire server, 168.63.129.16, that the machine is
ready; until a new machine does, Azure takes its deployment to be still
under way and ends it as failed. Azure documents the exchange for images
that carry no agent, and its instance metadata service at
169.254.169.254 gives the machine's name, the admin account's name and
SSH keys, and the user data.

## Decision

- **Finding Hyper-V and its clock.** The x86-64 layer finds Hyper-V by
  its CPUID leaves ("Microsoft Hv", interface "Hv#1"), reads the TSC's
  and the local APIC timer's rates from its frequency registers when the
  partition may, as a generation 2 machine has no PIT and no HPET to
  measure them against, and otherwise measures against the reference
  counter. KVM's own leaves, which come after Hyper-V's when KVM shows
  both, are still found.
- **Hypercalls and the SynIC on the boot CPU.** The kernel names itself
  in the guest OS ID register, maps the hypercall page the hypervisor
  fills read-only and executable, and sets up the boot CPU's SynIC: its
  message page, its event flags page and one synthetic interrupt source
  for VMBus on a vector of its own.
- **A `vmbus` thread, pinned to the boot CPU,** whose message slot and
  end-of-message register are that CPU's. It connects at the newest of
  VMBus 4.0, 3.0 and 2.4 the host takes, asks for the host's channels,
  and opens each it drives on ring buffers it names to the host as a
  GPADL. The memory of every ring and buffer the host is given is never
  freed, so that nothing the host may still write is reused. The boot
  waits up to 60 seconds for the channels offered at boot to be set up
  before it looks for the root.
- **The devices driven.**
  - The synthetic SCSI controller (storvsc): its disks take `sd` letters
    as SATA disks do, through the block queue, with READ and WRITE (10,
    or 16 past 32 bits of blocks) and SYNCHRONIZE CACHE, each request's
    pages named as one range. The scan shares the virtio SCSI adapter's
    REPORT LUNS and INQUIRY steps.
  - The synthetic network card (netvsc): NVSP from 6.1 down to 1, NDIS
    6.30, and Remote NDIS inside it; frames come through a 15 MiB
    receive buffer the guest gives the host, the most the oldest
    versions take, and go from 64 send slots. No offload is turned on.
  - The synthetic keyboard, whose set 1 scan codes reach `/devices/kbd`
    as a PS/2 keyboard's do.
  - The shutdown service, which presses the power button for Spark; the
    time service, which sets the clock at boot and when the host's
    sample is more than 5 seconds off; and the heartbeat.
  - Other channels (the mouse, the video device, dynamic memory, backup,
    key-value exchange) are logged and left.
- **The driver's protocols are a library, `radix-hyperv`,** with no
  unsafe code: it reads and writes shared pages through `radix-hw`'s
  DMA buffers, and the kernel gives it hypercalls as functions. Its unit
  tests check each message's layout, the rings' wrapping and signalling,
  and the network card against a simulated host.
- **Azure in cloud-setup.** cloud-setup (ADR 0328) knows Azure by the
  firmware's maker, Microsoft, with the chassis asset tag Azure gives its
  machines, and reads the instance metadata service: the machine's ID,
  its name, the admin account's name, which the default account takes
  unless the user data names one, its SSH keys, and the user data, in
  base64. At every boot it then asks the wire server for the goal state
  and answers with a health report whose state is Ready. Custom data,
  which only Azure's provisioning disk carries, is not read.
- **A fixed VHD for Azure.** `ob image --cloud` also writes
  `dukovo-VERSION-x86_64-cloud.vhd`, a fixed VHD whose size is the raw
  image's rounded up to a whole MiB, as Azure takes no other; the VHDX
  stays for Hyper-V.

## Consequences

- A generation 2 Hyper-V machine and an Azure machine of the Standard
  security type boot the cloud image on their synthetic disks, with a
  network card and a keyboard, and stop cleanly when the host asks.
  Trusted launch starts machines with Secure Boot on, which unsigned
  images cannot pass before 1.0.
- The `hyperv` boot test runs the kernel under KVM's Hyper-V
  enlightenments with QEMU's VMBus: the clock's rates, the SynIC, the
  connection and the offers. QEMU has no synthetic disk or network card,
  so those are tested by the library's unit tests and on real Hyper-V
  and Azure machines, in the lab.
- Disks added while the machine runs, the mouse, the video device past
  the firmware's screen, dynamic memory, key-value exchange and
  accelerated networking (MANA) are left for later; Azure's machines
  must be made without accelerated networking until MANA is driven.
