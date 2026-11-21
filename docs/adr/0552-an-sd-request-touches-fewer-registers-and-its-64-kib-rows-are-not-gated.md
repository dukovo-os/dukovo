<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0552. An SD request touches fewer registers, and its 64 KiB rows are not gated

- Status: Accepted
- Date: 2026-11-20

## Context

`ob bench` reads and writes an SD card in QEMU's `sdhci-pci` reader with
both systems. The 1 MiB rows were already reported but not gated: QEMU
moves each byte of a transfer through the card's emulation one call at a
time, which takes nearly all of both systems' time. The 64 KiB rows were
gated, and the write row went red on several runners at 1.01 to 1.05
times Linux's time, with no change to the SD driver.

A trace of QEMU's SD host controller under both systems, on CI, showed:

- Radix touched 16 of the controller's registers for each request and
  Linux 29 to 44.
- Between one request's end and the next one's start, Radix took 237 to
  377 µs and Linux 476 to 833 µs.
- Radix sends 64 KiB as 2 ADMA2 descriptors, Linux as 16.
- QEMU's time to move a block varied by about 10% from run to run, with
  how the host scheduled its threads, and a 64 KiB transfer takes it
  some 14 ms. Each system's own work is a few per cent of that.

A Radix that slept until the transfer was likely done, instead of reading
the controller's registers (each read lets QEMU carry on the transfer on
the CPU's own thread), ran no faster on CI.

## Decision

- **The SD 64 KiB rows are reported and never gated**, as the 1 MiB rows
  are (`Gate::Emulated`). The 4 KiB rows, where each system's own work
  per request shows, stay gated.
- **A request touches fewer registers.** The driver
  (`kernel/drivers/sdhci`):
  - writes Block Size and Block Count in one 32-bit write, and Transfer
    Mode and Command in another, as the SD Host Controller specification
    allows;
  - writes the DMA select in Host Control only when it changed, which a
    reset, a power-off or a change of bus width or speed clears;
  - clears the interrupt status after a request, not before the next one,
    and clears Error Interrupt Status wherever an error may be left set.

  A request takes 9 accesses with 64-bit ADMA2, 8 with 32-bit ADMA2 and
  8 with SDMA, against 16, 16 and 15 before. The DMA address is still
  written for each request: the controller advances it as it goes.

## Alternatives

- **Keep gating the 64 KiB rows.** They would go on failing now and then
  for the runner's own speed, which says nothing of either system.
- **A descriptor table that lets QEMU finish the transfer on its own
  thread.** It would help only under QEMU, and the experiment that took
  register reads away showed no change.

## Consequences

- The Benchmarks gate no longer fails on the SD 64 KiB rows. Their ratio
  is still printed.
- A request costs Radix fewer exits under a hypervisor and fewer bus
  cycles on a real reader. The unit test `a_request_touches_few_registers`
  counts them against a fake controller that models the 32-bit writes.
- The lab's real readers are where the SD rows can be gated again: the
  row in `docs/deferred.md` waits on that.
