bmc(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

bmc - show a server's baseboard management controller: its sensors, its
event log, its inventory and its network settings, and blink its identify
light

# SYNOPSIS

*bmc* [*--json*]

*bmc sensors* [*--json*]

*bmc events* [*--clear*] [*--json*]

*bmc fru* [*--json*]

*bmc identify* [_seconds_ | *on* | *off*] [*--json*]

*bmc lan* [*--json*]

*bmc lan set address* _address_*/*_prefix_ [*gateway* _gateway_] [*--json*]

*bmc lan set dhcp* [*--json*]

# DESCRIPTION

A server has a baseboard management controller (BMC): a small computer on
the board that watches the fans, the power supplies, the temperatures and
the voltages whether the system runs or not, keeps a log of what it saw,
and answers on a network port of its own. bmc talks to it from the
machine it is in, through the kernel's IPMI interface, _/devices/ipmi0_,
with the commands of the IPMI 2.0 specification. A machine with no BMC, or
one the kernel does not reach, has no such file, and bmc says so:
_this machine has no BMC that Dukovo reaches_.

The interface is for an administrator: its file belongs to root. Run bmc
with *elevate*(8); a person who is not one is told so, in one line.

*bmc* says what the BMC is: its maker, its firmware, the version of IPMI
it speaks, the functions it offers, the machine's GUID and the address of
its network port.

*bmc sensors* lists every sensor in the BMC's sensor data repository with
what it reads now: a temperature, a fan's speed, a voltage, a current or
a power in the unit of the sensor, converted as the sensor's record says,
and its state: *ok*, or the worst threshold it has crossed. A sensor that
has states, not a number, such as a power supply's, shows the states it is
in. A sensor that the BMC cannot read, or whose scanning is turned off, is
shown as *unavailable* or *scanning disabled*; one that belongs to another
controller on the board, which this program cannot ask, is shown as *not
reachable*.

*bmc events* lists the BMC's system event log, the oldest first: when each
event was logged, which sensor it came from, and what happened, as in
_upper critical going high, asserted_, with the reading and the threshold
for a threshold event. A BMC whose clock was not set logs the seconds
since it started, shown as _start +00:01:40_. The log is not changed. With
*--clear*, bmc shows the log and then erases it, with the reservation the
specification asks for, and waits for the erase to end.

*bmc fru* shows the BMC's inventory of the board: the chassis, the board
with the date it was made, and the product, each with its part number,
serial number and the other fields it has.

*bmc identify* turns on the light or beeper that finds the machine in a
rack, for _seconds_ (1 to 255, 15 without one), until it is turned off
with *on*, or off with *off*.

*bmc lan* shows the settings of the BMC's network port: how it gets its
address, the address, netmask, default gateway, MAC address and VLAN. The
port is the first channel of the BMC whose medium is 802.3 LAN.

*bmc lan set address* gives the port a static address with its prefix
length, and the default gateway if one is given; *bmc lan set dhcp* has it
ask a DHCP server. Both change the BMC's own network, so that a BMC
reached over it can be lost: run them from the machine itself. The change
is made between the "set in progress" and "set complete" writes the
specification asks for, and is refused, in a line, while another change is
in progress. Afterwards bmc shows the settings as the BMC now has them.

# OPTIONS

*--json*
	Print the answer as JSON instead of text. See *JSON*.

*--clear*
	With *events*, erase the log after showing it.

# JSON

Each command prints one line of JSON (ADR 0541): an object whose first
member is *"format": 1*. A member the BMC does not give is _null_. Times
are RFC 3339 in UTC.

*bmc --json* has _manufacturer_id_ (the IANA enterprise number),
_manufacturer_ (its name, or _enterprise N_), _product_id_, _device_id_,
_firmware_ (as _3.20_), _ipmi_version_, _functions_ (a list of words),
_guid_ and _lan_, which is the object of *bmc lan --json* or _null_.

*bmc sensors --json* has _sensors_, a list of objects with _number_, _name_,
_type_ (the sensor type, as _temperature_ or _power supply_), _entity_ and
_entity_instance_, _unit_ (as _°C_, _RPM_, _V_, _W_), _status_, _value_,
_raw_, _state_, _asserted_ and _thresholds_. _status_ is *ok*,
*unavailable*, *scanning_disabled* or *not_reachable*. _value_ is the
reading converted to _unit_ and _raw_ the byte the BMC gave; _state_ is for
a sensor with a number, *ok* or the worst threshold crossed, as *upper
critical*, and _asserted_ is for a sensor with states, the list of those
it is in. _thresholds_ has _lower_non_recoverable_, _lower_critical_,
_lower_non_critical_, _upper_non_critical_, _upper_critical_ and
_upper_non_recoverable_, each a value in _unit_ or _null_.

*bmc events --json* has _events_, a list of objects with _id_,
_record_type_ (2 for a system event; the others are the maker's own),
_time_, _since_start_ (seconds, for a BMC whose clock was not set),
_sensor_type_, _sensor_number_, _sensor_ (its name in the repository),
_direction_ (*asserted* or *deasserted*), _event_ (the words), _severity_
(*info*, *warning*, *critical* or *non-recoverable*), and for a threshold
event _reading_ and _threshold_ with _reading_raw_ and _threshold_raw_; and
_cleared_, true when *--clear* erased the log.

*bmc fru --json* has _chassis_, _board_ and _product_, each an object of
the fields of the area or _null_ for an area the FRU does not have, and
_notes_, a list of areas that were not shown because their checksum is
wrong. _board_ has _manufactured_ as a time, or _null_ when the board's
date is not given.

*bmc identify --json* has _mode_ (*seconds*, *on* or *off*) and _seconds_.

*bmc lan --json* has _channel_, _source_ (*static*, *dhcp*, *bios*,
*unspecified* or *other N*), _address_, _mask_, _prefix_ (the netmask's
length, _null_ if its ones are not all at the start), _gateway_, _mac_ and
_vlan_ (the VLAN's number, or _null_ when the BMC's traffic is not tagged).

# EXIT STATUS

0 when the command did what it was asked. 1 for a mistake in the command
line, a machine with no BMC that Dukovo reaches, a person who may not open
the interface, a BMC that refuses a command or does not answer, or a
BMC that has no sensors repository, event log, FRU or LAN channel to
ask. A refusal says the BMC's completion code with its words, as
_invalid data field in request (0xCC)_.

# FILES

_/devices/ipmi0_
	The kernel's IPMI interface, with Linux's ioctls, which bmc and
	*ipmitool* use. It belongs to root and is read and written by one
	program at a time as far as a request and its answer go.

# EXAMPLES

What the BMC is, and the sensors:

```
$ elevate bmc
Maker      Dell (674)
Product    0x0100
Firmware   3.20
IPMI       2.0
GUID       00112233-4455-6677-8899-aabbccddeeff
Functions  sensor device, SDR repository, SEL, FRU inventory
LAN        10.1.2.3/24 (static), MAC 00:11:22:33:44:55
$ elevate bmc sensors
NAME         TYPE          READING   STATE
Inlet Temp   temperature   22 °C     ok
Fan 1        fan           5280 RPM  ok
PS 1 Status  power supply  -         presence detected
```

The event log, saved and then erased:

```
elevate bmc events --json > events.json
elevate bmc events --clear
```

Blink the light for a technician, then turn it off:

```
elevate bmc identify 60
elevate bmc identify off
```

Give the BMC an address:

```
elevate bmc lan set address 10.0.5.20/24 gateway 10.0.5.1
elevate bmc lan set dhcp
```

# SEE ALSO

*elevate*(8), *hardware*(8), *watchdog*(8), *sensors*(1), *systools*(1)
