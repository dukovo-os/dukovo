<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: Projects whose ideas we follow
- [FreeBSD's Hyper-V drivers](https://github.com/freebsd/freebsd-src/tree/main/sys/dev/hyperv)
  (BSD-2-Clause): the layouts of VMBus's messages, its ring buffers and
  GPADLs, and the protocols of storvsc's SCSI requests, netvsc's NVSP
  and Remote NDIS messages, the synthetic keyboard and the integration
  services, which the kernel's Hyper-V drivers follow
  ([ADR 0423](docs/adr/0423-hyper-v-and-azure-guests.md)), and the
  virtual PCI bus's protocol (vmbus_pcib), the synthetic card's SR-IOV
  messages (hn) and the Azure Network Adapter's hardware channel and
  queues (mana), which accelerated networking follows
  ([ADR 0425](docs/adr/0425-azure-accelerated-networking.md)). No code
  is copied.
