<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0660. Every PCI root bus is scanned and each device knows its NUMA node

- Status: Accepted
- Date: 2026-11-23

## Context

Phase 19 brings Dukovo to rack servers. A two-socket server or an AMD EPYC
machine has several PCI root buses, each behind its own host bridge: the
second socket's root is bus 0x80 on a Xeon E5, the CPU's PCIe stacks are
buses 0x17, 0x3a and 0x5d (and 0x85, 0xae and 0xd7 on the second socket)
on a Xeon Scalable, and each quadrant of an EPYC is a root (0x00, 0x40,
0x80 and 0xc0 on each socket). Most of a server's slots, disks and network
ports sit on those other roots.

The kernel's scan called `radix_hw::pci::scan` once for each MCFG window,
from the window's first bus, and followed PCI-to-PCI bridges to their
secondary buses. A host bridge is not a PCI-to-PCI bridge, so no walk
reached another root, and the devices there were never found and never
bound to a driver.

ACPI describes the roots: each is a `PNP0A08` or `PNP0A03` device whose
`_SEG`, and the bus numbers its `_CRS` decodes or else its `_BBN`, name
the root bus, and its `_PXM` names the NUMA node. But uACPI starts
after the PCI scan, and with `aml=off` it does not start at all, so the
scan cannot depend on ACPI's list of roots.

Four tables were also sized for desktops: 128 recorded PCI functions, 8
IOMMU units, 128 functions with IOMMU domains, and 16 PCI buses the
firmware's AML may reach. A server's slots, uncore functions and NVMe bays
pass the first and third, a two-socket machine has more IOMMU units than
the second, and the firmware's AML on a machine with several roots reaches
more buses than the fourth.

Each CPU's time stamp counter should also agree with the others. A package
whose counter is offset from the boot CPU's makes times taken on different
packages go backwards.

## Decision

- **Roots are found by probing.** After the walk from a window's first
  bus, every later bus in the window that no walk has reached is probed:
  if some device's function 0 answers there, the bus is a root and is
  walked as the first bus is. This is what Linux does on machines without
  ACPI roots (`pcibios_fixup_peer_bridges`). The probe maps each bus only
  while it looks, so empty buses cost no lasting mapping. A bus is walked
  once, whichever walk reaches it, and a bus beyond the window's last bus
  is not probed. The devices found are bound like any other.
- **ACPI names the roots afterwards.** Once uACPI runs, each host bridge's
  `_SEG`, `_CRS` (or `_BBN`) and `_PXM` give its root bus a NUMA node,
  through the SRAT's proximity domains. The root is the first bus number
  `_CRS` decodes, and `_BBN` only where `_CRS` names no bus numbers, as in
  Linux: some firmware's `_BBN` is wrong. `_PXM` is looked for on the host bridge and
  then on each parent, as Linux's `acpi_get_node` does. The node is shown
  per device as `/kernel/bus/pci/devices/<address>/numa_node`, in decimal,
  or `-1` when it is not known (no `_PXM`, no SRAT, `aml=off`), as Linux
  writes it. ACPI is also a cross-check: the log names a root bus that
  ACPI describes and the scan did not find, and a root the scan found that
  no host bridge names is only a debug line, as uncore buses are on Linux.
- **The tables are server-sized.** The kernel records 1024 PCI functions
  (from 128), the IOMMU code drives 32 units (from 8) and gives domains to
  512 functions (from 128), and the firmware's AML may reach 64 PCI buses
  (from 16).
- **The time stamp counters are matched within each package.** Where the
  processor has `IA32_TSC_ADJUST`, every CPU's value is made equal to the
  first CPU started in its package, as Linux does, and the kernel logs how
  many CPUs differed. The boot CPU's value stays as it is, where Linux sets
  it to 0: the kernel's clock already counts from the boot CPU's counter. The first CPU of each other package is measured
  against the boot CPU for 2 ms, and the largest step backwards is logged.
  A warp is measured and logged, not corrected. The check is skipped under
  a hypervisor, which keeps its virtual counters in step.
- **`_OSC` is left alone.** Taking native control of PCIe errors and
  hot-plug from the firmware before anything handles them would hide
  errors the firmware reports today. Hardware error reporting and PCIe
  hot-plug, steps 4 and 14 of Phase 19, each ask for exactly the control
  they handle.

## Consequences

- Devices on every root bus bind to their drivers, on two-socket Xeons and
  on EPYC. A boot test with QEMU's expander buses (`pxb-pcie`) checks that
  a disk and a network card behind a second and a third root are found,
  and that a device on a root of node 1 reads `1` from `numa_node`.
- `numa_node` is visible to `lspci` and to programs that place work near
  a device. On a machine without NUMA information it reads `-1`.
- More than 256 CPU threads and more than 8 NUMA nodes still wait. The
  per-CPU TLB-tracking array that each address space keeps in its own
  4 KiB frame fills that frame at 256 CPUs, so it needs a new design first.
  That is Phase 19 step 1b, with a row in `docs/deferred.md`.
- A TSC warp between packages is measured on real servers first. Correcting
  it, as Linux's `tsc_sync` does, waits for a machine that shows one.
- A root bus with no device answering in function 0 is not found by the
  scan, as on Linux without ACPI. A device behind such a bus is not
  seen; none is known on the server platforms Phase 19 targets.
