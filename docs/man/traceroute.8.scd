traceroute(8) "Dukovo" "Dukovo Manual"

; SPDX-FileCopyrightText: 2026 The Dukovo contributors
; SPDX-License-Identifier: Apache-2.0 OR MIT

# NAME

traceroute - the routers on the way to a machine, one hop at a time

# SYNOPSIS

*traceroute* [*-46InU*] [*-f* _first_ttl_] [*-m* _max_ttl_] [*-q* _nqueries_] [*-N* _squeries_] [*-w* _MAX_[,_HERE_[,_NEAR_]]] [*-z* _sendwait_] [*-p* _port_] [*-s* _src_addr_] [*-i* _device_] _host_ [_packetlen_]

# DESCRIPTION

traceroute finds the way packets take to _host_, an address or a name,
as Dmitry Butskoy's traceroute does on Linux, with its options and what
it prints. Each probe it sends goes one hop further than the last: its
time to live, or its hop limit for IPv6, is one more. The router where
it runs out answers with an ICMP "time exceeded", and _host_ itself
with a "port unreachable" or an echo reply, and the kernel gives these
to the socket the probe went from, as Linux does (IP_RECVERR, ADR
0345), so that traceroute needs no privilege, only the network
capability's send right (ADR 0337).

```
$ traceroute far.example
traceroute to far.example (10.77.9.9), 30 hops max, 60 byte packets
 1  gw.example (10.77.1.1)  0.402 ms  0.311 ms  0.298 ms
 2  * * *
 3  10.77.3.1 (10.77.3.1)  1.204 ms  1.188 ms  1.090 ms
 4  far.example (10.77.9.9)  1.512 ms  1.433 ms  1.401 ms
```

Each line is a hop: the router or machine that answered there, by name
and address unless *-n* is given, and how long each probe's answer
took, or *\** for a probe nothing answered in time. A router that
answers from more than one address is named again before the times its
others took. It stops after the hop where _host_ answered, or one that
said _host_ cannot be reached, or after _max_ttl_ hops. After a time, a
note says what a router said besides:

[[ *!H*, *!N*, *!P*
:< the host, its network or the protocol cannot be reached
|  *!X*
:  communication is administratively prohibited
|  *!S*
:  the source route failed
|  *!F-*_mtu_
:  the packet needs fragmenting to _mtu_ bytes
|  *!V*, *!C*
:  host precedence violation, precedence cut-off
|  *!<*_code_*>*, *!<*_type_*-*_code_*>*
:  another ICMP or ICMPv6 code or type

traceroute sends up to _squeries_ probes at once, and waits for each up
to _MAX_ seconds, or _HERE_ times as long as another probe of its hop
took to be answered, or _NEAR_ times as long as one of a later hop took.
An IPv6 address, or a name with only IPv6 ones, is traced over IPv6.

# OPTIONS

*-4*, *-6*
	Only IPv4, or only IPv6: a name's address of that family. A name
	with both is traced over IPv4 without either.

*-I*, *--icmp*
	Send ICMP echo requests, through a ping socket (ADR 0336), not UDP
	datagrams.

*-U*, *--udp*
	Send UDP datagrams to one port, 53 unless *-p* gives another, not to
	a port one more for each probe.

*-n*
	Give addresses as numbers, without looking their names up.

*-f* _first_ttl_, *--first*=_first_ttl_
	Start at hop _first_ttl_, 1 unless it is given.

*-m* _max_ttl_, *--max-hops*=_max_ttl_
	Go _max_ttl_ hops at most, 30 unless it is given, and 255 at most.

*-q* _nqueries_, *--queries*=_nqueries_
	Send _nqueries_ probes to each hop, 3 unless it is given, and 10 at
	most.

*-N* _squeries_, *--sim-queries*=_squeries_
	Send up to _squeries_ probes at once, 16 unless it is given.

*-w* _MAX_[,_HERE_[,_NEAR_]], *--wait*=...
	How long to wait for a probe, as above: 5, 3 and 10 unless they are
	given. A _HERE_ or _NEAR_ of 0 is not used.

*-z* _sendwait_, *--sendwait*=_sendwait_
	Wait _sendwait_ between probes: seconds up to 10, or milliseconds
	above that.

*-p* _port_, *--port*=_port_
	With UDP, the port of the first probe, 33434 unless it is given, or
	with *-U* the port of every probe.

*-s* _src_addr_, *--source*=_src_addr_
	Send from _src_addr_, one of the machine's addresses.

*-i* _device_, *--interface*=_device_
	Send through the interface _device_ alone (SO_BINDTODEVICE).

_packetlen_
	The bytes of each probe, with its IP and UDP or ICMP header: 60
	unless it is given, or 80 for IPv6.

Not yet: *-T* and the other methods that need raw sockets, which a
program without privilege cannot have on Linux either, *-F*, *-t*,
*-l*, *-A*, *-e*, *-g*, *--mtu* and *--back*.

# EXIT STATUS

0 when it traced the way, whatever was found on it; 2 for an option or
an argument it does not take, a name that cannot be looked up, or any
other failure.

# SEE ALSO

*mtr*(8), *ping*(8), *traceroute6*(8), *ip*(8), *systools*(1)
