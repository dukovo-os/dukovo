radix-ipmi(4) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

radix-ipmi - the server's baseboard management controller, through IPMI

# DESCRIPTION

A server's baseboard management controller (BMC), such as Dell's iDRAC,
HPE's iLO or Lenovo's XClarity Controller, watches the machine's fans,
power supplies, temperatures and voltages, keeps an event log, holds the
inventory and drives the identify light. The Radix kernel talks to it
through its KCS interface, the two I/O ports of section 9 of the IPMI 2.0
specification (ADR 0661).

The kernel looks for the BMC once at boot: first the ACPI device
_IPI0001_ whose _\_IFT_ says KCS, its ports from its _\_CRS_, then the
SMBIOS structure of type 38. The first that answers Get Device ID is
kept, and the kernel logs one line for it:

```
IPMI: BMC Dell, firmware 3.40, IPMI 2.0, KCS at I/O 0xCA8 (spacing 4)
```

A machine with no BMC, or one the kernel does not reach, has no such line
and no device.

# THE DEVICE

_/devices/ipmi0_ (_/dev/ipmi0_ through the link) is root's alone (mode
0600); a service is given it by its service file (*service.toml*(5)).
Reads and writes fail with *EINVAL*. Programs talk to the BMC with
*ioctl*(2), with Linux's requests and structures, so *ipmitool*(1) and
other tools written for Linux's _/dev/ipmi0_ work unchanged; a
description opened only to read may not send.

*IPMICTL_SEND_COMMAND*
	Takes a *struct ipmi_req*: the address, a message id of the caller's,
	and the message (network function, command, data). The address must
	be the system interface's (*IPMI_SYSTEM_INTERFACE_ADDR_TYPE* on
	channel *IPMI_BMC_CHANNEL*): any other fails with *EOPNOTSUPP*. Data
	of over 270 bytes fails with *EMSGSIZE*. The kernel sends the
	command and waits for the answer, then keeps it for this
	description. A BMC that does not answer gives an answer whose
	completion code is 0xC3, as on Linux. With 16 answers kept and not
	received, it fails with *EBUSY*. *IPMICTL_SEND_COMMAND_SETTIME* is the
	same; its retry count and time are kept and not used.

*IPMICTL_RECEIVE_MSG_TRUNC*, *IPMICTL_RECEIVE_MSG*
	Fill in a *struct ipmi_recv* with the oldest answer kept: its type
	(*IPMI_RESPONSE_RECV_TYPE*), the address, the message id given with
	the command, the network function (the command's plus one), the
	command, and the data, the completion code first, written to the
	buffer the structure points at. With no answer kept, both fail with
	*EAGAIN*; *poll*(2) finds the device readable while one is. An
	answer longer than the buffer fails *IPMICTL_RECEIVE_MSG* with
	*EMSGSIZE* and stays; *IPMICTL_RECEIVE_MSG_TRUNC* writes what fits,
	takes it, and fails with *EMSGSIZE*.

*IPMICTL_SET_GETS_EVENTS_CMD*
	Taken. No events are delivered yet.

*IPMICTL_SET_MY_ADDRESS_CMD*, *IPMICTL_GET_MY_ADDRESS_CMD*, *IPMICTL_SET_MY_LUN_CMD*, *IPMICTL_GET_MY_LUN_CMD*, *IPMICTL_SET_TIMING_PARMS_CMD*, *IPMICTL_GET_TIMING_PARMS_CMD*
	Keep and give back their values for the description, as on Linux:
	the address is 0x20 and the LUN 2 at first.

The structures' pointers, to the address and to the data, are read and
written in the caller's memory with the same checks as the system calls,
and never more bytes than their lengths say.

# THE WATCHDOG

When the BMC has a watchdog, it is the machine's watchdog
(*radix-sysfs*(5), _/kernel/class/watchdog/watchdog0_, whose _identity_
reads "the BMC's watchdog (IPMI)"), in place of the chipset's: it is the
one firmware leaves working on a server, and its expiry is written to the
BMC's event log. The kernel leaves it stopped, with no action, until
Spark starts it; its timeout is 1 to 6553 seconds. If it restarted the
machine, the kernel says so at boot, as for any watchdog.

# NOT YET

SSIF, BT and memory-mapped KCS interfaces; commands to other controllers
behind the BMC (IPMB bridging); the ACPI IPMI operation region; the BMC's
events.

# SEE ALSO

*bmc*(8), *ipmitool*(1), *hardware*(8), *radix-files*(7), *radix-sysfs*(5)
