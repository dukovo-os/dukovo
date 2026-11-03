<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: Specifications and papers
- National Semiconductor's (now Texas Instruments') *PC16550D Universal
  Asynchronous Receiver/Transmitter with FIFOs* data sheet, whose
  registers `radix-uart` programs; ASIX's *AX99100 PCIe to Multi I/O
  Controller* data sheet, for its functions' IDs, subsystems and port
  layout; QEMU's documentation of its PCI serial devices
  (`docs/specs/pci-serial.rst`), for their IDs and port counts; and the
  device IDs of Intel AMT's Serial-over-LAN ports in the PCI ID
  Repository's `pci.ids`
  ([ADR 0360](docs/adr/0360-serial-ports-on-pci-and-amt-serial-over-lan-carry-the-console.md)).
  Linux's `8250_pci` (GPL-2.0) was not used. No code is taken from any
  of them.
