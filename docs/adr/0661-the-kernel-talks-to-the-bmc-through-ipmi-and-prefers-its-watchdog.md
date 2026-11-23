<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0661. The kernel talks to the BMC through IPMI, and prefers its watchdog

- Status: Accepted
- Date: 2026-11-23

## Context

Every rack server has a baseboard management controller (BMC): Dell's
iDRAC, HPE's iLO, Lenovo's XClarity Controller, Supermicro's and the
others'. It watches the fans, power supplies, inlet and exhaust
temperatures, voltages and power draw, keeps the event log in which
failed DIMMs, fans and power supplies are written, holds the inventory
(FRU), drives the identify light an engineer looks for in the rack, and
has a watchdog of its own. From the machine itself all of that is reached
through IPMI, over the BMC's system interface. Almost every server's is
KCS (keyboard controller style): two I/O ports, a data register and a
command and status register, and the handshake of section 9 of the IPMI
2.0 specification.

Without it, `hardware sensors` on a server shows the CPUs' own
temperatures and nothing of the chassis, a failing power supply is seen
only by whoever logs in to the BMC's web page, and the watchdog the kernel
drives is the chipset's, which on many servers the firmware has locked so
that it never restarts the machine.

Server administrators also script against `ipmitool`, which on Linux
talks to `/dev/ipmi0` with the ioctls of Linux's IPMI driver.

## Decision

- **The interface.** radix-ipmi (kernel/drivers/ipmi) runs the KCS state
  machine of IPMI 2.0 section 9: the write phase, the read phase and the
  error exit (abort, read the error code, back to idle), three tries a
  message and five seconds in all, every wait bounded. Messages go one at
  a time behind a sleeping lock; the debugger's waits spin and never wait
  for the lock.
- **Finding it**, in Linux's order: the ACPI device `IPI0001` whose `_IFT`
  says KCS, its ports from `_CRS`; else the SMBIOS structure of type 38.
  The first that answers Get Device ID is kept and logged with its maker,
  firmware and ports. The search runs after uACPI starts, or finds it is
  not to run, so SMBIOS still finds the BMC with `aml=off`.
- **`/devices/ipmi0`**, root's alone (mode 0600), takes Linux's ioctls and
  structures unchanged (radix-abi `ipmi`), so `ipmitool` and other tools
  written for Linux work as they are. A command is sent at once and its
  answer kept for the description that sent it, at most 16; `poll` finds
  the file readable while one is kept. The structures' own pointers are
  read and written with the system calls' checked copies, bounded by their
  lengths. Only the system interface address, the BMC itself, is taken;
  bridging to other controllers over IPMB is not yet.
- **The watchdog.** When the BMC has one, the kernel's watchdog (ADR 0143)
  is the BMC's: it is the one firmware leaves working, and its expiry is
  written to the BMC's event log, where the administrator looks. It is
  left stopped, with no action, until Spark starts it, as the chipset's
  is; its "restarted the machine" flag is the SMS/OS expiration flag,
  cleared once read. The watchdog's state lock is a sleeping lock now,
  since the BMC's commands wait for its answer.
- **Userspace.** The `bmc` command (bmc(8)) shows the BMC, its sensors,
  event log, inventory and LAN settings, sets its address, and blinks the
  identify light, each with `--json`. `ipmitool` is a port, built with its
  `open` interface only. `hardware sensors` and `hardware show`, run with
  the right to open the interface, add the BMC's sensors and the BMC
  itself, and `hardware show` counts the sockets that hold a processor.
- **History and alerts.** metricsd(8) keeps the BMC's temperatures, fans
  and power as series, a minute apart, reads the event log only when Get
  SEL Info says it moved, and tells alertd(8) of each new critical or
  non-recoverable entry. The last entry it looked at is kept, so a restart
  replays nothing.
- **The web console.** The Hardware page has a BMC card for an
  administrator whose console is unlocked: who the BMC is, its power,
  current and voltage sensors with any that are not well first, the event
  log's last ten entries and the identify button; the BMC's temperatures
  and fans join the temperatures card. Its three actions
  (`hardware.bmc-sensors`, `hardware.bmc-events`, `hardware.identify`) are
  an administrator's.

## Consequences

- An administrator sees the chassis's sensors and the BMC's event log
  from the machine, scripts keep using `ipmitool`, and a hung server
  restarts through the BMC with the expiry in its log.
- The BMC's sensors on other controllers behind it (Intel's Management
  Engine's, for one) need IPMB bridging, a deferred row; the BMC's own,
  which on Dell, HPE and Lenovo servers are nearly all of them, work.
- QEMU's `ipmi-bmc-sim` with `isa-ipmi-kcs` tests the interface, the
  device file, `ipmitool` and the watchdog in CI; the real BMCs are proved
  in Phase 19's hardware runs.
- Deferred rows: SSIF (IPMI over SMBus), BT and memory-mapped KCS,
  SPMI's description, IPMB bridging, the ACPI IPMI operation region
  through which firmware's AML reaches the BMC, and the BMC's
  asynchronous events (`SET_GETS_EVENTS` is taken and none are delivered).

## Alternatives considered

- **A Radix-only interface for the BMC.** Simpler to define, but every
  tool administrators already use would need porting. Linux's ioctls are
  small and stable.
- **Leaving the chipset's watchdog in charge when there is one.** It is
  the one firmware locks on servers, and its expiry leaves no trace in
  the BMC's log; Linux too prefers the BMC's when its IPMI watchdog is
  loaded.
