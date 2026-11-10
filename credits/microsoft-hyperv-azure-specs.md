<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: Specifications and papers
- Microsoft's [Hypervisor Top Level Functional
  Specification](https://learn.microsoft.com/virtualization/hyper-v-on-windows/tlfs/tlfs):
  Hyper-V's CPUID leaves, MSRs, hypercalls and synthetic interrupt
  controller, through which the kernel finds VMBus; and Azure's
  documentation of its [instance metadata
  service](https://learn.microsoft.com/azure/virtual-machines/instance-metadata-service)
  and of [provisioning a machine without its
  agent](https://learn.microsoft.com/azure/virtual-machines/linux/no-agent),
  whose goal state and health report cloud-setup sends
  ([ADR 0423](docs/adr/0423-hyper-v-and-azure-guests.md)).
