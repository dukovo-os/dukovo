discover(1) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

discover - list what the local networks have

# SYNOPSIS

*discover* [*-w* _seconds_] [*-r*] [_type_...]

# DESCRIPTION

discover lists what *discoverd*(8) finds on the networks the machine is
on (ADR 0338): the services their machines offer by DNS-Based Service
Discovery, as printers, shared folders and other Dukovo, Linux and Mac
machines do, and the devices that answer WS-Discovery's Probe, as
Windows machines and many printers and scanners do.

It looks for three seconds, for the services of each _type_, as
*\_ipp.\_tcp* or *\_smb.\_tcp*, or without one, of every type the
networks' machines say they offer, and then lists them, then the
devices:

```
$ discover
SERVICE  TYPE               AT                 ADDRESSES
b        _workstation._tcp  b.local:9          192.168.50.2,fd50::2
printer  _ipp._tcp          printer.local:631  192.168.50.7

DEVICE        TYPES                     ENDPOINT               DESCRIBED AT
192.168.50.9  wsdp:Device pub:Computer  urn:uuid:4f0b6a4e-...  http://192.168.50.9:5357/4f0b6a4e-...
```

A service's name is its instance's, AT is its machine's name and port,
and its addresses are IPv4 ones first. A device is listed by the address
it answered from, with the types it says it is, the URN it keeps, and
where it describes itself. A machine that offers its services by both is
listed in each table.

A program in a bundle runs discover only with the network.local
permission (*bundle*(1)), as discoverd's folder is not in its view
without it.

# OPTIONS

*-w*, *--wait* _seconds_
	Look for that long, thirty seconds at most, and as a fraction, as
	*0.5*; 3 unless it is given.

*-r*, *--raw*
	Print discoverd's lines as they come, by tabs, as *discoverd*(8)
	gives them, instead of the tables.

# EXIT STATUS

0 when it looked, even if nothing answered: it then says *Nothing
answered in 3.0s.*; 1 when discoverd is not running, or for an option
it does not know or a _type_ not as *\_ipp.\_tcp*.

# SEE ALSO

*discoverd*(8), *ping*(8), *resolvectl*(1), *bundle*(1), *systools*(1)
