vmware(4) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

vmware - the guest's tools for VMware's hypervisors

# DESCRIPTION

On a machine whose hypervisor is VMware's (ESXi, Workstation or
Fusion), the kernel runs the guest's side of VMware's tools itself, on
its *vmware* thread (ADR 0424). It speaks to the host through VMware's
backdoor, I/O ports the hypervisor answers itself, and the guest RPC
over them, polling the host's commands each second:

- The host shows the machine's tools as running, managed by the guest,
  and never offers to upgrade them.
- Shut Down Guest presses the machine's power button for *spark*(8),
  which shuts the machine down and powers it off, as *poweroff* does.
- Restart Guest asks *spark*(8) to shut down and restart, as *reboot*
  does, through *radix-power*(2)'s *notify_restart*.
- The host shows how long the machine has been up, told every 15
  seconds, and its first IPv4 address when the host asks.

The kernel log says when the tools start and when the host hears them:

```
VMware: backdoor version 6; the toolbox runs
VMware: the host hears the toolbox
VMware: the host registered the toolbox: its Shut Down and Restart work
```

A hypervisor that answers the backdoor but gives no guest RPC, as QEMU's
*vmport* does, is logged once and asked again every 15 seconds.

VMware's time synchronisation, shared folders, copy and paste and
quiescing for backups are not given: Dukovo keeps its own time.

# SEE ALSO

*pvscsi*(4), *vmxnet3*(4), *spark*(8)
