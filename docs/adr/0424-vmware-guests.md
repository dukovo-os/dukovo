<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0424. VMware guests

- Status: Accepted
- Date: 2026-11-10

## Context

VMware's hypervisors (ESXi, Workstation and Fusion) give a machine
emulated devices Dukovo already drives, an LSI or SATA disk controller
and an E1000 card, and their own paravirtual ones, which are what a
machine made for speed has: the PVSCSI disk controller and the VMXNET3
network card (ADR 0197). A machine whose disks are on PVSCSI found none
of them.

VMware's hosts also expect the guest's tools. Without them, the host's
Shut Down Guest and Restart Guest are greyed out, and the host shows
neither the machine's address nor how long it has been up. VMware's own
tools, open-vm-tools, are LGPL-2.1 and are not read. The guest's side is
small: the backdoor, an I/O port the hypervisor answers itself, and the
guest RPC over it, whose channels carry the host's commands to the guest
(TCLO) and the guest's requests to the host (RPCI).

Two BSD drivers describe both. FreeBSD's pvscsi, which VMware wrote and
licensed BSD-2-Clause (or GPL-2.0, used here under BSD-2-Clause), gives
the adapter's registers, rings and descriptors. OpenBSD's vmt(4), ISC,
gives the backdoor's registers, the RPC's steps and the commands a
guest answers.

## Decision

- **PVSCSI is a disk controller of the kernel's,** beside virtio SCSI:
  its disks take `sd` letters through the block queue, with READ and
  WRITE (10, or 16 past 32 bits of blocks) and SYNCHRONIZE CACHE. The
  adapter's rings are pages the kernel gives it: 8 of requests, 2 of
  completions and 1 of messages, with up to 256 commands in flight, each
  with its own list of up to 32 runs of memory and its own sense
  buffer. Reads and writes ring one doorbell, other commands the other,
  and completions raise MSI-X, or MSI. The scan asks each target the
  adapter says it has (16 if it does not say), through the steps virtio
  SCSI's scan uses, and the message ring logs disks the host adds or
  removes. A command that
  ended with no error moved all it was asked to, whatever the adapter's
  count says, as FreeBSD takes it: QEMU leaves the count at zero then.
- **The protocols are libraries with no unsafe code,** `radix-pvscsi`
  and `radix-vmware`, tested against a simulated adapter and a
  simulated host.
- **The backdoor in the x86-64 layer.** `in` from port 0x5658 and the
  string instructions through port 0x5659 load seven registers from a
  frame in memory and write them back after, as vmt(4) does, since Rust
  cannot name two of them. The kernel calls it only on a machine whose
  hypervisor names itself VMware's in CPUID, or whose firmware says
  VMware made it, and only once the version call answers as VMware's
  does.
- **The toolbox, a `vmware` thread.** Each second it answers the host's
  last command on the TCLO channel and reads the next: `reset` and
  `ping` are answered, `Capabilities_Register` registers the state
  change capability and says the guest's tools are not VMware's own
  (version 2147483647, so the host never offers to upgrade them),
  `Set_Option broadcastIP 1` sends the machine's first IPv4 address,
  and `OS_Halt` and `OS_Reboot` say the state changed and then reach
  Spark. Every 15 seconds the host is told the machine's uptime. A host
  with no guest RPC, as QEMU's `vmport` has none, is tried again every
  15 seconds and logged once.
- **The host's restart reaches Spark through the power capability.**
  Shut Down Guest presses the power button, as Hyper-V's shutdown
  service does (ADR 0423). Restart Guest has no button to press, so the
  power capability's interface gains `notify_restart` (version 4), a
  notification the kernel signals when the host asks for a restart,
  which Spark takes as `reboot`.

## Consequences

- A VMware machine boots from disks on PVSCSI and can be shut down and
  restarted from the host, which shows the machine's address and uptime
  and its tools as running and managed by the guest.
- The `storage-pvscsi` boot test runs the storage test on QEMU's
  `pvscsi`, at several targets. The `vmware` boot test names VMware in
  the firmware and checks that the kernel finds QEMU's backdoor and
  leaves the boot alone when there is no guest RPC; the toolbox's talk
  with a host is tested by its unit tests and on VMware's hypervisors,
  in the lab (`hardware/vmware`).
- The host's time synchronisation, shared folders, copy and paste,
  quiescing for backups and the NIC information in its third version
  are not given; VMware's hosts take a guest without them.
