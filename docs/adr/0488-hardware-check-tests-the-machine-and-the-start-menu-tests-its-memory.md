<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0488. hardware check tests the machine, and the start menu tests its memory

- Status: Accepted
- Date: 2026-11-13

## Context

When a server misbehaves, the first question is whether the machine
itself is sound: its memory, its disks, its processor under load, its
fans. On Linux the answer takes several tools, each with its own output:
smartctl for the disks' self-tests, badblocks for a read scan, stress-ng
for load, lm-sensors for the temperatures, and memtest86+ from a boot
menu that a distribution may or may not have. Windows has the Memory
Diagnostic at the next start, and vendors ship their own pre-boot
diagnostics (Dell's ePSA, Lenovo's Diagnostics). None of them gives one
verdict that names what failed.

Dukovo has the pieces already: diskhealth(8) reads each disk's SMART
record (ADR 0157), sensors(1) the hardware monitors (ADRs 0205 and 0207),
the kernel counts machine checks and memory errors (ADRs 0145 and 0169),
and `/kernel/class/power_supply` and `/kernel/class/net` say how the
battery and the network links are. What is missing is a disk's own
self-test, which needs commands the kernel did not send, a test of the
memory, and one command that runs everything and says what to do.

docs/deferred.md kept the memory test on the start menu (ADR 0501) for
this step. Secure Boot is off in 1.0, so the start menu can start a
program that is not signed.

## Decision

- **`hardware check`** (hardware(8)) tests the memory, the disks, the
  processor, the sensors and fans, the battery and the network links,
  or the areas named. Each check says `passed`, `warning`, `FAILED` or
  `skipped`, and a warning or failure carries an `HW-` error ID, whose
  page says what to do. The default run takes about three minutes:
  memory, processor load and the disks' short self-tests run side by
  side. `--full` runs the disks' extended self-tests and a read of every
  sector, a ten-minute load and most of the free memory, and says how long
  it will take before it starts. It needs an administrator, and Rescue
  has it. The exit status is 0 when every check passed, 1 when one
  failed, 2 for warnings only and 3 when it could not run. The last run is
  kept in `/system/state/hardware/last.json`, which `hardware last` shows.
- **The checks**:
  - Memory: the corrected and uncorrected memory errors counted since the
    start, and a test while the system runs, of a quarter of the free
    memory up to a gigabyte (three quarters with `--full`) in one thread
    per CPU: each word's own address, moving inversions and random
    values, each read back twice.
  - Disks: diskhealth's verdict and early warnings, the disk's own
    self-test, and a read scan through the block device, of 256 pieces
    spread over the disk by default and of all of it with `--full`, with
    an unreadable piece narrowed to its sectors.
  - Processor: one thread per CPU runs the same deterministic work, and
    every answer must agree with the first; the temperatures are read
    while it runs; corrected machine checks since the start are a warning.
  - Sensors and fans against their limits, the battery's capacity against
    its design, and each network link's speed, duplex and receive errors.
- **Disk self-tests** are a new radix-block(2) request, `SELF_TEST`: it
  starts the short or the extended test, aborts one, or only asks, and
  answers with whether one runs, how much is left, how long the disk says
  each takes and the disk's own log of its tests. On a SATA disk it is
  SMART EXECUTE OFF-LINE IMMEDIATE and SMART READ LOG of the self-test
  log, with the progress from SMART READ DATA, whose off-line capability
  byte says whether the disk has self-tests, as smartctl reads it
  (IDENTIFY's word 84 says so of some disks only); on an NVMe controller,
  Device Self-test and the Device Self-test log, where the controller
  says it has them. Starting or aborting needs the write right. Other
  disks answer `not_supported`, which hardware(8) shows as a disk that
  runs no self-test.
- **The memory test on the start menu** is memtest86+ 8.10 (GPL-2.0),
  which `./ob` builds unchanged from its pinned tarball with the build
  machine's GCC, as it builds GRUB (ADR 0306). Its one program starts
  under UEFI as an EFI program, through GRUB's `chainloader`, and under a
  BIOS through the Linux boot protocol, so the PC's GRUB gains the `linux`
  module. It is `memtest` in every release's startup folder: an update
  brings the new one, and Rescue's copy of the folder has it. The
  installed menu's Troubleshooting offers "Test the memory (memtest86+)"
  from the default slot, Rescue's entry offers it from Rescue's copy when
  no slot's menu can be read, and the install stick and the ISO offer it
  from their own copy. When the menu shows on COM1 to COM4, the ports
  memtest86+ drives, the test shows there too. The license manifest
  lists it with its download location, SHA-256 and license, and its build
  recipe in Dukovo's source is its corresponding source. The boot
  loader's allow-list (ADR 0021) gains `GPL-2.0-only` for it: like GRUB,
  it is a program of its own that shares no code with Dukovo's.

## Consequences

- One command answers whether the machine is sound, with an ID per
  finding that the support center explains, and the web console can show
  the last run from its file.
- The test of memory while the system runs reaches only the memory the
  system gives it; the full test is the start menu's, which runs before
  the system and reaches all of it. hardware(8) says so after its own.
- memtest86+ never returns: it runs until the machine is turned off or
  Esc restarts it. So `hardware check` does not start it at the next
  start on its own, as a remote machine would stay in it; docs/deferred.md
  keeps that for when it can end after one pass.
- A disk behind USB, virtio or a RAID controller runs no self-test here.
  Its read scan still runs.
- A change to the block ABI rebuilds the C library's headers, so the
  first build after it rebuilds the ports.
- Displays join `hardware check` with the window manager; docs/deferred.md
  keeps them, and a link slower than both its ends offer (HW-151), which
  needs the speeds a driver reports the card and its partner offer.
  Receive errors (HW-153) are judged from each card's `rx_errors`, which
  stays 0 until the drivers count frames with a bad checksum.

## Alternatives considered

- **smartctl, badblocks and stress-ng as ports.** Each brings its own
  output to explain, and smartctl needs raw command pass-through to the
  disks, which Dukovo does not give programs (ADR 0157). The kernel's own
  request sends only the self-test commands.
- **Passing ATA and NVMe commands through, as Linux's SG_IO and
  NVME_IOCTL_ADMIN_CMD do.** Any administrator's program could then send
  any command, firmware downloads and secure erases included. Only
  burn(1)'s MMC commands pass through, one list of them (ADR 0449).
- **MemTest86 (PassMark), or a new memory test.** MemTest86 is not
  open source. memtest86+ is the test the major distributions ship, it
  boots under UEFI and a BIOS from one file, and writing another would
  cost much and test less.
- **memtest86+ on the EFI system partition only.** The startup folder
  is updated with each release and copied into Rescue already, so the
  test comes up to date with the system and is there when the root
  cannot be read, with no new code to keep the partition's copy current.
