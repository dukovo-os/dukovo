<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0662. The kernel reads the firmware's error reports and keeps one record of hardware errors

- Status: Accepted
- Date: 2026-11-23

## Context

The kernel reads the CPUs' machine-check banks (ADR 0145) and names the
bank and channel of a memory error, but not the memory module. On most
servers that is not where the errors are told. Dell, HPE, Lenovo and
Supermicro firmware handles memory and PCIe errors first: it decodes
them with what it knows of the board, logs them in the BMC, and leaves
a record for the operating system in a Generic Error Status Block, one
for each error source its HEST lists (ACPI's APEI, ACPI 6.5 chapter
18). The records are in UEFI's Common Platform Error Record format, and
a memory error names the module by its SMBIOS handle. Errors that ended
the last boot are in the BERT. Without APEI, a server's failing DIMM is
seen only in the BMC's log, and a page with an uncorrected error is not
taken out of use.

The errors also need one place to be read. The machine checks were
logged and counted by bank; nothing kept the last errors, by part, for
`hardware` and the web console to show.

## Decision

- **One record.** `/kernel/ras/errors` holds the last 256 errors, each a
  line: its number, time, who reported it (`mce`, `ghes<n>`, `bert`,
  `aer`), its severity (UEFI's four), its kind (memory, PCIe, processor,
  other), the memory device's SMBIOS handle, the part by the name on the
  board, and what it was. `/kernel/ras/counts` counts them by severity,
  by kind and by memory device. The machine checks record there too
  (radix-mca's `record`, kernel/radix/src/ras.rs). Every error is
  recorded; the log has at most ten lines of corrected errors a minute
  for each part, and then says how many it left out, so a failing DIMM
  fills the record, not the log. Errors that were not corrected always
  have a line.
- **The firmware is told.** With uACPI running, `_OSC` on `\_SB` asks for
  WHEA (ED855E0C-6C90-47BF-A62A-26DE0FC5AD5C) as Linux asks, and the
  platform-wide `_OSC`'s APEI bit is taken when the firmware grants it:
  some firmware fills its blocks only once told.
- **Banks the firmware owns are left to it.** A HEST corrected
  machine-check source that is enabled, firmware-first and not
  GHES-assisted names banks whose corrected errors the kernel neither
  polls nor takes by CMCI (Linux's rule), so they are not taken from
  under the firmware nor counted twice. Uncorrected errors in those banks
  still reach the machine-check handler.
- **Reading a source.** Each enabled GHES (version 1 or 2) is read by the
  `apei` thread: a polled one at its interval, at least 100 ms; one
  signalled by SCI when the hardware error device (`PNP0C33`) is
  notified; any other every second (an SCI source too, on a machine with
  no such device). A block with its status set is copied, bounded by the
  length the HEST gives (at most 64 KiB), its status written to zero, and
  a version 2 source's read-ack register written as `(read & preserve) |
  write`, so the firmware can report the next error. Every length in a
  record is checked before it is used (radix-acpi's `cper`).
- **NMI sources.** The NMI handler reads them, taking no lock and
  allocating nothing: each has a copy buffer allocated at start, and a
  state (idle, busy, pending) decides whether the handler or the thread
  reads the block, so neither waits. The handler copies, clears and
  acknowledges; the thread decodes the copy within a second.
- **What is done with an error.** It is recorded and logged with its
  part: a memory module by its locator (its SMBIOS handle), else the
  record's FRU text; a PCIe function by its address; a processor by its
  APIC ID. A page of memory with an error that was not corrected is
  taken out of use (ADR 0168), as is one whose corrected errors the
  firmware says passed its threshold. A fatal error panics with its
  first section decoded, as Linux does.
- **The BERT** is read at start: each error is logged as "Hardware error
  before this boot" and recorded as `bert`; none panics.
- **Counts by DIMM.** The EDAC tree's `dimm<n>` directories are the
  populated memory devices, by locator, then the banks' channels; Linux's
  `ras-mc-ctl` and `edac-util` read them.
- **Userspace.** `hardware errors` shows the record; `hardware check` has
  a hardware-errors area; metricsd keeps the counts as series and tells
  alertd of each uncorrected error, each error from before this boot and
  each memory device with 24 corrected errors in a day; the web console's
  Hardware page has a Hardware errors card.

## Consequences

- A server's memory and PCIe errors are seen from the machine, by DIMM
  slot, with the same names the BMC's log uses.
- A source's block that the firmware moves to another address after the
  kernel mapped it is not read; the log says so once. No firmware seen so
  far moves them.
- The tests bring their own HEST and BERT to QEMU, which has no APEI on
  x86, with the `memreserve=` boot flag keeping the kernel off the pages
  the records are in. The R740's firmware is checked in step 15.
- Deferred rows: the ERST (error records kept across boots by the
  firmware), EINJ (error injection), GHES signalled by GPIO, SEA, SEI or
  SDEI (Arm), external-interrupt and GSIV sources routed instead of read
  every second, blocks that move, the second memory error section
  format, and PCIe errors and their recovery (step 4b).

## Alternatives considered

- **Reading only the machine-check banks.** Firmware that handles errors
  first keeps them out of the banks, so the kernel would miss most of a
  server's errors.
- **A record per source in sysfs.** One record in time order is what an
  administrator reads, and the console and `hardware` need one place.
- **Decoding in the NMI handler.** The handler only copies: decoding
  takes the record's lock and may log, which an NMI must not.
