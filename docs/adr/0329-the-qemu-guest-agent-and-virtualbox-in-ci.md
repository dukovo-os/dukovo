<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0329. The QEMU guest agent, and VirtualBox in CI

- Status: Accepted
- Date: 2026-11-10

## Context

A virtual machine's host knows little of what runs inside it: not its
addresses, which DHCP gave it, nor how to end it but by pulling its
power. QEMU's guest agent answers the host. Proxmox shows the
addresses it gives in the machine's summary and shuts the machine down
through it; libvirt's `virsh domifaddr --source agent` and
`virsh shutdown --mode agent` use it; OpenStack and others too. On Linux
it is qemu-ga, GPL-2.0, which Dukovo neither ships nor reads.

The protocol is QEMU's machine protocol in JSON, over the virtio console
port the host names `org.qemu.guest_agent.0`, which the kernel links
from `/devices/virtio-ports/` (ADR 0327). Requests come one after
another on a byte stream, `{"execute": "guest-ping"}`, perhaps with
arguments and an id; answers are lines of JSON, `{"return": ...}` or an
error with a class. A client starts with `guest-sync-delimited` after a
0xFF byte, which no JSON holds, so that it can tell its answer from
what an earlier client left unread.

The cloud image (ADR 0328) is meant for VirtualBox too, which reads its
VMDK, but nothing tested Dukovo in VirtualBox: its firmware, its AHCI
controller and disc drive, its Intel PRO/1000 card on NAT, and its
ACPI power button.

## Decision

- **guest-agent(8), a systools program and a service that is on.** It
  opens the port, and answers on it until the port goes; without the
  port, as on a computer, it says so and ends, so it costs nothing where
  there is no host to answer. Its file is on by default: the agent is
  wanted on every virtual machine, installed from the stick or not, and
  Spark starts it again if the port goes and comes back.
- **The commands that report and end the machine.** `guest-sync`,
  `guest-sync-delimited`, `guest-ping`, `guest-info`,
  `guest-get-osinfo`, `guest-get-host-name`, `guest-get-time`,
  `guest-network-get-interfaces` (the interfaces as ip(8) reads them,
  with their IPv4 and settled IPv6 addresses and their counts), and
  `guest-shutdown`, which asks Spark to power off, restart or halt as
  shutdown(8) does and answers nothing, as qemu-ga does not. A command
  it does not answer gets `CommandNotFound`; a request that is not a
  JSON object, or has no `execute`, gets `GenericError`. An id comes back
  with its answer.
- **Nothing that acts for the host inside the machine.** Running
  programs (`guest-exec`), reading and writing files (`guest-file-*`),
  setting passwords and freezing filesystems are not answered. The host
  can do anything to the machine anyway, but each of these is a way into
  it that needs its own design; they are left for later.
- **The stream is cut by braces.** Requests have no delimiter of their
  own, so the agent counts braces and brackets outside strings: a
  request ends at the brace that closes its first. A 0xFF byte throws
  away what came before it. A request over 64 KiB is thrown away whole
  and refused, and a stray word between requests is refused as not JSON.
- **The JSON is serde_json's**, which systools already uses.
- **VirtualBox in CI.** The build job writes the cloud image with
  `ob image --cloud`, and a new job boots its VMDK in VirtualBox 7.2
  from Oracle's repository, whose key is checked by its fingerprint
  (Ubuntu 24.04's VirtualBox 7.0 does not build its driver for the
  runners' Linux 7.0 kernels, which 7.2.8 and later do). The machine
  runs under VirtualBox's UEFI, on its AHCI controller, with a NoCloud
  config disc labelled `cidata` in ISO 9660 on the same controller and
  an Intel PRO/1000 card on NAT. From the serial log it
  waits for cloud-setup to set the machine up from the disc; the host's
  ssh signs in with the disc's key through a forwarded port and
  elevates without a password; and VirtualBox's ACPI power button
  powers the machine off cleanly. GitHub's runners have the CPU's
  virtualization, which VirtualBox needs; KVM's modules are unloaded
  first, as the two cannot hold it at once. The job runs when the
  changes reach the cloud image, the drivers VirtualBox's machine uses,
  or the job itself, and on main.

## Consequences

- `guest-agent` boots with the port held by the test as Proxmox holds
  it: a request cut off by 0xFF, `guest-sync-delimited`, ping with an
  id, the commands listed, the system, the time, the machine's name as
  `hostname` says it, the card with its MAC address and its address
  from DHCP, the refusals with their classes, and `guest-shutdown`
  powering the machine off with every service stopped.
- The default services number 24, of which 20 are on.
- The VirtualBox job tests the ISO 9660 config disc, which the boot
  tests leave to the FAT one, and VirtualBox's own hardware.
- Proxmox's backups freeze a machine's filesystems through the agent
  when it answers `guest-fsfreeze-freeze`; Dukovo's does not, so Proxmox
  backs it up without freezing, as it does a machine with no agent, and
  says so in the backup's log.
