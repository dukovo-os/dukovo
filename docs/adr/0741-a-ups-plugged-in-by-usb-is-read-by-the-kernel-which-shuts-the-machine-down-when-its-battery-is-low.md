<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

# 0741. A UPS plugged in by USB is read by the kernel, which shuts the machine down when its battery is low

- Status: Accepted
- Date: 2026-11-25

## Context

A server or a workstation on a UPS should shut down cleanly before the
UPS's battery runs out in a power cut: filesystems written back, services
stopped, databases closed. Without that, the UPS only moves the hard
power loss a few minutes later. The UPSes sold for one machine or a small
rack (APC's Back-UPS and Smart-UPS, CyberPower's, Eaton's, Tripp Lite's)
plug in by USB and speak the HID power device class (*Universal Serial
Bus Usage Tables for HID Power Devices 1.0*). The lab's is a CyberPower
CP1500PFCLCD.

Such a UPS is a HID device whose top-level collection is on the Power
Device page (84h). Its battery's state is on the Battery System page
(85h): Remaining Capacity, Run Time To Empty, AC Present, Charging,
Discharging, Below Remaining Capacity Limit, Need Replacement, and the
limits the UPS sets itself. Most of it is in Feature reports, which the
host asks for with GET_REPORT; some UPSes also send some of it in Input
reports on their interrupt endpoint, and some send nothing there at all.

On Linux the kernel only binds the interface, and Network UPS Tools'
`usbhid-ups` driver reads it from user space through libusb, with
`upsmon` deciding when to shut down. Dukovo has no libusb, and its USB
devices are driven by the kernel's controller threads. The kernel already
parses HID report descriptors (radix-hid, ADR 0148) and already asks
Spark to power the machine off for the power button, a host's shutdown
and a thermal zone past its critical point (ADR 0142, 0203).

## Decision

radix-hid keeps, besides the Input fields it decodes, the place of each
value a UPS's state is read from, in Input and Feature items alike, the
first of each usage (a UPS says its charge in more than one collection,
the first the one that sums up the rest). A layout is a UPS when it has a
top-level collection on the Power Device page and a charge or run time to
read; a mouse that says how full its battery is, on the Battery System
page alone, is not. A descriptor whose only values are a UPS's Feature
reports is no longer refused for having no input.

The USB controller's thread does not listen on a UPS's interrupt
endpoint. It asks for each report that holds a value of the UPS's every
2 seconds, from the loop it runs anyway, and hands what they said to the
kernel's list of UPSes. Polling the Feature reports works for every UPS
of the class, whatever it sends on its own; 2 seconds is what
`usbhid-ups` polls at.

Each UPS is shown as `/kernel/class/power_supply/ups0`, `ups1` and on,
the lowest number free, with Linux's power supply class's files where
they fit (`type` `UPS`, `status`, `online`, `capacity`, `capacity_level`,
`capacity_alert_min`, `time_to_empty_now`, `health`, `model_name`,
`manufacturer`) and three of Dukovo's own: `load`,
`time_to_empty_alert_min` and `shuts_down`. `/kernel/class/power_supply`
is now there on every machine and lists its entries when read, as a UPS
may be plugged in at any time.

The battery is low when the UPS runs the machine on its battery and
either says so itself (Below Remaining Capacity Limit, Remaining Time
Limit Expired, Shutdown Imminent) or its charge or run time is at the
limits it sets (Remaining Capacity Limit, Remaining Time Limit), else at
10% or 120 seconds. Two reads in a row that find it low, so that one read
as the UPS changes over is not enough, ask Spark to power the machine off
as a press of the power button does. If the machine is still on 60
seconds later, the kernel writes the filesystems back and powers it off
itself, as for heat. The boot flag `ups=show` shows the UPS but never
shuts down, for a machine that another program or another machine shuts
down.

`hardware power` shows the batteries, adapters and UPSes, and the web
console's Hardware page has a Battery and UPS card, on a machine that
has either, read again every minute. `hardware check battery` warns when
a UPS says its battery should be replaced (HW-142).

## Consequences

- A machine on a USB UPS shuts down cleanly in a long power cut with no
  setup, and says in its log when the mains goes, comes back, and when
  the UPS's battery should be replaced.
- The thresholds are the UPS's own, set from its front panel or its
  maker's tool; Dukovo does not write them. Changing them from the
  console is left for later.
- The UPS is not told to turn its outlets off after the machine is off,
  as NUT's `upsdrvctl shutdown` does. If the mains comes back before the
  battery runs out, the machine stays off until it is started again,
  unless its firmware is set to start when power returns and the UPS
  cuts it. Telling the UPS is left for later.
- A UPS on the network (SNMP, NUT's protocol) and one on a serial line
  are not read. Several machines on one UPS need one of those, or each
  its own USB cable.
- A QEMU machine has no UPS, so the code is tested on the host
  (radix-hid's descriptors and states, systools' files) and on the lab's
  CyberPower.
