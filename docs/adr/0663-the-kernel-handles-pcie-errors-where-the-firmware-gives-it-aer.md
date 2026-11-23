<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0663. The kernel handles PCIe errors where the firmware gives it AER

- Status: Accepted
- Date: 2026-11-23

## Context

A PCIe link or device that goes wrong says so: each function's Advanced
Error Reporting (AER) capability logs what happened, and the function
sends an error message up to its root port, which can interrupt. Who
listens is agreed with the firmware. Each PCI host bridge's `_OSC` (PCI
Firmware 3.3, 4.5) hands the operating system control of AER, of the
PCIe capability's registers and of Downstream Port Containment (DPC), or
keeps them. A server's firmware usually keeps them and reports PCIe
errors through APEI (ADR 0662); workstations, many servers set to "OS
native" and QEMU give them to the operating system. Until now the kernel
asked for nothing (ADR 0660's deferred row), so where the firmware gave
up AER no one watched, and a failing card or a marginal link was not
seen until something broke.

Linux handles AER and DPC natively where `_OSC` gives them. Its sysfs
files `aer_dev_correctable`, `aer_dev_nonfatal`, `aer_dev_fatal`
and `aer_rootport_total_err_*` are what monitoring tools read.

## Decision

- **`_OSC` on each host bridge**, once uACPI runs: a query, then a
  request for what the query offered. The kernel says it supports
  extended configuration space, segments and MSI, and asks for AER, the
  PCIe capability (AER's device control writes need it) and DPC. AER
  without the capability, and DPC without AER, are not asked for.
  Native hot-plug waits for step 14, PME and LTR for something that
  handles them. Ownership comes from `_OSC` alone, as in current
  Linux: HEST's firmware-first flags are not read, and a bridge with no
  `_OSC` keeps AER. One line per bridge says what it gave.
- **Where AER is the kernel's**: every function below gets device
  control's four reporting enables, each bridge passes the messages from
  below it on (bridge control's SERR enable, as Linux sets it), and what
  the firmware's start left logged is cleared. Each root port and root
  complex event collector with an AER capability stops sending the
  messages to the firmware as system errors (root control) and
  interrupts instead, through MSI-X or MSI, or is read every second.
- **The interrupt** reads and clears the port's Root Error Status, keeps
  it and the Error Source Identification, counts the message for the
  port's `aer_rootport_total_err_*`, and wakes the `pcie-aer` thread.
  The thread reads the function the source names and every other
  function below the port, as messages that come while it is busy name
  only the first. A function's unmasked status says what it had, its
  severity register which uncorrectable bits were fatal, and its header
  log the TLP of the first error. Each error is recorded in `/kernel/ras`
  (source `aer`, kind `pcie`, severity corrected, recoverable or fatal)
  with Linux's name for each bit, counted for the function's
  `aer_dev_*` files, logged within the record's limit, and its status
  cleared.
- **Recovery**: a corrected error needs none; after a non-fatal one the
  driver carries on; a fatal one marks the function failed
  (`error_state`), as its driver cannot be told to reset it yet.
- **DPC**, on root and downstream ports where the bridge also gave DPC:
  the trigger on ERR_FATAL and its interrupt on. When it fires, the
  kernel logs the reason and the sender, waits for the link to go down,
  clears the trigger, waits the 100 ms the specification asks and up to a
  second for the link, and marks the functions below failed: their
  device has been reset under the driver.
- **Firmware-first errors** (CPER PCIe sections, ADR 0662) are counted
  for the same files from the record's copy of the AER registers, by the
  record's severity, as Linux counts them.
- **Files** in `/kernel/bus/pci/devices/<address>`: Linux's three
  `aer_dev_*` files and `aer_rootport_total_err_*` in its format, and
  `aer_owner` (`kernel` or `firmware`) and `error_state` (`normal` or
  `failed`), which Linux does not have. `hardware devices` shows the
  counts, `hardware check` fails on a failed device (HW-163), and the web
  console's Devices card shows both.

## Consequences

- On QEMU, on workstations and on servers set to OS-native error
  handling, PCIe errors are seen, named and counted as they happen, and
  the boot test `aer` injects them through QEMU's monitor.
- Where the firmware keeps AER nothing changes: its records still come
  through APEI, and now count by function too.
- Asking for AER changes what the firmware does: it stops handling those
  errors. A kernel that asked must listen; if a port gets no interrupt it
  is read every second.
- The kernel writes device control while drivers run, as `_OSC` is
  answered after the PCI scan. Only the four reporting bits are set, in
  one read-modify-write; a driver that rewrites device control later may
  clear them, which no driver here does.
- A device behind a fatal error or a containment stays failed until the
  machine restarts: driver recovery is step 14's, with hot-plug.
- Functions behind an Intel VMD are not watched: their configuration
  space is reached through the VMD, which the walk does not use. Linux
  handles them through the VMD's own host bridge flags.

## Alternatives considered

- **Reading HEST's firmware-first flags as well**, as older Linux did.
  The flags describe the firmware's own handling, not the
  agreement; `_OSC` is the agreement, and Linux dropped the flags for
  that reason.
- **Asking for native hot-plug, PME and LTR now.** Taking control of a
  feature nothing handles leaves it unhandled; each is asked for by the
  step that drives it.
- **A reset of the link after every fatal error without DPC** (Linux's
  secondary bus reset). Without a driver that can set its device up
  again the reset only loses the device's state; it comes with step 14.
- **Keeping the counts in the record alone.** Monitoring tools read
  Linux's per-device files, and the console wants the count per device
  without reading the whole record.
